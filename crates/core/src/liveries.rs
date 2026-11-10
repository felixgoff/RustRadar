//! Port of `liveries.ts` (S1.15, doc 03 §17): airline liveries for the 3D models, a hand-made
//! scheme for major airlines (the curated table, with the subsidiaries that fly their parent's
//! colours and the grey of the air forces), and colours derived from the airline's logo for
//! everyone else.
//!
//! The logo colours come from the backend's `airline_colors`, asked for in batches. The
//! TypeScript's 400 ms `setTimeout` and promise become calls with the time:
//! [`Liveries::tick`] returns a batch of IATA codes to ask for once the timer is due, and the
//! caller hands the answer back to [`Liveries::settled`]. `reference.airline(icao)?.iata` is a
//! closure the caller passes, so this module needs no reference data.

use std::collections::{HashMap, HashSet};
use std::ops::Index;

use indexmap::IndexSet;

pub use crate::geo::Rgb;
use crate::model::{LiveFlight, LogoColors};
use crate::reference::airline_icao;

/// A part of an aircraft model that takes a livery colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    Top,
    Belly,
    Wings,
    Tail,
    Engines,
}

impl Part {
    /// The name in the TypeScript and in the models' `index.json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Part::Top => "top",
            Part::Belly => "belly",
            Part::Wings => "wings",
            Part::Tail => "tail",
            Part::Engines => "engines",
        }
    }

    /// The part a name stands for, as [`Part::as_str`] writes it.
    pub fn from_name(name: &str) -> Option<Part> {
        PARTS.into_iter().find(|part| part.as_str() == name)
    }
}

pub const PARTS: [Part; 5] = [Part::Top, Part::Belly, Part::Wings, Part::Tail, Part::Engines];

/// A colour for each [`Part`] (`Record<Part, RGB>`); `livery[part]` reads one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Livery {
    pub top: Rgb,
    pub belly: Rgb,
    pub wings: Rgb,
    pub tail: Rgb,
    pub engines: Rgb,
}

impl Index<Part> for Livery {
    type Output = Rgb;

    fn index(&self, part: Part) -> &Rgb {
        match part {
            Part::Top => &self.top,
            Part::Belly => &self.belly,
            Part::Wings => &self.wings,
            Part::Tail => &self.tail,
            Part::Engines => &self.engines,
        }
    }
}

const fn nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => panic!("not a hex digit"),
    }
}

/// `#RRGGBB` to a colour (`parseInt` of each pair, base 16).
const fn hex(h: &str) -> Rgb {
    let b = h.as_bytes();
    [nibble(b[1]) * 16 + nibble(b[2]), nibble(b[3]) * 16 + nibble(b[4]), nibble(b[5]) * 16 + nibble(b[6])]
}

const WHITE: Rgb = hex("#F5F5F3");
// Wings cover most of an airliner seen from above, so this grey sets how the whole fleet reads:
// dark enough to separate wing from fuselage, light enough that the aircraft still look white
// rather than grey.
const WING_GREY: Rgb = hex("#E3E6EA");
const ENGINE_GREY: Rgb = hex("#D6DAE0");

/// A scheme as written in the table: the tail colour, and the parts that are not the default.
#[derive(Clone, Copy)]
struct Spec {
    tail: Option<&'static str>,
    top: Option<&'static str>,
    belly: Option<&'static str>,
    wings: Option<&'static str>,
    engines: Option<&'static str>,
}

impl Spec {
    const NONE: Spec = Spec { tail: None, top: None, belly: None, wings: None, engines: None };
}

const fn or_hex(h: Option<&str>, otherwise: Rgb) -> Rgb {
    match h {
        Some(h) => hex(h),
        None => otherwise,
    }
}

/// The top defaults to white, the belly to the top, the wings and engines to their greys.
const fn livery(spec: Spec) -> Livery {
    let t = or_hex(spec.top, WHITE);
    let Some(tail) = spec.tail else { panic!("a livery has a tail colour") };
    Livery {
        top: t,
        belly: or_hex(spec.belly, t),
        wings: or_hex(spec.wings, WING_GREY),
        tail: hex(tail),
        engines: or_hex(spec.engines, ENGINE_GREY),
    }
}

/// `livery!(tail: "#…", top: "#…", …)`: the TypeScript's `livery({ tail: "#…", … })`.
macro_rules! livery {
    ($($part:ident: $hex:literal),* $(,)?) => {{
        // a scheme that gives every part leaves nothing to `NONE`
        #[allow(clippy::needless_update)]
        let spec = Spec { $($part: Some($hex),)* ..Spec::NONE };
        livery(spec)
    }};
}

/// Private, general aviation and unknown operators.
pub const DEFAULT_LIVERY: Livery =
    Livery { top: WHITE, belly: WHITE, wings: WING_GREY, tail: WHITE, engines: ENGINE_GREY };

const MILITARY_LIVERY: Livery = livery!(top: "#8C9298", tail: "#8C9298", wings: "#80868C", engines: "#7A8086");

/// Simplified schemes for high-traffic airlines, keyed by ICAO designator: the tail colour, plus
/// the fuselage, belly and engines where they are not the usual white and grey.
const CURATED: &[(&str, Livery)] = &[
    // Europe
    ("RYR", livery!(tail: "#073590", belly: "#073590", engines: "#073590")),
    ("EZY", livery!(tail: "#FF6600", engines: "#FF6600")),
    ("WZZ", livery!(tail: "#C6007E", engines: "#3E1A61")),
    ("DLH", livery!(tail: "#05164D", engines: "#05164D")),
    ("EWG", livery!(tail: "#A6005E")),
    ("BAW", livery!(tail: "#1B2A59", belly: "#1B2A59", engines: "#1B2A59")),
    ("AFR", livery!(tail: "#002157")),
    ("KLM", livery!(top: "#00A1DE", belly: "#F5F5F3", tail: "#00A1DE", engines: "#00A1DE")),
    ("IBE", livery!(tail: "#D7192D")),
    ("VLG", livery!(tail: "#FFCC00", engines: "#57585A")),
    ("SWR", livery!(tail: "#E2001A")),
    ("AUA", livery!(tail: "#EE2E24")),
    ("AEE", livery!(tail: "#00539F")),
    ("THY", livery!(tail: "#C8102E")),
    ("FIN", livery!(tail: "#0B1560")),
    ("NOZ", livery!(tail: "#D81939")),
    ("EIN", livery!(tail: "#006D60", engines: "#006D60")),
    ("LOT", livery!(tail: "#0D2C6C")),
    ("VIR", livery!(top: "#E6E7EA", tail: "#DA0530")),
    ("EXS", livery!(tail: "#E2231A")),
    ("AFL", livery!(top: "#E5E7EA", tail: "#0055A5")),
    // Middle East and Africa
    ("UAE", livery!(tail: "#D71921")),
    ("QTR", livery!(top: "#F0F0F0", belly: "#B9BBBE", tail: "#5C0632", engines: "#B9BBBE")),
    ("ETD", livery!(top: "#E1D9C9", belly: "#C5B597", tail: "#B7924D", engines: "#C5B597")),
    ("SVA", livery!(belly: "#D9C59B", tail: "#0F5A99")),
    ("MSR", livery!(tail: "#003366")),
    ("ETH", livery!(tail: "#078930")),
    ("RAM", livery!(tail: "#C1272D")),
    ("ELY", livery!(tail: "#0C2E66")),
    // Americas
    ("DAL", livery!(belly: "#0B2A5B", tail: "#0B2A5B")),
    ("AAL", livery!(top: "#C9CED4", belly: "#A9B0B8", wings: "#AEB4BA", tail: "#0B5AB0", engines: "#C9CED4")),
    ("UAL", livery!(belly: "#0C2C6B", tail: "#0C2C6B", engines: "#0C2C6B")),
    ("SWA", livery!(top: "#304CB2", tail: "#304CB2", engines: "#304CB2")),
    ("JBU", livery!(belly: "#0033A0", tail: "#0033A0", engines: "#0033A0")),
    ("ASA", livery!(tail: "#01426A")),
    ("NKS", livery!(top: "#FFE600", tail: "#FFE600", engines: "#FFE600")),
    ("ACA", livery!(belly: "#1A1A1A", tail: "#1A1A1A", engines: "#1A1A1A")),
    ("AMX", livery!(tail: "#0B2343")),
    ("LAN", livery!(tail: "#1B0088")),
    ("GLO", livery!(tail: "#FF6600", engines: "#FF6600")),
    ("AZU", livery!(tail: "#1E4FA3", engines: "#1E4FA3")),
    ("CMP", livery!(tail: "#002F6C")),
    // Asia-Pacific
    ("QFA", livery!(tail: "#E40000")),
    ("VOZ", livery!(tail: "#CE0E2D")),
    ("JST", livery!(tail: "#FF5C00")),
    ("ANZ", livery!(tail: "#111111", engines: "#111111")),
    ("SIA", livery!(tail: "#0B2363")),
    ("CPA", livery!(tail: "#006564")),
    ("CSN", livery!(tail: "#005BAC")),
    ("CHH", livery!(tail: "#C8102E")),
    ("AXM", livery!(tail: "#E31F26", engines: "#E31F26")),
    ("THA", livery!(tail: "#5F2D86")),
    ("GIA", livery!(tail: "#00758D")),
    ("KAL", livery!(top: "#7FB8E6", belly: "#DDE0E3", tail: "#F2F2F2", engines: "#DDE0E3")),
    ("JAL", livery!(tail: "#F5F5F3", engines: "#F5F5F3")),
    ("IGO", livery!(belly: "#1F2A72", tail: "#1F2A72", engines: "#1F2A72")),
    // Cargo
    ("FDX", livery!(belly: "#9DA3A9", tail: "#4D148C")),
    ("UPS", livery!(top: "#F1EFEA", belly: "#351C15", tail: "#351C15", engines: "#351C15")),
    ("DHK", livery!(top: "#FFCC00", tail: "#FFCC00", engines: "#FFCC00")),
];

/// Group callsigns and subsidiaries that fly their parent's colours.
const ALIASES: &[(&str, &str)] = &[
    ("RUK", "RYR"),
    ("MAY", "RYR"),
    ("EJU", "EZY"),
    ("EZS", "EZY"),
    ("WMT", "WZZ"),
    ("WAZ", "WZZ"),
    ("WUK", "WZZ"),
    ("CLH", "DLH"),
    ("SHT", "BAW"),
    ("CFE", "BAW"),
    ("EFW", "BAW"),
    ("HOP", "AFR"),
    ("KLC", "KLM"),
    ("IBS", "IBE"),
    ("NSZ", "NOZ"),
    ("NAX", "NOZ"),
    ("ENY", "AAL"),
    ("PDT", "AAL"),
    ("JIA", "AAL"),
    ("LPE", "LAN"),
    ("TAM", "LAN"),
    ("QLK", "QFA"),
    ("AIQ", "AXM"),
    ("AWQ", "AXM"),
    ("BCS", "DHK"),
    ("DHX", "DHK"),
];

/// Air force and other state callsigns, painted grey.
const MILITARY: [&str; 12] = ["RCH", "RRR", "GAF", "IAM", "BAF", "CTM", "CFC", "HKY", "ASY", "PAT", "CNV", "NAF"];

/// Logo colours are asked for this long after the first code of a batch is queued, ms.
const BATCH_DELAY_MS: i64 = 400;
/// At most this many codes per `airline_colors` call.
const BATCH_SIZE: usize = 40;

/// `CURATED[ALIASES[icao] ?? icao]`.
fn curated(icao: &str) -> Option<&'static Livery> {
    let key = ALIASES.iter().find(|(from, _)| *from == icao).map_or(icao, |&(_, to)| to);
    CURATED.iter().find(|(code, _)| *code == key).map(|(_, livery)| livery)
}

fn from_logo(colors: &LogoColors) -> Livery {
    Livery { tail: colors.primary, engines: colors.secondary.unwrap_or(ENGINE_GREY), ..DEFAULT_LIVERY }
}

/// The airline IATA code from a flight number such as `LH26` or `U25634`: `/^([A-Z0-9]{2})\d/`.
fn iata_of_flight_number(flight: &str) -> Option<&str> {
    let b = flight.as_bytes();
    let code = |c: &u8| c.is_ascii_uppercase() || c.is_ascii_digit();
    (b.len() >= 3 && b[..2].iter().all(code) && b[2].is_ascii_digit()).then(|| &flight[..2])
}

/// The liveries of the flights on the globe: curated, military or from the airline's logo.
#[derive(Debug, Clone, Default)]
pub struct Liveries {
    /// Bumped whenever logo colours arrive, so that colours drawn from them refresh.
    version: u64,
    /// By IATA code: the livery from the logo, or `None` when the logo gave none.
    from_logos: HashMap<String, Option<Livery>>,
    /// In the order they were queued: a batch takes the first.
    queued: IndexSet<String>,
    in_flight: HashSet<String>,
    /// When the batch timer fires, if set.
    timer: Option<i64>,
}

impl Liveries {
    pub fn new() -> Self {
        Liveries::default()
    }

    /// Bumped whenever logo colours arrive.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The livery a flight is drawn in. `airline_iata` is `reference.airline(icao)?.iata`: the
    /// IATA code of the airline with that ICAO designator, if known. A flight whose airline
    /// needs logo colours not yet known is drawn in the default livery, and the colours are
    /// queued for the next batch.
    pub fn for_flight<'r>(
        &mut self,
        f: &LiveFlight,
        airline_iata: impl FnOnce(&str) -> Option<&'r str>,
        now_ms: i64,
    ) -> Livery {
        let Some(icao) = airline_icao(&f.callsign) else { return DEFAULT_LIVERY };
        if let Some(curated) = curated(icao) {
            return *curated;
        }
        if MILITARY.contains(&icao) {
            return MILITARY_LIVERY;
        }
        // `reference.airline(icao)?.iata || iataOfFlightNumber(f.flight)`: an empty code is none
        let iata = match airline_iata(icao).filter(|code| !code.is_empty()) {
            Some(code) => Some(code.to_owned()),
            None => iata_of_flight_number(&f.flight).map(str::to_owned),
        };
        let Some(iata) = iata else { return DEFAULT_LIVERY };
        if let Some(known) = self.from_logos.get(&iata) {
            return known.unwrap_or(DEFAULT_LIVERY);
        }
        self.request(iata, now_ms);
        DEFAULT_LIVERY
    }

    /// Runs the batch timer: once it is due, the next batch of IATA codes (at most 40, the
    /// earliest queued first) to ask the backend's `airline_colors` for. Hand its answer to
    /// [`settled`](Self::settled).
    pub fn tick(&mut self, now_ms: i64) -> Option<Vec<String>> {
        if !self.timer.is_some_and(|due| now_ms >= due) {
            return None;
        }
        self.timer = None;
        let n = self.queued.len().min(BATCH_SIZE);
        let batch: Vec<String> = self.queued.drain(..n).collect();
        for code in &batch {
            self.in_flight.insert(code.clone());
        }
        Some(batch)
    }

    /// When [`tick`](Self::tick) next has something to do, if ever: for scheduling a wake-up.
    pub fn deadline(&self) -> Option<i64> {
        self.timer
    }

    /// The `airline_colors` call for `batch` (as [`tick`](Self::tick) returned it) has settled:
    /// its answer, or `None` when it failed (nothing is cached: tried again on a later sighting).
    pub fn settled(&mut self, batch: &[String], result: Option<&HashMap<String, Option<LogoColors>>>, now_ms: i64) {
        if let Some(result) = result {
            for code in batch {
                let colors = result.get(code).copied().flatten();
                self.from_logos.insert(code.clone(), colors.map(|colors| from_logo(&colors)));
            }
            self.version += 1;
        }
        for code in batch {
            self.in_flight.remove(code);
        }
        if !self.queued.is_empty() && self.timer.is_none() {
            self.timer = Some(now_ms + BATCH_DELAY_MS);
        }
    }

    fn request(&mut self, iata: String, now_ms: i64) {
        if self.in_flight.contains(&iata) || self.queued.contains(&iata) {
            return;
        }
        self.queued.insert(iata);
        if self.timer.is_none() {
            self.timer = Some(now_ms + BATCH_DELAY_MS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flight(callsign: &str, number: &str) -> LiveFlight {
        LiveFlight { callsign: callsign.into(), flight: number.into(), ..LiveFlight::default() }
    }

    /// Reference data with Scandinavian (SAS → SK) and an airline without an IATA code.
    fn iata_of(icao: &str) -> Option<&'static str> {
        match icao {
            "SAS" => Some("SK"),
            "NOI" => Some(""),
            _ => None,
        }
    }

    fn colors(primary: Rgb, secondary: Option<Rgb>) -> Option<LogoColors> {
        Some(LogoColors { primary, secondary })
    }

    #[test]
    fn hex_reads_each_pair_in_base_16() {
        assert_eq!(hex("#F5F5F3"), [245, 245, 243]);
        assert_eq!(hex("#0b5ab0"), [11, 90, 176]);
        assert_eq!((WHITE, WING_GREY, ENGINE_GREY), ([245, 245, 243], [227, 230, 234], [214, 218, 224]));
    }

    #[test]
    fn parts_by_name_and_index() {
        assert_eq!(PARTS.map(Part::as_str), ["top", "belly", "wings", "tail", "engines"]);
        for part in PARTS {
            assert_eq!(Part::from_name(part.as_str()), Some(part));
        }
        assert_eq!(Part::from_name("fin"), None);
        let l = curated("AAL").unwrap();
        assert_eq!([l[Part::Top], l[Part::Belly], l[Part::Wings], l[Part::Tail], l[Part::Engines]], [
            l.top, l.belly, l.wings, l.tail, l.engines
        ]);
    }

    #[test]
    fn unspecified_parts_default_to_white_and_grey() {
        assert_eq!(DEFAULT_LIVERY, Livery {
            top: WHITE,
            belly: WHITE,
            wings: WING_GREY,
            tail: WHITE,
            engines: ENGINE_GREY
        });
        // only a tail
        assert_eq!(*curated("EWG").unwrap(), Livery { tail: hex("#A6005E"), ..DEFAULT_LIVERY });
        // the belly follows the top
        let vir = curated("VIR").unwrap();
        assert_eq!((vir.top, vir.belly, vir.tail), (hex("#E6E7EA"), hex("#E6E7EA"), hex("#DA0530")));
        assert_eq!((vir.wings, vir.engines), (WING_GREY, ENGINE_GREY));
        // unless it has its own
        let klm = curated("KLM").unwrap();
        assert_eq!((klm.top, klm.belly), (hex("#00A1DE"), WHITE));
        // every part given
        assert_eq!(*curated("AAL").unwrap(), Livery {
            top: hex("#C9CED4"),
            belly: hex("#A9B0B8"),
            wings: hex("#AEB4BA"),
            tail: hex("#0B5AB0"),
            engines: hex("#C9CED4"),
        });
        assert_eq!(MILITARY_LIVERY, Livery {
            top: hex("#8C9298"),
            belly: hex("#8C9298"),
            wings: hex("#80868C"),
            tail: hex("#8C9298"),
            engines: hex("#7A8086"),
        });
    }

    #[test]
    fn the_tables_hold_together() {
        assert_eq!(CURATED.len(), 59);
        let codes: HashSet<&str> = CURATED.iter().map(|(code, _)| *code).collect();
        assert_eq!(codes.len(), CURATED.len());
        for (from, to) in ALIASES {
            assert!(codes.contains(to), "{from} → {to}");
            assert!(!codes.contains(from), "{from} is curated itself");
        }
        for code in MILITARY {
            assert!(!codes.contains(code) && curated(code).is_none());
        }
        for code in codes.iter().chain(ALIASES.iter().map(|(from, _)| from)).chain(MILITARY.iter()) {
            assert!(code.len() == 3 && code.bytes().all(|c| c.is_ascii_uppercase()), "{code}");
        }
    }

    #[test]
    fn curated_airlines_and_their_subsidiaries() {
        let mut liveries = Liveries::new();
        let ryanair = *curated("RYR").unwrap();
        assert_eq!(ryanair, Livery {
            top: WHITE,
            belly: hex("#073590"),
            wings: WING_GREY,
            tail: hex("#073590"),
            engines: hex("#073590"),
        });
        assert_eq!(liveries.for_flight(&flight("RYR12AB", "FR1234"), iata_of, 0), ryanair);
        assert_eq!(liveries.for_flight(&flight("RUK4", "RK4"), iata_of, 0), ryanair);
        assert_eq!(liveries.for_flight(&flight("MAY801", ""), iata_of, 0), ryanair);
        assert_eq!(liveries.for_flight(&flight("EJU77", "U277"), iata_of, 0), *curated("EZY").unwrap());
        assert_eq!(liveries.for_flight(&flight("DHX3", ""), iata_of, 0), *curated("DHK").unwrap());
        // curated before military, and neither asks for logo colours
        assert_eq!(liveries.for_flight(&flight("RCH871", "RC871"), iata_of, 0), MILITARY_LIVERY);
        assert_eq!(liveries.for_flight(&flight("NAF01", ""), iata_of, 0), MILITARY_LIVERY);
        assert_eq!(liveries.deadline(), None);
        assert_eq!(liveries.tick(10_000), None);
    }

    #[test]
    fn callsigns_without_an_airline_get_the_default() {
        let mut liveries = Liveries::new();
        for callsign in ["", "N123AB", "DLH", "DLHX", "dlh12", "DL123", "ÅÄÖ1", "G-ABCD"] {
            assert_eq!(liveries.for_flight(&flight(callsign, "LH26"), iata_of, 0), DEFAULT_LIVERY, "{callsign}");
        }
        assert_eq!(liveries.deadline(), None);
        assert_eq!(airline_icao("DLH4XK"), Some("DLH"));
        assert_eq!(iata_of_flight_number("U25634"), Some("U2"));
        assert_eq!(iata_of_flight_number("LH26"), Some("LH"));
        assert_eq!(iata_of_flight_number("LHX"), None);
        assert_eq!(iata_of_flight_number("l26"), None);
    }

    #[test]
    fn other_airlines_wear_their_logo_colours_once_known() {
        let mut liveries = Liveries::new();
        let sas = flight("SAS1415", "SK1415");
        assert_eq!(liveries.for_flight(&sas, iata_of, 1_000), DEFAULT_LIVERY);
        assert_eq!(liveries.deadline(), Some(1_400));
        // seen again before the batch goes: still queued once
        assert_eq!(liveries.for_flight(&sas, iata_of, 1_200), DEFAULT_LIVERY);
        assert_eq!(liveries.tick(1_399), None);
        let batch = liveries.tick(1_400).unwrap();
        assert_eq!(batch, ["SK"]);
        // while in flight: not asked for again
        assert_eq!(liveries.for_flight(&sas, iata_of, 1_500), DEFAULT_LIVERY);
        assert_eq!(liveries.deadline(), None);
        let answer = HashMap::from([("SK".to_owned(), colors([0, 0, 255], Some([200, 0, 0])))]);
        liveries.settled(&batch, Some(&answer), 1_600);
        assert_eq!(liveries.version(), 1);
        assert_eq!(liveries.deadline(), None);
        let livery = Livery { tail: [0, 0, 255], engines: [200, 0, 0], ..DEFAULT_LIVERY };
        assert_eq!(liveries.for_flight(&sas, iata_of, 1_700), livery);
        // without a secondary colour the engines stay grey
        let mut liveries = Liveries::new();
        liveries.for_flight(&sas, iata_of, 0);
        let batch = liveries.tick(400).unwrap();
        liveries.settled(&batch, Some(&HashMap::from([("SK".to_owned(), colors([9, 9, 9], None))])), 500);
        assert_eq!(liveries.for_flight(&sas, iata_of, 600), Livery { tail: [9, 9, 9], ..DEFAULT_LIVERY });
    }

    #[test]
    fn the_iata_code_falls_back_to_the_flight_number() {
        let mut liveries = Liveries::new();
        // unknown to the reference data, and known with an empty code
        liveries.for_flight(&flight("XYZ12", "X35634"), iata_of, 0);
        liveries.for_flight(&flight("NOI3", "N0300"), iata_of, 0);
        // neither: nothing to ask for
        liveries.for_flight(&flight("QQQ1", "QQ"), iata_of, 0);
        liveries.for_flight(&flight("QQQ2", ""), |_| None, 0);
        assert_eq!(liveries.tick(400).unwrap(), ["X3", "N0"]);
    }

    #[test]
    fn a_logo_without_colours_is_remembered_and_a_failure_is_not() {
        let mut liveries = Liveries::new();
        let sas = flight("SAS1", "SK1");
        let other = flight("ABC1", "AB1");
        liveries.for_flight(&sas, iata_of, 0);
        liveries.for_flight(&other, iata_of, 0);
        let batch = liveries.tick(400).unwrap();
        assert_eq!(batch, ["SK", "AB"]);
        // SK has no colours; AB is missing from the answer altogether
        liveries.settled(&batch, Some(&HashMap::from([("SK".to_owned(), None)])), 500);
        assert_eq!(liveries.version(), 1);
        assert_eq!(liveries.for_flight(&sas, iata_of, 600), DEFAULT_LIVERY);
        assert_eq!(liveries.for_flight(&other, iata_of, 600), DEFAULT_LIVERY);
        assert_eq!(liveries.deadline(), None);
        // a failed call caches nothing: asked again on a later sighting
        let mut liveries = Liveries::new();
        liveries.for_flight(&sas, iata_of, 0);
        let batch = liveries.tick(400).unwrap();
        liveries.settled(&batch, None, 500);
        assert_eq!(liveries.version(), 0);
        assert_eq!(liveries.deadline(), None);
        liveries.for_flight(&sas, iata_of, 600);
        assert_eq!(liveries.deadline(), Some(1_000));
        assert_eq!(liveries.tick(1_000).unwrap(), ["SK"]);
    }

    #[test]
    fn batches_take_forty_codes_and_the_rest_follow() {
        let mut liveries = Liveries::new();
        let codes: Vec<String> = (0..45).map(|i| format!("{}{}", (b'A' + i / 10) as char, i % 10)).collect();
        for (i, code) in codes.iter().enumerate() {
            liveries.for_flight(&flight(&format!("ZZZ{i}"), &format!("{code}1")), |_| None, i as i64);
        }
        // the timer runs from the first code queued
        assert_eq!(liveries.deadline(), Some(400));
        let first = liveries.tick(400).unwrap();
        assert_eq!(first, codes[..40]);
        // the other five wait for the batch out to settle
        assert_eq!(liveries.deadline(), None);
        assert_eq!(liveries.tick(10_000), None);
        liveries.settled(&first, Some(&HashMap::new()), 500);
        assert_eq!(liveries.deadline(), Some(900));
        let second = liveries.tick(900).unwrap();
        assert_eq!(second, codes[40..]);
        // a code seen while a batch is out arms the timer itself, and settling keeps it
        liveries.for_flight(&flight("ZZZ99", "Z91"), |_| None, 950);
        assert_eq!(liveries.deadline(), Some(1_350));
        liveries.settled(&second, Some(&HashMap::new()), 1_000);
        assert_eq!(liveries.deadline(), Some(1_350));
        assert_eq!(liveries.tick(1_350).unwrap(), ["Z9"]);
        assert_eq!(liveries.version(), 2);
        // codes answered without colours are not asked for again
        liveries.for_flight(&flight("ZZZ0", "A01"), |_| None, 1_400);
        assert_eq!(liveries.deadline(), None);
    }
}
