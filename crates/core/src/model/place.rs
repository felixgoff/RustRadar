//! `api.ts` place and geocoding DTOs (S1.5).

use serde::{Deserialize, Serialize};

/// A coarse category of a place, for its icon and how close to fly (`api.ts:287`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaceKind {
    /// A house number, or a building without a name.
    Address,
    Street,
    /// A named building, landmark, shop, station, park, canal...
    Poi,
    Airport,
    /// A suburb, neighbourhood or locality within a town.
    District,
    City,
    /// A town, village or hamlet.
    Town,
    Postcode,
    /// A state, province, county or region.
    Region,
    Country,
}

/// A place or address from Photon, © OpenStreetMap contributors, ODbL (`api.ts:300`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    /// Its name, or the street and number, e.g. `Damrak 1`.
    pub name: String,
    /// A readable address line, e.g. `Damrak 1, 1012 LG Amsterdam, Netherlands`.
    pub label: String,
    pub kind: PlaceKind,
    /// What a named place is, e.g. `Hotel`, `Canal` (points of interest only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub lat: f64,
    pub lon: f64,
    /// `[west, south, east, north]`, degrees, when the place has an extent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn place_round_trips_with_and_without_its_optional_fields() {
        let wire = json!({
            "name": "Eiffel Tower", "label": "Eiffel Tower, 5 Avenue Anatole France, 75007 Paris, France",
            "kind": "poi", "category": "Attraction", "lat": 48.8582599, "lon": 2.2945006,
            "bbox": [2.2933084, 48.857457, 2.2956897, 48.8590453],
        });
        let place: Place = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(place.kind, PlaceKind::Poi);
        assert_eq!(serde_json::to_value(&place).unwrap(), wire);

        let bare = json!({"name": "10115", "label": "10115 Berlin, Germany", "kind": "postcode", "lat": 52.53, "lon": 13.38});
        let place: Place = serde_json::from_value(bare.clone()).unwrap();
        assert_eq!((place.category.as_ref(), place.bbox), (None, None));
        assert_eq!(serde_json::to_value(&place).unwrap(), bare);
    }

    #[test]
    fn place_kinds_are_lowercase() {
        let kinds = [
            (PlaceKind::Address, "address"),
            (PlaceKind::Street, "street"),
            (PlaceKind::Poi, "poi"),
            (PlaceKind::Airport, "airport"),
            (PlaceKind::District, "district"),
            (PlaceKind::City, "city"),
            (PlaceKind::Town, "town"),
            (PlaceKind::Postcode, "postcode"),
            (PlaceKind::Region, "region"),
            (PlaceKind::Country, "country"),
        ];
        for (kind, name) in kinds {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(name));
            assert_eq!(serde_json::from_value::<PlaceKind>(json!(name)).unwrap(), kind);
        }
    }
}
