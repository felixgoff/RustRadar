//! In dev builds (`tauri dev`), every call the app makes to the outside
//! APIs (Trafikverket, the train and ship sources, the track tiles): what was
//! asked and what came back. Each is summed up in one line in the terminal
//! and kept (the last few hundred) for the dev panel (Ctrl/Cmd+Shift+D).
//! `RUSTRADAR_LOG_BODIES=1` also prints each answer in full to the terminal.
//!
//! Release builds record nothing. API keys are blanked out of what is kept.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde::Serialize;

/// Whether calls are recorded: dev builds only.
pub const ENABLED: bool = cfg!(debug_assertions);
/// Calls kept.
const KEEP: usize = 400;
/// Each answer is kept up to this many bytes.
const MAX_BODY: usize = 512 * 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: u64,
    /// Unix ms.
    pub at_ms: i64,
    pub source: String,
    pub method: String,
    pub url: String,
    /// What was sent (a query's body), keys blanked.
    pub request: Option<String>,
    pub status: Option<u16>,
    pub ms: u64,
    /// The answer's full size, bytes.
    pub size: usize,
    /// The answer as text (cut at `MAX_BODY`); a summary for binary ones.
    pub body: String,
    pub error: Option<String>,
}

struct Log {
    next: u64,
    entries: VecDeque<Entry>,
}

fn log() -> &'static Mutex<Log> {
    static L: OnceLock<Mutex<Log>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(Log { next: 1, entries: VecDeque::new() }))
}

/// Blanks API keys: Trafikverket's in its query, and `key=`/`apikey=`-like URL parameters.
pub fn redact(s: &str) -> String {
    let mut out = s.to_owned();
    for marker in ["authenticationkey=\"", "authenticationkey='"] {
        let mut from = 0;
        while let Some(i) = out[from..].find(marker) {
            let start = from + i + marker.len();
            let quote = &marker[marker.len() - 1..];
            let Some(len) = out[start..].find(quote) else { break };
            out.replace_range(start..start + len, "•••");
            from = start + "•••".len();
        }
    }
    for param in ["key=", "apikey=", "api_key=", "token="] {
        let mut from = 0;
        while let Some(i) = out[from..].find(param) {
            let at = from + i;
            let starts_param = at == 0 || matches!(out.as_bytes()[at - 1], b'?' | b'&');
            let start = at + param.len();
            if !starts_param {
                from = start;
                continue;
            }
            let len = out[start..].find(['&', '#', ' ', '"']).unwrap_or(out.len() - start);
            out.replace_range(start..start + len, "•••");
            from = start + "•••".len();
        }
    }
    out
}

/// A call under way; `finish` or `fail` records it. None when not recording.
pub struct Call {
    source: String,
    method: String,
    url: String,
    request: Option<String>,
    started: Instant,
}

/// Starts recording a call to `source`.
pub fn start(source: &str, method: &str, url: &str, request: Option<&str>) -> Option<Call> {
    ENABLED.then(|| Call {
        source: source.to_owned(),
        method: method.to_owned(),
        url: redact(url),
        request: request.map(redact),
        started: Instant::now(),
    })
}

impl Call {
    pub fn finish(self, status: u16, body: &[u8]) {
        let text = match std::str::from_utf8(body) {
            Ok(t) => {
                let mut end = t.len().min(MAX_BODY);
                while !t.is_char_boundary(end) {
                    end -= 1;
                }
                let mut t = redact(&t[..end]);
                if end < body.len() {
                    t.push_str(&format!("\n… cut at {MAX_BODY} of {} bytes", body.len()));
                }
                t
            }
            Err(_) => format!("({} bytes of binary data)", body.len()),
        };
        self.record(Some(status), body.len(), text, None);
    }

    pub fn fail(self, error: &str) {
        self.record(None, 0, String::new(), Some(error.to_owned()));
    }

    fn record(self, status: Option<u16>, size: usize, body: String, error: Option<String>) {
        let ms = self.started.elapsed().as_millis() as u64;
        let outcome = match (&error, status) {
            (Some(e), _) => format!("failed: {e}"),
            (None, Some(s)) => format!("{s}, {size} B"),
            (None, None) => String::new(),
        };
        let url: String = self.url.chars().take(160).collect();
        eprintln!("[api] {} {} {url} → {outcome} in {ms} ms", self.source, self.method);
        if std::env::var_os("RUSTRADAR_LOG_BODIES").is_some_and(|v| v != "0") && !body.is_empty() {
            eprintln!("{body}");
        }
        let mut log = log().lock().unwrap_or_else(|e| e.into_inner());
        let id = log.next;
        log.next += 1;
        if log.entries.len() >= KEEP {
            log.entries.pop_front();
        }
        log.entries.push_back(Entry {
            id,
            at_ms: crate::trains::now_ms(),
            source: self.source,
            method: self.method,
            url: self.url,
            request: self.request,
            status,
            ms,
            size,
            body,
            error,
        });
    }
}

/// Records something that arrived on its own (a stream's message).
pub fn received(source: &str, url: &str, body: &str) {
    if let Some(call) = start(source, "STREAM", url, None) {
        call.finish(200, body.as_bytes());
    }
}

/// The calls recorded after `since` (an entry id), oldest first; none in release builds.
#[tauri::command]
pub fn dev_log(since: Option<u64>) -> Vec<Entry> {
    let since = since.unwrap_or(0);
    let log = log().lock().unwrap_or_else(|e| e.into_inner());
    log.entries.iter().filter(|e| e.id > since).cloned().collect()
}

#[tauri::command]
pub fn dev_log_clear() {
    log().lock().unwrap_or_else(|e| e.into_inner()).entries.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_blanked() {
        assert_eq!(
            redact(r#"<LOGIN authenticationkey="abc123"/><QUERY/>"#),
            r#"<LOGIN authenticationkey="•••"/><QUERY/>"#
        );
        assert_eq!(redact("https://x/y?apikey=s3cret&z=1"), "https://x/y?apikey=•••&z=1");
        assert_eq!(redact("https://x/y?a=1&key=k#f"), "https://x/y?a=1&key=•••#f");
        assert_eq!(redact("https://x/monkey=1"), "https://x/monkey=1", "only whole parameters");
    }
}
