//! The flight panel's Aircraft tab (item U11; doc 06 §7.3; `FlightPanel.svelte`'s
//! `tab === "aircraft"`), drawn in the tab panel's width ([`super::Parts::aircraft`]).
//!
//! The facts in three columns ([`facts`]): `Type` across the row (`full_description`, else the
//! feed's type code), then `Type code`, `Registration`, `Mode S` (the 24-bit address in six
//! upper-case hex digits), `Serial number`, `Age` (`{n} year` or `{n} years`), `First flight`,
//! and `Owner` across the row; `—` for what is not known. The type and the owner are words, the
//! rest Mono (`.data`).
//!
//! **Checked** against Chromium's layout of today's component (`tests/flight_panel_tabs.rs`):
//! every label and value on Chromium's place, text up to a pixel lower (egui's baseline).

use egui::Ui;

use super::super::fonts::Face;
use super::super::panel::{self, Fact, Fit, Piece, Span};
use super::super::shell::Pass;
use super::{DASH, View};

/// A fact of the tab: its label, its value, whether it is in the data face, its span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub label: &'static str,
    pub value: String,
    pub data: bool,
    pub wide: bool,
}

/// `a || b`.
fn or<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() { b } else { a }
}

/// The tab's facts, in order.
pub fn facts(view: &View<'_>) -> Vec<Item> {
    let aircraft = view.aircraft();
    let flight = view.flight;
    let text = |s: Option<&str>| or(s.unwrap_or(""), DASH).to_owned();
    let item = |label, value: String, data, wide| Item { label, value, data, wide };
    let mode_s = aircraft.map_or(0, |a| a.icao_address);
    let age = aircraft.and_then(|a| a.age).map(|n| format!("{n} year{}", if n == 1 { "" } else { "s" }));
    vec![
        item("Type", or(or(aircraft.map_or("", |a| a.full_description.as_str()), &flight.typecode), DASH).to_owned(), false, true),
        item("Type code", or(view.typecode(), DASH).to_owned(), true, false),
        item("Registration", or(view.registration(), DASH).to_owned(), true, false),
        item("Mode S", if mode_s != 0 { format!("{mode_s:06X}") } else { DASH.to_owned() }, true, false),
        item("Serial number", text(aircraft.and_then(|a| a.msn.as_deref())), true, false),
        item("Age", age.unwrap_or_else(|| DASH.to_owned()), true, false),
        item("First flight", text(aircraft.and_then(|a| a.birth_date.as_deref())), true, false),
        item("Owner", or(aircraft.map_or("", |a| a.registered_owners.as_str()), DASH).to_owned(), false, true),
    ]
}

/// The tab's content in `ui` (the tab panel's inner width).
pub fn show(ui: &mut Ui, view: &View<'_>, _pass: &mut Pass<'_>) {
    let items = facts(view)
        .into_iter()
        .map(|i| {
            let style = panel::dd_style(if i.data { Face::Mono } else { Face::Ui }, false);
            Fact::new(i.label, Piece::text(&i.value, &style, Fit::Wrap)).span(if i.wide { Span::All } else { Span::One })
        })
        .collect();
    panel::facts(ui, 3, panel::FACTS_GAP, items);
}

#[cfg(test)]
mod tests {
    use rustradar_core::model::{AircraftInfo, FlightDetails, LiveFlight};
    use rustradar_core::time::Clocks;

    use super::*;
    use crate::state::flight::FollowState;

    fn view<'a>(flight: &'a LiveFlight, details: Option<&'a FlightDetails>) -> View<'a> {
        View {
            flight,
            details,
            route: None,
            status: FollowState::Live,
            last_seen_ms: 0.0,
            authenticated: false,
            opensky_added: 0,
            measured: None,
            following: false,
            back: None,
            airline: None,
            now_ms: 0.0,
            clocks: Clocks::default(),
        }
    }

    fn values(items: &[Item]) -> Vec<(&str, &str)> {
        items.iter().map(|i| (i.label, i.value.as_str())).collect()
    }

    #[test]
    fn the_facts_read_as_today() {
        let flight = LiveFlight { reg: "SE-ROR".into(), typecode: "A20N".into(), ..Default::default() };
        let details = FlightDetails {
            aircraft: Some(AircraftInfo {
                icao_address: 0x4aca8f,
                reg: "SE-ROR".into(),
                typecode: "A20N".into(),
                full_description: "Airbus A320-251N".into(),
                registered_owners: "SAS Scandinavian Airlines".into(),
                msn: Some("7894".into()),
                birth_date: Some("2017-11-21".into()),
                age: Some(8),
                ..Default::default()
            }),
            ..Default::default()
        };
        let items = facts(&view(&flight, Some(&details)));
        // Chromium's text of today's component for these props
        assert_eq!(
            values(&items),
            [
                ("Type", "Airbus A320-251N"),
                ("Type code", "A20N"),
                ("Registration", "SE-ROR"),
                ("Mode S", "4ACA8F"),
                ("Serial number", "7894"),
                ("Age", "8 years"),
                ("First flight", "2017-11-21"),
                ("Owner", "SAS Scandinavian Airlines"),
            ]
        );
        assert_eq!(items.iter().map(|i| (i.data, i.wide)).collect::<Vec<_>>()[..2], [(false, true), (true, false)]);
        assert!(items[7].wide);

        // one year; an address under six hex digits padded
        let mut d = details.clone();
        let a = d.aircraft.as_mut().unwrap();
        a.age = Some(1);
        a.icao_address = 0xabc;
        let items = facts(&view(&flight, Some(&d)));
        assert_eq!(items[5].value, "1 year");
        assert_eq!(items[3].value, "000ABC");
        let a = d.aircraft.as_mut().unwrap();
        a.age = Some(0);
        assert_eq!(facts(&view(&flight, Some(&d)))[5].value, "0 years", "0 is a known age");

        // no details: the feed's type and registration, dashes for the rest
        let items = facts(&view(&flight, None));
        assert_eq!(
            values(&items),
            [("Type", "A20N"), ("Type code", "A20N"), ("Registration", "SE-ROR"), ("Mode S", "—"), ("Serial number", "—"), ("Age", "—"), ("First flight", "—"), ("Owner", "—")]
        );
        // empty strings read as missing
        let d = FlightDetails { aircraft: Some(AircraftInfo { msn: Some(String::new()), birth_date: Some(String::new()), ..Default::default() }), ..Default::default() };
        let bare = LiveFlight::default();
        let items = facts(&view(&bare, Some(&d)));
        assert!(items.iter().all(|i| i.value == "—"), "{items:?}");
    }
}
