//! RainViewer's radar frames (item W1; doc 02 §3.2, doc 01 §5.11): which picture of the world's
//! weather radar the map's radar tiles come from, as today's `refreshRadar` (G:1111-1124) finds it.
//!
//! - The frame list is RainViewer's public `weather-maps.json` ([`WEATHER_MAPS`]). The newest of
//!   its past frames (`maps.radar.past`, last entry) is the one drawn; its tiles are
//!   `${maps.host}${latest.path}/256/{z}/{x}/{y}/2/1_1.png` ([`tile_url`]): 256 px tiles, colour
//!   scheme 2 ("Universal Blue", the only one the public API serves, which the map reads back to
//!   reflectivity), options `1_1` (smoothed, snow shown). A list with no past frame has no frame.
//! - The list is asked again once the last ask is more than ten minutes old ([`is_due`], G:2681:
//!   `Date.now() - radarCheckedAt > 600_000`, checked while the weather is on). The time of the
//!   ask counts from its start, success or not, and today's app drops its frame when an ask fails
//!   (`catch { radarUrl = null }`): [`frame`] answers the error, and the caller (the radar state,
//!   item W5) shows no RainViewer until the next ask.
//! - The tiles themselves are fetched by the map's tile runtime as the source `rainviewer`
//!   (`map::io::RAINVIEWER`): the app sets that source's template to [`Frame::url`]
//!   (`tile_fetcher::TileSources::set_template`) before the scene carries the new frame, and they
//!   are kept on disk by `backend::tiles` like every map tile.
//!
//! - The answer is read as JSON whatever its status, as `(await fetch(…)).json()` reads it: an
//!   error page is no JSON and so an error, a list sent with any status gives its frame.
//!
//! Deviations: a frame whose host or path is not a string is no frame (the TypeScript would make a
//! URL with `undefined` in it, whose every tile fails).

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

use crate::CmdResult;

/// RainViewer's public frame list (G:1117).
pub const WEATHER_MAPS: &str = "https://api.rainviewer.com/public/weather-maps.json";

/// How old the last ask for the frame list may get before the list is asked again, in ms
/// (G:2681).
pub const REFRESH_MS: f64 = 600_000.0;

/// How long an ask for the frame list may take.
const TIMEOUT: Duration = Duration::from_secs(20);

/// A RainViewer radar frame: one picture of the world's weather radar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// When it was taken, Unix seconds (the list's `time`; `None` when it has none).
    pub time: Option<i64>,
    /// Its tiles' URL template, `{z}`, `{x}` and `{y}` in it ([`tile_url`]).
    pub url: String,
}

/// The tile URL template of the frame at `path` on `host` (G:1119): 256 px tiles in the
/// "Universal Blue" colour scheme (`2`), smoothed and with snow (`1_1`).
pub fn tile_url(host: &str, path: &str) -> String {
    format!("{host}{path}/256/{{z}}/{{x}}/{{y}}/2/1_1.png")
}

/// The newest past frame of a frame list (`maps.radar?.past?.at(-1)`, G:1118); `None` when the
/// list has none.
pub fn latest(maps: &Value) -> Option<Frame> {
    let latest = maps.get("radar")?.get("past")?.as_array()?.last()?;
    // a frame (`latest` truthy) with a host and a path
    if latest.is_null() {
        return None;
    }
    let host = maps.get("host")?.as_str()?;
    let path = latest.get("path")?.as_str()?;
    Some(Frame { time: latest.get("time").and_then(Value::as_i64), url: tile_url(host, path) })
}

/// [`latest`] of a frame list's bytes; an error when they are not JSON.
pub fn parse(body: &[u8]) -> CmdResult<Option<Frame>> {
    let maps: Value = serde_json::from_slice(body).map_err(|e| format!("RainViewer's frame list: {e}"))?;
    Ok(latest(&maps))
}

/// Whether the frame list is due again: the last ask, at `checked_at_ms`, is more than
/// [`REFRESH_MS`] before `now_ms` (both Unix ms; `0` for never, which is always due).
pub fn is_due(checked_at_ms: f64, now_ms: f64) -> bool {
    now_ms - checked_at_ms > REFRESH_MS
}

/// The client the frame list is asked with: the app's (`fr24::client_builder()`, doc 07 §2.3).
fn client() -> Option<&'static reqwest::Client> {
    static C: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    C.get_or_init(|| {
        fr24::client_builder()
            .user_agent(concat!("RustRadar/", env!("CARGO_PKG_VERSION"), " (+https://github.com/felixgoff/RustRadar)"))
            .timeout(TIMEOUT)
            .build()
            .ok()
    })
    .as_ref()
}

/// The newest frame of the frame list at `url`, asked with `http`: `None` when the list has none,
/// an error when it cannot be had or read. The answer is read as JSON whatever its status, as
/// `(await fetch(…)).json()` reads it (an error page is no JSON, so it is an error all the same).
pub async fn fetch(http: &reqwest::Client, url: &str) -> CmdResult<Option<Frame>> {
    let call = crate::devlog::start("RainViewer", "GET", url, None);
    let answer = async {
        let response = http.get(url).send().await.map_err(|e| format!("can't reach RainViewer: {e}"))?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(|e| e.to_string())?;
        Ok::<_, String>((status, body))
    }
    .await;
    if let Some(c) = call {
        match &answer {
            Ok((status, body)) => c.finish(*status, body),
            Err(e) => c.fail(e),
        }
    }
    let (status, body) = answer?;
    parse(&body).map_err(|e| if (200..300).contains(&status) { e } else { format!("RainViewer answered {status}: {e}") })
}

/// The frame to draw now (`refreshRadar`): the newest of RainViewer's frame list.
pub async fn frame() -> CmdResult<Option<Frame>> {
    let http = client().ok_or("no HTTP client")?;
    fetch(http, WEATHER_MAPS).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// The frame list as RainViewer sent it for the fixtures (`map/tests/data/tiles/rainviewer`),
    /// with fewer past frames.
    const MAPS: &str = r#"{"version":"2.0","generated":1794328223,"host":"https://tilecache.rainviewer.com","radar":{"past":[{"time":1794327000,"path":"/v2/radar/778318fd52bb"},{"time":1794327600,"path":"/v2/radar/7bd3dae948bf"},{"time":1794328200,"path":"/v2/radar/47622fe604ca"}],"nowcast":[]},"satellite":{"infrared":[]}}"#;

    #[test]
    fn the_newest_past_frame_is_drawn() {
        let frame = parse(MAPS.as_bytes()).unwrap().unwrap();
        assert_eq!(frame.time, Some(1794328200));
        // the fixtures' template (their NOTICE)
        assert_eq!(frame.url, "https://tilecache.rainviewer.com/v2/radar/47622fe604ca/256/{z}/{x}/{y}/2/1_1.png");
    }

    #[test]
    fn the_committed_fixture_list_gives_the_fixture_frame() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../map/tests/data/tiles/rainviewer/weather-maps.json");
        let body = std::fs::read(path).expect("the fixtures' frame list");
        let frame = parse(&body).unwrap().unwrap();
        let notice = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../map/tests/data/tiles/NOTICE")).unwrap();
        assert!(notice.contains(&format!("Template: {}\n", frame.url)), "{}", frame.url);
    }

    #[test]
    fn a_list_without_a_past_frame_has_no_frame() {
        assert_eq!(latest(&json!({"host": "https://h", "radar": {"past": []}})), None);
        assert_eq!(latest(&json!({"host": "https://h", "radar": {}})), None);
        assert_eq!(latest(&json!({"host": "https://h"})), None);
        assert_eq!(latest(&json!({"host": "https://h", "radar": {"past": [null]}})), None);
        // no host or no path: no URL to ask
        assert_eq!(latest(&json!({"radar": {"past": [{"time": 1, "path": "/p"}]}})), None);
        assert_eq!(latest(&json!({"host": "https://h", "radar": {"past": [{"time": 1}]}})), None);
        // a frame without a time still draws
        let frame = latest(&json!({"host": "https://h", "radar": {"past": [{"path": "/p"}]}})).unwrap();
        assert_eq!(frame, Frame { time: None, url: "https://h/p/256/{z}/{x}/{y}/2/1_1.png".into() });
        assert!(parse(b"<html>").is_err());
    }

    #[test]
    fn the_list_is_asked_again_after_ten_minutes() {
        // never asked: due at once
        assert!(is_due(0.0, 1_794_328_223_000.0));
        let at = 1_794_328_223_000.0;
        assert!(!is_due(at, at));
        assert!(!is_due(at, at + 600_000.0), "exactly ten minutes is not more than ten");
        assert!(is_due(at, at + 600_001.0));
    }

    /// An HTTP/1.1 server on 127.0.0.1 answering every request with `status` and `body`, counting
    /// the requests.
    async fn serve(status: u16, body: &'static str) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let h = h.clone();
                tokio::spawn(async move {
                    let (mut head, mut buf) = (Vec::new(), [0u8; 1024]);
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    h.fetch_add(1, Ordering::SeqCst);
                    let out = format!(
                        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(out.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (base, hits, task)
    }

    fn local_client() -> reqwest::Client {
        fr24::client_builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap()
    }

    #[tokio::test]
    async fn the_frame_is_read_from_the_list_served() {
        let (base, hits, task) = serve(200, MAPS).await;
        let frame = fetch(&local_client(), &format!("{base}/public/weather-maps.json")).await.unwrap().unwrap();
        assert_eq!(frame.url, "https://tilecache.rainviewer.com/v2/radar/47622fe604ca/256/{z}/{x}/{y}/2/1_1.png");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        task.abort();
    }

    #[tokio::test]
    async fn a_failed_ask_is_an_error_and_an_empty_list_no_frame() {
        let (base, _, task) = serve(503, "<html>Service Unavailable</html>").await;
        let err = fetch(&local_client(), &base).await.unwrap_err();
        assert!(err.starts_with("RainViewer answered 503"), "{err}");
        task.abort();
        // the body is read whatever the status, as `fetch(…).json()` reads it
        let (base, _, task) = serve(503, MAPS).await;
        assert_eq!(fetch(&local_client(), &base).await.unwrap().unwrap().time, Some(1794328200));
        task.abort();
        let (base, _, task) = serve(200, r#"{"host":"https://h","radar":{"past":[]}}"#).await;
        assert_eq!(fetch(&local_client(), &base).await, Ok(None));
        task.abort();
        let (base, _, task) = serve(200, "not json").await;
        assert!(fetch(&local_client(), &base).await.is_err());
        task.abort();
        // nobody listening
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let gone = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let err = fetch(&local_client(), &gone).await.unwrap_err();
        assert!(err.starts_with("can't reach RainViewer"), "{err}");
    }
}
