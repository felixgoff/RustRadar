//! The 3D surface's height tiles (item Z1; doc 04 §14.1, §14.3; `terrain-layer.ts`'s
//! `MAPTERHORN_URL`, `MAPZEN_TERRARIUM` and `terrainFetch`): Mapterhorn's terrarium WebP tiles
//! (mapterhorn.com: Copernicus GLO-30 everywhere to zoom 12, and deeper the national lidar models
//! it has, Sweden's 1 m to zoom 16), and where Mapterhorn has none this deep (it answers 404),
//! Mapzen's terrarium PNG of the same `z/x/y` on AWS Open Data (a quarter of the pixels).
//!
//! Both are kept like every other tile in `backend::tiles`' disk cache, each in its own folder
//! ([`MAPTERHORN`], [`TERRARIUM`], the map's `io::MAPTERHORN` and `io::TERRARIUM`; the hillshade
//! reads the same Mapzen folder): Mapterhorn's "nothing here" is kept as a tile is, so a tile
//! Mapzen stands in for is not asked of Mapterhorn again while it is fresh.
//!
//! The app's tile fetcher hands every request for the source [`MAPTERHORN`] to [`tile`], with the
//! templates it resolved for both names ([`MAPTERHORN_TILES`] and [`MAPZEN_TILES`] in the app).
//! The bytes go to the map's worker pool as they came (the terrain layer decodes either format).
//!
//! Differences from `terrainFetch`:
//! - Mapterhorn's 204 stands down to Mapzen as its 404 does (`backend::tiles` reports both as
//!   nothing there; today a 204's empty body fails to parse and leaves no tile);
//! - where Mapzen has nothing either the tile is nothing there (`Ok(None)`, settled and not asked
//!   again), where today's fetch throws and deck counts the tile failed: the map draws neither.

use crate::tiles::{Source, Tiles};

/// Mapterhorn's tiles (`MAPTERHORN_URL`): 512 px terrarium WebP, lossless.
pub const MAPTERHORN_TILES: &str = "https://tiles.mapterhorn.com/{z}/{x}/{y}.webp";
/// Mapzen's terrarium tiles on AWS (`MAPZEN_TERRARIUM` + `{z}/{x}/{y}.png`, the hillshade's
/// `TERRARIUM_URL`): 256 px PNG, zooms 0 to 15.
pub const MAPZEN_TILES: &str = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png";
/// Mapterhorn's source name: the map's `io::MAPTERHORN` and its cache folder.
pub const MAPTERHORN: &str = "mapterhorn";
/// Mapzen's source name: the map's `io::TERRARIUM` and its cache folder.
pub const TERRARIUM: &str = "terrarium";

/// Mapterhorn as the app asks it: [`MAPTERHORN_TILES`], kept in [`MAPTERHORN`].
pub fn mapterhorn() -> Source {
    Source::new(MAPTERHORN, MAPTERHORN_TILES)
}

/// Mapzen as the app asks it: [`MAPZEN_TILES`], kept in [`TERRARIUM`].
pub fn mapzen() -> Source {
    Source::new(TERRARIUM, MAPZEN_TILES)
}

/// Height tile `z/x/y` (`terrainFetch`): `mapterhorn`'s (normally [`mapterhorn`]), or where it has
/// nothing there, `mapzen`'s of the same place (normally [`mapzen`]); `Ok(None)` where neither has
/// one. A failed request to Mapterhorn is the tile's failure (Mapzen is asked only after a 404).
pub async fn tile(tiles: &Tiles, mapterhorn: &Source, mapzen: &Source, z: u32, x: u32, y: u32) -> Result<Option<Vec<u8>>, String> {
    match tiles.tile(mapterhorn, z, x, y).await? {
        Some(bytes) => Ok(Some(bytes)),
        None => tiles.tile(mapzen, z, x, y).await,
    }
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

    /// A server on 127.0.0.1 answering each path with what `answer` gives (status, body), noting
    /// each path asked for.
    async fn server(answer: fn(&str) -> (u16, &'static [u8])) -> (String, Arc<Mutex<Vec<String>>>) {
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
                    let (status, body) = answer(&path);
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
            let root = std::env::temp_dir().join(format!("rustradar-terrain-{name}-{}", std::process::id()));
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

    /// Mapterhorn has 16/1/1 and fails 16/5/5; Mapzen has 16/2/2; nobody has 16/3/3.
    fn answer(path: &str) -> (u16, &'static [u8]) {
        match path {
            "/mt/16/1/1.webp" => (200, b"mapterhorn"),
            "/mt/16/5/5.webp" => (500, b""),
            "/mz/16/2/2.png" => (200, b"mapzen"),
            _ => (404, b""),
        }
    }

    #[test]
    fn the_sources_are_today_s() {
        assert_eq!(mapterhorn().url(16, 34177, 23310), "https://tiles.mapterhorn.com/16/34177/23310.webp");
        assert_eq!(mapzen().url(15, 17088, 11652), "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/15/17088/11652.png");
        assert_eq!((mapterhorn().name.as_str(), mapzen().name.as_str()), ("mapterhorn", "terrarium"));
    }

    /// `terrainFetch`: Mapterhorn's tile when it has one (Mapzen unasked); Mapzen's of the same
    /// `z/x/y` after Mapterhorn's 404, kept under Mapzen's folder; nothing where neither has one; a
    /// failed request to Mapterhorn is a failure, Mapzen unasked. Kept answers need no server.
    #[tokio::test]
    async fn mapzen_stands_in_where_mapterhorn_has_nothing() {
        let root = TempRoot::new("fallback");
        let (base, paths) = server(answer).await;
        let mapterhorn = Source::new(MAPTERHORN, format!("{base}/mt/{{z}}/{{x}}/{{y}}.webp"));
        let mapzen = Source::new(TERRARIUM, format!("{base}/mz/{{z}}/{{x}}/{{y}}.png"));
        let tiles = root.tiles();
        let get = |x| tile(&tiles, &mapterhorn, &mapzen, 16, x, x);
        assert_eq!(get(1).await, Ok(Some(b"mapterhorn".to_vec())));
        assert_eq!(get(2).await, Ok(Some(b"mapzen".to_vec())));
        assert!(root.0.join("mapterhorn/16/2/2").is_file() && root.0.join("terrarium/16/2/2").is_file());
        assert_eq!(get(3).await, Ok(None));
        assert!(get(5).await.is_err());
        assert_eq!(
            *paths.lock().unwrap(),
            ["/mt/16/1/1.webp", "/mt/16/2/2.webp", "/mz/16/2/2.png", "/mt/16/3/3.webp", "/mz/16/3/3.png", "/mt/16/5/5.webp"]
        );
        // what was kept answers without the servers
        assert_eq!(get(2).await, Ok(Some(b"mapzen".to_vec())));
        assert_eq!(get(3).await, Ok(None));
        assert_eq!(paths.lock().unwrap().len(), 6);
    }
}
