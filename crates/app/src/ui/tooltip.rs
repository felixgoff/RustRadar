//! The map's tooltip (item T8; doc 03 §10.6, doc 02 §11.4): the small box at the pointer over a
//! ship, a train, a signal or an airport's code. A port of the globe's `getTooltip` (`tooltip`,
//! G:3378-3422), of how deck 9.4's `TooltipWidget` shows what it returns, and of the
//! `.aircraft-tooltip` styles (G:3884-3902). Aircraft get the hover card instead (U18).
//!
//! **What it says** ([`Tip`]): a head row (`.tt-head`) and sub rows (`.tt-sub`, the last of a
//! train's in `.tt-warn` when its position is stale):
//!
//! | Picked | Head | Sub rows |
//! |---|---|---|
//! | a ship ([`Tip::ship`]) | its name | its kind, ` · {bow + stern} m` with its dimensions; `MMSI {mmsi}`, ` · old position` when old |
//! | a train ([`Tip::train`]) | its name | its kind and operator joined by ` · `; `to {destination}` and its delay joined by ` · `, when either is there; the position's age |
//! | a signal ([`Tip::signal`]) | `Signal {name}` or `Main signal`, ` · Stop`, `Caution` or `Clear` | who is in the block after it or the next ones; `From where the trains are, not the signal itself` |
//! | an airport's code ([`Tip::airport`]) | the airport's name | `Open departures and arrivals` |
//!
//! The texts are worked out when the pick lands, as deck calls `getTooltip` at each hover pick:
//! the train's age is the one at that moment and stays until the pointer moves again.
//!
//! **When it shows** (the app's `state::pick` keeps a [`Shown`]): at each hover pick (the pointer
//! moved over the map with no button held) it shows what the pick landed on at the pointer, or
//! goes when nothing it describes is there (an aircraft, a place's name, nothing, the pointer off
//! the map); it goes when the camera has moved since the last frame (`onViewportChange`) and comes
//! back only at the next hover pick.
//!
//! **How it looks** ([`paint`]): its top left corner at the pointer (`translate(x, y)`, no offset
//! and no flipping at the edges; clipped to the map), as wide as its widest row; `--surface-raised`
//! with a 1 px `--line` border, radius `--radius-sm`, padding `--space-2` by `--space-3`; 12 px
//! Overpass at line height 1.45 in `--text-1`; the head 14 px at weight 650, sub rows in
//! `--text-2`, the warning in `--warn`; no shadow (`--shadow: none`). HTML's white space rules
//! apply to the texts (runs of spaces and line breaks are one space, none at either end; an empty
//! row takes no height); a row wider than the map wraps. It lies over the map and under every
//! panel, as today's (`.globe { isolation: isolate }`, the tooltip inside it).
//!
//! Deviation: the texts are drawn as they are, so nothing is HTML-escaped (`escape()` only keeps
//! them from being read as markup).

use std::sync::Arc;

use egui::epaint::text::Galley;
use egui::{CornerRadius, Id, LayerId, Order, Pos2, Rect, Stroke, StrokeKind, Vec2};
use rustradar_core::model::{Ship, Train};
use rustradar_core::ships::ship_kind_label;
use rustradar_core::trains::{delay_text, train_label, train_position_age};
use rustradar_scene::blocks::SignalTip;

use super::text::{self, Style};
use super::theme;

/// `.aircraft-tooltip`'s line height (`font: var(--text-sm) / 1.45`).
pub const LINE_HEIGHT: f32 = 1.45;
/// `.tt-head`'s weight.
pub const HEAD_WEIGHT: u16 = 650;
/// The padding (`--space-2` by `--space-3`).
pub const PADDING: Vec2 = Vec2::new(theme::SPACE_3, theme::SPACE_2);
/// What a train's signal tooltip adds under who is where.
pub const SIGNAL_NOTE: &str = "From where the trains are, not the signal itself";
/// An airport's tooltip under its name.
pub const AIRPORT_NOTE: &str = "Open departures and arrivals";

/// A row's class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// `.tt-head`: 14 px at weight 650 in `--text-1`.
    Head,
    /// `.tt-sub`: 12 px in `--text-2`.
    Sub,
    /// `.tt-sub.tt-warn`: 12 px in `--warn`.
    Warn,
}

impl Class {
    /// The text style of a row of this class.
    pub fn style(self) -> Style {
        let style = match self {
            Class::Head => Style::ui(theme::TEXT_BASE, HEAD_WEIGHT, theme::TEXT_1),
            Class::Sub => Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::TEXT_2),
            Class::Warn => Style::ui(theme::TEXT_SM, theme::WEIGHT_REGULAR, theme::WARN),
        };
        style.line_height(LINE_HEIGHT)
    }
}

/// One row (a `<div>` of the tooltip's HTML).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub class: Class,
    pub text: String,
}

/// What the tooltip says (the `html` `getTooltip` returns), row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tip {
    pub rows: Vec<Row>,
}

/// `[a, b].filter(Boolean).join(" · ")`.
fn joined<'a>(parts: impl IntoIterator<Item = Option<&'a str>>) -> String {
    parts.into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

impl Tip {
    fn new(rows: impl IntoIterator<Item = (Class, String)>) -> Tip {
        Tip { rows: rows.into_iter().map(|(class, text)| Row { class, text }).collect() }
    }

    /// A ship's (G:3383-3389): its name; its kind, with its length when its dimensions are known
    /// (bow plus stern); its MMSI, and ` · old position` when the position is old.
    pub fn ship(ship: &Ship) -> Tip {
        let length = ship.dims.map(|d| format!(" · {} m", u32::from(d[0]) + u32::from(d[1]))).unwrap_or_default();
        let old = if ship.old { " · old position" } else { "" };
        Tip::new([
            (Class::Head, ship.name.clone()),
            (Class::Sub, format!("{}{length}", ship_kind_label(ship.kind))),
            (Class::Sub, format!("MMSI {}{old}", ship.mmsi)),
        ])
    }

    /// A train's at local time `now_ms` (`Date.now()`, G:3391-3399): its name; its kind and
    /// operator; where to and how late, when either is known; how old its position is, in the
    /// warning colour when stale.
    pub fn train(train: &Train, now_ms: i64) -> Tip {
        let kind = joined([Some(train_label(train.kind)), train.operator.as_deref()]);
        let to = train.destination.as_deref().filter(|d| !d.is_empty()).map(|d| format!("to {d}"));
        let delay = delay_text(train.delay_minutes);
        let place = joined([to.as_deref(), Some(delay.as_str())]);
        let age = train_position_age(train.position_unix, now_ms);
        let mut rows = vec![(Class::Head, train.name.clone()), (Class::Sub, kind)];
        if !place.is_empty() {
            rows.push((Class::Sub, place));
        }
        rows.push((if age.stale { Class::Warn } else { Class::Sub }, age.label));
        Tip::new(rows)
    }

    /// A signal's (G:3401-3413), from the block layers' [`SignalTip`] (its name and aspect, and
    /// who is in the blocks it guards, as the trains are drawn).
    pub fn signal(tip: &SignalTip) -> Tip {
        Tip::new([(Class::Head, tip.head.clone()), (Class::Sub, tip.sub.clone()), (Class::Sub, SIGNAL_NOTE.to_owned())])
    }

    /// An airport code's (G:3414-3421): the airport's name (`nameOf`) and what a click does.
    pub fn airport(name: &str) -> Tip {
        Tip::new([(Class::Head, name.to_owned()), (Class::Sub, AIRPORT_NOTE.to_owned())])
    }

    /// The rows' texts as HTML lays them out (see [`collapse`]), one per line, without the rows
    /// that come out empty.
    pub fn text(&self) -> String {
        self.rows.iter().map(|r| collapse(&r.text)).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n")
    }
}

/// The tooltip shown: what it says and where the pointer was when it was picked (CSS px from the
/// map's top left, the window's points).
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub tip: Tip,
    pub at: [f64; 2],
}

/// HTML's `white-space: normal` on a row's text: every run of spaces, tabs and line breaks is one
/// space, and none is left at either end.
pub fn collapse(text: &str) -> String {
    text.split([' ', '\t', '\n', '\r', '\u{c}']).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// One row laid out: its galley and the height its line boxes take (CSS's: each line
/// [`LINE_HEIGHT`] times the size, where egui rounds a row's height to whole pixels).
#[derive(Debug, Clone)]
pub struct Laid {
    pub galley: Arc<Galley>,
    pub height: f32,
}

/// The tooltip's rows laid out in `ctx` within `max_width` (the map's width less the padding and
/// border), empty rows left out.
pub fn layout(ctx: &egui::Context, tip: &Tip, max_width: f32) -> Vec<Laid> {
    tip.rows
        .iter()
        .filter_map(|row| {
            let text = collapse(&row.text);
            if text.is_empty() {
                return None;
            }
            let style = row.class.style();
            let galley = ctx.fonts_mut(|f| f.layout_job(text::wrapped(&text, &style, max_width)));
            let height = galley.rows.len() as f32 * style.line_px();
            Some(Laid { galley, height })
        })
        .collect()
}

/// What [`paint`] drew, screen points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Painted {
    /// The box, border included.
    pub rect: Rect,
    /// Where the first row's line box starts.
    pub text: Pos2,
}

/// The border and padding round the rows, either way, points.
fn chrome() -> Vec2 {
    2.0 * (PADDING + Vec2::splat(theme::BORDER))
}

/// The layer the tooltip is painted on: over the map, under every panel.
pub fn layer() -> LayerId {
    LayerId::new(Order::Background, Id::unique("rustradar::map_tooltip"))
}

/// Paints `shown` over the map (the whole of `ctx`'s screen) and returns where. The rows follow
/// each other a line box apart from the padding's top; the box's edges are snapped to whole
/// pixels, as the browser paints a box whose height is a fraction.
pub fn paint(ctx: &egui::Context, shown: &Shown) -> Painted {
    let map = ctx.content_rect();
    let rows = layout(ctx, &shown.tip, (map.width() - chrome().x).max(0.0));
    let content = rows.iter().fold(Vec2::ZERO, |size, r| Vec2::new(size.x.max(r.galley.size().x), size.y + r.height));
    // its top left at the pointer
    let ppp = ctx.pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let min = Pos2::new(snap(map.min.x + shown.at[0] as f32), snap(map.min.y + shown.at[1] as f32));
    let max = min + content + chrome();
    let rect = Rect::from_min_max(min, Pos2::new(snap(max.x), snap(max.y)));
    let painter = ctx.layer_painter(layer()).with_clip_rect(map);
    painter.rect(rect, CornerRadius::same(theme::RADIUS_SM), theme::SURFACE_RAISED, Stroke::new(theme::BORDER, theme::LINE), StrokeKind::Inside);
    let text = min + PADDING + Vec2::splat(theme::BORDER);
    let mut y = text.y;
    for row in rows {
        // egui centres the text in its rounded row as CSS does in the line box
        let lift = 0.5 * (row.height - row.galley.size().y);
        painter.galley(Pos2::new(text.x, y + lift), row.galley, theme::TEXT_1);
        y += row.height;
    }
    Painted { rect, text }
}

/// The tooltip in the UI pass: `shown`, if there is one.
pub fn ui(ctx: &egui::Context, shown: Option<&Shown>) -> Option<Painted> {
    shown.map(|shown| paint(ctx, shown))
}

#[cfg(test)]
mod tests {
    use rustradar_core::model::{TrainKind, TrainSource};

    use super::*;

    /// 2026-06-21 10:00 UTC.
    const NOW: i64 = 1_782_036_000_000;

    fn ship() -> Ship {
        Ship { mmsi: 265_547_250, lat: 57.7, lon: 11.93, name: "STENA DANICA".into(), kind: 3, size: 2, heading: Some(10.0), old: false, dims: None, frames: Vec::new() }
    }

    fn train() -> Train {
        Train {
            id: "trafikverket:2026-06-21:523".into(),
            source: TrainSource::Trafikverket,
            name: "SJ Snabbtåg 523".into(),
            kind: TrainKind::HighSpeed,
            lat: 59.33,
            lon: 18.07,
            heading: Some(200.0),
            speed_kmh: Some(180.0),
            destination: Some("Göteborg C".into()),
            operator: Some("SJ".into()),
            delay_minutes: Some(4),
            position_unix: NOW / 1000 - 20,
            frames: Vec::new(),
            reports: Vec::new(),
            predicted: Vec::new(),
            cars: None,
            tail: Vec::new(),
            structures: Vec::new(),
            ended_unix: None,
            shape: None,
            model: None,
        }
    }

    fn rows(tip: &Tip) -> Vec<(Class, &str)> {
        tip.rows.iter().map(|r| (r.class, r.text.as_str())).collect()
    }

    #[test]
    fn a_ships_tooltip_names_it_its_kind_length_and_mmsi() {
        let mut s = ship();
        assert_eq!(rows(&Tip::ship(&s)), [(Class::Head, "STENA DANICA"), (Class::Sub, "Passenger"), (Class::Sub, "MMSI 265547250")]);
        s.dims = Some([180, 60, 14, 14]);
        s.old = true;
        s.kind = 9;
        assert_eq!(rows(&Tip::ship(&s)), [(Class::Head, "STENA DANICA"), (Class::Sub, "Other · 240 m"), (Class::Sub, "MMSI 265547250 · old position")]);
        // the sum of bow and stern past what a u16 holds; dimensions all 0 still count
        s.dims = Some([65_535, 2, 0, 0]);
        assert_eq!(Tip::ship(&s).rows[1].text, "Other · 65537 m");
        s.dims = Some([0; 4]);
        assert_eq!(Tip::ship(&s).rows[1].text, "Other · 0 m");
        // a ship that broadcast no name goes by its MMSI as its name
        s.name = "265547250".into();
        assert_eq!(Tip::ship(&s).rows[0].text, "265547250");
    }

    #[test]
    fn a_trains_tooltip_names_it_its_kind_operator_destination_delay_and_age() {
        let mut t = train();
        assert_eq!(
            rows(&Tip::train(&t, NOW)),
            [(Class::Head, "SJ Snabbtåg 523"), (Class::Sub, "High-speed · SJ"), (Class::Sub, "to Göteborg C · 4 min late"), (Class::Sub, "Position 20 s ago")]
        );
        // no operator, early, no destination
        t.operator = None;
        t.destination = None;
        t.delay_minutes = Some(-2);
        assert_eq!(rows(&Tip::train(&t, NOW))[1..3], [(Class::Sub, "High-speed"), (Class::Sub, "2 min early")]);
        // on time, empty strings count as none (`filter(Boolean)`)
        t.operator = Some(String::new());
        t.destination = Some(String::new());
        t.delay_minutes = Some(0);
        assert_eq!(rows(&Tip::train(&t, NOW))[1..3], [(Class::Sub, "High-speed"), (Class::Sub, "on time")]);
        // nothing known of where or when: no row for it
        t.delay_minutes = None;
        t.kind = TrainKind::Suburban;
        assert_eq!(rows(&Tip::train(&t, NOW)), [(Class::Head, "SJ Snabbtåg 523"), (Class::Sub, "Suburban"), (Class::Sub, "Position 20 s ago")]);
        t.destination = Some("Södertälje centrum".into());
        assert_eq!(rows(&Tip::train(&t, NOW))[2], (Class::Sub, "to Södertälje centrum"));
        // a stale position, in the warning colour; one reported ahead of this clock is no age
        t.position_unix = NOW / 1000 - 6 * 60;
        assert_eq!(*rows(&Tip::train(&t, NOW)).last().unwrap(), (Class::Warn, "Position 6 min old"));
        t.position_unix = NOW / 1000 + 30;
        assert_eq!(*rows(&Tip::train(&t, NOW)).last().unwrap(), (Class::Sub, "Position 0 s ago"));
        for kind in [TrainKind::Intercity, TrainKind::Regional, TrainKind::Other] {
            t.kind = kind;
            assert_eq!(Tip::train(&t, NOW).rows[1].text, train_label(kind));
        }
    }

    #[test]
    fn a_signals_and_an_airports_tooltips() {
        let tip = SignalTip { head: "Signal 27 · Stop".into(), sub: "Pendeltåg 2245 in the block after it".into() };
        assert_eq!(
            rows(&Tip::signal(&tip)),
            [(Class::Head, "Signal 27 · Stop"), (Class::Sub, "Pendeltåg 2245 in the block after it"), (Class::Sub, "From where the trains are, not the signal itself")]
        );
        assert_eq!(rows(&Tip::airport("Stockholm Arlanda Airport")), [(Class::Head, "Stockholm Arlanda Airport"), (Class::Sub, "Open departures and arrivals")]);
    }

    #[test]
    fn white_space_is_laid_out_as_html_lays_it_out() {
        assert_eq!(collapse("  SJ \t Snabbtåg\n523  "), "SJ Snabbtåg 523");
        assert_eq!(collapse(" \n "), "");
        // a no-break space stays
        assert_eq!(collapse("a\u{a0} b"), "a\u{a0} b");
        let mut t = train();
        t.name = "   ".into();
        assert_eq!(Tip::train(&t, NOW).text(), "High-speed · SJ\nto Göteborg C · 4 min late\nPosition 20 s ago");
        // markup is text
        assert_eq!(Tip::airport("<b>A & B</b>").text(), "<b>A & B</b>\nOpen departures and arrivals");
    }

    #[test]
    fn the_rows_styles() {
        let head = Class::Head.style();
        assert_eq!((head.size, head.weight, head.color, head.line_height), (14.0, 650, theme::TEXT_1, 1.45));
        let sub = Class::Sub.style();
        assert_eq!((sub.size, sub.weight, sub.color, sub.line_height), (12.0, 400, theme::TEXT_2, 1.45));
        let warn = Class::Warn.style();
        assert_eq!((warn.size, warn.weight, warn.color), (12.0, 400, theme::WARN));
        assert_eq!(chrome(), Vec2::new(26.0, 18.0));
    }

    /// Lays the tooltip out in a context with the app's fonts, painting it at `at`.
    fn paint_in(tip: &Tip, at: [f64; 2], screen: Vec2) -> (Painted, Vec<Vec2>) {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let shown = Shown { tip: tip.clone(), at };
        let mut out = None;
        // a first pass loads the fonts
        for _ in 0..2 {
            let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, screen)), ..Default::default() };
            let mut output = ctx.run_ui(raw, |ui| {
                let painted = paint(ui.ctx(), &shown);
                let sizes = layout(ui.ctx(), &shown.tip, screen.x - 26.0).iter().map(|r| Vec2::new(r.galley.size().x, r.height)).collect();
                out = Some((painted, sizes));
            });
            output.textures_delta.clear();
        }
        out.expect("painted")
    }

    #[test]
    fn the_box_starts_at_the_pointer_one_line_box_a_row_padded_and_bordered() {
        let tip = Tip::train(&train(), NOW);
        let (painted, rows) = paint_in(&tip, [300.4, 200.6], Vec2::new(1440.0, 900.0));
        assert_eq!(rows.len(), 4);
        // line boxes: 14 × 1.45 and 12 × 1.45
        assert!((rows[0].y - 20.3).abs() < 0.01 && rows[1..].iter().all(|r| (r.y - 17.4).abs() < 0.01), "{rows:?}");
        let widest = rows.iter().map(|r| r.x).fold(0.0, f32::max);
        assert_eq!(painted.rect.min, Pos2::new(300.0, 201.0), "at the pointer, on the pixel grid");
        assert!((painted.rect.width() - (widest + 26.0)).abs() <= 0.5, "{} for {widest}", painted.rect.width());
        // 90.5 px tall, its bottom edge on the next whole pixel
        assert_eq!(painted.rect.height(), 91.0, "{}", painted.rect.height());
        assert_eq!(painted.text, Pos2::new(313.0, 210.0));
        // the head row reads wider than its 12 px rows would: 14 px at 650
        assert!(widest > 100.0 && widest < 200.0, "{widest}");
        // near the right edge it does not move or flip
        let (edge, _) = paint_in(&tip, [1400.0, 880.0], Vec2::new(1440.0, 900.0));
        assert_eq!(edge.rect.min, Pos2::new(1400.0, 880.0));
        // a row wider than the map wraps
        let long = Tip::airport(&"Flygplats ".repeat(30));
        let (_, rows) = paint_in(&long, [0.0, 0.0], Vec2::new(400.0, 300.0));
        assert!(rows[0].x <= 400.0 - 26.0 && rows[0].y > 2.0 * 20.3 - 0.01, "{rows:?}");
    }
}
