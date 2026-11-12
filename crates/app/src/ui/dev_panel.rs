//! DevPanel (doc 06 §25; `DevPanel.svelte`), debug builds only: what the app asked of each API and
//! what came back (the backend's `devlog`), and the selected train as the backend made it. Opened
//! and closed by Ctrl/Cmd+Shift+D (`state.shortcuts.dev_open()`, U6) and its own `×`.
//!
//! **Layout** (`.dev.surface`, docked right: `top 0; right 0; bottom 0; width min(560px, 100%)`,
//! z 50; Overpass Mono 12 px / 1.4 throughout, `--text-1`):
//! - the header (`padding: 6px 8px`, items 6 apart, a 1 px `--line` rule under it): **Dev** (700),
//!   the tabs `API calls` and `Selected train` 8 further on, 2 apart (the open one `--text-1` with
//!   `--surface` text), and `×` (16 px, line height 1) at the right;
//! - every button, the select and the field: `--surface-raised`, a 1 px `--line` border, radius 4,
//!   `padding: 2px 8px`; no hover look, the default cursor;
//! - **API calls**: the tools row (the source select, as wide as its widest choice, with
//!   Chromium's dark arrow; the field `Search URL or data` taking the rest, its placeholder in
//!   Chromium's grey; `Pause`/`Resume`; `Clear`), then `No calls yet. They show up as the app makes
//!   them (dev builds only).` (`--text-3`, `padding: 8px`) while nothing is listed, and the calls
//!   newest first, scrolling: each a full-width row (`padding: 4px 8px`, `--surface-raised` under
//!   the pointer, a rule under each call) of the time (`--text-3`), the source (600), the status
//!   or `failed` (`--warn` for a failure or a status from 400), `{size} · {ms} ms` (`--text-3`),
//!   8 apart, over `{method} {url}` (`--text-2`, cut with an ellipsis, the URL as its tooltip).
//!   A click opens the call (one at a time; again closes it): the error (`--warn`), `Request` and
//!   `Response` (`--text-3`) each with `Copy` and its text laid out ([`pretty`]) in a block
//!   (`oklch(0.12 0 0)`, radius 4, `padding: 6px`, at most half the window high, scrolling,
//!   wrapping anywhere);
//! - **Selected train**: `Copy` and the train's id (or `Click a train on the map`, `--text-3`,
//!   `padding: 8px`), then its dump ([`train_dump`]) in a block filling the rest, 8 in.
//!
//! **Behaviour** (the component's script): while open and not paused, the calls recorded after the
//! last one held are fetched at once and every second (again at once after any arrive), the last
//! 400 kept ([`Panel::poll`]); the select and the field filter them ([`Panel::shown`]: the
//! source, then the text in the URL, the answer or the request, case folded); Clear forgets them
//! here and in the backend; Copy puts the raw text (or the dump) on the clipboard.
//!
//! **Order**: over everything, the update toast included (an `Order::Foreground` area brought to
//! the top each pass), except the modal dialogs: while one is open it is an `Order::Middle` area
//! under the dialog's backdrop ([`super::toast::order`]). The source list opens as its own area
//! above the panel (`Order::Tooltip`), as the browser's native list does.
//!
//! **Checked** against Chromium's layout of today's component (server-rendered by Svelte, open, its
//! rows, an opened call and the train tab built from its markup, laid out with app.css at 1440 ×
//! 900 and 390 × 844; `tests/dev_panel.rs`).
//!
//! Deviations:
//! - The blocks' text is Overpass Mono, as everything else in the panel; today's `<pre>` takes the
//!   browser's own `monospace` face (DejaVu Sans Mono on Linux, Consolas on Windows, Menlo on a
//!   Mac). It cannot be selected with the pointer (today's can); Copy copies it.
//! - The source list is drawn like the panel's controls rather than as the system's native list.
//! - A scroll bar floats over the content (egui's) rather than taking its width from the text.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use egui::epaint::PathStroke;
use egui::text::{LayoutJob, TextWrapping};
use egui::Galley;
use egui::{Area, Color32, Context, CornerRadius, CursorIcon, Id, LayerId, Order, Pos2, Rect, ScrollArea, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2};
use rustradar_backend::devlog::{self, Entry};
use rustradar_core::model::{Train, TrainRoute, TrainStop};
use rustradar_core::num::{js_string, to_fixed};

use super::fonts::Face;
use super::text::{self, Style};
use super::widgets::token_input::js_trim;
use super::widgets::{text_field, tooltip};
use super::{theme, toast};
use crate::state::shortcuts::Open;
use crate::state::{Action, AppState};

/// The header's title and tabs, and the close button (`×`, named `Close`).
pub const TITLE: &str = "Dev";
pub const TAB_CALLS: &str = "API calls";
pub const TAB_TRAIN: &str = "Selected train";
pub const CLOSE: &str = "×";
pub const CLOSE_LABEL: &str = "Close";
/// The API calls' tools.
pub const ALL_SOURCES: &str = "All sources";
pub const SEARCH: &str = "Search URL or data";
pub const PAUSE: &str = "Pause";
pub const RESUME: &str = "Resume";
pub const CLEAR: &str = "Clear";
/// With nothing listed.
pub const EMPTY: &str = "No calls yet. They show up as the app makes them (dev builds only).";
/// A call's status when it failed, and its opened parts.
pub const FAILED: &str = "failed";
pub const REQUEST: &str = "Request";
pub const RESPONSE: &str = "Response";
pub const COPY: &str = "Copy";
/// The train tab's hint without a train.
pub const NO_TRAIN: &str = "Click a train on the map";

/// Calls kept (`KEEP`), and how often they are asked for (`POLL_MS`).
pub const KEEP: usize = 400;
pub const POLL_MS: f64 = 1000.0;

/// `width: min(560px, 100%)`, of the content box.
pub const WIDTH: f32 = 560.0;
/// `font: 12px / 1.4 var(--font-data)`.
pub const FONT_SIZE: f32 = 12.0;
/// The close button's `font-size: 16px; line-height: 1`.
pub const CLOSE_FONT_SIZE: f32 = 16.0;
/// The header's and the tools' `padding: 6px 8px` and `gap: 6px`.
pub const BAR_PAD: Vec2 = Vec2::new(8.0, 6.0);
pub const BAR_GAP: f32 = 6.0;
/// The tabs' `margin-left: 8px` and `gap: 2px`.
pub const TABS_MARGIN: f32 = 8.0;
pub const TABS_GAP: f32 = 2.0;
/// Every control's `padding: 2px 8px` inside its 1 px border.
pub const CONTROL_PAD: Vec2 = Vec2::new(8.0, 2.0);
/// Chromium's dark `<select>` with that padding: 24 px high, and its width beyond its widest
/// choice (the padding, the border and the arrow; rounded up to a whole pixel).
pub const SELECT_HEIGHT: f32 = 24.0;
pub const SELECT_EXTRA: f32 = 38.0;
/// Where Chromium's dark `<select>` starts its text beyond its padding.
pub const SELECT_TEXT_INSET: f32 = 4.0;
/// The arrow's centre left of the select's right edge, and its half width and height.
const ARROW_INSET: f32 = 9.0;
const ARROW: Vec2 = Vec2::new(3.5, 2.0);
/// `.empty, .hint { padding: 8px }`.
pub const NOTE_PAD: f32 = 8.0;
/// A call's row: `padding: 4px 8px`, `gap: 2px 8px`.
pub const ROW_PAD: Vec2 = Vec2::new(8.0, 4.0);
pub const ROW_GAP: Vec2 = Vec2::new(8.0, 2.0);
/// An opened call: `padding: 0 8px 8px`; its labels' `margin: 6px 0 2px`; the blocks' `padding:
/// 6px` and `max-height: 50vh` (of the content box).
pub const DETAIL_PAD: f32 = 8.0;
pub const LABEL_MARGIN_TOP: f32 = 6.0;
pub const LABEL_MARGIN_BOTTOM: f32 = 2.0;
pub const PRE_PAD: f32 = 6.0;
pub const PRE_MAX_VH: f32 = 0.5;
/// The dump's `margin: 8px`.
pub const DUMP_MARGIN: f32 = 8.0;
/// Chromium's placeholder grey in a dark page (`::placeholder`, `#757575`).
pub const PLACEHOLDER: Color32 = Color32::from_rgb(117, 117, 117);

/// The panel's text: Overpass Mono 12 px / 1.4 at `weight` in `color` (no `.data` letter spacing).
pub const fn mono(weight: u16, color: Color32) -> Style {
    Style { face: Face::Mono, size: FONT_SIZE, weight, color, line_height: theme::LINE_HEIGHT, letter_spacing: 0.0 }
}

/// `text` in `style` on one row, as one run: the panel's face is Overpass Mono alone
/// (`--font-data`, without the UI face's "Middle Dot"), whose `·` takes a full cell, so the middle
/// dot is not set apart as [`text::job`] sets it.
pub fn job(text: &str, style: &Style) -> LayoutJob {
    let mut job = LayoutJob { wrap: TextWrapping::no_max_width(), break_on_newline: true, ..Default::default() };
    job.append(text, 0.0, style.format());
    job
}

/// [`job`] cut with an ellipsis at `width`.
fn truncated(text: &str, style: &Style, width: f32) -> LayoutJob {
    let mut job = job(text, style);
    text::truncate(&mut job, width);
    job
}

/// [`job`] wrapped at word boundaries within `width`.
fn wrapped(text: &str, style: &Style, width: f32) -> LayoutJob {
    let mut job = job(text, style);
    job.wrap = TextWrapping::wrap_at_width(width);
    job
}

/// A control's height: one line, its padding and border.
pub fn control_height() -> f32 {
    mono(400, theme::TEXT_1).line_px() + 2.0 * (CONTROL_PAD.y + theme::BORDER)
}

// ------------------------------------------------------------------------------------------------
// what the panel shows

/// The open tab (`tab`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Calls,
    Train,
}

/// Where the calls come from: the backend's record, or a test's.
pub trait DevLog: Send {
    /// `dev_log {since}`: the calls recorded after the call `since`, oldest first.
    fn since(&mut self, since: u64) -> Vec<Entry>;
    /// `dev_log_clear`.
    fn clear(&mut self);
}

/// The backend's record ([`devlog`]; empty in release builds).
struct BackendLog;

impl DevLog for BackendLog {
    fn since(&mut self, since: u64) -> Vec<Entry> {
        devlog::entries(Some(since))
    }

    fn clear(&mut self) {
        devlog::clear();
    }
}

/// The component's state, kept by egui between passes (and while the panel is closed, as the
/// component stays mounted).
#[derive(Default)]
pub struct Panel {
    log: Option<Box<dyn DevLog>>,
    /// The calls held, oldest first (`entries`).
    pub entries: Vec<Entry>,
    pub tab: Tab,
    /// The source chosen in the select; `None` is `All sources`.
    pub source: Option<String>,
    pub search: String,
    pub paused: bool,
    /// The opened call's id.
    pub expanded: Option<u64>,
    open: bool,
    next_poll: Option<f64>,
    /// The source list is open.
    choosing: bool,
    /// The listed calls' places in `entries`, and what they were worked out for.
    shown_for: Option<ShownKey>,
    shown_cache: Vec<usize>,
    /// The opened call's request and answer as laid out ([`pretty`]).
    opened: Option<Opened>,
    /// The viewer's IANA zone, the calls' times are read in (`AppState::viewer_clocks`; UTC when
    /// empty or unknown; item F1).
    pub zone: String,
}

/// What the listed calls depend on: the calls held (first id, last id, count), the source, the
/// search.
type ShownKey = (u64, u64, usize, Option<String>, String);

/// An opened call's id, and its request (when it has one) and answer laid out.
pub type Opened = Arc<(u64, Option<String>, String)>;

impl std::fmt::Debug for Panel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Panel")
            .field("entries", &self.entries.len())
            .field("tab", &self.tab)
            .field("source", &self.source)
            .field("search", &self.search)
            .field("paused", &self.paused)
            .field("expanded", &self.expanded)
            .finish_non_exhaustive()
    }
}

impl Panel {
    fn log(&mut self) -> &mut dyn DevLog {
        self.log.get_or_insert_with(|| Box::new(BackendLog)).as_mut()
    }

    /// The panel opens or closes; opening asks for the calls at once.
    pub fn set_open(&mut self, open: bool) {
        if open && !self.open {
            self.next_poll = None;
        }
        self.open = open;
    }

    /// The poll (the `$effect`): while open, at once and every [`POLL_MS`], skipped while paused;
    /// the calls after the last one held, the last [`KEEP`] kept; when any came, again at once.
    /// Returns when to look again.
    pub fn poll(&mut self, now_ms: f64) -> Option<f64> {
        if !self.open {
            return None;
        }
        if let Some(at) = self.next_poll
            && now_ms < at
        {
            return Some(at);
        }
        self.next_poll = Some(now_ms + POLL_MS);
        if !self.paused {
            let since = self.entries.last().map_or(0, |e| e.id);
            let fresh = self.log().since(since);
            if !fresh.is_empty() {
                self.entries.extend(fresh);
                let excess = self.entries.len().saturating_sub(KEEP);
                self.entries.drain(..excess);
                // `entries` changed: the effect starts over, polling at once
                self.next_poll = Some(now_ms);
            }
        }
        self.next_poll
    }

    /// `clear()`: the calls forgotten here and in the backend, none opened.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.expanded = None;
        self.log().clear();
    }

    /// The sources seen, sorted (`sources`).
    pub fn sources(&self) -> Vec<String> {
        let mut sources: Vec<String> = self.entries.iter().map(|e| e.source.clone()).collect();
        sources.sort();
        sources.dedup();
        sources
    }

    /// The calls listed, newest first (`shown`): of the chosen source, holding the trimmed search
    /// (case folded) in the URL, the answer or the request.
    pub fn shown(&self) -> Vec<&Entry> {
        self.filter().into_iter().map(|i| &self.entries[i]).collect()
    }

    /// [`Self::shown`] as places in `entries`.
    fn filter(&self) -> Vec<usize> {
        let q = js_trim(&self.search).to_lowercase();
        let mut shown: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                self.source.as_ref().is_none_or(|s| &e.source == s)
                    && (q.is_empty()
                        || e.url.to_lowercase().contains(&q)
                        || e.body.to_lowercase().contains(&q)
                        || e.request.as_ref().is_some_and(|r| r.to_lowercase().contains(&q)))
            })
            .collect();
        shown.reverse();
        shown
    }

    /// [`Self::filter`], worked out again only when the calls, the source or the search changed
    /// (the answers can be half a megabyte each).
    fn shown_indices(&mut self) -> Vec<usize> {
        let key = (self.entries.first().map_or(0, |e| e.id), self.entries.last().map_or(0, |e| e.id), self.entries.len(), self.source.clone(), self.search.clone());
        if self.shown_for.as_ref() != Some(&key) {
            self.shown_cache = self.filter();
            self.shown_for = Some(key);
        }
        self.shown_cache.clone()
    }

    /// The opened call's request and answer laid out ([`pretty`]), worked out once per call.
    fn opened(&mut self) -> Option<Opened> {
        let id = self.expanded?;
        if self.opened.as_ref().is_none_or(|o| o.0 != id) {
            let e = self.entries.iter().find(|e| e.id == id)?;
            self.opened = Some(Arc::new((id, e.request.as_deref().map(pretty), pretty(&e.body))));
        }
        self.opened.clone()
    }

    /// A click on a call's row: opened, or closed when it was.
    pub fn toggle(&mut self, id: u64) {
        self.expanded = if self.expanded == Some(id) { None } else { Some(id) };
    }
}

/// Whether a call failed (`e.error || (e.status ?? 200) >= 400`).
pub fn failed(e: &Entry) -> bool {
    e.error.is_some() || e.status.unwrap_or(200) >= 400
}

/// The status column: `failed` with an error, else the status (empty without one).
pub fn status(e: &Entry) -> String {
    if e.error.is_some() { FAILED.to_owned() } else { e.status.map(|s| s.to_string()).unwrap_or_default() }
}

/// `time(ms)`: `HH:MM:SS` (24-hour) in the viewer's IANA `zone` (`toLocaleTimeString(undefined,
/// { hour12: false })`); UTC for an empty or unknown zone.
pub fn time(at_ms: i64, zone: &str) -> String {
    let zone = jiff::tz::TimeZone::get(zone).unwrap_or(jiff::tz::TimeZone::UTC);
    let at = jiff::Timestamp::from_millisecond(at_ms).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    let wall = zone.to_datetime(at);
    format!("{:02}:{:02}:{:02}", wall.hour(), wall.minute(), wall.second())
}

/// `size(b)`: `{b} B`, `{kB with one decimal} kB` from 1024, `{MB} MB` from 1024².
pub fn size(bytes: usize) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{} MB", to_fixed(b / 1024.0 / 1024.0, 1))
    } else if b >= 1024.0 {
        format!("{} kB", to_fixed(b / 1024.0, 1))
    } else {
        format!("{bytes} B")
    }
}

/// The size column: `{size} · {ms} ms`.
pub fn size_and_time(e: &Entry) -> String {
    format!("{} · {} ms", size(e.size), e.ms)
}

/// `pretty(text)`: JSON laid out (`JSON.stringify(JSON.parse(t), null, 2)`), XML a tag per line
/// (`t.replace(/>\s*</g, ">\n<")`); anything else, and JSON that does not parse, as it came.
pub fn pretty(text: &str) -> String {
    let t = js_trim(text);
    if t.starts_with('{') || t.starts_with('[') {
        return json::parse(t).map_or_else(|| text.to_owned(), |v| json::stringify(&v));
    }
    if t.starts_with('<') {
        return tag_per_line(t);
    }
    text.to_owned()
}

/// JavaScript's `\s`: Unicode's white space without U+0085, and U+FEFF.
fn js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// `t.replace(/>\s*</g, ">\n<")`.
fn tag_per_line(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(at) = rest.find('>') {
        out.push_str(&rest[..=at]);
        let after = &rest[at + 1..];
        let spaced = after.trim_start_matches(js_space);
        if spaced.starts_with('<') {
            out.push('\n');
            rest = spaced;
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// The train tab's dump (`trainDump`): `JSON.stringify({train, stops, route}, null, 2)` with the
/// train's frames summed up (`frames: "{n} frames"`, the list as `framesList` at the end) and the
/// route's stretches as `{n} points, from {first} to {last}`.
pub fn train_dump(train: Option<&Train>, stops: Option<&[TrainStop]>, route: Option<&TrainRoute>) -> String {
    use json::Json;
    let train = train.map_or(Json::Null, |t| {
        let mut value = json::of(serde_json::to_string(t));
        if let Json::Object(fields) = &mut value {
            let list = fields.iter().find(|(k, _)| k == "frames").map_or(Json::Array(Vec::new()), |(_, v)| v.clone());
            json::set(fields, "frames", Json::String(format!("{} frames", t.frames.len())));
            json::set(fields, "framesList", list);
        }
        value
    });
    let stops = stops.map_or(Json::Null, |s| json::of(serde_json::to_string(s)));
    let route = route.map_or(Json::Null, |r| {
        let segments = r
            .segments
            .iter()
            .map(|s| {
                let end = |p: Option<&[f64; 2]>| p.map_or_else(|| "undefined".to_owned(), |[a, b]| format!("{},{}", js_string(*a), js_string(*b)));
                Json::String(format!("{} points, from {} to {}", s.len(), end(s.first()), end(s.last())))
            })
            .collect();
        Json::Object(vec![("complete".into(), Json::Bool(r.complete)), ("segments".into(), Json::Array(segments)), ("stops".into(), json::of(serde_json::to_string(&r.stops)))])
    });
    json::stringify(&Json::Object(vec![("train".into(), train), ("stops".into(), stops), ("route".into(), route)]))
}

/// JSON as JavaScript reads and writes it: numbers are doubles, an object's keys keep their order
/// (array indices first, ascending, as JavaScript orders an object's own keys), and
/// `JSON.stringify(v, null, 2)`'s layout.
pub mod json {
    use rustradar_core::num::js_string;

    /// A parsed value.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Json {
        Null,
        Bool(bool),
        Number(f64),
        String(String),
        Array(Vec<Json>),
        Object(Vec<(String, Json)>),
    }

    /// `JSON.parse(text)`, or `None` where it throws.
    pub fn parse(text: &str) -> Option<Json> {
        let mut p = Parser { s: text.as_bytes(), at: 0, text };
        p.space();
        let v = p.value(0)?;
        p.space();
        (p.at == p.s.len()).then_some(v)
    }

    /// A value serialised by the backend (`serde_json::to_string`) as JavaScript holds it after
    /// receiving it.
    pub fn of(serialized: serde_json::Result<String>) -> Json {
        serialized.ok().and_then(|s| parse(&s)).unwrap_or(Json::Null)
    }

    /// `object[key] = value`: replaced where the key is, else added at the end.
    pub fn set(fields: &mut Vec<(String, Json)>, key: &str, value: Json) {
        match fields.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => fields.push((key.to_owned(), value)),
        }
    }

    /// `JSON.stringify(value, null, 2)`.
    pub fn stringify(value: &Json) -> String {
        let mut out = String::new();
        write(value, 0, &mut out);
        out
    }

    fn write(value: &Json, depth: usize, out: &mut String) {
        let pad = |out: &mut String, depth: usize| out.extend(std::iter::repeat_n(' ', 2 * depth));
        match value {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Number(n) => out.push_str(&if n.is_finite() { js_string(*n) } else { "null".to_owned() }),
            Json::String(s) => quote(s, out),
            Json::Array(items) if items.is_empty() => out.push_str("[]"),
            Json::Object(fields) if fields.is_empty() => out.push_str("{}"),
            Json::Array(items) => {
                out.push_str("[\n");
                for (n, item) in items.iter().enumerate() {
                    if n > 0 {
                        out.push_str(",\n");
                    }
                    pad(out, depth + 1);
                    write(item, depth + 1, out);
                }
                out.push('\n');
                pad(out, depth);
                out.push(']');
            }
            Json::Object(fields) => {
                out.push_str("{\n");
                for (n, (key, item)) in fields.iter().enumerate() {
                    if n > 0 {
                        out.push_str(",\n");
                    }
                    pad(out, depth + 1);
                    quote(key, out);
                    out.push_str(": ");
                    write(item, depth + 1, out);
                }
                out.push('\n');
                pad(out, depth);
                out.push('}');
            }
        }
    }

    /// `JSON.stringify` of a string: `"` and `\` escaped, `\b \f \n \r \t`, other control
    /// characters as `\u00xx`.
    fn quote(s: &str, out: &mut String) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
    }

    /// Whether `key` is an array index (a canonical integer below 2³² − 1), which JavaScript lists
    /// first among an object's keys.
    fn array_index(key: &str) -> Option<u32> {
        let n: u32 = key.parse().ok()?;
        (n != u32::MAX && n.to_string() == key).then_some(n)
    }

    /// How deep values may nest (V8 stops far deeper; this keeps the stack safe).
    const MAX_DEPTH: usize = 512;

    struct Parser<'a> {
        s: &'a [u8],
        at: usize,
        text: &'a str,
    }

    impl Parser<'_> {
        fn space(&mut self) {
            while self.at < self.s.len() && matches!(self.s[self.at], b' ' | b'\t' | b'\n' | b'\r') {
                self.at += 1;
            }
        }

        fn eat(&mut self, b: u8) -> bool {
            let ok = self.s.get(self.at) == Some(&b);
            if ok {
                self.at += 1;
            }
            ok
        }

        fn literal(&mut self, word: &str, value: Json) -> Option<Json> {
            self.s[self.at..].starts_with(word.as_bytes()).then(|| {
                self.at += word.len();
                value
            })
        }

        fn value(&mut self, depth: usize) -> Option<Json> {
            if depth > MAX_DEPTH {
                return None;
            }
            match *self.s.get(self.at)? {
                b'{' => self.object(depth),
                b'[' => self.array(depth),
                b'"' => self.string().map(Json::String),
                b't' => self.literal("true", Json::Bool(true)),
                b'f' => self.literal("false", Json::Bool(false)),
                b'n' => self.literal("null", Json::Null),
                _ => self.number(),
            }
        }

        fn object(&mut self, depth: usize) -> Option<Json> {
            self.at += 1;
            let mut fields: Vec<(String, Json)> = Vec::new();
            self.space();
            if !self.eat(b'}') {
                loop {
                    self.space();
                    if self.s.get(self.at) != Some(&b'"') {
                        return None;
                    }
                    let key = self.string()?;
                    self.space();
                    if !self.eat(b':') {
                        return None;
                    }
                    self.space();
                    let value = self.value(depth + 1)?;
                    set(&mut fields, &key, value);
                    self.space();
                    if self.eat(b'}') {
                        break;
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            // array indices first, ascending; the rest in the order they came
            let (mut indices, rest): (Vec<_>, Vec<_>) = fields.into_iter().partition(|(k, _)| array_index(k).is_some());
            indices.sort_by_key(|(k, _)| array_index(k));
            indices.extend(rest);
            Some(Json::Object(indices))
        }

        fn array(&mut self, depth: usize) -> Option<Json> {
            self.at += 1;
            let mut items = Vec::new();
            self.space();
            if !self.eat(b']') {
                loop {
                    self.space();
                    items.push(self.value(depth + 1)?);
                    self.space();
                    if self.eat(b']') {
                        break;
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            Some(Json::Array(items))
        }

        fn hex4(&mut self) -> Option<u32> {
            let digits = self.text.get(self.at..self.at + 4)?;
            if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            self.at += 4;
            u32::from_str_radix(digits, 16).ok()
        }

        fn string(&mut self) -> Option<String> {
            self.at += 1;
            let mut out = String::new();
            loop {
                let start = self.at;
                while self.at < self.s.len() && !matches!(self.s[self.at], b'"' | b'\\') && self.s[self.at] >= 0x20 {
                    self.at += 1;
                }
                out.push_str(self.text.get(start..self.at)?);
                match *self.s.get(self.at)? {
                    b'"' => {
                        self.at += 1;
                        return Some(out);
                    }
                    b'\\' => {
                        self.at += 1;
                        let c = *self.s.get(self.at)?;
                        self.at += 1;
                        match c {
                            b'"' => out.push('"'),
                            b'\\' => out.push('\\'),
                            b'/' => out.push('/'),
                            b'b' => out.push('\u{8}'),
                            b'f' => out.push('\u{c}'),
                            b'n' => out.push('\n'),
                            b'r' => out.push('\r'),
                            b't' => out.push('\t'),
                            b'u' => {
                                let mut code = self.hex4()?;
                                // a surrogate pair is one character; a lone surrogate (which a
                                // JavaScript string can hold) is U+FFFD here
                                if (0xd800..0xdc00).contains(&code) && self.s[self.at..].starts_with(b"\\u") {
                                    let back = self.at;
                                    self.at += 2;
                                    match self.hex4() {
                                        Some(low) if (0xdc00..0xe000).contains(&low) => code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00),
                                        _ => self.at = back,
                                    }
                                }
                                out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                            }
                            _ => return None,
                        }
                    }
                    // a control character in a string
                    _ => return None,
                }
            }
        }

        fn number(&mut self) -> Option<Json> {
            let start = self.at;
            self.eat(b'-');
            let digits = |p: &mut Self| {
                let from = p.at;
                while p.at < p.s.len() && p.s[p.at].is_ascii_digit() {
                    p.at += 1;
                }
                p.at - from
            };
            if self.eat(b'0') {
                // no leading zeros
            } else if digits(self) == 0 {
                return None;
            }
            if self.eat(b'.') && digits(self) == 0 {
                return None;
            }
            if self.eat(b'e') || self.eat(b'E') {
                let _ = self.eat(b'+') || self.eat(b'-');
                if digits(self) == 0 {
                    return None;
                }
            }
            self.text[start..self.at].parse().ok().map(Json::Number)
        }
    }
}

// ------------------------------------------------------------------------------------------------
// state in egui

fn panel_id() -> Id {
    Id::unique("rustradar::dev_panel")
}

/// The panel's state in `ctx`.
pub fn panel(ctx: &Context) -> Arc<Mutex<Panel>> {
    ctx.data_mut(|d| d.get_temp_mut_or_insert_with(panel_id().with("state"), || Arc::new(Mutex::new(Panel::default()))).clone())
}

/// Hands the calls' list to `log` (tests).
pub fn install_log(ctx: &Context, log: Box<dyn DevLog>) {
    panel(ctx).lock().unwrap_or_else(PoisonError::into_inner).log = Some(log);
}

// ------------------------------------------------------------------------------------------------
// drawing

/// What the window's state says about the panel this pass.
#[derive(Debug, Clone, Copy)]
pub struct Shown<'a> {
    pub open: bool,
    /// The keys dialog or the shortcuts' help is open: the panel goes under it.
    pub dialog_open: bool,
    /// The local clock, Unix ms.
    pub now_ms: f64,
    /// The selected train, its stops and its route (`selectedTrain`, `liveStops`, `shownRoute`).
    pub train: Option<&'a Train>,
    pub stops: Option<&'a [TrainStop]>,
    pub route: Option<&'a TrainRoute>,
}

/// The panel for the window's state (the window's UI pass, last, debug builds only, as the page
/// mounts it only in Vite's dev builds).
pub fn ui(ctx: &Context, state: &AppState, now_ms: f64, actions: &mut Vec<Action>) {
    if !cfg!(debug_assertions) {
        return;
    }
    let s = &state.shortcuts;
    let shown = Shown {
        open: s.dev_open(),
        dialog_open: s.keys_open() || s.help_open(),
        now_ms,
        train: state.trains.selected(),
        stops: state.trains.live_stops(),
        route: state.trains.route(),
    };
    state.viewer_clocks.own_zone().clone_into(&mut panel(ctx).lock().unwrap_or_else(PoisonError::into_inner).zone);
    if show(ctx, shown).close {
        actions.push(Action::Open(Open::Dev, false));
    }
}

/// What [`show`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Answer {
    /// The panel's box, border included, when open.
    pub rect: Option<Rect>,
    /// `×` was clicked.
    pub close: bool,
}

/// Where the panel goes in a window `screen`: `min(560, width)` of content and the 1 px border,
/// against the right edge, top to bottom.
pub fn rect(screen: Rect) -> Rect {
    let width = WIDTH.min(screen.width()) + 2.0 * theme::BORDER;
    Rect::from_min_max(Pos2::new(screen.right() - width, screen.top()), screen.max)
}

/// Draws the panel as `shown` says and answers its controls.
pub fn show(ctx: &Context, shown: Shown<'_>) -> Answer {
    let state = panel(ctx);
    let mut p = state.lock().unwrap_or_else(PoisonError::into_inner);
    p.set_open(shown.open);
    if !shown.open {
        p.choosing = false;
        return Answer::default();
    }
    if let Some(at) = p.poll(shown.now_ms)
        && !p.paused
    {
        ctx.request_repaint_after(Duration::from_secs_f64(((at - shown.now_ms) / 1000.0).max(0.0)));
    }
    let screen = ctx.content_rect();
    let outer = rect(screen);
    let order = toast::order(shown.dialog_open);
    let id = panel_id().with("area");
    ctx.move_to_top(LayerId::new(order, id));
    let mut close = false;
    Area::new(id).order(order).fixed_pos(outer.min).fade_in(false).constrain(false).show(ctx, |ui: &mut Ui| {
        ui.painter().rect(outer, CornerRadius::same(theme::RADIUS), theme::SURFACE, Stroke::new(theme::BORDER, theme::LINE), StrokeKind::Inside);
        ui.allocate_rect(outer, Sense::hover());
        let inner = outer.shrink(theme::BORDER);
        let header_bottom = header(ui, &mut p, inner, &mut close);
        let body = Rect::from_min_max(Pos2::new(inner.left(), header_bottom), inner.max);
        match p.tab {
            Tab::Calls => calls(ui, &mut p, body, screen),
            Tab::Train => train(ui, body, &shown),
        }
    });
    Answer { rect: Some(outer), close }
}

/// A control's box in `rect`: `--surface-raised` (or `fill`), the 1 px `--line` border, radius 4.
fn paint_control(ui: &Ui, rect: Rect, fill: Color32) {
    ui.painter().rect(rect, CornerRadius::same(theme::RADIUS), fill, Stroke::new(theme::BORDER, theme::LINE), StrokeKind::Inside);
}

/// `text` in `style` with its left edge at `x`, centred on `centre_y`.
fn paint_text(ui: &Ui, x: f32, centre_y: f32, job: LayoutJob) -> Vec2 {
    let galley = ui.painter().layout_job(job);
    let size = galley.size();
    ui.painter().galley(Pos2::new(x, centre_y - size.y / 2.0), galley, Color32::PLACEHOLDER);
    size
}

fn text_width(ui: &Ui, text: &str, style: &Style) -> f32 {
    text::measure(ui.ctx(), job(text, style)).x
}

/// A button of the panel at `left`, centred on `centre_y`: its text in `style` on `fill`.
fn button(ui: &mut Ui, id: Id, left: f32, centre_y: f32, label: &str, style: &Style, fill: Color32) -> egui::Response {
    let height = style.line_px() + 2.0 * (CONTROL_PAD.y + theme::BORDER);
    let width = text_width(ui, label, style) + 2.0 * (CONTROL_PAD.x + theme::BORDER);
    let rect = Rect::from_min_size(Pos2::new(left, centre_y - height / 2.0), Vec2::new(width, height));
    let response = ui.interact(rect, id, Sense::click());
    paint_control(ui, rect, fill);
    paint_text(ui, rect.left() + theme::BORDER + CONTROL_PAD.x, rect.center().y, job(label, style));
    response
}

/// The width a button of `label` takes.
fn button_width(ui: &Ui, label: &str) -> f32 {
    text_width(ui, label, &mono(400, theme::TEXT_1)) + 2.0 * (CONTROL_PAD.x + theme::BORDER)
}

/// The header: title, tabs, close. Returns the y under its rule.
fn header(ui: &mut Ui, p: &mut Panel, inner: Rect, close: &mut bool) -> f32 {
    let row = control_height();
    let centre = inner.top() + BAR_PAD.y + row / 2.0;
    let bottom = inner.top() + 2.0 * BAR_PAD.y + row;
    ui.painter().hline(inner.x_range(), bottom + theme::BORDER / 2.0, Stroke::new(theme::BORDER, theme::LINE));
    let mut x = inner.left() + BAR_PAD.x;
    x += paint_text(ui, x, centre, job(TITLE, &mono(700, theme::TEXT_1))).x + BAR_GAP + TABS_MARGIN;
    for (tab, label) in [(Tab::Calls, TAB_CALLS), (Tab::Train, TAB_TRAIN)] {
        let on = p.tab == tab;
        let (fill, color) = if on { (theme::TEXT_1, theme::SURFACE) } else { (theme::SURFACE_RAISED, theme::TEXT_1) };
        if button(ui, panel_id().with(label), x, centre, label, &mono(400, color), fill).clicked() {
            p.tab = tab;
        }
        x += button_width(ui, label) + TABS_GAP;
    }
    let style = Style { size: CLOSE_FONT_SIZE, line_height: 1.0, ..mono(400, theme::TEXT_1) };
    let width = text_width(ui, CLOSE, &style) + 2.0 * (CONTROL_PAD.x + theme::BORDER);
    let response = button(ui, panel_id().with("close"), inner.right() - BAR_PAD.x - width, centre, CLOSE, &style, theme::SURFACE_RAISED);
    let response = tooltip_free(response, CLOSE_LABEL);
    *close |= response.clicked();
    bottom + theme::BORDER
}

/// A button whose name differs from its text (`aria-label`), without a tooltip (no `title`).
fn tooltip_free(response: egui::Response, label: &str) -> egui::Response {
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    response
}

/// The API calls tab in `body` (under the header).
fn calls(ui: &mut Ui, p: &mut Panel, body: Rect, screen: Rect) {
    let base = mono(400, theme::TEXT_1);
    let top = body.top();
    let bottom = top + 2.0 * BAR_PAD.y + SELECT_HEIGHT;
    ui.painter().hline(body.x_range(), bottom + theme::BORDER / 2.0, Stroke::new(theme::BORDER, theme::LINE));
    let centre = top + BAR_PAD.y + SELECT_HEIGHT / 2.0;
    // the select, as wide as its widest choice
    let sources = p.sources();
    let widest = std::iter::once(ALL_SOURCES).chain(sources.iter().map(String::as_str)).map(|s| text_width(ui, s, &base)).fold(0.0, f32::max);
    let select = Rect::from_min_size(Pos2::new(body.left() + BAR_PAD.x, top + BAR_PAD.y), Vec2::new((widest + SELECT_EXTRA).ceil(), SELECT_HEIGHT));
    let select_response = ui.interact(select, panel_id().with("select"), Sense::click());
    paint_control(ui, select, theme::SURFACE_RAISED);
    paint_text(ui, select.left() + theme::BORDER + CONTROL_PAD.x + SELECT_TEXT_INSET, centre, job(p.source.as_deref().unwrap_or(ALL_SOURCES), &base));
    let arrow = Pos2::new(select.right() - ARROW_INSET, centre);
    ui.painter().line(
        vec![arrow + Vec2::new(-ARROW.x, -ARROW.y), arrow + Vec2::new(0.0, ARROW.y), arrow + Vec2::new(ARROW.x, -ARROW.y)],
        PathStroke::new(1.5, theme::TEXT_1),
    );
    if select_response.clicked() {
        p.choosing = !p.choosing;
    }
    if p.choosing {
        choose_source(ui.ctx(), p, select, &sources);
    }
    // Clear and Pause at the right, the field taking what is left
    let clear_w = button_width(ui, CLEAR);
    let clear_x = body.right() - BAR_PAD.x - clear_w;
    if button(ui, panel_id().with("clear"), clear_x, centre, CLEAR, &base, theme::SURFACE_RAISED).clicked() {
        p.clear();
    }
    let pause = if p.paused { RESUME } else { PAUSE };
    let pause_x = clear_x - BAR_GAP - button_width(ui, pause);
    if button(ui, panel_id().with("pause"), pause_x, centre, pause, &base, theme::SURFACE_RAISED).clicked() {
        p.paused = !p.paused;
    }
    let field_h = control_height();
    let field = Rect::from_min_max(Pos2::new(select.right() + BAR_GAP, centre - field_h / 2.0), Pos2::new(pause_x - BAR_GAP, centre + field_h / 2.0));
    paint_control(ui, field, theme::SURFACE_RAISED);
    let edit = field.shrink2(Vec2::new(theme::BORDER + CONTROL_PAD.x, theme::BORDER + CONTROL_PAD.y));
    if p.search.is_empty() {
        // a field clips its placeholder (no ellipsis)
        let galley = ui.painter().layout_job(job(SEARCH, &base.color(PLACEHOLDER)));
        let at = Pos2::new(edit.left(), edit.center().y - galley.size().y / 2.0);
        ui.painter().with_clip_rect(edit.intersect(ui.clip_rect())).galley(at, galley, Color32::PLACEHOLDER);
    }
    text_field::edit(ui, panel_id().with("search"), &mut p.search, edit, &base, "", false);

    // the list, and the note while nothing is listed
    let mut list = Rect::from_min_max(Pos2::new(body.left(), bottom + theme::BORDER), body.max);
    let shown = p.shown_indices();
    let opened = p.opened();
    if shown.is_empty() {
        let note = mono(400, theme::TEXT_3);
        let job = wrapped(EMPTY, &note, list.width() - 2.0 * NOTE_PAD);
        let galley = ui.painter().layout_job(job);
        let height = galley.size().y + 2.0 * NOTE_PAD;
        ui.painter().galley(list.min + Vec2::splat(NOTE_PAD), galley, Color32::PLACEHOLDER);
        list.min.y += height;
    }
    let pre_max = PRE_MAX_VH * screen.height();
    let mut child = ui.new_child(UiBuilder::new().max_rect(list));
    child.set_clip_rect(list.intersect(ui.clip_rect()));
    let scroll = &mut child.spacing_mut().scroll;
    scroll.floating = true;
    scroll.fade.strength = 0.0;
    let mut clicked = None;
    ScrollArea::vertical().id_salt(panel_id().with("calls")).auto_shrink([false, false]).max_height(list.height()).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        for &i in &shown {
            let e = &p.entries[i];
            let open = opened.as_ref().filter(|o| o.0 == e.id);
            if call(ui, e, open, pre_max, &p.zone) {
                clicked = Some(e.id);
            }
        }
    });
    if let Some(id) = clicked {
        p.toggle(id);
    }
}

/// The source list under the select, over the panel: `All sources` and each source.
fn choose_source(ctx: &Context, p: &mut Panel, select: Rect, sources: &[String]) {
    let base = mono(400, theme::TEXT_1);
    let row = base.line_px() + 2.0 * CONTROL_PAD.y;
    let choices: Vec<Option<&str>> = std::iter::once(None).chain(sources.iter().map(|s| Some(s.as_str()))).collect();
    let list = Rect::from_min_size(select.left_bottom() + Vec2::new(0.0, 2.0), Vec2::new(select.width(), row * choices.len() as f32 + 2.0 * theme::BORDER));
    let mut chosen = None;
    let area = Area::new(panel_id().with("sources")).order(Order::Tooltip).fixed_pos(list.min).fade_in(false).constrain(false).show(ctx, |ui| {
        paint_control(ui, list, theme::SURFACE_RAISED);
        ui.allocate_rect(list, Sense::hover());
        for (n, choice) in choices.iter().enumerate() {
            let r = Rect::from_min_size(list.min + Vec2::new(theme::BORDER, theme::BORDER + row * n as f32), Vec2::new(list.width() - 2.0 * theme::BORDER, row));
            let response = ui.interact(r, panel_id().with(("source", n)), Sense::click());
            let current = *choice == p.source.as_deref();
            if response.hovered() || current {
                ui.painter().rect_filled(r, CornerRadius::ZERO, if response.hovered() { theme::SURFACE_HOVER } else { theme::SURFACE });
            }
            paint_text(ui, r.left() + CONTROL_PAD.x, r.center().y, truncated(choice.unwrap_or(ALL_SOURCES), &base, r.width() - 2.0 * CONTROL_PAD.x));
            if response.clicked() {
                chosen = Some(choice.map(str::to_owned));
            }
        }
    });
    if let Some(choice) = chosen {
        p.source = choice;
        p.choosing = false;
    } else if ctx.input(|i| i.pointer.any_pressed() || i.key_pressed(egui::Key::Escape))
        && !ctx.pointer_interact_pos().is_some_and(|at| area.response.rect.contains(at) || select.contains(at))
    {
        p.choosing = false;
    }
}

/// One call: its row, and its detail when it is the opened one. Returns whether its row was
/// clicked.
fn call(ui: &mut Ui, e: &Entry, opened: Option<&Opened>, pre_max: f32, zone: &str) -> bool {
    let line = mono(400, theme::TEXT_1).line_px();
    let width = ui.available_width();
    let (row, response) = ui.allocate_exact_size(Vec2::new(width, 2.0 * ROW_PAD.y + 2.0 * line + ROW_GAP.y), Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    if response.hovered() {
        ui.painter().rect_filled(row, CornerRadius::ZERO, theme::SURFACE_RAISED);
    }
    let x0 = row.left() + ROW_PAD.x;
    let first = row.top() + ROW_PAD.y + line / 2.0;
    let st = if failed(e) { theme::WARN } else { theme::TEXT_1 };
    let mut x = x0;
    for (text, style) in [
        (time(e.at_ms, zone), mono(400, theme::TEXT_3)),
        (e.source.clone(), mono(600, theme::TEXT_1)),
        (status(e), mono(400, st)),
        (size_and_time(e), mono(400, theme::TEXT_3)),
    ] {
        x += paint_text(ui, x, first, job(&text, &style)).x + ROW_GAP.x;
    }
    let second = first + line + ROW_GAP.y;
    let inner_w = width - 2.0 * ROW_PAD.x;
    paint_text(ui, x0, second, truncated(&format!("{} {}", e.method, e.url), &mono(400, theme::TEXT_2), inner_w));
    let url_rect = Rect::from_min_size(Pos2::new(x0, second - line / 2.0), Vec2::new(inner_w, line));
    if ui.rect_contains_pointer(url_rect) {
        tooltip::show(response.clone(), &e.url);
    }
    if let Some(opened) = opened {
        detail(ui, e, opened, inner_w, pre_max);
    }
    // the rule under the call
    let (rule, _) = ui.allocate_exact_size(Vec2::new(width, theme::BORDER), Sense::hover());
    ui.painter().rect_filled(rule, CornerRadius::ZERO, theme::LINE);
    response.clicked()
}

/// An opened call: the error, the request and the answer with their Copy buttons (the raw text),
/// each laid out in its block.
fn detail(ui: &mut Ui, e: &Entry, opened: &Opened, width: f32, pre_max: f32) {
    let left = ui.cursor().left() + DETAIL_PAD;
    if let Some(error) = &e.error {
        let job = wrapped(error, &mono(400, theme::WARN), width);
        let galley = ui.painter().layout_job(job);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), galley.size().y), Sense::hover());
        ui.painter().galley(Pos2::new(left, rect.top()), galley, Color32::PLACEHOLDER);
    }
    let request = e.request.as_deref().zip(opened.1.as_deref()).map(|(raw, shown)| (REQUEST, raw, shown));
    for (label, raw, shown) in request.into_iter().chain(std::iter::once((RESPONSE, e.body.as_str(), opened.2.as_str()))) {
        ui.add_space(LABEL_MARGIN_TOP);
        let height = control_height();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
        let label_w = paint_text(ui, left, rect.center().y, job(label, &mono(400, theme::TEXT_3))).x;
        if button(ui, panel_id().with((e.id, label)), left + label_w + BAR_GAP, rect.center().y, COPY, &mono(400, theme::TEXT_1), theme::SURFACE_RAISED).clicked() {
            ui.ctx().copy_text(raw.to_owned());
        }
        ui.add_space(LABEL_MARGIN_BOTTOM);
        block(ui, (e.id, label), left, width, shown, Some(pre_max));
    }
    ui.add_space(DETAIL_PAD);
}

/// `galley`'s rows placed as Chromium places line boxes: every `line` px from `top`, each on the
/// nearest whole pixel (epaint puts them a whole number of pixels apart: 17 for 16.8, which adds
/// up over a long block).
fn css_rows(galley: &Arc<Galley>, top: f32, line: f32) -> Arc<Galley> {
    let mut g = (**galley).clone();
    for (n, row) in g.rows.iter_mut().enumerate() {
        row.pos.y = (top + n as f32 * line).round() - top;
    }
    Arc::new(g)
}

/// The height of `galley`'s rows of `text` at `line` px each (no text, and a last empty row after
/// a final newline, make no line box).
fn css_height(galley: &Galley, text: &str, line: f32) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let rows = galley.rows.len() - usize::from(text.ends_with('\n') && galley.rows.len() > 1);
    rows as f32 * line
}

/// `text` laid out as a `<pre>`'s (`white-space: pre-wrap; word-break: break-all`) within `width`.
fn pre_galley(ui: &Ui, text: &str, width: f32) -> Arc<Galley> {
    let mut job = job(text, &mono(400, theme::TEXT_1));
    job.wrap = TextWrapping { max_width: width, max_rows: usize::MAX, break_anywhere: true, overflow_character: None };
    ui.painter().layout_job(job)
}

/// A `<pre>` block at `left`, `width` wide: the text on `oklch(0.12 0 0)`, wrapping anywhere,
/// scrolling past `max_height` of content (none: as tall as the text).
fn block(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, left: f32, width: f32, text: &str, max_height: Option<f32>) -> Rect {
    let line = mono(400, theme::TEXT_1).line_px();
    let galley = pre_galley(ui, text, width - 2.0 * PRE_PAD);
    let content = css_height(&galley, text, line);
    let shown = max_height.map_or(content, |m| content.min(m));
    let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), shown + 2.0 * PRE_PAD), Sense::hover());
    let rect = Rect::from_min_size(Pos2::new(left, row.top()), Vec2::new(width, row.height()));
    ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS), theme::DEV_PANEL_CODE_BG);
    let inner = rect.shrink(PRE_PAD);
    if content <= shown + 0.5 {
        ui.painter().galley(inner.min, css_rows(&galley, inner.min.y, line), Color32::PLACEHOLDER);
        return rect;
    }
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner));
    child.set_clip_rect(inner.intersect(ui.clip_rect()));
    child.spacing_mut().scroll.floating = true;
    ScrollArea::vertical().id_salt(panel_id().with(("pre", salt))).auto_shrink([false, false]).max_height(inner.height()).show(&mut child, |ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::new(galley.size().x, content), Sense::hover());
        ui.painter().galley(r.min, css_rows(&galley, r.min.y, line), Color32::PLACEHOLDER);
    });
    rect
}

/// The selected train tab in `body`.
fn train(ui: &mut Ui, body: Rect, shown: &Shown<'_>) {
    let base = mono(400, theme::TEXT_1);
    let note = mono(400, theme::TEXT_3);
    let top = body.top();
    let hint_h = note.line_px() + 2.0 * NOTE_PAD;
    let bottom = top + 2.0 * BAR_PAD.y + hint_h;
    ui.painter().hline(body.x_range(), bottom + theme::BORDER / 2.0, Stroke::new(theme::BORDER, theme::LINE));
    let centre = top + BAR_PAD.y + hint_h / 2.0;
    let dump = train_dump(shown.train, shown.stops, shown.route);
    let copy = button(ui, panel_id().with("copy-train"), body.left() + BAR_PAD.x, centre, COPY, &base, theme::SURFACE_RAISED);
    if copy.clicked() {
        ui.ctx().copy_text(dump.clone());
    }
    let hint_x = body.left() + BAR_PAD.x + button_width(ui, COPY) + BAR_GAP + NOTE_PAD;
    paint_text(ui, hint_x, centre, job(shown.train.map_or(NO_TRAIN, |t| t.id.as_str()), &note));
    // the dump fills the rest, 8 in
    let area = Rect::from_min_max(Pos2::new(body.left() + DUMP_MARGIN, bottom + theme::BORDER + DUMP_MARGIN), body.max - Vec2::splat(DUMP_MARGIN));
    ui.painter().rect_filled(area, CornerRadius::same(theme::RADIUS), theme::DEV_PANEL_CODE_BG);
    let inner = area.shrink(PRE_PAD);
    let line = base.line_px();
    let galley = pre_galley(ui, &dump, inner.width());
    let content = css_height(&galley, &dump, line);
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner));
    child.set_clip_rect(inner.intersect(ui.clip_rect()));
    child.spacing_mut().scroll.floating = true;
    ScrollArea::vertical().id_salt(panel_id().with("dump")).auto_shrink([false, false]).max_height(inner.height()).show(&mut child, |ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::new(galley.size().x, content), Sense::hover());
        ui.painter().galley(r.min, css_rows(&galley, r.min.y, line), Color32::PLACEHOLDER);
    });
}

#[cfg(test)]
mod tests {
    use super::json::Json;
    use super::*;

    fn entry(id: u64, source: &str, url: &str, status: Option<u16>, error: Option<&str>, body: &str) -> Entry {
        Entry {
            id,
            at_ms: 1_782_036_000_000 + id as i64 * 1000,
            source: source.into(),
            method: "GET".into(),
            url: url.into(),
            request: None,
            status,
            ms: 12,
            size: body.len(),
            body: body.into(),
            error: error.map(Into::into),
        }
    }

    /// What the fake log holds: its calls, the `since` it was asked with, how often it was cleared.
    type Held = (Vec<Entry>, Vec<u64>, usize);

    /// A log with calls to hand out, and what was asked of it.
    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<Held>>);

    impl DevLog for Fake {
        fn since(&mut self, since: u64) -> Vec<Entry> {
            let mut s = self.0.lock().unwrap();
            s.1.push(since);
            s.0.iter().filter(|e| e.id > since).cloned().collect()
        }

        fn clear(&mut self) {
            let mut s = self.0.lock().unwrap();
            s.0.clear();
            s.2 += 1;
        }
    }

    #[test]
    fn json_is_laid_out_as_json_stringify_does() {
        assert_eq!(
            pretty(r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[18.07,59.33,28]},"properties":{"meta":{"updated_at":"2026-06-21T09:30:12Z"}}}"#),
            "{\n  \"type\": \"Feature\",\n  \"geometry\": {\n    \"type\": \"Point\",\n    \"coordinates\": [\n      18.07,\n      59.33,\n      28\n    ]\n  },\n  \"properties\": {\n    \"meta\": {\n      \"updated_at\": \"2026-06-21T09:30:12Z\"\n    }\n  }\n}"
        );
        // numbers as JavaScript prints them; empty containers on one line; the trimmed text parsed
        assert_eq!(pretty(" [1.0, -0, 1e21, 1E-7, 12345678901234567890, {}, [], true, null] "), "[\n  1,\n  0,\n  1e+21,\n  1e-7,\n  12345678901234567000,\n  {},\n  [],\n  true,\n  null\n]");
        // array indices first, ascending; a repeated key keeps its first place with its last value
        assert_eq!(pretty(r#"{"b":1,"10":2,"a":3,"2":4,"b":5,"01":6}"#), "{\n  \"2\": 4,\n  \"10\": 2,\n  \"b\": 5,\n  \"a\": 3,\n  \"01\": 6\n}");
        // strings escaped as JSON.stringify escapes them
        assert_eq!(pretty(r#"["a\"b\\c\/d\u0001\né🚀"]"#), "[\n  \"a\\\"b\\\\c/d\\u0001\\né🚀\"\n]");
        assert_eq!(pretty("[1e400]"), "[\n  null\n]", "Infinity is written as null");
    }

    #[test]
    fn what_does_not_parse_comes_back_as_it_came() {
        for text in ["{\"a\":1,}", "[01]", "{'a':1}", "[1] x", "[\"tab\tinside\"]", "[-]", "[1.]", "{\"a\" 1}"] {
            assert_eq!(pretty(text), text, "{text}");
        }
        assert_eq!(pretty("plain text"), "plain text");
        assert_eq!(pretty("  not json  "), "  not json  ", "only the JSON and XML branches trim");
    }

    #[test]
    fn xml_gets_a_tag_per_line() {
        assert_eq!(
            pretty("  <REQUEST><LOGIN authenticationkey=\"•••\"/>\n   <QUERY objecttype=\"TrainAnnouncement\"> <FILTER/></QUERY></REQUEST> "),
            "<REQUEST>\n<LOGIN authenticationkey=\"•••\"/>\n<QUERY objecttype=\"TrainAnnouncement\">\n<FILTER/>\n</QUERY>\n</REQUEST>"
        );
        assert_eq!(pretty("<a>text</a>"), "<a>text</a>", "text between tags stays");
    }

    #[test]
    fn sizes_times_and_statuses() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1024), "1.0 kB");
        assert_eq!(size(49_357), "48.2 kB");
        assert_eq!(size(1_363_149), "1.3 MB");
        assert_eq!(time(1_782_036_007_000, "UTC"), "10:00:07");
        assert_eq!(time(1_782_000_000_999, ""), "00:00:00");
        // in the viewer's zone (F1): Stockholm's summer time, New York's
        assert_eq!(time(1_782_036_007_000, "Europe/Stockholm"), "12:00:07");
        assert_eq!(time(1_782_036_007_000, "America/New_York"), "06:00:07");
        let ok = entry(1, "A", "u", Some(200), None, "");
        let not_found = entry(2, "A", "u", Some(404), None, "");
        let broken = entry(3, "A", "u", None, Some("timed out"), "");
        let none = entry(4, "A", "u", None, None, "");
        assert_eq!((failed(&ok), status(&ok)), (false, "200".to_owned()));
        assert_eq!((failed(&not_found), status(&not_found)), (true, "404".to_owned()));
        assert_eq!((failed(&broken), status(&broken)), (true, FAILED.to_owned()));
        assert_eq!((failed(&none), status(&none)), (false, String::new()));
        assert_eq!(size_and_time(&ok), "0 B · 12 ms");
    }

    #[test]
    fn polls_at_once_every_second_and_not_while_paused() {
        let fake = Fake::default();
        let mut p = Panel { log: Some(Box::new(fake.clone())), ..Default::default() };
        assert_eq!(p.poll(0.0), None, "closed: nothing asked");
        fake.0.lock().unwrap().0.push(entry(1, "Trafikverket", "https://x/1", Some(200), None, "{}"));
        p.set_open(true);
        assert_eq!(p.poll(0.0), Some(0.0), "asked at once; something came: again at once");
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.poll(10.0), Some(1010.0), "nothing new: a second later");
        assert_eq!(p.poll(500.0), Some(1010.0), "not before");
        assert_eq!(fake.0.lock().unwrap().1, vec![0, 1], "after the last held");
        p.paused = true;
        fake.0.lock().unwrap().0.push(entry(2, "Trafikverket", "https://x/2", Some(200), None, "{}"));
        p.poll(1010.0);
        assert_eq!(p.entries.len(), 1, "paused");
        p.paused = false;
        p.poll(2010.0);
        assert_eq!(p.entries.len(), 2);
        // closed and opened again: at once
        p.set_open(false);
        assert_eq!(p.poll(2100.0), None);
        p.set_open(true);
        p.poll(2200.0);
        assert_eq!(*fake.0.lock().unwrap().1.last().unwrap(), 2);
    }

    #[test]
    fn keeps_the_last_400_and_clear_forgets_them_here_and_there() {
        let fake = Fake::default();
        fake.0.lock().unwrap().0 = (1..=450).map(|n| entry(n, "S", "u", Some(200), None, "")).collect();
        let mut p = Panel { log: Some(Box::new(fake.clone())), ..Default::default() };
        p.set_open(true);
        p.poll(0.0);
        assert_eq!(p.entries.len(), KEEP);
        assert_eq!(p.entries[0].id, 51);
        p.expanded = Some(60);
        p.clear();
        assert!(p.entries.is_empty() && p.expanded.is_none());
        assert_eq!(fake.0.lock().unwrap().2, 1, "the backend's record cleared");
    }

    #[test]
    fn filters_by_source_and_text_newest_first() {
        let mut p = Panel {
            entries: vec![
                entry(1, "Trafikverket", "https://api.trafikinfo.trafikverket.se/v2/data.json", Some(200), None, "{\"RESPONSE\":1}"),
                entry(2, "VesselFinder", "https://www.vesselfinder.com/api/pub/vesselsonmap", None, Some("x"), ""),
                entry(3, "Trafikverket", "https://api.trafikinfo.trafikverket.se/v2/data.json", Some(200), None, "Stockholm C"),
            ],
            ..Default::default()
        };
        p.entries[1].request = Some("<QUERY>Malmö</QUERY>".into());
        assert_eq!(p.sources(), ["Trafikverket", "VesselFinder"]);
        let ids = |p: &Panel| p.shown().iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(ids(&p), [3, 2, 1]);
        p.source = Some("Trafikverket".into());
        assert_eq!(ids(&p), [3, 1]);
        p.search = "  STOCKHOLM ".into();
        assert_eq!(ids(&p), [3], "the answer, case folded, trimmed");
        p.source = None;
        p.search = "malmö".into();
        assert_eq!(ids(&p), [2], "the request");
        p.search = "vesselsonmap".into();
        assert_eq!(ids(&p), [2], "the URL");
        p.toggle(2);
        assert_eq!(p.expanded, Some(2));
        p.toggle(2);
        assert_eq!(p.expanded, None);
    }

    #[test]
    fn the_train_dump() {
        assert_eq!(train_dump(None, None, None), "{\n  \"train\": null,\n  \"stops\": null,\n  \"route\": null\n}");
        let train = crate::dev::demo_trains(59.33, 18.07, 1_782_036_000_000).remove(0);
        let route = TrainRoute { segments: vec![vec![[18.07, 59.0], [18.1, 59.5]], vec![]], stops: Vec::new(), complete: false };
        let stops = vec![TrainStop { name: "Stockholm C".into(), ..Default::default() }];
        let dump = train_dump(Some(&train), Some(&stops), Some(&route));
        let Some(Json::Object(top)) = json::parse(&dump) else { panic!("{dump}") };
        let keys: Vec<&str> = top.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["train", "stops", "route"]);
        let Json::Object(t) = &top[0].1 else { panic!() };
        assert_eq!(t[0].0, "id", "the train's own order");
        assert_eq!(t.last().unwrap().0, "framesList", "the list at the end");
        let frames = t.iter().find(|(k, _)| k == "frames").unwrap();
        assert_eq!(frames.1, Json::String(format!("{} frames", train.frames.len())));
        assert!(dump.contains("\"segments\": [\n      \"2 points, from 18.07,59 to 18.1,59.5\",\n      \"0 points, from undefined to undefined\"\n    ]"), "{dump}");
        assert!(dump.contains("\"name\": \"Stockholm C\""));
        assert!(dump.contains("\"complete\": false"));
    }
}
