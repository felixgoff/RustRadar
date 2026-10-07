//! Default HTTP headers and device fingerprinting.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

/// See upstream `fr24/proto/README.md`.
pub const PLATFORM_VERSION: &str = "25.061.0929";

pub const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:150.0) Gecko/20100101 Firefox/150.0";

/// Headers shared by all requests. `Accept-Encoding` is managed by reqwest.
const DEFAULT_HEADERS: &[(&str, &str)] = &[
    ("user-agent", USER_AGENT),
    (
        "accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
    ),
    ("accept-language", "en-US,en;q=0.5"),
    ("origin", "https://www.flightradar24.com"),
    ("referer", "https://www.flightradar24.com/"),
    ("sec-fetch-dest", "empty"),
    ("sec-fetch-mode", "cors"),
    ("sec-fetch-site", "same-site"),
    ("sec-gpc", "1"),
];

const GRPC_HEADERS: &[(&str, &str)] = &[
    ("accept", "*/*"),
    ("x-envoy-retry-grpc-on", "unavailable"),
    ("content-type", "application/grpc-web+proto"),
    ("x-user-agent", "grpc-web-javascript/0.1"),
    ("x-grpc-web", "1"),
    ("dnt", "1"),
];

fn header_map(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    extend(&mut map, pairs);
    map
}

fn extend(map: &mut HeaderMap, pairs: &[(&'static str, &'static str)]) {
    for (k, v) in pairs {
        map.insert(HeaderName::from_static(k), HeaderValue::from_static(v));
    }
}

/// Headers for the JSON endpoints (`api.flightradar24.com`, `www.flightradar24.com`).
pub fn default_headers() -> HeaderMap {
    header_map(DEFAULT_HEADERS)
}

/// Headers for the JSON endpoints, including the device ID.
pub fn json_headers(device_id: &str) -> HeaderMap {
    let mut map = default_headers();
    insert_device_id(&mut map, device_id);
    map
}

/// Headers for the gRPC endpoint (`data-feed.flightradar24.com`).
///
/// `access_token` is the bearer token from [`crate::auth::Authentication`].
pub fn grpc_headers(device_id: &str, access_token: Option<&str>) -> HeaderMap {
    let mut map = default_headers();
    extend(&mut map, GRPC_HEADERS);
    map.insert(
        HeaderName::from_static("fr24-platform"),
        HeaderValue::from_str(&format!("web-{PLATFORM_VERSION}")).expect("valid header"),
    );
    insert_device_id(&mut map, device_id);
    if let Some(token) = access_token
        && let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}"))
    {
        map.insert(reqwest::header::AUTHORIZATION, value);
    }
    map
}

fn insert_device_id(map: &mut HeaderMap, device_id: &str) {
    if let Ok(value) = HeaderValue::from_str(device_id) {
        map.insert(HeaderName::from_static("fr24-device-id"), value);
    }
}

const NANOID_ALPHABET: &[u8; 64] =
    b"useandom-26T198340PX75pxJACKVERYMINDBUSHWOLF_GQZbfghjklqvwyzrict";
const BASE32_DIGITS: &[u8; 32] = b"0123456789abcdefghijklmnopqrstuv";

/// Equivalent to JavaScript's `n.toString(32)` for non-negative integers.
pub fn jsnum_to_base32(mut n: u64) -> String {
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(BASE32_DIGITS[(n % 32) as usize]);
        n /= 32;
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

fn nanoid(size: usize) -> String {
    let random_bytes: Vec<u8> = (0..size).map(|_| rand::random::<u8>()).collect();
    // nanoid reads bytes in reverse: https://github.com/ai/nanoid/blob/main/non-secure/index.js
    random_bytes
        .iter()
        .rev()
        .map(|b| NANOID_ALPHABET[(b & 63) as usize] as char)
        .collect()
}

/// Generate a device ID in the same format as the web client:
/// `web-{base32(now_ms)}-{nanoid(21)}`.
pub fn generate_device_id(now_ms: u64) -> String {
    format!("web-{}-{}", jsnum_to_base32(now_ms), nanoid(21))
}

/// A process-wide device ID.
///
/// The web client generates the nanoid once and reuses it from
/// `localStorage`; since we do not persist to disk, we cache it for the
/// lifetime of the process to mimic that behaviour.
pub fn device_id() -> &'static str {
    static DEVICE_ID: OnceLock<String> = OnceLock::new();
    DEVICE_ID.get_or_init(|| generate_device_id(now_ms()))
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

pub(crate) fn now_s() -> i64 {
    (now_ms() / 1000) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_matches_js() {
        // (1758467194561).toString(32) in JavaScript
        assert_eq!(jsnum_to_base32(1758467194561), "1j5mcvvm1");
        assert_eq!(jsnum_to_base32(0), "0");
        assert_eq!(jsnum_to_base32(31), "v");
        assert_eq!(jsnum_to_base32(32), "10");
    }

    #[test]
    fn device_id_format() {
        let id = generate_device_id(1758467194561);
        let parts: Vec<_> = id.splitn(3, '-').collect();
        assert_eq!(parts[0], "web");
        assert_eq!(parts[1], "1j5mcvvm1");
        assert_eq!(parts[2].len(), 21);
    }
}
