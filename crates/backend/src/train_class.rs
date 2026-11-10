//! What kind of train a Trafikverket product is: its top speed, and which of
//! the track's speed limits it may use.
//!
//! Track limits (STH) depend on how much cant deficiency (rälsförhöjningsbrist)
//! a train is approved for; Trafikverket groups trains in categories
//! (Underlag till linjebok, avd. B; TRVINFRA-00398 bilaga 1):
//! A under 150 mm, B 150 mm, C 180 mm, S 245 mm (tilting). Its vehicle tables
//! (Underlag till linjebok, TC Stockholm avd. B, pages B4–B5, version
//! 2026-09-01) give each vehicle's top speed.
//!
//! Trafikverket's API says only which product a train runs as
//! (`ProductInformation`: a code like `PNA044` and a name like
//! "Öresundståg"); no public list ties products to vehicles, so this table
//! does, from the operators' fleets. Where a product runs several kinds of
//! train, the most restrictive is taken (Trafikverket's own rule for mixed
//! trainsets). Categories of vehicles other than the X2 (S) and the X74 (C,
//! an individual approval) are assumptions: modern multiple units B,
//! locomotive-hauled trains A.

use serde::Serialize;

/// A train category by approved cant deficiency.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Category {
    /// Under 150 mm: the base limit.
    A = 0,
    /// 150 mm.
    B = 1,
    /// 180 mm.
    C = 2,
    /// 245 mm, tilting trains (no product in the table is only tilting trains).
    #[allow(dead_code)]
    S = 3,
}

// How the map draws its cars (train-models.ts): `rustradar_core::model`.
pub use rustradar_core::model::TrainShape as Shape;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainClass {
    /// The vehicle(s) the product runs with, for people reading it.
    pub vehicle: &'static str,
    pub top_kmh: f64,
    pub category: Category,
    pub shape: Shape,
}

impl TrainClass {
    /// m/s.
    pub fn top(&self) -> f64 {
        self.top_kmh / 3.6
    }
}

const fn class(vehicle: &'static str, top_kmh: f64, category: Category, shape: Shape) -> TrainClass {
    TrainClass { vehicle, top_kmh, category, shape }
}

use Category::{A, B, C};
use Shape::{DoubleDeck, HighSpeed, Loco, Railcar, Regional};

// Vehicles, with their top speeds from the line book (pages B4–B5).
const X31: TrainClass = class("X31–32 (Öresundståg)", 180.0, B, Regional);
const X55: TrainClass = class("X2 / X55", 200.0, B, HighSpeed);
const X40: TrainClass = class("X40 / X55 / Rc6 + coaches", 160.0, A, DoubleDeck);
const RC6: TrainClass = class("Rc6 + coaches", 160.0, A, Loco);
const X74: TrainClass = class("X74", 200.0, C, HighSpeed);
const X3: TrainClass = class("X3", 200.0, B, HighSpeed);
const X62: TrainClass = class("X62", 180.0, B, Regional);
const X60: TrainClass = class("X60–61", 160.0, B, Regional);
const REGINA: TrainClass = class("X50–53 (Regina)", 180.0, B, Regional);
const X61: TrainClass = class("X61 / X11–14", 160.0, B, Regional);
const Y31: TrainClass = class("Y31–32 / X61", 140.0, B, Regional);
const Y1: TrainClass = class("Y1", 130.0, A, Railcar);
/// Anything not known: a locomotive-hauled train at the common 160 km/h.
pub const DEFAULT: TrainClass = class("unknown", 160.0, A, Regional);

/// By product code, as Trafikverket sends it.
const BY_CODE: &[(&str, TrainClass)] = &[("PNA044", X31)];

/// By words in the product's name (lower case), the first that matches.
const BY_NAME: &[(&str, TrainClass)] = &[
    ("öresundståg", X31),
    // SJ Snabbtåg runs the tilting X2 and the X55, which isn't: the X55's limits
    ("snabbtåg", X55),
    ("nattåg", RC6),
    ("sj intercity", RC6),
    ("sj regional", X40),
    ("snälltåget", RC6),
    ("flixtrain", RC6),
    ("mtr", X74),
    ("arlanda express", X3),
    ("norrtåg", X62),
    ("pendeltåg", X60),
    ("upptåget", REGINA),
    ("mälartåg", REGINA),
    ("tåg i bergslagen", REGINA),
    ("västtåg", X61),
    ("pågatåg", X61),
    ("östgötapendeln", X61),
    ("kustpilen", X61),
    ("krösatåg", Y31),
    ("värmlandstrafik", Y31),
    ("inlandsbanan", Y1),
];

/// The class of a product, by its code, else its name, else the default.
pub fn class_of(code: Option<&str>, name: Option<&str>) -> TrainClass {
    if let Some(code) = code.map(str::trim)
        && let Some((_, c)) = BY_CODE.iter().find(|(k, _)| k.eq_ignore_ascii_case(code))
    {
        return *c;
    }
    let name = name.unwrap_or_default().to_lowercase();
    BY_NAME.iter().find(|(k, _)| name.contains(k)).map_or(DEFAULT, |(_, c)| *c)
}

/// The 3D model a train is drawn with (static/trains, built by
/// scripts/trains/build.mjs), by its product, operator and the stations it
/// calls at; `None` draws the plain cars of its `Shape`. Liveries are the
/// operators', so a model is only taken where the operator matches.
pub fn model_of(code: Option<&str>, name: Option<&str>, operator: Option<&str>, stations: &[&str]) -> Option<&'static str> {
    let name = name.unwrap_or_default().to_lowercase();
    let operator = operator.unwrap_or_default().to_lowercase();
    let sj = operator.starts_with("sj");
    let class = class_of(code, Some(&name));
    if class == X31 {
        Some("contessa")
    } else if sj && name.contains("snabbtåg") {
        // the X55 runs SJ's high-speed trains up the coast to Sundsvall and Umeå; the X2000 the rest
        let north = ["sundsvall", "härnösand", "örnsköldsvik", "umeå"];
        let up_north = stations.iter().any(|s| north.iter().any(|n| s.to_lowercase().starts_with(n)));
        Some(if up_north { "x55" } else { "x2000" })
    } else if sj && class == X40 {
        Some("x40")
    } else if sj && class == RC6 {
        Some("sj-intercity")
    } else if class == X60 {
        Some("x60")
    } else if name.contains("pågatåg") {
        Some("x61-skane")
    } else if name.contains("västtåg") {
        Some("x61-vasttrafik")
    } else {
        None
    }
}

/// Whether the table knows the product (by code or name).
pub fn known(code: Option<&str>, name: Option<&str>) -> bool {
    class_of(code, name) != DEFAULT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn products_by_code_then_name() {
        assert_eq!(class_of(Some("PNA044"), None), X31);
        assert_eq!(class_of(Some("pna044 "), Some("anything")), X31, "the code wins");
        assert_eq!(class_of(None, Some("SJ Snabbtåg")).top_kmh, 200.0);
        assert_eq!(class_of(Some("PNA999"), Some("Norrtåg")), X62);
        assert_eq!(class_of(None, Some("SJ Nattåg")).category, A);
        assert_eq!(class_of(None, Some("Something new")), DEFAULT);
        assert!(!known(None, None));
        assert_eq!(class_of(None, Some("SJ Snabbtåg")).shape, Shape::HighSpeed);
        assert_eq!(class_of(None, Some("SJ Nattåg")).shape, Shape::Loco);
    }

    #[test]
    fn models_by_product_operator_and_route() {
        let model = |name, operator, stations: &[&str]| model_of(None, Some(name), Some(operator), stations);
        assert_eq!(model("SJ Snabbtåg", "SJ", &["Stockholm C", "Göteborg C"]), Some("x2000"));
        assert_eq!(model("SJ Snabbtåg", "SJ", &["Stockholm C", "Sundsvall C", "Umeå C"]), Some("x55"));
        assert_eq!(model("SJ Regional", "SJ", &[]), Some("x40"));
        assert_eq!(model("SJ Intercity", "SJ", &[]), Some("sj-intercity"));
        assert_eq!(model("Snälltåget", "Snälltåget", &[]), None, "another operator's livery");
        assert_eq!(model("Öresundståg", "Transdev", &[]), Some("contessa"));
        assert_eq!(model("Pendeltåg", "SJ Stockholmståg", &[]), Some("x60"));
        assert_eq!(model("Pågatåg", "Arriva", &[]), Some("x61-skane"));
        assert_eq!(model("Västtåg", "SJ Götalandståg", &[]), Some("x61-vasttrafik"));
        assert_eq!(model("Norrtåg", "Vy", &[]), None);
    }
}
