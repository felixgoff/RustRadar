//! Authentication: credentials from the environment / config file, and login.

use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::headers;

/// Credentials used to log in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credentials {
    UsernamePassword {
        username: String,
        password: String,
    },
    SubscriptionKey {
        subscription_key: String,
        token: Option<String>,
    },
}

/// The subset of the `/user/login` response that is needed to authenticate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authentication {
    #[serde(rename = "userData")]
    pub user_data: UserData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserData {
    /// Bearer token for the gRPC API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// Sent as the `token=` query parameter for the JSON API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_key: Option<String>,
    /// Unix timestamp in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_expires: Option<i64>,
}

impl Authentication {
    pub fn access_token(&self) -> Option<&str> {
        self.user_data.access_token.as_deref()
    }

    pub fn subscription_key(&self) -> Option<&str> {
        self.user_data.subscription_key.as_deref()
    }
}

/// Default location of the config file, matching Python's
/// `platformdirs.user_config_dir("fr24") / "fr24.conf"`.
pub fn default_config_file() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        dirs::config_local_dir().map(|p| p.join("fr24").join("fr24"))
    } else {
        dirs::config_dir().map(|p| p.join("fr24"))
    };
    base.map(|p| p.join("fr24.conf"))
}

/// Reads credentials from the environment variables (`fr24_username`,
/// `fr24_password`, `fr24_subscription_key`, `fr24_token`), overriding them
/// with the `[global]` section of the config file if it exists.
pub fn get_credentials(config_file: Option<&Path>) -> Option<Credentials> {
    let env = |k: &str| std::env::var(k).ok();
    let mut username = env("fr24_username");
    let mut password = env("fr24_password");
    let mut subscription_key = env("fr24_subscription_key");
    let mut token = env("fr24_token");

    if let Some(text) = config_file.and_then(|p| std::fs::read_to_string(p).ok()) {
        let global = parse_ini_section(&text, "global");
        let get = |k: &str| {
            global
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        username = get("username");
        password = get("password");
        subscription_key = get("subscription_key");
        token = get("token");
    }

    let non_empty = |v: Option<String>| v.filter(|s| !s.is_empty());
    match (
        non_empty(username),
        non_empty(password),
        non_empty(subscription_key),
        non_empty(token),
    ) {
        (Some(username), Some(password), _, _) => {
            Some(Credentials::UsernamePassword { username, password })
        }
        (_, _, Some(subscription_key), Some(token)) => Some(Credentials::SubscriptionKey {
            subscription_key,
            token: Some(token),
        }),
        _ => None,
    }
}

/// Minimal `configparser`-compatible reader for `key = value` / `key: value`.
fn parse_ini_section(text: &str, section: &str) -> Vec<(String, String)> {
    let mut in_section = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_section = name.trim() == section;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((k, v)) = line.split_once(['=', ':']) {
            out.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    out
}

/// Log in with the given credentials.
///
/// - `username` + `password`: makes a POST request to the login endpoint
/// - `subscription_key` + optional `token`: returns immediately
///
/// Returns `Ok(None)` if the credentials were rejected or expired, in which
/// case the client should fall back to anonymous access.
pub async fn login(
    client: &reqwest::Client,
    creds: &Credentials,
) -> Result<Option<Authentication>> {
    match creds {
        Credentials::UsernamePassword { username, password } => {
            login_with_username_password(client, username, password).await
        }
        Credentials::SubscriptionKey {
            subscription_key,
            token,
        } => login_with_token_subscription_key(subscription_key, token.as_deref()),
    }
}

/// Retrieve the bearer token and subscription key from the API.
pub async fn login_with_username_password(
    client: &reqwest::Client,
    username: &str,
    password: &str,
) -> Result<Option<Authentication>> {
    let response = client
        .post("https://www.flightradar24.com/user/login")
        .headers(headers::default_headers())
        .form(&[("email", username), ("password", password)])
        .send()
        .await?
        .error_for_status()?;
    let json: serde_json::Value = response.json().await?;
    if json.get("userData").is_some_and(|v| v.is_object()) {
        return Ok(Some(serde_json::from_value(json)?));
    }
    let message = json
        .get("msg")
        .or_else(|| json.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("unknown error");
    Err(Error::Auth(format!(
        "login did not return credentials: {message}"
    )))
}

/// Login with subscription key and/or token.
///
/// Returns `Ok(None)` if the token is expired, falling back to anonymous access.
pub fn login_with_token_subscription_key(
    subscription_key: &str,
    token: Option<&str>,
) -> Result<Option<Authentication>> {
    let Some(token) = token else {
        return Ok(Some(Authentication {
            user_data: UserData {
                subscription_key: Some(subscription_key.to_owned()),
                ..Default::default()
            },
            message: Some("using environment `subscription_key`".into()),
        }));
    };

    let exp = jwt_exp(token).ok_or_else(|| Error::Auth("failed to parse token".into()))?;
    if headers::now_s() > exp {
        return Ok(None);
    }

    Ok(Some(Authentication {
        user_data: UserData {
            subscription_key: Some(subscription_key.to_owned()),
            access_token: Some(token.to_owned()),
            date_expires: Some(exp),
        },
        message: Some("using environment `subscription_key` and `token`".into()),
    }))
}

/// Extract the `exp` claim from a JWT without verifying its signature.
fn jwt_exp(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("exp")?.as_i64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini() {
        let text = "[other]\nusername=x\n[global]\nusername = me@example.com\npassword: hunter2\n# comment\ntoken=\n";
        let s = parse_ini_section(text, "global");
        assert_eq!(
            s,
            vec![
                ("username".into(), "me@example.com".into()),
                ("password".into(), "hunter2".into()),
                ("token".into(), "".into()),
            ]
        );
    }

    #[test]
    fn jwt() {
        // {"alg":"HS256"}.{"exp":4102444800,"userId":1}.sig
        let token = "eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjQxMDI0NDQ4MDAsInVzZXJJZCI6MX0.c2ln";
        assert_eq!(jwt_exp(token), Some(4102444800));
        let auth = login_with_token_subscription_key("key", Some(token))
            .unwrap()
            .unwrap();
        assert_eq!(auth.access_token(), Some(token));
    }
}
