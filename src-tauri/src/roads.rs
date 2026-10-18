//! Sweden's numbered roads as Trafikverket's road database (NVDB) has them,
//! for the realistic map: how wide each stretch is and whether it is paved.
//! Two object types of the open API, with the key from the keys page:
//!
//! - `RoadGeometry`: each road's line, one for each direction it is measured
//!   in ("Med" and "Mot"), found near a point.
//! - `RoadData`: the road's properties stretch by stretch, from and to
//!   metres along that direction's line (0 to its `Length`).
//!
//! Only numbered roads are there (state roads and a few towns' own), not a
//! town's streets. Asked for by square of the map (`CELL_M`), kept a day.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::trafikverket::{includes, post, results, rows};

/// The squares the roads are asked for by, metres (about, east–west and north–south).
const CELL_M: f64 = 2000.0;
/// How long a square's roads are kept.
const TTL: Duration = Duration::from_secs(24 * 3600);
/// Squares kept at most.
const MAX_CELLS: usize = 48;
const EARTH_RADIUS: f64 = 6_371_008.8;

/// A stretch of road: its line (longitude, latitude), how wide it is, metres, and whether paved.
#[derive(Clone, Debug, Serialize)]
pub struct RoadPiece {
    pub line: Vec<[f64; 2]>,
    pub width: f32,
    pub paved: bool,
}

/// A road measured in one direction: county, main and sub number, direction's code.
type RoadKey = (i64, i64, i64, i64);

fn key_of(row: &Value) -> Option<RoadKey> {
    Some((
        row["County"].as_i64()?,
        row["RoadMainNumber"].as_i64()?,
        row["RoadSubNumber"].as_i64().unwrap_or(0),
        row["Direction"]["Code"].as_i64()?,
    ))
}

/// The roads with a line within `radius` metres of a point.
fn geometry_query(lon: f64, lat: f64, radius: f64) -> String {
    format!(
        "<QUERY objecttype=\"RoadGeometry\" schemaversion=\"1\">\
         <FILTER><NEAR name=\"Geometry.WGS843D\" value=\"{lon:.6} {lat:.6}\" mindistance=\"0\" maxdistance=\"{radius:.0}\"/></FILTER>{}</QUERY>",
        includes(&["County", "RoadMainNumber", "RoadSubNumber", "Direction", "Length", "Geometry.WGS843D"])
    )
}

/// The stretches of these roads.
fn data_query(roads: &[RoadKey]) -> String {
    let each: String = roads
        .iter()
        .map(|(county, main, sub, direction)| {
            format!(
                "<AND><EQ name=\"County\" value=\"{county}\"/><EQ name=\"RoadMainNumber\" value=\"{main}\"/>\
                 <EQ name=\"RoadSubNumber\" value=\"{sub}\"/><EQ name=\"Direction.Code\" value=\"{direction}\"/></AND>"
            )
        })
        .collect();
    format!(
        "<QUERY objecttype=\"RoadData\" schemaversion=\"1\"><FILTER><OR>{each}</OR></FILTER>{}</QUERY>",
        includes(&[
            "County",
            "RoadMainNumber",
            "RoadSubNumber",
            "Direction",
            "StartContinuousLength",
            "EndContinuousLength",
            "RoadWidth",
            "WearLayer",
        ])
    )
}

/// `LINESTRING Z (13.1 55.7 12.3, ...)`: its points, longitude first.
fn linestring(s: &str) -> Option<Vec<[f64; 2]>> {
    let inner = s.trim().strip_prefix("LINESTRING")?.trim_start();
    let inner = inner.strip_prefix('Z').unwrap_or(inner).trim().strip_prefix('(')?.strip_suffix(')')?;
    let points: Vec<[f64; 2]> = inner
        .split(',')
        .filter_map(|p| {
            let mut it = p.split_whitespace().map(|x| x.parse::<f64>().ok());
            let (lon, lat) = (it.next()??, it.next()??);
            (lon.is_finite() && lat.is_finite()).then_some([lon, lat])
        })
        .collect();
    (points.len() >= 2).then_some(points)
}

/// A road's line, and its length (metres, as Trafikverket measures it).
type Road = (RoadKey, Vec<[f64; 2]>, f64);

fn parse_geometry(body: &str) -> Result<Vec<Road>, String> {
    let results = results(body)?;
    Ok(results
        .first()
        .map(|r| rows(r, "RoadGeometry"))
        .unwrap_or_default()
        .iter()
        .filter_map(|row| {
            let line = linestring(row["Geometry"]["WGS843D"].as_str()?)?;
            Some((key_of(row)?, line, row["Length"].as_f64()?))
        })
        .collect())
}

/// A stretch: from and to metres along its road, its width, paved.
type Stretch = (f64, f64, f32, bool);

fn parse_data(body: &str) -> Result<HashMap<RoadKey, Vec<Stretch>>, String> {
    let results = results(body)?;
    let mut out: HashMap<RoadKey, Vec<Stretch>> = HashMap::new();
    for row in results.first().map(|r| rows(r, "RoadData")).unwrap_or_default() {
        let (Some(key), Some(from), Some(to), Some(width)) = (
            key_of(row),
            row["StartContinuousLength"].as_f64(),
            row["EndContinuousLength"].as_f64(),
            row["RoadWidth"].as_f64(),
        ) else {
            continue;
        };
        if width <= 0.0 || to <= from {
            continue;
        }
        // the wearing course: 1 laid (asphalt and the like), 2 gravel
        let paved = row["WearLayer"].as_i64() != Some(2);
        out.entry(key).or_default().push((from, to, width as f32, paved));
    }
    Ok(out)
}

/// Metres between two points (near enough over a road's vertices).
fn metres(a: [f64; 2], b: [f64; 2]) -> f64 {
    let lat = ((a[1] + b[1]) / 2.0).to_radians();
    let dx = (b[0] - a[0]).to_radians() * lat.cos();
    let dy = (b[1] - a[1]).to_radians();
    dx.hypot(dy) * EARTH_RADIUS
}

/// The part of `line` from `from` to `to` metres along it, `along` the metres at each point.
fn slice(line: &[[f64; 2]], along: &[f64], from: f64, to: f64) -> Vec<[f64; 2]> {
    let at = |d: f64| -> [f64; 2] {
        let i = along.partition_point(|&a| a < d).clamp(1, line.len() - 1);
        let (a0, a1) = (along[i - 1], along[i]);
        let t = if a1 > a0 { ((d - a0) / (a1 - a0)).clamp(0.0, 1.0) } else { 0.0 };
        [line[i - 1][0] + (line[i][0] - line[i - 1][0]) * t, line[i - 1][1] + (line[i][1] - line[i - 1][1]) * t]
    };
    let mut out = vec![at(from)];
    out.extend(line.iter().zip(along).filter(|(_, a)| **a > from && **a < to).map(|(p, _)| *p));
    out.push(at(to));
    out
}

/// The stretches as lines, the parts of them within `radius` metres of `centre`.
fn pieces(
    roads: &[Road],
    data: &HashMap<RoadKey, Vec<Stretch>>,
    centre: [f64; 2],
    radius: f64,
) -> Vec<RoadPiece> {
    let mut out = Vec::new();
    for (key, line, length) in roads {
        let Some(stretches) = data.get(key) else { continue };
        // metres along, scaled to the length Trafikverket measures its stretches by
        let mut along = vec![0.0];
        for w in line.windows(2) {
            along.push(along.last().unwrap() + metres(w[0], w[1]));
        }
        let total = *along.last().unwrap();
        if total <= 0.0 {
            continue;
        }
        let scale = length / total;
        along.iter_mut().for_each(|a| *a *= scale);
        for &(from, to, width, paved) in stretches {
            let part = slice(line, &along, from, to);
            // in runs of points near enough to the square, each with one more at each end
            let near: Vec<bool> = part.iter().map(|p| metres(*p, centre) <= radius).collect();
            let mut i = 0;
            while i < part.len() {
                if !near[i] {
                    i += 1;
                    continue;
                }
                let start = i.saturating_sub(1);
                while i < part.len() && near[i] {
                    i += 1;
                }
                let end = (i + 1).min(part.len());
                if end - start >= 2 {
                    out.push(RoadPiece { line: part[start..end].to_vec(), width, paved });
                }
            }
        }
    }
    out
}

type Cells = HashMap<(i64, i64), (Instant, Arc<Vec<RoadPiece>>)>;

fn cells() -> &'static Mutex<(String, Cells)> {
    static C: OnceLock<Mutex<(String, Cells)>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The square a point is in, and its middle.
fn cell_of(lon: f64, lat: f64) -> ((i64, i64), [f64; 2]) {
    let step_lat = (CELL_M / EARTH_RADIUS).to_degrees();
    let j = (lat / step_lat).floor();
    let mid_lat = (j + 0.5) * step_lat;
    let step_lon = step_lat / mid_lat.to_radians().cos().max(0.05);
    let i = (lon / step_lon).floor();
    ((i as i64, j as i64), [(i + 0.5) * step_lon, mid_lat])
}

async fn load(http: &reqwest::Client, key: &str, centre: [f64; 2]) -> Result<Vec<RoadPiece>, String> {
    // out to the square's corners and a little over
    let radius = CELL_M * 0.75;
    let roads = parse_geometry(&post(http, key, &geometry_query(centre[0], centre[1], radius)).await?)?;
    if roads.is_empty() {
        return Ok(Vec::new());
    }
    let mut wanted: Vec<RoadKey> = roads.iter().map(|(k, _, _)| *k).collect();
    wanted.sort_unstable();
    wanted.dedup();
    let data = parse_data(&post(http, key, &data_query(&wanted)).await?)?;
    Ok(pieces(&roads, &data, centre, radius))
}

/// The numbered roads' stretches around a point (its square of the map's),
/// for the realistic map; none without a Trafikverket key or outside Sweden.
#[tauri::command]
pub async fn road_widths(lon: f64, lat: f64) -> crate::CmdResult<Arc<Vec<RoadPiece>>> {
    let key = crate::keys::current().trafikverket;
    // (Sweden and a little round it)
    if key.is_empty() || !(10.0..=25.0).contains(&lon) || !(55.0..=69.5).contains(&lat) {
        return Ok(Arc::new(Vec::new()));
    }
    let (cell, centre) = cell_of(lon, lat);
    {
        let mut cache = cells().lock().unwrap();
        if cache.0 != key {
            *cache = (key.clone(), HashMap::new());
        }
        if let Some((at, pieces)) = cache.1.get(&cell)
            && at.elapsed() < TTL
        {
            return Ok(pieces.clone());
        }
    }
    let http = crate::trains::client()?;
    let found = Arc::new(load(http, &key, centre).await?);
    let mut cache = cells().lock().unwrap();
    if cache.1.len() >= MAX_CELLS {
        let oldest = cache.1.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| *k);
        if let Some(k) = oldest {
            cache.1.remove(&k);
        }
    }
    cache.1.insert(cell, (Instant::now(), found.clone()));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_3d_linestring() {
        let line = linestring("LINESTRING Z (13.1 55.7 12.5, 13.2 55.71 13)").unwrap();
        assert_eq!(line, vec![[13.1, 55.7], [13.2, 55.71]]);
        assert!(linestring("POINT (1 2)").is_none());
    }

    #[test]
    fn stretches_are_cut_along_their_road() {
        // a straight road about 1113 m east, measured as 1000 m: a stretch from 250 to 750 m is its middle half
        let line = vec![[13.0, 0.0], [13.01, 0.0]];
        let key = (12, 108, 0, 1);
        let roads = vec![(key, line, 1000.0)];
        let data = HashMap::from([(key, vec![(250.0, 750.0, 7.5, true), (750.0, 1000.0, 5.0, false)])]);
        let out = pieces(&roads, &data, [13.005, 0.0], 5000.0);
        assert_eq!(out.len(), 2);
        assert!((out[0].line[0][0] - 13.0025).abs() < 1e-9);
        assert!((out[0].line.last().unwrap()[0] - 13.0075).abs() < 1e-9);
        assert_eq!(out[0].width, 7.5);
        assert!(!out[1].paved);
    }

    #[test]
    fn only_the_part_near_the_square() {
        let line: Vec<[f64; 2]> = (0..=100).map(|i| [13.0 + i as f64 * 0.001, 0.0]).collect();
        let key = (12, 22, 0, 2);
        let roads = vec![(key, line, 11_132.0)];
        let data = HashMap::from([(key, vec![(0.0, 11_132.0, 9.0, true)])]);
        let out = pieces(&roads, &data, [13.05, 0.0], 500.0);
        assert_eq!(out.len(), 1);
        // about a kilometre of it, a point past each end
        let span = metres(out[0].line[0], *out[0].line.last().unwrap());
        assert!(span > 1000.0 && span < 1300.0, "{span}");
    }

    #[test]
    fn the_query_asks_for_each_road_and_direction() {
        let q = data_query(&[(12, 108, 0, 1), (12, 108, 0, 2)]);
        assert_eq!(q.matches("<AND>").count(), 2);
        assert!(q.contains("Direction.Code\" value=\"2\""));
    }

    /// Trafikverket itself (`TRAFIKVERKET_KEY`): the roads round Dalby, east of Lund.
    #[tokio::test]
    #[ignore]
    async fn real_roads() {
        let key = std::env::var("TRAFIKVERKET_KEY").unwrap();
        let (_, centre) = cell_of(13.35, 55.665);
        let pieces = load(&reqwest::Client::new(), &key, centre).await.unwrap();
        let widths: Vec<f32> = pieces.iter().map(|p| p.width).collect();
        println!("{} stretches, widths {:?}", pieces.len(), &widths[..widths.len().min(20)]);
        assert!(!pieces.is_empty());
        assert!(pieces.iter().all(|p| p.width > 2.0 && p.width < 40.0 && p.line.len() >= 2));
    }

    #[test]
    fn squares_hold_their_points() {
        let ((i, j), mid) = cell_of(13.19, 55.705);
        let ((i2, j2), _) = cell_of(mid[0], mid[1]);
        assert_eq!((i, j), (i2, j2));
        assert!(metres(mid, [13.19, 55.705]) < CELL_M);
    }
}
