//! API keys and logins the user enters on the keys page, kept in
//! `keys.json` in the app's config folder so they survive a restart. Plain
//! text, like the file the page exports: the folder is the user's own.

use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

const FILE: &str = "keys.json";

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Keys {
    pub fr24_email: String,
    pub fr24_password: String,
    /// Trafiklab "GTFS Sverige 3" realtime: regional vehicle positions.
    pub trafiklab_gtfs: String,
    /// Trafiklab Oxyfi: live positions from on-train wifi.
    pub trafiklab_oxyfi: String,
    /// Trafikverket's open API: every train in Sweden.
    pub trafikverket: String,
}

impl Keys {
    fn trimmed(self) -> Keys {
        Keys {
            fr24_email: self.fr24_email.trim().to_owned(),
            // a password may well start or end with a space
            fr24_password: self.fr24_password,
            trafiklab_gtfs: self.trafiklab_gtfs.trim().to_owned(),
            trafiklab_oxyfi: self.trafiklab_oxyfi.trim().to_owned(),
            trafikverket: self.trafikverket.trim().to_owned(),
        }
    }
}

fn cell() -> &'static RwLock<Keys> {
    static KEYS: OnceLock<RwLock<Keys>> = OnceLock::new();
    KEYS.get_or_init(Default::default)
}

/// The keys in use now.
pub fn current() -> Keys {
    cell().read().unwrap().clone()
}

fn path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(FILE))
        .map_err(|e| format!("No settings folder: {e}"))
}

/// Reads the saved keys at launch; none saved (or unreadable) leaves them empty.
pub fn load(app: &AppHandle) -> Keys {
    let keys = path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Keys>(&s).ok())
        .unwrap_or_default();
    *cell().write().unwrap() = keys.clone();
    keys
}

#[tauri::command]
pub fn get_keys() -> Keys {
    current()
}

/// Saves the keys and puts them in use. Signing in to Flightradar24 with a
/// changed login is the page's next call, so a wrong password is reported there.
#[tauri::command]
pub fn save_keys(app: AppHandle, keys: Keys) -> Result<Keys, String> {
    let keys = keys.trimmed();
    let path = path(&app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't open {}: {e}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(&keys).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("Couldn't save {}: {e}", path.display()))?;
    *cell().write().unwrap() = keys.clone();
    Ok(keys)
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
        let keys = Keys {
            fr24_email: " me@example.com ".into(),
            fr24_password: " secret ".into(),
            trafikverket: "key\n".into(),
            ..Default::default()
        }
        .trimmed();
        assert_eq!(keys.fr24_email, "me@example.com");
        assert_eq!(keys.fr24_password, " secret ");
        assert_eq!(keys.trafikverket, "key");
    }
}
