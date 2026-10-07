//! Signing in to Flightradar24. A subscription raises the feed limits and
//! unlocks data such as filed flight plans; credentials stay in memory.

use std::sync::Arc;

use fr24::Fr24;
use fr24::auth::Credentials;
use serde::Serialize;
use tauri::State;

use crate::{AppState, CmdResult};

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

#[tauri::command]
pub fn session_info(state: State<'_, AppState>) -> SessionInfo {
    info(&state.client())
}

#[tauri::command]
pub async fn sign_in(
    email: String,
    password: String,
    state: State<'_, AppState>,
) -> CmdResult<SessionInfo> {
    let mut fr24 = Fr24::clone(&state.client());
    let creds = Credentials::UsernamePassword {
        username: email.trim().to_owned(),
        password,
    };
    match fr24.login(Some(creds)).await {
        Ok(Some(_)) => {
            let session = info(&fr24);
            *state.fr24.write().unwrap() = Arc::new(fr24);
            Ok(session)
        }
        Ok(None) => Err("Flightradar24 didn't return a session for these details.".into()),
        Err(e) => Err(format!("Sign-in failed: {e}")),
    }
}

#[tauri::command]
pub fn sign_out(state: State<'_, AppState>) -> SessionInfo {
    let mut fr24 = Fr24::clone(&state.client());
    fr24.set_auth(None);
    let session = info(&fr24);
    *state.fr24.write().unwrap() = Arc::new(fr24);
    session
}
