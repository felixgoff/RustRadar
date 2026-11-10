//! Port of `signin-values.ts` (doc 05 §6.8): the flight values Flightradar24 sends only to
//! signed-in users, and the flight panel's note about the ones it has nothing to show for.

/// A flight value Flightradar24 sends only to signed-in users (`SignInValue`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignInValue {
    VerticalSpeed,
    Squawk,
    Airspace,
}

impl SignInValue {
    /// The name in the TypeScript: `verticalSpeed`, `squawk`, `airspace`.
    pub fn as_str(self) -> &'static str {
        match self {
            SignInValue::VerticalSpeed => "verticalSpeed",
            SignInValue::Squawk => "squawk",
            SignInValue::Airspace => "airspace",
        }
    }

    /// How a sentence names it.
    fn name(self) -> &'static str {
        match self {
            SignInValue::VerticalSpeed => "vertical speed",
            SignInValue::Squawk => "squawk",
            SignInValue::Airspace => "airspace",
        }
    }
}

/// The sign-in values, in the order the flight panel lists them.
pub const SIGN_IN_VALUES: [SignInValue; 3] = [SignInValue::VerticalSpeed, SignInValue::Squawk, SignInValue::Airspace];

/// Which of the sign-in values the panel has from some source (`Record<SignInValue, boolean>`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Have {
    pub vertical_speed: bool,
    pub squawk: bool,
    pub airspace: bool,
}

impl Have {
    /// `have[v]`.
    pub fn get(self, value: SignInValue) -> bool {
        match value {
            SignInValue::VerticalSpeed => self.vertical_speed,
            SignInValue::Squawk => self.squawk,
            SignInValue::Airspace => self.airspace,
        }
    }
}

/// The sign-in-only values the panel has nothing to show for: none when signed in, and none that
/// another source already supplies (the aircraft's own reports via adsb.lol, or Flightradar24
/// sending one anyway).
pub fn missing_without_sign_in(authenticated: bool, have: Have) -> Vec<SignInValue> {
    if authenticated { Vec::new() } else { SIGN_IN_VALUES.into_iter().filter(|v| !have.get(*v)).collect() }
}

/// "Airspace needs a Flightradar24 sign-in", "Squawk and airspace need …": one or two values by
/// name, so nobody has to go looking for them; all three counted, "3 values need a Flightradar24
/// sign-in". Empty when none do.
pub fn sign_in_note(missing: &[SignInValue]) -> String {
    if missing.is_empty() {
        return String::new();
    }
    if missing.len() >= SIGN_IN_VALUES.len() {
        return format!("{} values need a Flightradar24 sign-in", missing.len());
    }
    let names = missing.iter().map(|v| v.name()).collect::<Vec<_>>().join(" and ");
    // `charAt(0).toUpperCase()` + `slice(1)`: the names are ASCII
    let mut chars = names.chars();
    let first = chars.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    let verb = if missing.len() == 1 { "needs" } else { "need" };
    format!("{first}{} {verb} a Flightradar24 sign-in", chars.as_str())
}

#[cfg(test)]
mod tests {
    use super::SignInValue::{Airspace, Squawk, VerticalSpeed};
    use super::*;

    const NONE: Have = Have { vertical_speed: false, squawk: false, airspace: false };

    // missingWithoutSignIn

    #[test]
    fn lists_all_three_when_signed_out_with_no_other_source() {
        assert_eq!(missing_without_sign_in(false, NONE), [VerticalSpeed, Squawk, Airspace]);
        let names: Vec<_> = missing_without_sign_in(false, NONE).into_iter().map(SignInValue::as_str).collect();
        assert_eq!(names, ["verticalSpeed", "squawk", "airspace"]);
    }

    #[test]
    fn lists_nothing_when_signed_in_even_with_values_absent() {
        assert_eq!(missing_without_sign_in(true, NONE), []);
    }

    #[test]
    fn leaves_out_values_another_source_supplies() {
        assert_eq!(
            missing_without_sign_in(false, Have { vertical_speed: true, squawk: true, airspace: false }),
            [Airspace]
        );
        assert_eq!(
            missing_without_sign_in(false, Have { vertical_speed: true, squawk: false, airspace: false }),
            [Squawk, Airspace]
        );
    }

    #[test]
    fn lists_nothing_when_every_value_has_a_source() {
        assert_eq!(missing_without_sign_in(false, Have { vertical_speed: true, squawk: true, airspace: true }), []);
    }

    // signInNote

    #[test]
    fn counts_all_three() {
        assert_eq!(sign_in_note(&[VerticalSpeed, Squawk, Airspace]), "3 values need a Flightradar24 sign-in");
    }

    #[test]
    fn names_one_or_two_so_the_reader_knows_which() {
        assert_eq!(sign_in_note(&[Airspace]), "Airspace needs a Flightradar24 sign-in");
        assert_eq!(sign_in_note(&[Squawk, Airspace]), "Squawk and airspace need a Flightradar24 sign-in");
        assert_eq!(sign_in_note(&[VerticalSpeed, Airspace]), "Vertical speed and airspace need a Flightradar24 sign-in");
    }

    #[test]
    fn says_nothing_when_no_value_needs_it() {
        assert_eq!(sign_in_note(&[]), "");
    }
}
