//! Runways from OurAirports (public domain), cached on disk: they are what an
//! estimated route lines its final approach and initial climb up with. The
//! list is a 4 MB download and changes rarely.

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const URL: &str = "https://davidmegginson.github.io/ourairports-data/runways.csv";
/// Refetch after a week; a stale copy is still used if the refetch fails.
const MAX_AGE_S: i64 = 7 * 86_400;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunwayEnd {
    /// e.g. `22L`
    pub ident: String,
    /// The physical end of the runway (not the displaced threshold).
    pub lat: f64,
    pub lon: f64,
    /// Feet, where known.
    #[serde(default)]
    pub elevation_ft: Option<f64>,
    /// Feet from the physical end to the landing threshold.
    #[serde(default)]
    pub displaced_ft: f64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Runway {
    #[serde(default)]
    pub length_ft: Option<f64>,
    #[serde(default)]
    pub width_ft: Option<f64>,
    pub surface: String,
    /// The two ends, low-numbered first.
    pub ends: [RunwayEnd; 2],
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunwayData {
    /// Unix seconds.
    pub fetched_at: i64,
    /// By the airport's OurAirports ident, which is its ICAO code where it has one.
    pub airports: HashMap<String, Vec<Runway>>,
}

fn now_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

/// The cached copy if it is fresh, otherwise a new download (falling back to
/// the stale copy if the download fails).
pub async fn load(http: &reqwest::Client, cache_file: &Path) -> Result<RunwayData, String> {
    let cached = std::fs::read(cache_file)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<RunwayData>(&bytes).ok());
    if let Some(data) = &cached
        && now_s() - data.fetched_at < MAX_AGE_S
    {
        return Ok(data.clone());
    }
    match fetch(http).await {
        Ok(data) => {
            if let Some(dir) = cache_file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(bytes) = serde_json::to_vec(&data) {
                let _ = std::fs::write(cache_file, bytes);
            }
            Ok(data)
        }
        Err(e) => cached.ok_or(e),
    }
}

async fn fetch(http: &reqwest::Client) -> Result<RunwayData, String> {
    let body = http
        .get(URL)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("couldn't download the runway list: {e}"))?
        .text()
        .await
        .map_err(|e| format!("couldn't download the runway list: {e}"))?;
    let airports = parse(&body)?;
    Ok(RunwayData {
        fetched_at: now_s(),
        airports,
    })
}

/// One CSV record's fields; OurAirports quotes text fields and may put commas
/// inside the quotes.
fn fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    out.push(field);
    out
}

/// Open runways with both ends located, grouped by airport. Runways without
/// end coordinates (mostly small fields) are left out: their direction is
/// only known to the nearest ten degrees.
pub fn parse(csv: &str) -> Result<HashMap<String, Vec<Runway>>, String> {
    let mut lines = csv.lines();
    let header = fields(lines.next().ok_or("the runway list is empty")?);
    let column = |name: &str| {
        header
            .iter()
            .position(|h| h == name)
            .ok_or_else(|| format!("the runway list has no {name} column"))
    };
    let airport = column("airport_ident")?;
    let length = column("length_ft")?;
    let width = column("width_ft")?;
    let surface = column("surface")?;
    let closed = column("closed")?;
    let end_columns = |prefix: &str| -> Result<[usize; 5], String> {
        Ok([
            column(&format!("{prefix}_ident"))?,
            column(&format!("{prefix}_latitude_deg"))?,
            column(&format!("{prefix}_longitude_deg"))?,
            column(&format!("{prefix}_elevation_ft"))?,
            column(&format!("{prefix}_displaced_threshold_ft"))?,
        ])
    };
    let le = end_columns("le")?;
    let he = end_columns("he")?;

    let number = |row: &[String], i: usize| row.get(i).and_then(|s| s.trim().parse::<f64>().ok());
    let end = |row: &[String], [ident, lat, lon, elevation, displaced]: [usize; 5]| {
        Some(RunwayEnd {
            ident: row.get(ident)?.trim().to_string(),
            lat: number(row, lat).filter(|v| v.abs() <= 90.0)?,
            lon: number(row, lon).filter(|v| v.abs() <= 180.0)?,
            elevation_ft: number(row, elevation),
            displaced_ft: number(row, displaced).unwrap_or(0.0).max(0.0),
        })
    };

    let mut out: HashMap<String, Vec<Runway>> = HashMap::new();
    for line in lines.filter(|l| !l.trim().is_empty()) {
        let row = fields(line);
        if row.get(closed).map(String::as_str) == Some("1") {
            continue;
        }
        let (Some(a), Some(b)) = (end(&row, le), end(&row, he)) else {
            continue;
        };
        // both ends at one spot is a data entry slip, not a runway
        if (a.lat - b.lat).abs() < 1e-5 && (a.lon - b.lon).abs() < 1e-5 {
            continue;
        }
        let Some(code) = row.get(airport).map(|s| s.trim()).filter(|s| !s.is_empty()) else {
            continue;
        };
        out.entry(code.to_string()).or_default().push(Runway {
            length_ft: number(&row, length),
            width_ft: number(&row, width),
            surface: row.get(surface).cloned().unwrap_or_default(),
            ends: [a, b],
        });
    }
    if out.is_empty() {
        return Err("the runway list has no usable runways".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#""id","airport_ref","airport_ident","length_ft","width_ft","surface","lighted","closed","le_ident","le_latitude_deg","le_longitude_deg","le_elevation_ft","le_heading_degT","le_displaced_threshold_ft","he_ident","he_latitude_deg","he_longitude_deg","he_elevation_ft","he_heading_degT","he_displaced_threshold_ft"
269408,6523,"00A",80,80,"ASPH-G",1,0,"H1",,,,,,,,,,,
2412,2183,"EKCH",11811,148,"ASP",1,0,"04L",55.592201,12.603536,13,41,,"22R",55.6168,12.6524,10,221,
2413,2183,"EKCH",9186,148,"ASP, CON",1,0,"12",55.626293,12.633331,13,123,2313,"30",55.6127,12.6704,17,303,
2414,2183,"EKCH",5000,100,"ASP",1,1,"09",55.6,12.6,13,90,,"27",55.6,12.7,13,270,
"#;

    #[test]
    fn keeps_located_open_runways() {
        let airports = parse(SAMPLE).unwrap();
        assert!(
            !airports.contains_key("00A"),
            "a helipad without coordinates"
        );
        let ekch = &airports["EKCH"];
        assert_eq!(ekch.len(), 2, "the closed runway is dropped");
        assert_eq!(ekch[0].ends[0].ident, "04L");
        assert_eq!(ekch[0].ends[1].ident, "22R");
        assert_eq!(ekch[0].length_ft, Some(11811.0));
        // a quoted comma stays inside its field
        assert_eq!(ekch[1].surface, "ASP, CON");
        assert_eq!(ekch[1].ends[0].displaced_ft, 2313.0);
        assert_eq!(ekch[1].ends[1].elevation_ft, Some(17.0));
    }

    #[test]
    fn splits_quoted_fields() {
        assert_eq!(
            fields(r#"1,"a, b","say ""hi""",,x"#),
            vec!["1", "a, b", r#"say "hi""#, "", "x"]
        );
    }

    /// Downloads the real list; run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn downloads_the_list() {
        let data = fetch(&reqwest::Client::new()).await.unwrap();
        assert!(data.airports.len() > 5_000);
        assert!(data.airports["EKCH"].len() >= 3);
    }
}
