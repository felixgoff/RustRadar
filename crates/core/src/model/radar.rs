//! `api.ts` weather-radar DTOs (S1.5).

use serde::{Deserialize, Serialize};

/// The area a radar source covers, degrees (`RS:radar.rs:33`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct RadarBounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

/// One source's current 3D radar scan: NOAA MRMS, or DWD over central Europe (`api.ts:373`,
/// `RS:radar.rs:61` `VolumeInfo`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RadarVolume {
    /// The name images are requested by, e.g. `mrms`.
    pub source: String,
    /// Unix seconds of the scan.
    pub time: i64,
    pub bounds: RadarBounds,
    /// Heights (km) with polygons.
    pub levels: Vec<f32>,
    /// Size of the coverage mask, one byte per cell, north row first.
    pub coverage_width: u32,
    pub coverage_height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn radar_volume_round_trips() {
        let wire = json!({
            "source": "mrms", "time": 1_700_000_000,
            "bounds": {"west": -130.0, "south": 20.0, "east": -60.0, "north": 55.0},
            "levels": [1.0, 1.5, 3.0], "coverageWidth": 700, "coverageHeight": 350,
        });
        let volume: RadarVolume = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(volume.levels, [1.0, 1.5, 3.0]);
        assert_eq!(volume.bounds.north, 55.0);
        assert_eq!(serde_json::to_value(&volume).unwrap(), wire);
    }
}
