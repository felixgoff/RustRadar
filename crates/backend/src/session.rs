//! Signing in to Flightradar24. A subscription raises the feed limits and
//! unlocks data such as filed flight plans; credentials stay in memory.

use fr24::Fr24;
use fr24::auth::Credentials;
use serde::Serialize;

use crate::CmdResult;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub authenticated: bool,
    /// Unix seconds when the token expires, if known.
    pub expires: Option<i64>,
}

fn info(fr24: &Fr24) -> SessionInfo {
    let auth = fr24.auth();
    SessionInfo {
        authenticated: auth.and_then(|a| a.access_token()).is_some(),
        expires: auth.and_then(|a| a.user_data.date_expires),
    }
}

/// The session `fr24` holds.
pub fn session_info(fr24: &Fr24) -> SessionInfo {
    info(fr24)
}

/// A copy of `fr24` signed in with these details, and its session: the caller
/// swaps it in for the client it holds.
pub async fn sign_in(fr24: &Fr24, email: &str, password: String) -> CmdResult<(Fr24, SessionInfo)> {
    let mut fr24 = Fr24::clone(fr24);
    let creds = Credentials::UsernamePassword {
        username: email.trim().to_owned(),
        password,
    };
    match fr24.login(Some(creds)).await {
        Ok(Some(_)) => {
            let session = info(&fr24);
            Ok((fr24, session))
        }
        Ok(None) => Err("Flightradar24 didn't return a session for these details.".into()),
        Err(e) => Err(format!("Sign-in failed: {e}")),
    }
}

/// A copy of `fr24` signed out, and its session: the caller swaps it in.
pub fn sign_out(fr24: &Fr24) -> (Fr24, SessionInfo) {
    let mut fr24 = Fr24::clone(fr24);
    fr24.set_auth(None);
    let session = info(&fr24);
    (fr24, session)
}
