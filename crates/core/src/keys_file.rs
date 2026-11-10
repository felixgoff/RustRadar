//! Port of `keys.ts` (S1.9): the file the keys page shares API keys and logins in.
//!
//! Storage stays in `backend::keys` (`keys.json` in the config folder): the TypeScript's
//! `loadKeys` and `saveKeys` become direct calls to it, and their localStorage branch is dropped.
//! `EMPTY_KEYS` is `Keys::default()`.

use std::fmt;

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;

pub use crate::model::Keys;

/// What a keys file says it is (`keys.ts:20`).
pub const FORMAT: &str = "rustradar-keys";

/// The keys' names in the file, in the order of [`Keys`] (`keys.ts:19` `FIELDS`).
pub const FIELDS: [&str; 6] =
    ["fr24Email", "fr24Password", "trafiklabGtfs", "trafiklabGtfsStatic", "trafiklabOxyfi", "trafikverket"];

/// The Flightradar24 login, which a file leaves out unless asked.
const LOGIN: [&str; 2] = ["fr24Email", "fr24Password"];

fn fields(keys: &Keys) -> [(&'static str, &str); 6] {
    [
        ("fr24Email", &keys.fr24_email),
        ("fr24Password", &keys.fr24_password),
        ("trafiklabGtfs", &keys.trafiklab_gtfs),
        ("trafiklabGtfsStatic", &keys.trafiklab_gtfs_static),
        ("trafiklabOxyfi", &keys.trafiklab_oxyfi),
        ("trafikverket", &keys.trafikverket),
    ]
}

fn fields_mut(keys: &mut Keys) -> [(&'static str, &mut String); 6] {
    [
        ("fr24Email", &mut keys.fr24_email),
        ("fr24Password", &mut keys.fr24_password),
        ("trafiklabGtfs", &mut keys.trafiklab_gtfs),
        ("trafiklabGtfsStatic", &mut keys.trafiklab_gtfs_static),
        ("trafiklabOxyfi", &mut keys.trafiklab_oxyfi),
        ("trafikverket", &mut keys.trafikverket),
    ]
}

/// The shareable file (`keys.ts:24`): `{ "format", "version": 1, "keys": { the keys that are set } }`,
/// pretty-printed with two spaces as `JSON.stringify(…, null, 2)` does, and a final newline.
/// Without `with_login`, the Flightradar24 login stays out of it.
pub fn keys_file(keys: &Keys, with_login: bool) -> String {
    #[derive(Serialize)]
    struct File<'a> {
        format: &'a str,
        version: u32,
        keys: IndexMap<&'a str, &'a str>,
    }
    let mut out = IndexMap::new();
    for (field, value) in fields(keys) {
        if !with_login && LOGIN.contains(&field) {
            continue;
        }
        if !value.is_empty() {
            out.insert(field, value);
        }
    }
    let file = File { format: FORMAT, version: 1, keys: out };
    serde_json::to_string_pretty(&file).expect("strings and an integer always serialise") + "\n"
}

/// Why a file could not be read as keys (`keys.ts` throws these messages).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeysFileError {
    /// Not JSON, another `format`, or `keys` not an object.
    NotAKeysFile,
    /// A keys file without a single key that is set.
    NoKeys,
}

impl fmt::Display for KeysFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            KeysFileError::NotAKeysFile => "That file isn't a RustRadar keys file.",
            KeysFileError::NoKeys => "That file has no keys in it.",
        })
    }
}

impl std::error::Error for KeysFileError {}

/// What [`read_keys_file`] read: the merged keys and how many the file gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadKeys {
    pub keys: Keys,
    pub count: usize,
}

/// `!s.trim()` in JavaScript: nothing but JavaScript's white space, which takes in U+FEFF and
/// leaves out U+0085, unlike `char::is_whitespace`.
fn js_blank(s: &str) -> bool {
    s.chars().all(|c| c == '\u{feff}' || (c != '\u{85}' && c.is_whitespace()))
}

/// Reads a keys file over `current` (`keys.ts:37`): what the file has replaces what's there,
/// what it leaves out stays. A key counts when it is a string with more than white space in it,
/// and is taken untrimmed (`backend::keys` trims on save).
///
/// `serde_json` refuses a few texts `JSON.parse` takes (numbers beyond `f64`, lone surrogate
/// escapes, nesting deeper than 128); those read as [`KeysFileError::NotAKeysFile`].
pub fn read_keys_file(text: &str, current: &Keys) -> Result<ReadKeys, KeysFileError> {
    let Ok(json) = serde_json::from_str::<Value>(text) else {
        return Err(KeysFileError::NotAKeysFile);
    };
    // `typeof keys === "object"` holds for an array too, which then has none of the names
    let given = match (json.get("format"), json.get("keys")) {
        (Some(Value::String(format)), Some(given @ (Value::Object(_) | Value::Array(_)))) if format == FORMAT => given,
        _ => return Err(KeysFileError::NotAKeysFile),
    };
    let mut keys = current.clone();
    let mut count = 0;
    for (field, slot) in fields_mut(&mut keys) {
        if let Some(Value::String(value)) = given.get(field)
            && !js_blank(value)
        {
            *slot = value.clone();
            count += 1;
        }
    }
    if count == 0 {
        return Err(KeysFileError::NoKeys);
    }
    Ok(ReadKeys { keys, count })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Keys {
        Keys {
            fr24_email: "me@example.com".to_owned(),
            fr24_password: "pw".to_owned(),
            trafiklab_gtfs: "gtfs".to_owned(),
            trafiklab_gtfs_static: String::new(),
            trafiklab_oxyfi: String::new(),
            trafikverket: "tv".to_owned(),
        }
    }

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn leaves_the_login_out_unless_asked() {
        let file = parse(&keys_file(&all(), false));
        assert_eq!(file["keys"], serde_json::json!({ "trafiklabGtfs": "gtfs", "trafikverket": "tv" }));
        assert_eq!(parse(&keys_file(&all(), true))["keys"]["fr24Password"], "pw");
    }

    #[test]
    fn round_trips_over_whats_there_keeping_what_the_file_lacks() {
        let current = Keys { fr24_email: "keep@example.com".to_owned(), trafikverket: "old".to_owned(), ..Keys::default() };
        let ReadKeys { keys, count } = read_keys_file(&keys_file(&all(), false), &current).unwrap();
        assert_eq!(count, 2);
        assert_eq!(keys, Keys { trafiklab_gtfs: "gtfs".to_owned(), trafikverket: "tv".to_owned(), ..current });
    }

    #[test]
    fn rejects_other_files() {
        let empty = Keys::default();
        let message = |text: &str| read_keys_file(text, &empty).unwrap_err().to_string();
        assert!(message("not json").contains("isn't a RustRadar keys file"));
        assert!(message(r#"{"keys":{}}"#).contains("isn't a RustRadar keys file"));
        assert!(message(r#"{"format":"rustradar-keys","keys":{"x":"1"}}"#).contains("no keys"));
    }

    #[test]
    fn the_file_is_written_as_keys_ts_writes_it() {
        // keysFile's output, printed by bun from keys.ts
        assert_eq!(
            keys_file(&all(), false),
            "{\n  \"format\": \"rustradar-keys\",\n  \"version\": 1,\n  \"keys\": {\n    \"trafiklabGtfs\": \"gtfs\",\n    \"trafikverket\": \"tv\"\n  }\n}\n"
        );
        assert_eq!(keys_file(&Keys::default(), true), "{\n  \"format\": \"rustradar-keys\",\n  \"version\": 1,\n  \"keys\": {}\n}\n");
        let odd = Keys {
            fr24_password: " \"quoted\"\\ ".to_owned(),
            trafiklab_oxyfi: "line\nbreak\ttab\u{1}ctl\u{7f}".to_owned(),
            trafikverket: "åäö \u{2028} 😀".to_owned(),
            ..Keys::default()
        };
        assert_eq!(
            keys_file(&odd, true),
            "{\n  \"format\": \"rustradar-keys\",\n  \"version\": 1,\n  \"keys\": {\n    \"fr24Password\": \" \\\"quoted\\\"\\\\ \",\n    \"trafiklabOxyfi\": \"line\\nbreak\\ttab\\u0001ctl\u{7f}\",\n    \"trafikverket\": \"åäö \u{2028} 😀\"\n  }\n}\n"
        );
    }

    #[test]
    fn reads_what_keys_ts_reads() {
        let current = Keys { trafikverket: "old".to_owned(), ..Keys::default() };
        let read = |text: &str| read_keys_file(text, &current);
        // U+0085 is not white space to JavaScript, U+FEFF is; the value is kept untrimmed
        let nel = read(r#"{"format":"rustradar-keys","keys":{"trafiklabGtfs":"\u0085","trafiklabOxyfi":"﻿","fr24Email":" x "}}"#);
        let want = Keys { fr24_email: " x ".to_owned(), trafiklab_gtfs: "\u{85}".to_owned(), ..current.clone() };
        assert_eq!(nel, Ok(ReadKeys { keys: want, count: 2 }));
        assert_eq!(read(r#"{"format":"rustradar-keys","keys":["a"]}"#), Err(KeysFileError::NoKeys));
        let nested = read(r#"{"format":"rustradar-keys","keys":{"trafikverket":1,"fr24Password":"p"},"version":7}"#);
        let want = Keys { fr24_password: "p".to_owned(), ..current.clone() };
        assert_eq!(nested, Ok(ReadKeys { keys: want, count: 1 }));
        assert_eq!(read("null"), Err(KeysFileError::NotAKeysFile));
        assert_eq!(read(r#""rustradar-keys""#), Err(KeysFileError::NotAKeysFile));
        assert_eq!(read(r#"{"format":"rustradar-keys","keys":null}"#), Err(KeysFileError::NotAKeysFile));
        assert_eq!(read(r#"[{"format":"rustradar-keys","keys":{"trafikverket":"x"}}]"#), Err(KeysFileError::NotAKeysFile));
        assert_eq!(read(r#"{"format":"rustradar-keys","keys":{"trafikverket":"   \t\n"}}"#), Err(KeysFileError::NoKeys));
    }
}
