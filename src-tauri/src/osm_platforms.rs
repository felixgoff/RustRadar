//! Stations' platforms as mapped in OpenStreetMap, with the numbers of the
//! tracks they serve (`ref = "2;3"`): where the track a train is announced
//! at is when the track itself isn't numbered in the map. Asked of the
//! Overpass API once for each station, in the background, and kept on disk.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A platform: the track numbers it serves and its outline (or middle line).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Platform {
    pub tracks: Vec<String>,
    pub line: Vec<(f64, f64)>,
}

/// Platforms this near a station's point are its.
const AROUND_M: u32 = 700;
/// Overpass servers, tried in turn.
const SERVERS: [&str; 2] = ["https://overpass-api.de/api/interpreter", "https://overpass.kumi.systems/api/interpreter"];
/// A station's platforms on disk are used without asking again for this long.
const DISK_FRESH: Duration = Duration::from_secs(30 * 24 * 3600);
/// A station whose platforms couldn't be had is asked again after this.
const RETRY: Duration = Duration::from_secs(10 * 60);
/// Stations asked for at once (Overpass is a shared service).
const MAX_LOADING: usize = 2;

/// A station, by its point to about 10 m.
type Key = (i64, i64);

fn key(station: (f64, f64)) -> Key {
    ((station.0 * 1e4).round() as i64, (station.1 * 1e4).round() as i64)
}

#[derive(Default)]
struct Store {
    found: HashMap<Key, Arc<Vec<Platform>>>,
    loading: HashSet<Key>,
    failed: HashMap<Key, Instant>,
}

fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn cache_dir() -> &'static OnceLock<PathBuf> {
    static D: OnceLock<PathBuf> = OnceLock::new();
    &D
}

/// Where stations' platforms are kept between runs (the app's cache directory).
pub fn set_cache_dir(dir: PathBuf) {
    let _ = cache_dir().set(dir.join("osm-platforms"));
}

/// The platforms round a station, once they are here (they are sent for,
/// from disk or Overpass, the first time); none mapped is an empty list.
pub fn near(station: (f64, f64)) -> Option<Arc<Vec<Platform>>> {
    let k = key(station);
    let mut s = store().lock().unwrap();
    if let Some(found) = s.found.get(&k) {
        return Some(found.clone());
    }
    // tests never go to the network
    if cfg!(test) {
        return None;
    }
    if s.failed.get(&k).is_some_and(|at| at.elapsed() < RETRY) || s.loading.len() >= MAX_LOADING || !s.loading.insert(k)
    {
        return None;
    }
    drop(s);
    tauri::async_runtime::spawn(load(k, station));
    None
}

fn disk_path(k: Key) -> Option<PathBuf> {
    Some(cache_dir().get()?.join(format!("{}_{}.json", k.0, k.1)))
}

async fn load(k: Key, station: (f64, f64)) {
    let found = match from_disk(k) {
        Some((platforms, true)) => Ok(platforms),
        stale => match fetch(station).await {
            Ok(platforms) => {
                to_disk(k, &platforms);
                Ok(platforms)
            }
            // older ones beat none
            Err(e) => stale.map(|s| s.0).ok_or(e),
        },
    };
    let mut s = store().lock().unwrap();
    s.loading.remove(&k);
    match found {
        Ok(platforms) => {
            s.found.insert(k, Arc::new(platforms));
        }
        Err(e) => {
            eprintln!("platforms at {station:?}: {e}");
            s.failed.insert(k, Instant::now());
        }
    }
}

fn from_disk(k: Key) -> Option<(Vec<Platform>, bool)> {
    let path = disk_path(k)?;
    let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
    let fresh = SystemTime::now().duration_since(modified).is_ok_and(|age| age < DISK_FRESH);
    Some((serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?, fresh))
}

fn to_disk(k: Key, platforms: &[Platform]) {
    let Some(path) = disk_path(k) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(platforms) {
        let _ = std::fs::write(path, json);
    }
}

fn query(station: (f64, f64)) -> String {
    let (lat, lon) = station;
    format!(
        "[out:json][timeout:25];(nwr[\"railway\"=\"platform\"](around:{AROUND_M},{lat},{lon});\
         nwr[\"public_transport\"=\"platform\"][\"train\"=\"yes\"](around:{AROUND_M},{lat},{lon}););out tags geom;"
    )
}

async fn fetch(station: (f64, f64)) -> Result<Vec<Platform>, String> {
    let client = crate::rail::http().ok_or("no HTTP client")?;
    let body = query(station);
    let mut last = String::new();
    for server in SERVERS {
        let call = crate::devlog::start("OpenStreetMap", "POST", server, Some(&body));
        let answer = async {
            let response =
                client.post(server).form(&[("data", body.as_str())]).send().await.map_err(|e| e.to_string())?;
            let status = response.status().as_u16();
            let text = response.text().await.map_err(|e| e.to_string())?;
            if !(200..300).contains(&status) {
                return Err(format!("Overpass answered {status}"));
            }
            Ok(text)
        }
        .await;
        if let Some(c) = call {
            match &answer {
                Ok(t) => c.finish(200, t.as_bytes()),
                Err(e) => c.fail(e),
            }
        }
        match answer.and_then(|t| parse(&t)) {
            Ok(platforms) => return Ok(platforms),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// The platforms in an Overpass answer (`out tags geom`) that say which tracks they serve.
pub fn parse(body: &str) -> Result<Vec<Platform>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("not Overpass JSON: {e}"))?;
    let points = |geometry: &Value| -> Vec<(f64, f64)> {
        geometry
            .as_array()
            .map(|g| g.iter().filter_map(|p| Some((p["lat"].as_f64()?, p["lon"].as_f64()?))).collect())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for e in v["elements"].as_array().ok_or("no elements")? {
        let tags = &e["tags"];
        let mut tracks = Vec::new();
        for k in ["railway:track_ref", "ref", "local_ref"] {
            for t in tracks_in(tags[k].as_str().unwrap_or_default()) {
                if !tracks.contains(&t) {
                    tracks.push(t);
                }
            }
        }
        if tracks.is_empty() {
            continue;
        }
        let line = match e["type"].as_str() {
            Some("node") => e["lat"].as_f64().zip(e["lon"].as_f64()).into_iter().collect(),
            Some("way") => points(&e["geometry"]),
            Some("relation") => e["members"]
                .as_array()
                .map(|m| m.iter().flat_map(|m| points(&m["geometry"])).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        if !line.is_empty() {
            out.push(Platform { tracks, line });
        }
    }
    Ok(out)
}

/// The track numbers a platform's `ref` names: `"2;3"`, `"2-3"`, `"Spår 4a"`, `"1/2"`.
fn tracks_in(r: &str) -> Vec<String> {
    let words: Vec<String> = r
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| {
            let digits = w.find(|c: char| !c.is_ascii_digit()).unwrap_or(w.len());
            digits > 0 && w.len() - digits <= 1
        })
        .collect();
    // "2-4": the tracks from 2 to 4
    if let [a, b] = &words[..]
        && r.contains('-')
        && let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>())
        && a < b
        && b - a <= 3
    {
        return (a..=b).map(|n| n.to_string()).collect();
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_platforms_ref_names_the_tracks_it_serves() {
        assert_eq!(tracks_in("2;3"), ["2", "3"]);
        assert_eq!(tracks_in("Spår 4a"), ["4a"]);
        assert_eq!(tracks_in("1/2"), ["1", "2"]);
        assert_eq!(tracks_in("2-4"), ["2", "3", "4"]);
        assert_eq!(tracks_in("A"), Vec::<String>::new());
        assert_eq!(tracks_in(""), Vec::<String>::new());
    }

    #[test]
    fn platforms_are_read_from_an_overpass_answer() {
        let body = r#"{"elements":[
            {"type":"way","id":1,"tags":{"railway":"platform","ref":"2;3"},
             "geometry":[{"lat":55.56,"lon":12.97},{"lat":55.561,"lon":12.971}]},
            {"type":"node","id":2,"lat":55.57,"lon":12.98,"tags":{"public_transport":"platform","train":"yes","local_ref":"1"}},
            {"type":"way","id":3,"tags":{"railway":"platform"},"geometry":[{"lat":55.5,"lon":12.9}]},
            {"type":"relation","id":4,"tags":{"railway":"platform","railway:track_ref":"5"},
             "members":[{"type":"way","role":"outer","geometry":[{"lat":55.58,"lon":12.99},{"lat":55.581,"lon":12.991}]}]}]}"#;
        let p = parse(body).unwrap();
        assert_eq!(p.len(), 3, "the one without a number is left out");
        assert_eq!(p[0].tracks, ["2", "3"]);
        assert_eq!(p[0].line.len(), 2);
        assert_eq!(p[1], Platform { tracks: vec!["1".into()], line: vec![(55.57, 12.98)] });
        assert_eq!(p[2].tracks, ["5"]);
        assert!(query((55.5, 12.9)).contains("around:700,55.5,12.9"));
    }
}
