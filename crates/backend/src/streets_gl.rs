//! Streets GL's own vector tiles (item R3; doc 04 §6.6; `streets-gl.ts`'s `SGL_TILES`,
//! `SGL_ZOOM` and `sglFetch`): tiles.streets.gl, its author's server (MIT code, © OpenStreetMap
//! contributors data), which the realistic map reads close in on the flat map: each field, garden,
//! wood and road as mapped (the city ground, `ground-city`), and later each building and tree.
//!
//! Only zoom [`ZOOM`] is served. Today's `sglFetch` asks for nothing else (deck.gl wants coarser
//! tiles towards a tilted view's horizon, which the server does not have) and resolves `null`, a
//! tile with nothing in it; [`tile`] does the same before the network is asked. The tiles are
//! kept like every other in `backend::tiles`' disk cache (folder [`NAME`]: fresh for the
//! server's `max-age`, else 7 days; a stale copy beats none when the server fails; at most
//! `tiles::PER_HOST` requests to the host at once, and left alone when it asks to be: one
//! person's server, plan K12). The bytes are the MVT as sent, the HTTP compression undone; the
//! map's worker pool decodes them (`scene::tess::ground` with `SGL_GROUND_LAYERS`).
//!
//! The app's tile fetcher hands every request for the source [`NAME`] (the map's
//! `io::STREETS_GL`) to [`tile`] with the template it resolved for it ([`TILES`] in the app).

use crate::tiles::{Source, Tiles};

/// The tiles (`SGL_TILES`); only [`ZOOM`] is served.
pub const TILES: &str = "https://tiles.streets.gl/vector/{z}/{x}/{y}";
/// The one zoom the server has tiles at (`SGL_ZOOM`).
pub const ZOOM: u32 = 16;
/// The source's name: the map's `io::STREETS_GL` and the cache folder under `<cache>/tiles`.
pub const NAME: &str = "streets-gl";
/// Their credit, with OpenStreetMap's (`SGL_ATTRIBUTION`).
pub const ATTRIBUTION: &str = "Map data close in: Streets GL (tiles.streets.gl), © OpenStreetMap contributors";

/// The source as the app asks it: [`TILES`], kept in the folder [`NAME`].
pub fn source() -> Source {
    Source::new(NAME, TILES)
}

/// Whether the server has tiles at zoom `z` (`sglFetch`'s `url.includes("/vector/16/")`).
pub fn served(z: u32) -> bool {
    z == ZOOM
}

/// Tile `z/x/y` of `source` (normally [`source`]) through the cache: `Ok(None)` at once for a
/// zoom the server does not serve (nothing is asked), else what `Tiles::tile` answers (`Ok(None)`
/// for a 404 or 204: nothing there).
pub async fn tile(tiles: &Tiles, source: &Source, z: u32, x: u32, y: u32) -> Result<Option<Vec<u8>>, String> {
    if !served(z) {
        return Ok(None);
    }
    tiles.tile(source, z, x, y).await
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::tiles::{CAP_BYTES, Config, PER_HOST};

    /// A server on 127.0.0.1 answering every path with `body` (200) except those ending in
    /// `missing` (404), noting each path asked for.
    async fn server(body: &'static [u8], missing: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let paths = Arc::new(Mutex::new(Vec::new()));
        let noted = paths.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let noted = noted.clone();
                tokio::spawn(async move {
                    let (mut head, mut buf) = (Vec::new(), [0u8; 1024]);
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let path = String::from_utf8_lossy(&head).split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let (status, body) = if path.ends_with(missing) { (404, &b""[..]) } else { (200, body) };
                    noted.lock().unwrap().push(path);
                    let head = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (base, paths)
    }

    /// A cache folder of its own for one test, removed when it ends.
    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("rustradar-streets-gl-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            Self(root)
        }

        fn tiles(&self) -> Tiles {
            let client = fr24::client_builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap();
            Tiles::with_config(Config { root: self.0.clone(), cap_bytes: CAP_BYTES, per_host: PER_HOST, client: Some(client) }).unwrap()
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_source_is_today_s() {
        assert_eq!(source().url(16, 36057, 19273), "https://tiles.streets.gl/vector/16/36057/19273");
        assert_eq!(source().name, "streets-gl");
        assert!(served(16) && !served(15) && !served(14) && !served(17));
        assert_eq!(ATTRIBUTION, "Map data close in: Streets GL (tiles.streets.gl), © OpenStreetMap contributors");
    }

    /// `sglFetch`: only zoom 16 goes to the server; any other zoom has nothing, unasked. A tile
    /// the server lacks is nothing too, and a tile is kept on disk under the source's folder.
    #[tokio::test]
    async fn only_zoom_16_is_asked_for() {
        let root = TempRoot::new("zoom");
        let (base, paths) = server(b"a tile", "/16/2/3").await;
        let source = Source::new(NAME, format!("{base}/vector/{{z}}/{{x}}/{{y}}"));
        let tiles = root.tiles();
        assert_eq!(tile(&tiles, &source, 16, 36057, 19273).await, Ok(Some(b"a tile".to_vec())));
        assert!(root.0.join("streets-gl/16/36057/19273").is_file());
        for z in [0, 9, 14, 15, 17] {
            assert_eq!(tile(&tiles, &source, z, 1, 1).await, Ok(None), "zoom {z}");
        }
        assert_eq!(tile(&tiles, &source, 16, 2, 3).await, Ok(None));
        assert_eq!(*paths.lock().unwrap(), ["/vector/16/36057/19273", "/vector/16/2/3"]);
        // the kept tile answers without the server
        assert_eq!(tile(&tiles, &source, 16, 36057, 19273).await, Ok(Some(b"a tile".to_vec())));
        assert_eq!(paths.lock().unwrap().len(), 2);
    }
}
