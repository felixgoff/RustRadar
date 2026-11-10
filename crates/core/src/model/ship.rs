//! `api.ts` ship DTOs (S1.5), as `RS:ships.rs` sends them.

use serde::{Deserialize, Serialize};

/// A ship as VesselFinder's public map shows it, AIS (`api.ts:400`). The server thins ships out
/// by zoom: zoomed out only some of them come back.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ship {
    pub mmsi: u32,
    pub lat: f64,
    pub lon: f64,
    /// Its name, or its MMSI when it hasn't broadcast one.
    pub name: String,
    /// VesselFinder's type colour: 2 tug/special, 3 passenger, 4 cargo, 5 fishing, 6 tanker,
    /// 7 SAR/law enforcement, 8 pleasure/sailing, 0 unknown; anything else is "other".
    pub kind: u8,
    /// 0 small, 1 medium, 2 large (always medium when zoomed out).
    pub size: u8,
    /// Degrees clockwise from north; none when moored or not reported.
    pub heading: Option<f32>,
    /// The position is old.
    pub old: bool,
    /// Metres from the AIS reference point to bow, stern, port and starboard; close in only.
    pub dims: Option<[u16; 4]>,
    /// Where it is expected over the next couple of minutes as `(lat, lon, unix_ms)`, worked
    /// out from how it was seen to move; empty when it isn't moving.
    pub frames: Vec<(f64, f64, i64)>,
}

/// An AIS navigational status: the standard code (0..14) and its wording (`api.ts:437`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavStatus {
    pub code: u8,
    pub label: String,
}

/// One ship's particulars and last report, from VesselFinder's ship card (`api.ts:446`).
/// Values it leaves out, or gives as 0 meaning "not known", are `None`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShipDetails {
    pub name: Option<String>,
    pub imo: Option<u32>,
    pub mmsi: u32,
    /// VesselFinder's own type wording, e.g. "Container Ship".
    pub type_label: Option<String>,
    /// The flag state's name.
    pub flag: Option<String>,
    /// ISO 3166 code, upper case.
    pub flag_code: Option<String>,
    pub length_m: Option<f64>,
    pub beam_m: Option<f64>,
    /// As last entered by the crew.
    pub draught_m: Option<f64>,
    pub max_draught_m: Option<f64>,
    pub gross_tonnage: Option<u32>,
    pub deadweight: Option<u32>,
    pub year_built: Option<u16>,
    /// As entered by the crew: often a UN/LOCODE such as `UKSOU`, sometimes free text.
    pub destination: Option<String>,
    /// As entered by the crew, Unix seconds.
    pub eta_unix: Option<i64>,
    /// Speed over ground, knots.
    pub speed_kn: Option<f64>,
    /// Course over ground, degrees.
    pub course_deg: Option<f64>,
    pub nav_status: Option<NavStatus>,
    /// When the position was reported, Unix seconds.
    pub position_unix: Option<i64>,
    /// The position came by satellite AIS.
    pub satellite: bool,
    pub photo_id: Option<String>,
    /// A search-and-rescue aircraft.
    pub sar: bool,
}

/// JavaScript's `encodeURIComponent`: everything but `A-Z a-z 0-9 - _ . ! ~ * ' ( )` as `%XX`
/// of its UTF-8 bytes.
fn encode_uri_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// VesselFinder's photo of a ship: 768x512, or the 180x120 thumbnail (`api.ts:486`).
pub fn ship_photo_url(photo_id: &str, thumbnail: bool) -> String {
    format!(
        "https://static.vesselfinder.net/ship-photo/{}/{}",
        encode_uri_component(photo_id),
        if thumbnail { 0 } else { 1 }
    )
}

/// The ship's page on VesselFinder (`api.ts:490`).
pub fn ship_page_url(mmsi: u32) -> String {
    format!("https://www.vesselfinder.com/vessels/details/{mmsi}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn round_trip<T: Serialize + serde::de::DeserializeOwned>(wire: Value) -> T {
        let value: T = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&value).unwrap(), wire);
        value
    }

    #[test]
    fn ship_round_trips() {
        let ship: Ship = round_trip(json!({
            "mmsi": 244_660_000, "lat": 52.4, "lon": 4.85, "name": "STENA HOLLANDICA", "kind": 3, "size": 2,
            "heading": 271.5, "old": false, "dims": [180, 60, 16, 16],
            "frames": [[52.4, 4.85, 1_700_000_000_000i64], [52.4001, 4.8491, 1_700_000_010_000i64]],
        }));
        assert_eq!(ship.frames[1], (52.4001, 4.8491, 1_700_000_010_000));
        assert_eq!(ship.dims, Some([180, 60, 16, 16]));
        let moored: Ship = round_trip(json!({
            "mmsi": 1, "lat": 0.0, "lon": 0.0, "name": "1", "kind": 0, "size": 1, "heading": null,
            "old": true, "dims": null, "frames": [],
        }));
        assert_eq!((moored.heading, moored.dims), (None, None));
    }

    #[test]
    fn ship_details_round_trip() {
        let details: ShipDetails = round_trip(json!({
            "name": "EVER GIVEN", "imo": 9_811_000, "mmsi": 353_136_000, "typeLabel": "Container Ship",
            "flag": "Panama", "flagCode": "PA", "lengthM": 400.0, "beamM": 59.0, "draughtM": 14.5,
            "maxDraughtM": 16.0, "grossTonnage": 220_940, "deadweight": 199_629, "yearBuilt": 2018,
            "destination": "NLRTM", "etaUnix": 1_700_100_000, "speedKn": 12.5, "courseDeg": 254.0,
            "navStatus": {"code": 0, "label": "Under way using engine"}, "positionUnix": 1_700_000_000,
            "satellite": false, "photoId": "9811000-353136000-abc", "sar": false,
        }));
        assert_eq!(details.nav_status.unwrap().code, 0);
        let bare: ShipDetails = round_trip(json!({
            "name": null, "imo": null, "mmsi": 111_232_511, "typeLabel": null, "flag": null, "flagCode": null,
            "lengthM": null, "beamM": null, "draughtM": null, "maxDraughtM": null, "grossTonnage": null,
            "deadweight": null, "yearBuilt": null, "destination": null, "etaUnix": null, "speedKn": null,
            "courseDeg": null, "navStatus": null, "positionUnix": null, "satellite": true, "photoId": null,
            "sar": true,
        }));
        assert_eq!(bare, ShipDetails { mmsi: 111_232_511, satellite: true, sar: true, ..Default::default() });
    }

    #[test]
    fn ship_photo_url_encodes_the_id() {
        assert_eq!(
            ship_photo_url("9811000-353136000-abc", false),
            "https://static.vesselfinder.net/ship-photo/9811000-353136000-abc/1"
        );
        assert_eq!(ship_photo_url("a b/ö_(1)", true), "https://static.vesselfinder.net/ship-photo/a%20b%2F%C3%B6_(1)/0");
    }

    #[test]
    fn ship_page_url_by_mmsi() {
        assert_eq!(ship_page_url(353_136_000), "https://www.vesselfinder.com/vessels/details/353136000");
    }
}
