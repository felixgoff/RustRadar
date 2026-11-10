//! Port of `reference.svelte.ts` (S1.12; doc 05 §6.9): the airlines and airports the app knows,
//! indexed for lookups, the runways fetched per airport as routes need them, and
//! [`airline_icao`].
//!
//! The TypeScript's `Reference` was Svelte state that loaded itself. Here [`Reference`] is a
//! plain value built once from the backend's [`ReferenceData`] (`backend::reference`) and shared
//! as an `Arc`; the loading itself (`load()`: once, again after a failure) and the `ready` and
//! `error` flags are the app's. [`Runways`] keeps the TypeScript's bookkeeping of which
//! airports were asked for, with the 60 s retry as a deadline checked against `now_ms` instead of
//! a `setTimeout`; the app fetches what [`Runways::take_wanted`] hands it
//! (`backend::airport_runways`) and reports back with [`Runways::settled`].
//!
//! Motion keyed airports by identity (`Map<Airport, …>`, `tr.field === tr.destAp`); the lookups
//! here also give each airport's index into [`Reference::airports`], a stable key for that.

use std::collections::HashMap;

pub use crate::model::{Airline, Airport, ReferenceData, Runway};

/// The airline's ICAO designator from an airline-style callsign (`DLH4XK` → `DLH`): the
/// TypeScript's `/^([A-Z]{3})\d/`, ASCII letters and digit as in JavaScript. Private and
/// military callsigns usually don't follow the pattern.
pub fn airline_icao(callsign: &str) -> Option<&str> {
    let b = callsign.as_bytes();
    (b.len() >= 4 && b[..3].iter().all(u8::is_ascii_uppercase) && b[3].is_ascii_digit()).then(|| &callsign[..3])
}

/// The airlines and airports of the reference data, with their indexes.
#[derive(Debug, Clone, Default)]
pub struct Reference {
    airlines: Vec<Airline>,
    airports: Vec<Airport>,
    /// Indexes into `airlines`, by ICAO designator.
    airlines_by_icao: HashMap<String, usize>,
    /// Indexes into `airports`, by Flightradar24's id.
    airports_by_id: HashMap<u32, usize>,
    /// Indexes into `airports`, by ICAO and by IATA code in one map: the lengths never collide.
    airports_by_code: HashMap<String, usize>,
}

impl Reference {
    /// The indexes over `data`. As in the TypeScript's `Map.set`, a later entry with the same key
    /// replaces an earlier one, and empty codes are not indexed.
    pub fn new(data: ReferenceData) -> Self {
        let ReferenceData { airlines, airports, .. } = data;
        let mut airlines_by_icao = HashMap::with_capacity(airlines.len());
        for (i, a) in airlines.iter().enumerate() {
            airlines_by_icao.insert(a.icao.clone(), i);
        }
        let mut airports_by_id = HashMap::with_capacity(airports.len());
        let mut airports_by_code = HashMap::with_capacity(airports.len() * 2);
        for (i, a) in airports.iter().enumerate() {
            airports_by_id.insert(a.id, i);
            if !a.icao.is_empty() {
                airports_by_code.insert(a.icao.clone(), i);
            }
            // IATA after ICAO: the two never collide in length
            if !a.iata.is_empty() {
                airports_by_code.insert(a.iata.clone(), i);
            }
        }
        Reference { airlines, airports, airlines_by_icao, airports_by_id, airports_by_code }
    }

    /// Every airline, in the reference data's order.
    pub fn airlines(&self) -> &[Airline] {
        &self.airlines
    }

    /// Every airport, in the reference data's order; the `*_index` lookups index this.
    pub fn airports(&self) -> &[Airport] {
        &self.airports
    }

    /// The airline with this ICAO designator; `None` for no designator or an empty one.
    pub fn airline(&self, icao: Option<&str>) -> Option<&Airline> {
        let icao = icao.filter(|c| !c.is_empty())?;
        self.airlines_by_icao.get(icao).map(|&i| &self.airlines[i])
    }

    /// The index of the airport with Flightradar24's id `id`; 0 is no id.
    pub fn airport_index_by_id(&self, id: Option<u32>) -> Option<usize> {
        let id = id.filter(|&id| id != 0)?;
        self.airports_by_id.get(&id).copied()
    }

    /// The airport with Flightradar24's id `id`; 0 is no id.
    pub fn airport_by_id(&self, id: Option<u32>) -> Option<&Airport> {
        self.airport_index_by_id(id).map(|i| &self.airports[i])
    }

    /// The index of the airport with this IATA (3 letters) or ICAO (4 letters) code, in any case.
    pub fn airport_index(&self, code: Option<&str>) -> Option<usize> {
        let code = code.filter(|c| !c.is_empty())?;
        self.airports_by_code.get(&code.to_uppercase()).copied()
    }

    /// The airport with this IATA (3 letters) or ICAO (4 letters) code, in any case.
    pub fn airport(&self, code: Option<&str>) -> Option<&Airport> {
        self.airport_index(code).map(|i| &self.airports[i])
    }
}

impl From<ReferenceData> for Reference {
    fn from(data: ReferenceData) -> Self {
        Reference::new(data)
    }
}

/// After a failed runway download, the airport is asked for again this long after the failure.
pub const RUNWAYS_RETRY_MS: i64 = 60_000;

/// Runways by ICAO code, filled in as they are asked for.
#[derive(Debug, Clone, Default)]
pub struct Runways {
    by_icao: HashMap<String, Vec<Runway>>,
    /// The codes asked for: `None` while the answer is awaited, `Some(t)` after a failure, to be
    /// asked again from `t` (the TypeScript's `setTimeout(() => runwaysAsked.delete(code), 60_000)`).
    asked: HashMap<String, Option<i64>>,
    /// Asked for and not yet handed to the app, oldest first.
    wanted: Vec<String>,
}

impl Runways {
    pub fn new() -> Self {
        Runways::default()
    }

    /// The airport's runways, or `None` until they have loaded (and for no airport). An empty list
    /// means none are known, also for an airport without an ICAO code. An airport not asked for
    /// yet, or whose download failed at least [`RUNWAYS_RETRY_MS`] ago, is queued for
    /// [`Runways::take_wanted`].
    pub fn get(&mut self, airport: Option<&Airport>, now_ms: i64) -> Option<&[Runway]> {
        let airport = airport?;
        let code = airport.icao.to_uppercase();
        if code.is_empty() {
            return Some(&[]);
        }
        if self.by_icao.contains_key(&code) {
            return self.by_icao.get(&code).map(Vec::as_slice);
        }
        let ask = match self.asked.get(&code) {
            None => true,
            Some(None) => false,
            Some(&Some(retry_at)) => now_ms >= retry_at,
        };
        if ask {
            self.asked.insert(code.clone(), None);
            self.wanted.push(code);
        }
        None
    }

    /// The ICAO codes to download now, each once, in the order they were asked for.
    pub fn take_wanted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.wanted)
    }

    /// The download for `code` ended: `found` is the backend's answer by ICAO code (`code`
    /// missing from it means no runways are known), or `None` when it failed (offline, or the
    /// download failed), and then the code is asked for again [`RUNWAYS_RETRY_MS`] later.
    pub fn settled(&mut self, code: &str, found: Option<&HashMap<String, Vec<Runway>>>, now_ms: i64) {
        match found {
            Some(found) => {
                self.by_icao.insert(code.to_owned(), found.get(code).cloned().unwrap_or_default());
            }
            None => {
                self.asked.insert(code.to_owned(), Some(now_ms + RUNWAYS_RETRY_MS));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn airline(name: &str, iata: &str, icao: &str) -> Airline {
        Airline { name: name.into(), iata: iata.into(), icao: icao.into() }
    }

    fn airport(id: u32, iata: &str, icao: &str, name: &str) -> Airport {
        Airport { id, iata: iata.into(), icao: icao.into(), name: name.into(), ..Airport::default() }
    }

    fn data() -> ReferenceData {
        ReferenceData {
            fetched_at: 1_700_000_000,
            airlines: vec![airline("Lufthansa", "LH", "DLH"), airline("SAS", "SK", "SAS"), airline("No code", "", "")],
            airports: vec![
                airport(1, "ARN", "ESSA", "Stockholm Arlanda"),
                airport(2, "LHR", "EGLL", "London Heathrow"),
                airport(3, "", "ESKN", "Stockholm Skavsta, without IATA"),
                airport(4, "XXX", "", "No ICAO"),
                airport(0, "ZZZ", "ZZZZ", "Id 0"),
            ],
        }
    }

    fn runway(ident: &str) -> Runway {
        let mut r = Runway::default();
        r.ends[0].ident = ident.into();
        r
    }

    #[test]
    fn airline_icao_takes_the_three_letters_before_a_digit() {
        assert_eq!(airline_icao("DLH4XK"), Some("DLH"));
        assert_eq!(airline_icao("SAS1234"), Some("SAS"));
        assert_eq!(airline_icao("BAW1"), Some("BAW"));
        // private and military callsigns, and anything shorter
        for callsign in ["SEABC", "N123AB", "DLH", "DL4X", "dlh4xk", "DLHX4", "", "ÅLH4", "DLH٣"] {
            assert_eq!(airline_icao(callsign), None, "{callsign}");
        }
        // only the start counts, and what follows the digit is anything
        assert_eq!(airline_icao("SAS9ö"), Some("SAS"));
        assert_eq!(airline_icao(" SAS9"), None);
    }

    #[test]
    fn airlines_by_designator() {
        let r = Reference::new(data());
        assert_eq!(r.airline(Some("DLH")).map(|a| a.name.as_str()), Some("Lufthansa"));
        assert_eq!(r.airline(airline_icao("SAS1234")).map(|a| a.iata.as_str()), Some("SK"));
        assert_eq!(r.airline(Some("dlh")), None, "designators are not upper-cased");
        assert_eq!(r.airline(Some("XYZ")), None);
        // no designator, or an empty one, is no airline: not the one stored without a code
        assert_eq!(r.airline(None), None);
        assert_eq!(r.airline(Some("")), None);
        assert_eq!(r.airlines().len(), 3);
    }

    #[test]
    fn airports_by_id_where_0_is_no_id() {
        let r = Reference::new(data());
        assert_eq!(r.airport_by_id(Some(2)).map(|a| a.iata.as_str()), Some("LHR"));
        assert_eq!(r.airport_index_by_id(Some(2)), Some(1));
        assert_eq!(r.airport_by_id(Some(99)), None);
        assert_eq!(r.airport_by_id(None), None);
        assert_eq!(r.airport_by_id(Some(0)), None);
    }

    #[test]
    fn airports_by_iata_or_icao_code_in_any_case() {
        let r = Reference::new(data());
        let name = |code: &str| r.airport(Some(code)).map(|a| a.name.as_str());
        assert_eq!(name("ARN"), Some("Stockholm Arlanda"));
        assert_eq!(name("ESSA"), Some("Stockholm Arlanda"));
        assert_eq!(name("egll"), Some("London Heathrow"));
        assert_eq!(name("Lhr"), Some("London Heathrow"));
        assert_eq!(name("ESKN"), Some("Stockholm Skavsta, without IATA"));
        assert_eq!(name("XXX"), Some("No ICAO"));
        // airports without an id are still found by code
        assert_eq!(name("ZZZZ"), Some("Id 0"));
        assert_eq!(name("QQQ"), None);
        assert_eq!(name(""), None);
        assert_eq!(r.airport(None), None);
        assert_eq!(r.airport_index(Some("essa")), Some(0));
        assert_eq!(r.airports().len(), 5);
    }

    #[test]
    fn a_later_entry_with_the_same_key_wins() {
        let mut d = data();
        d.airlines.push(airline("Lufthansa again", "LH", "DLH"));
        // an IATA code equal to another airport's ICAO code is not real, but shows the order:
        // IATA is set after ICAO
        d.airports.push(airport(5, "ESSA", "", "IATA ESSA"));
        d.airports.push(airport(1, "", "", "Id 1 again"));
        let r = Reference::new(d);
        assert_eq!(r.airline(Some("DLH")).map(|a| a.name.as_str()), Some("Lufthansa again"));
        assert_eq!(r.airport(Some("ESSA")).map(|a| a.name.as_str()), Some("IATA ESSA"));
        assert_eq!(r.airport_by_id(Some(1)).map(|a| a.name.as_str()), Some("Id 1 again"));
        assert_eq!(r.airport(Some("ARN")).map(|a| a.name.as_str()), Some("Stockholm Arlanda"));
    }

    #[test]
    fn nothing_is_known_before_the_data_is_in() {
        let r = Reference::default();
        assert!(r.airlines().is_empty() && r.airports().is_empty());
        assert_eq!(r.airline(Some("DLH")), None);
        assert_eq!(r.airport(Some("ARN")), None);
        assert_eq!(r.airport_by_id(Some(1)), None);
    }

    #[test]
    fn runways_are_asked_for_once_and_known_after() {
        let arlanda = airport(1, "ARN", "essa", "Stockholm Arlanda");
        let mut rw = Runways::new();
        assert_eq!(rw.get(Some(&arlanda), 0), None);
        assert_eq!(rw.get(Some(&arlanda), 10), None);
        // the code is upper-cased, and asked for once
        assert_eq!(rw.take_wanted(), ["ESSA"]);
        assert!(rw.take_wanted().is_empty());
        assert_eq!(rw.get(Some(&arlanda), 20), None);
        assert!(rw.take_wanted().is_empty());
        let found = HashMap::from([("ESSA".to_owned(), vec![runway("01L"), runway("08")])]);
        rw.settled("ESSA", Some(&found), 30);
        let known = rw.get(Some(&arlanda), 40).expect("known");
        assert_eq!(known.iter().map(|r| r.ends[0].ident.as_str()).collect::<Vec<_>>(), ["01L", "08"]);
        assert!(rw.take_wanted().is_empty());
    }

    #[test]
    fn runways_none_known_are_an_empty_list() {
        let mut rw = Runways::new();
        // no airport: unknown; an airport without an ICAO code: none known, and nothing asked
        assert_eq!(rw.get(None, 0), None);
        let no_icao = airport(4, "XXX", "", "No ICAO");
        assert_eq!(rw.get(Some(&no_icao), 0), Some(&[][..]));
        assert!(rw.take_wanted().is_empty());
        // an answer without the airport: none known (`found[code] ?? []`)
        let skavsta = airport(3, "NYO", "ESKN", "Skavsta");
        assert_eq!(rw.get(Some(&skavsta), 0), None);
        assert_eq!(rw.take_wanted(), ["ESKN"]);
        rw.settled("ESKN", Some(&HashMap::new()), 5);
        assert_eq!(rw.get(Some(&skavsta), 6), Some(&[][..]));
        assert!(rw.take_wanted().is_empty());
    }

    #[test]
    fn runways_are_asked_for_again_a_minute_after_a_failure() {
        let heathrow = airport(2, "LHR", "EGLL", "London Heathrow");
        let mut rw = Runways::new();
        assert_eq!(rw.get(Some(&heathrow), 1_000), None);
        assert_eq!(rw.take_wanted(), ["EGLL"]);
        rw.settled("EGLL", None, 2_000);
        assert_eq!(rw.get(Some(&heathrow), 2_000 + RUNWAYS_RETRY_MS - 1), None);
        assert!(rw.take_wanted().is_empty(), "not before the minute is up");
        assert_eq!(rw.get(Some(&heathrow), 2_000 + RUNWAYS_RETRY_MS), None);
        assert_eq!(rw.take_wanted(), ["EGLL"]);
        // and once only, while the second try is out
        assert_eq!(rw.get(Some(&heathrow), 2_000 + 2 * RUNWAYS_RETRY_MS), None);
        assert!(rw.take_wanted().is_empty());
        let found = HashMap::from([("EGLL".to_owned(), vec![runway("09L")])]);
        rw.settled("EGLL", Some(&found), 70_000);
        assert_eq!(rw.get(Some(&heathrow), 70_001).map(<[Runway]>::len), Some(1));
    }

    #[test]
    fn runways_of_several_airports_are_wanted_in_the_order_asked() {
        let mut rw = Runways::new();
        for icao in ["ESSA", "EGLL", "ESSA", "EKCH"] {
            rw.get(Some(&airport(1, "", icao, "")), 0);
        }
        assert_eq!(rw.take_wanted(), ["ESSA", "EGLL", "EKCH"]);
    }
}
