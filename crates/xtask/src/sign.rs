//! `cargo xtask sign <file>...`: updater signatures as the Tauri CLI writes them (doc 07 §7.2, §12.1,
//! §12.2), so that copies installed before the port keep accepting updates. For each file, `<file>.sig`:
//!
//! - the key is `TAURI_SIGNING_PRIVATE_KEY` (the key file's text, base64 of a minisign secret key box, or
//!   a path to that file) with `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (an empty password when unset), as
//!   the CLI reads them;
//! - the signature is minisign's pre-hashed one (`ED`), untrusted comment
//!   `signature from tauri secret key`, trusted comment `timestamp:<unix s>\tfile:<name>\tversion:<v>`,
//!   as in v0.3.15's published `.sig` files (the version is the files' agreed one, [`bump::current`]);
//! - the `.sig` file holds base64 of the signature box's text (which ends in a newline), with no newline
//!   of its own: `latest.json` carries it as it is.
//!
//! Nothing here prints the key or its password. [`public_key`] and [`verify`] read Tauri's public key and
//! check a signature as the updater does; `latest-json --pubkey` uses them.

use crate::bump;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use minisign::{PublicKey, SecretKey, SecretKeyBox, SignatureBox};
use std::fs;
use std::io::{Read, Seek};
use std::path::Path;

pub const KEY_VAR: &str = "TAURI_SIGNING_PRIVATE_KEY";
pub const PASSWORD_VAR: &str = "TAURI_SIGNING_PRIVATE_KEY_PASSWORD";
/// The untrusted comment of every Tauri signature.
pub const UNTRUSTED_COMMENT: &str = "signature from tauri secret key";

/// The secret key from Tauri's key text (base64 of the key file) and its password.
pub fn secret_key(key: &str, password: &str) -> Result<SecretKey, String> {
    let decoded = STANDARD.decode(key.trim()).map_err(|_| format!("{KEY_VAR} isn't base64: not a Tauri key file"))?;
    let text = String::from_utf8(decoded).map_err(|_| format!("{KEY_VAR} isn't a Tauri key file"))?;
    let key_box = SecretKeyBox::from_string(&text).map_err(|e| format!("failed to load the updater private key: {e}"))?;
    key_box.into_secret_key(Some(password.to_owned())).map_err(|e| format!("incorrect updater private key password: {e}"))
}

/// The `.sig` file's text for `data`, named `file_name`, signed at `timestamp` (Unix seconds).
pub fn signature(key: &SecretKey, data: impl Read, file_name: &str, version: &str, timestamp: u64) -> Result<String, String> {
    if file_name.contains(['\t', '\n', '\r']) {
        return Err(format!("the file {file_name} cannot be signed because its name contains a tab or newline"));
    }
    let trusted = format!("timestamp:{timestamp}\tfile:{file_name}\tversion:{version}");
    let signed = minisign::sign(None, key, data, Some(&trusted), Some(UNTRUSTED_COMMENT)).map_err(|e| format!("failed to sign {file_name}: {e}"))?;
    Ok(STANDARD.encode(signed.to_string()))
}

/// Tauri's public key (`plugins.updater.pubkey`: base64 of the `.pub` file's text), or a bare minisign key.
pub fn public_key(key: &str) -> Result<PublicKey, String> {
    let key = key.trim();
    let text = STANDARD.decode(key).ok().and_then(|b| String::from_utf8(b).ok());
    let line = match &text {
        Some(text) if text.starts_with("untrusted comment:") => text.lines().nth(1).unwrap_or_default(),
        _ => key,
    };
    PublicKey::from_base64(line.trim()).map_err(|e| format!("not a minisign public key: {e}"))
}

/// Checks a `.sig` file's text against `data`, legacy signatures allowed as the Tauri updater allows them.
pub fn verify(key: &PublicKey, signature: &str, data: impl Read + Seek) -> Result<(), String> {
    let decoded = STANDARD.decode(signature.trim()).map_err(|_| "the signature isn't base64".to_owned())?;
    let text = String::from_utf8(decoded).map_err(|_| "the signature isn't text".to_owned())?;
    let signature = SignatureBox::from_string(&text).map_err(|e| format!("not a minisign signature: {e}"))?;
    minisign::verify(key, &signature, data, true, false, true).map_err(|e| format!("the signature does not verify: {e}"))
}

pub fn run(args: &[String]) -> Result<(), String> {
    if args.is_empty() || args.iter().any(|a| a.starts_with("--")) {
        return Err(format!("usage: cargo xtask sign <file>...   (with {KEY_VAR} and {PASSWORD_VAR} set)"));
    }
    let key = std::env::var(KEY_VAR).map_err(|_| format!("{KEY_VAR} is not set"))?;
    // the CLI also takes a path to the key file
    let key = if Path::new(key.trim()).is_file() { fs::read_to_string(key.trim()).map_err(|e| format!("{KEY_VAR}: {e}"))? } else { key };
    let password = std::env::var(PASSWORD_VAR).unwrap_or_else(|_| {
        eprintln!("No updater private key password provided, assuming an empty password");
        String::new()
    });
    let key = secret_key(&key, &password)?;
    let version = bump::current()?;
    let now = (rustradar_backend::now_ms() / 1000) as u64;
    for file in args {
        let path = Path::new(file);
        let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| format!("{file}: not a file name"))?;
        let data = fs::File::open(path).map_err(|e| format!("{file}: {e}"))?;
        let text = signature(&key, data, name, &version, now)?;
        let sig = format!("{file}.sig");
        fs::write(&sig, text).map_err(|e| format!("{sig}: {e}"))?;
        println!("signed {file} -> {sig}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use minisign::KeyPair;
    use std::io::Cursor;

    /// A fresh key in Tauri's two forms: the private key file's text and the public key as
    /// `tauri.conf.json` holds it.
    fn tauri_keys(password: &str) -> (String, String) {
        let pair = KeyPair::generate_encrypted_keypair(Some(password.to_owned())).unwrap();
        let private = STANDARD.encode(pair.sk.to_box(None).unwrap().to_string());
        let public = STANDARD.encode(pair.pk.to_box().unwrap().to_string());
        (private, public)
    }

    #[test]
    fn signatures_verify_as_the_updater_checks_them() {
        let (private, public) = tauri_keys("hunter2");
        let key = secret_key(&private, "hunter2").unwrap();
        let data = b"an installer's bytes".repeat(1000);
        let sig = signature(&key, &data[..], "RustRadar_0.3.16_x64-setup.exe", "0.3.16", 1_794_385_122).unwrap();
        assert!(!sig.ends_with('\n') && !sig.contains(char::is_whitespace), "one line, as Tauri's .sig files");

        // the box, as the published .sig files lay it out
        let text = String::from_utf8(STANDARD.decode(&sig).unwrap()).unwrap();
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "untrusted comment: signature from tauri secret key");
        assert_eq!(STANDARD.decode(lines[1]).unwrap()[..2], *b"ED", "pre-hashed");
        assert_eq!(lines[2], "trusted comment: timestamp:1794385122\tfile:RustRadar_0.3.16_x64-setup.exe\tversion:0.3.16");
        assert_eq!(lines[4], "");

        // tauri-plugin-updater's check (and the app's, P2): minisign-verify with the config's public key
        let pub_text = String::from_utf8(STANDARD.decode(&public).unwrap()).unwrap();
        let updater_key = minisign_verify::PublicKey::from_base64(pub_text.lines().nth(1).unwrap()).unwrap();
        let decoded = minisign_verify::Signature::decode(&text).unwrap();
        updater_key.verify(&data, &decoded, false).unwrap();
        let mut tampered = data.clone();
        tampered[7] ^= 1;
        assert!(updater_key.verify(&tampered, &decoded, true).is_err());

        // and this module's own check, used by latest-json --pubkey
        let pk = public_key(&public).unwrap();
        verify(&pk, &sig, Cursor::new(&data)).unwrap();
        assert!(verify(&pk, &sig, Cursor::new(&tampered)).is_err());
        let (_, other) = tauri_keys("");
        assert!(verify(&public_key(&other).unwrap(), &sig, Cursor::new(&data)).is_err(), "another key's signature is refused");
    }

    #[test]
    fn keys_are_read_as_the_cli_reads_them() {
        let (private, public) = tauri_keys("");
        // GitHub often keeps a final newline on a secret
        assert!(secret_key(&format!("{private}\n"), "").is_ok());
        let wrong = secret_key(&private, "not it").err().unwrap();
        assert!(wrong.starts_with("incorrect updater private key password"), "{wrong}");
        assert!(secret_key("not base64 at all!", "").is_err());
        assert!(secret_key(&public, "").is_err(), "the public key is no private key");
        for message in [secret_key(&private, "x").err().unwrap(), secret_key("%%%", "").err().unwrap()] {
            assert!(!message.contains(&private[..20]), "errors never repeat the key");
        }
        // the bare minisign line works as well as Tauri's form
        let pub_text = String::from_utf8(STANDARD.decode(&public).unwrap()).unwrap();
        let bare = pub_text.lines().nth(1).unwrap();
        assert_eq!(public_key(bare).unwrap().to_base64(), public_key(&public).unwrap().to_base64());
        assert!(public_key("RWQ").is_err());
    }

    #[test]
    fn names_with_tabs_or_newlines_are_refused() {
        let (private, _) = tauri_keys("");
        let key = secret_key(&private, "").unwrap();
        for name in ["a\tb.msi", "a\nb.msi"] {
            assert!(signature(&key, &b"x"[..], name, "1.0.0", 0).unwrap_err().contains("contains a tab or newline"));
        }
    }
}
