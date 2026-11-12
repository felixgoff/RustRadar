//! `cargo xtask testflight-age`: how many days ago the app's newest build reached App Store Connect (and
//! so TestFlight, where a build expires after 90 days), from Apple's App Store Connect API with the same
//! API key the iOS job uploads with. A port of `scripts/testflight-age.mjs` (doc 07 §9.4, §15.1):
//!
//! ```text
//! APPLE_API_ISSUER=… APPLE_API_KEY_ID=… APPLE_API_KEY_PATH=AuthKey.p8 cargo xtask testflight-age
//! ```
//!
//! Prints the age in days (a very large number when there is no build yet) and, in GitHub Actions, sets
//! the step's `days` and `version` outputs. The bundle id is the app's identifier,
//! `rustradar_backend::APP_ID` (the script read `tauri.conf.json`'s `identifier`, which goes in stage 12;
//! a test keeps the two equal while it exists). Deviation: an `uploadedDate` that does not parse is an
//! error, where the script went on with `NaN` days.

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use rustradar_backend::APP_ID;
use rustradar_core::num::{js_string, to_fixed};
use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::Write;

const API: &str = "https://api.appstoreconnect.apple.com";

#[derive(Serialize)]
struct Header<'a> {
    alg: &'a str,
    kid: &'a str,
    typ: &'a str,
}

#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    iat: i64,
    exp: i64,
    aud: &'a str,
}

/// A token for the API: ES256, good for 15 minutes from `now` (Unix seconds), signed with the `.p8` key
/// (`pem`).
pub fn token(issuer: &str, key_id: &str, pem: &str, now: i64) -> Result<String, String> {
    let b64 = |bytes: &[u8]| URL_SAFE_NO_PAD.encode(bytes);
    let header = stringify(&Header { alg: "ES256", kid: key_id, typ: "JWT" });
    let claims = stringify(&Claims { iss: issuer, iat: now, exp: now + 900, aud: "appstoreconnect-v1" });
    let body = format!("{}.{}", b64(header.as_bytes()), b64(claims.as_bytes()));
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pem_der(pem)?)
        .map_err(|e| format!("the API key isn't a P-256 key in PKCS #8: {e}"))?;
    // IEEE P1363 (r ‖ s), as the script's `dsaEncoding: "ieee-p1363"`
    let signature = key.sign(&SystemRandom::new(), body.as_bytes()).map_err(|_| "could not sign the token".to_owned())?;
    Ok(format!("{body}.{}", b64(signature.as_ref())))
}

/// `JSON.stringify` of a plain object: compact, keys in their written order.
fn stringify(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("plain fields serialise")
}

/// The DER inside a PEM file: the base64 between its `-----BEGIN` and `-----END` lines.
fn pem_der(pem: &str) -> Result<Vec<u8>, String> {
    let body: String = pem
        .lines()
        .map(str::trim)
        .skip_while(|line| !line.starts_with("-----BEGIN"))
        .skip(1)
        .take_while(|line| !line.starts_with("-----END"))
        .collect();
    STANDARD.decode(body).map_err(|_| "the API key file isn't a PEM file".to_owned())
}

/// JavaScript's `encodeURIComponent`.
fn encode_uri_component(text: &str) -> String {
    text.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) { char::from(b).to_string() } else { format!("%{b:02X}") })
        .collect()
}

pub fn apps_path(bundle_id: &str) -> String {
    format!("/v1/apps?filter[bundleId]={}&limit=1", encode_uri_component(bundle_id))
}

pub fn builds_path(app_id: &str) -> String {
    format!("/v1/builds?filter[app]={app_id}&sort=-uploadedDate&limit=1&fields[builds]=uploadedDate,version")
}

/// A value as a JavaScript template literal shows it (`${v}`), absent as `undefined`.
fn js_text(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// What the builds answer says.
#[derive(Debug, PartialEq)]
pub struct Age {
    /// the line printed
    pub line: String,
    /// days since the newest upload; 1e6 when there is none
    pub days: f64,
    /// the newest build's version, or empty
    pub version: String,
}

/// The newest build's age at `now_ms` from the `/v1/builds` answer.
pub fn age(builds: &Value, now_ms: i64) -> Result<Age, String> {
    let newest = builds.get("data").and_then(|d| d.get(0)).and_then(|b| b.get("attributes")).filter(|a| !a.is_null());
    let uploaded = newest.and_then(|a| a.get("uploadedDate")).filter(|v| !matches!(v, Value::Null | Value::Bool(false)) && v.as_str() != Some(""));
    let days = match uploaded {
        Some(date) => {
            let text = js_text(Some(date));
            let at: jiff::Timestamp = text.parse().map_err(|e| format!("App Store Connect's uploadedDate {text}: {e}"))?;
            (now_ms - at.as_millisecond()) as f64 / 86_400_000.0
        }
        None => 1e6,
    };
    let line = match newest {
        Some(build) => format!(
            "newest build {}, uploaded {}: {} days ago",
            js_text(build.get("version")),
            js_text(build.get("uploadedDate")),
            to_fixed(days, 1)
        ),
        None => "no build uploaded yet".to_owned(),
    };
    let version = newest.and_then(|b| b.get("version")).filter(|v| !v.is_null()).map(|v| js_text(Some(v))).unwrap_or_default();
    Ok(Age { line, days, version })
}

/// The step outputs appended to `$GITHUB_OUTPUT`.
pub fn outputs(age: &Age) -> String {
    format!("days={}\nversion={}\n", js_string(age.days.floor()), age.version)
}

async fn get(http: &reqwest::Client, auth: &str, path: &str) -> Result<Value, String> {
    let answer = http.get(format!("{API}{path}")).header("Authorization", auth).send().await.map_err(|e| format!("App Store Connect: {e}"))?;
    let status = answer.status();
    let text = answer.text().await.map_err(|e| format!("App Store Connect: {e}"))?;
    if !status.is_success() {
        return Err(format!("App Store Connect answered {} for {path}: {text}", status.as_u16()));
    }
    serde_json::from_str(&text).map_err(|e| format!("App Store Connect's answer for {path}: {e}"))
}

pub fn run(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("usage: cargo xtask testflight-age   (with APPLE_API_ISSUER, APPLE_API_KEY_ID and APPLE_API_KEY_PATH set)".to_owned());
    }
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let (Some(issuer), Some(key_id), Some(key_path)) = (var("APPLE_API_ISSUER"), var("APPLE_API_KEY_ID"), var("APPLE_API_KEY_PATH")) else {
        eprintln!("needs APPLE_API_ISSUER, APPLE_API_KEY_ID and APPLE_API_KEY_PATH");
        std::process::exit(2);
    };
    let pem = fs::read_to_string(&key_path).map_err(|e| format!("{key_path}: {e}"))?;
    let auth = format!("Bearer {}", token(&issuer, &key_id, &pem, rustradar_backend::now_ms().div_euclid(1000))?);
    let http = reqwest::Client::builder().build().map_err(|e| format!("an HTTP client: {e}"))?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    let builds = runtime.block_on(async {
        let apps = get(&http, &auth, &apps_path(APP_ID)).await?;
        let app = apps.get("data").and_then(|d| d.get(0)).filter(|a| !a.is_null());
        let app = app.ok_or_else(|| format!("no app in App Store Connect with the bundle id {APP_ID}"))?;
        get(&http, &auth, &builds_path(&js_text(app.get("id")))).await
    })?;
    let age = age(&builds, rustradar_backend::now_ms())?;
    println!("{}", age.line);
    if let Some(file) = var("GITHUB_OUTPUT") {
        let mut out = fs::OpenOptions::new().create(true).append(true).open(&file).map_err(|e| format!("{file}: {e}"))?;
        out.write_all(outputs(&age).as_bytes()).map_err(|e| format!("{file}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED, KeyPair, UnparsedPublicKey};
    use serde_json::json;

    /// A fresh P-256 key as Apple's `.p8` files hold one. The PEM label is put together here so that a
    /// search of the sources for committed keys finds none.
    fn p8() -> (String, Vec<u8>) {
        let der = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new()).unwrap();
        let public = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, der.as_ref()).unwrap().public_key().as_ref().to_vec();
        let label = ["PRIVATE", "KEY"].join(" ");
        let body = STANDARD.encode(der.as_ref());
        let lines: Vec<&str> = body.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
        (format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n")), public)
    }

    #[test]
    fn tokens_are_es256_jwts_good_for_15_minutes() {
        let (pem, public) = p8();
        let token = token("an-issuer-id", "KEYID12345", &pem, 1_794_400_000).unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let text = |part: &str| String::from_utf8(URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap();
        assert_eq!(text(parts[0]), r#"{"alg":"ES256","kid":"KEYID12345","typ":"JWT"}"#);
        assert_eq!(
            text(parts[1]),
            r#"{"iss":"an-issuer-id","iat":1794400000,"exp":1794400900,"aud":"appstoreconnect-v1"}"#
        );
        assert!(!token.contains(['=', '+', '/']), "base64url without padding");
        let signature = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        assert_eq!(signature.len(), 64, "r and s, 32 bytes each");
        let signed = format!("{}.{}", parts[0], parts[1]);
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public).verify(signed.as_bytes(), &signature).unwrap();
        assert!(UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public).verify(b"other", &signature).is_err());
        assert!(super::token("i", "k", "not a key", 0).is_err());
    }

    #[test]
    fn asks_for_the_apps_newest_build() {
        assert_eq!(apps_path(APP_ID), "/v1/apps?filter[bundleId]=com.felix.rustradar&limit=1");
        assert_eq!(builds_path("1234567890"), "/v1/builds?filter[app]=1234567890&sort=-uploadedDate&limit=1&fields[builds]=uploadedDate,version");
        assert_eq!(encode_uri_component("a b/ü?&=,()"), "a%20b%2F%C3%BC%3F%26%3D%2C()");
    }

    #[test]
    fn the_bundle_id_is_tauri_confs_identifier() {
        let conf = crate::bump::repo_root().join("src-tauri/tauri.conf.json");
        let Ok(text) = fs::read_to_string(conf) else { return }; // gone after stage 12
        let conf: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(conf["identifier"], APP_ID);
    }

    #[test]
    fn ages_read_as_the_script_printed_them() {
        let now = 1_794_400_000_000; // 2026-11-11T12:26:40Z
        let builds = json!({ "data": [{ "type": "builds", "id": "b", "attributes": { "uploadedDate": "2026-09-01T05:00:00.000-07:00", "version": "202609011" } }] });
        let dated = age_of(builds, now);
        assert_eq!(dated.line, "newest build 202609011, uploaded 2026-09-01T05:00:00.000-07:00: 71.0 days ago");
        assert!((dated.days - 71.018_519).abs() < 1e-5, "{}", dated.days);
        assert_eq!(outputs(&dated), "days=71\nversion=202609011\n");

        let none = age_of(json!({ "data": [] }), now);
        assert_eq!(none, Age { line: "no build uploaded yet".into(), days: 1e6, version: String::new() });
        assert_eq!(outputs(&none), "days=1000000\nversion=\n");
        assert_eq!(age_of(json!({}), now).line, "no build uploaded yet");

        // a build without a date still counts as none uploaded, as `newest?.uploadedDate ? … : 1e6`
        let undated = age_of(json!({ "data": [{ "attributes": { "version": "7" } }] }), now);
        assert_eq!(undated.line, "newest build 7, uploaded undefined: 1000000.0 days ago");
        assert_eq!(outputs(&undated), "days=1000000\nversion=7\n");

        assert!(age(&json!({ "data": [{ "attributes": { "uploadedDate": "yesterday" } }] }), now).is_err());
    }

    fn age_of(builds: Value, now: i64) -> Age {
        age(&builds, now).unwrap()
    }
}
