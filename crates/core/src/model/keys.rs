//! The keys DTO of `keys.ts` (S1.5), as `RS:keys.rs` stores and sends it.

use std::fmt;

use serde::{Deserialize, Serialize};

/// API keys and logins from the keys page (`keys.ts:8`). A field the JSON leaves out is empty.
///
/// `Debug` never prints a value, only whether each field is set.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Keys {
    pub fr24_email: String,
    pub fr24_password: String,
    /// Trafiklab "GTFS Sverige 3" realtime: vehicle positions and delays.
    pub trafiklab_gtfs: String,
    /// Trafiklab "GTFS Sverige 3" static: the timetable that names them
    /// (operators, line colours, stops).
    pub trafiklab_gtfs_static: String,
    /// Trafiklab Oxyfi: live positions from on-train wifi.
    pub trafiklab_oxyfi: String,
    /// Trafikverket's open API: every train in Sweden.
    pub trafikverket: String,
}

impl fmt::Debug for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let set = |value: &str| if value.is_empty() { "empty" } else { "set" };
        f.debug_struct("Keys")
            .field("fr24_email", &set(&self.fr24_email))
            .field("fr24_password", &set(&self.fr24_password))
            .field("trafiklab_gtfs", &set(&self.trafiklab_gtfs))
            .field("trafiklab_gtfs_static", &set(&self.trafiklab_gtfs_static))
            .field("trafiklab_oxyfi", &set(&self.trafiklab_oxyfi))
            .field("trafikverket", &set(&self.trafikverket))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_use_the_names_of_keys_ts() {
        let wire = json!({
            "fr24Email": "a", "fr24Password": "b", "trafiklabGtfs": "c", "trafiklabGtfsStatic": "d",
            "trafiklabOxyfi": "e", "trafikverket": "f",
        });
        let keys: Keys = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(keys.trafiklab_gtfs_static, "d");
        assert_eq!(serde_json::to_value(&keys).unwrap(), wire);
    }

    #[test]
    fn keys_left_out_are_empty() {
        let keys: Keys = serde_json::from_value(json!({"trafikverket": "f"})).unwrap();
        assert_eq!(keys, Keys { trafikverket: "f".to_owned(), ..Default::default() });
        assert_eq!(serde_json::from_value::<Keys>(json!({})).unwrap(), Keys::default());
    }

    #[test]
    fn debug_does_not_print_the_values() {
        let keys = Keys { fr24_password: "placeholder-value".to_owned(), ..Default::default() };
        let shown = format!("{keys:?}");
        assert!(!shown.contains("placeholder-value"));
        assert!(shown.contains("fr24_password: \"set\""));
        assert!(shown.contains("trafikverket: \"empty\""));
    }
}
