//! API keys and logins the user enters on the keys page, kept in
//! `keys.json` in the app's config folder so they survive a restart. Plain
//! text, like the file the page exports: the folder is the user's own.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use crate::Dirs;

const FILE: &str = "keys.json";

// The keys, as `keys.json` holds them (camelCase, every field optional): `rustradar_core::model`.
pub use rustradar_core::model::Keys;

fn trimmed(keys: Keys) -> Keys {
    Keys {
        fr24_email: keys.fr24_email.trim().to_owned(),
        // a password may well start or end with a space
        fr24_password: keys.fr24_password,
        trafiklab_gtfs: keys.trafiklab_gtfs.trim().to_owned(),
        trafiklab_gtfs_static: keys.trafiklab_gtfs_static.trim().to_owned(),
        trafiklab_oxyfi: keys.trafiklab_oxyfi.trim().to_owned(),
        trafikverket: keys.trafikverket.trim().to_owned(),
    }
}

pub(crate) fn cell() -> &'static RwLock<Keys> {
    static KEYS: OnceLock<RwLock<Keys>> = OnceLock::new();
    KEYS.get_or_init(Default::default)
}

/// The keys in use now.
pub fn current() -> Keys {
    cell().read().unwrap().clone()
}

fn path(dirs: &Dirs) -> PathBuf {
    dirs.config_file(FILE)
}

/// Reads the saved keys at launch; none saved (or unreadable) leaves them empty.
pub fn load(dirs: &Dirs) -> Keys {
    let keys = std::fs::read_to_string(path(dirs))
        .ok()
        .and_then(|s| serde_json::from_str::<Keys>(&s).ok())
        .unwrap_or_default();
    *cell().write().unwrap() = keys.clone();
    keys
}

/// Saves the keys and puts them in use. Signing in to Flightradar24 with a
/// changed login is the page's next call, so a wrong password is reported there.
pub fn save(dirs: &Dirs, keys: Keys) -> Result<Keys, String> {
    let keys = trimmed(keys);
    let path = path(dirs);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't open {}: {e}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(&keys).map_err(|e| e.to_string())?;
    // a crash mid-write must not leave the old keys half-overwritten
    let tmp = path.with_extension("json.tmp");
    write_private(&tmp, json.as_bytes())
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("Couldn't save {}: {e}", path.display()))?;
    *cell().write().unwrap() = keys.clone();
    Ok(keys)
}

/// Readable by the user alone: the file holds a password.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // a leftover file would keep its old permissions
    let _ = std::fs::remove_file(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_read_as_empty() {
        let keys: Keys = serde_json::from_str(r#"{"trafiklabGtfs":"abc"}"#).unwrap();
        assert_eq!(keys.trafiklab_gtfs, "abc");
        assert_eq!(keys.fr24_email, "");
    }

    #[test]
    fn keys_are_trimmed_but_passwords_kept() {
        let keys = trimmed(Keys {
            fr24_email: " me@example.com ".into(),
            fr24_password: " secret ".into(),
            trafikverket: "key\n".into(),
            ..Default::default()
        });
        assert_eq!(keys.fr24_email, "me@example.com");
        assert_eq!(keys.fr24_password, " secret ");
        assert_eq!(keys.trafikverket, "key");
    }
}
