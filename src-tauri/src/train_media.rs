//! Pictures for the train panel, from Wikimedia (no key): a photo of the
//! train from the Wikipedia article on its product ("Öresundståg", "SJ
//! Snabbtåg"), else the one on its operator, and the operator's logo from
//! Wikidata (property P154, a file on Wikimedia Commons).
//!
//! Looked up once per product and operator and kept for the session.
//! Wikimedia asks for a descriptive User-Agent and a light touch.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::CmdResult;

const USER_AGENT: &str =
    concat!("RustRadar/", env!("CARGO_PKG_VERSION"), " (+https://github.com/felixgoff/RustRadar; train pictures)");
const PHOTO_WIDTH: u32 = 640;
const LOGO_WIDTH: u32 = 240;
/// Words in a Wikidata description that say it's about railways, not a namesake.
const RAIL_WORDS: &[&str] =
    &["rail", "train", "tåg", "järnväg", "jernbane", "tog", "rautatie", "juna", "bahn", "zug", "transport", "trafik"];

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Photo {
    pub url: String,
    /// The article it comes from, for the credit and a link.
    pub title: String,
    pub page_url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainMedia {
    pub photo: Option<Photo>,
    /// The operator's logo, a PNG rendering of the file on Commons.
    pub logo: Option<String>,
}

/// What the summary endpoint says about an article.
#[derive(Clone, Debug, Default, PartialEq)]
struct Summary {
    title: String,
    page_url: String,
    image: Option<String>,
    item: Option<String>,
    description: String,
}

fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    let c = fr24::client_builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|e| e.to_string())?;
    Ok(CLIENT.get_or_init(|| c))
}

/// Wikipedias to look in, the source's country's first.
fn languages(source: &str) -> &'static [&'static str] {
    match source {
        "trafikverket" => &["sv", "en"],
        "digitraffic" => &["fi", "en"],
        "entur" => &["no", "en"],
        "vbb" => &["de", "en"],
        _ => &["en"],
    }
}

fn encode(s: &str) -> String {
    s.trim()
        .replace(' ', "_")
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'(' | b')' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A thumbnail URL resized: Wikimedia's thumbs carry their width in the path.
fn resized(url: &str, width: u32) -> String {
    match url.rsplit_once('/') {
        Some((dir, file)) if file.contains("px-") => {
            let rest = file.split_once("px-").map_or(file, |(_, r)| r);
            format!("{dir}/{width}px-{rest}")
        }
        _ => url.to_owned(),
    }
}

fn parse_summary(body: &str) -> Option<Summary> {
    let v: Value = serde_json::from_str(body).ok()?;
    if v["type"].as_str() == Some("disambiguation") {
        return None;
    }
    Some(Summary {
        title: v["title"].as_str()?.to_owned(),
        page_url: v["content_urls"]["desktop"]["page"].as_str().unwrap_or_default().to_owned(),
        image: v["thumbnail"]["source"].as_str().map(|u| resized(u, PHOTO_WIDTH)),
        item: v["wikibase_item"].as_str().map(str::to_owned),
        description: v["description"].as_str().unwrap_or_default().to_lowercase(),
    })
}

async fn get(url: &str) -> Option<String> {
    let response = client().ok()?.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.text().await.ok()
}

async fn summary(lang: &str, title: &str) -> Option<Summary> {
    let url = format!("https://{lang}.wikipedia.org/api/rest_v1/page/summary/{}?redirect=true", encode(title));
    parse_summary(&get(&url).await?)
}

/// The first article about this name in these languages that is about railways.
async fn railway_article(langs: &[&str], name: &str) -> Option<Summary> {
    for lang in langs {
        if let Some(s) = summary(lang, name).await
            && (s.description.is_empty() || RAIL_WORDS.iter().any(|w| s.description.contains(w)))
        {
            return Some(s);
        }
    }
    None
}

/// The logo file named on a Wikidata item, as an image URL.
fn parse_logo(body: &str, item: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    let file = v["entities"][item]["claims"]["P154"][0]["mainsnak"]["datavalue"]["value"].as_str()?;
    Some(format!("https://commons.wikimedia.org/wiki/Special:FilePath/{}?width={LOGO_WIDTH}", encode(file)))
}

async fn logo(item: &str) -> Option<String> {
    let body = get(&format!("https://www.wikidata.org/wiki/Special:EntityData/{item}.json")).await?;
    parse_logo(&body, item)
}

/// A lead image that is a drawing (a logo or a map) is no photo of a train.
fn is_photo(url: &str) -> bool {
    let lower = url.to_lowercase();
    !lower.contains(".svg") && !lower.contains("logo") && !lower.contains("karta") && !lower.contains("map")
}

async fn lookup(source: &str, product: Option<&str>, operator: Option<&str>) -> TrainMedia {
    let langs = languages(source);
    let (product, operator) = futures_util::future::join(
        async {
            match product {
                Some(p) => railway_article(langs, p).await,
                None => None,
            }
        },
        async {
            match operator {
                Some(o) => railway_article(langs, o).await,
                None => None,
            }
        },
    )
    .await;
    let photo = [&product, &operator].into_iter().flatten().find_map(|s| {
        let url = s.image.clone().filter(|u| is_photo(u))?;
        Some(Photo { url, title: s.title.clone(), page_url: s.page_url.clone() })
    });
    let logo = match operator.as_ref().and_then(|s| s.item.clone()) {
        Some(item) => logo(&item).await,
        None => None,
    };
    TrainMedia { photo, logo }
}

type Cache = Mutex<HashMap<(String, String, String), TrainMedia>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// A photo of the train and its operator's logo, as far as Wikimedia has them.
#[tauri::command]
pub async fn train_media(source: String, product: Option<String>, operator: Option<String>) -> CmdResult<TrainMedia> {
    let clean = |s: Option<String>| s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let (product, operator) = (clean(product), clean(operator));
    let key = (source.clone(), product.clone().unwrap_or_default(), operator.clone().unwrap_or_default());
    if let Some(hit) = cache().lock().unwrap().get(&key) {
        return Ok(hit.clone());
    }
    let media = lookup(&source, product.as_deref(), operator.as_deref()).await;
    // an empty answer may be a network blip: asked again next time
    if media != TrainMedia::default() {
        cache().lock().unwrap().insert(key, media.clone());
    }
    Ok(media)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_give_a_wider_photo_and_the_item() {
        let body = r#"{"type":"standard","title":"Öresundståg","description":"tågsystem i Öresundsregionen",
            "wikibase_item":"Q1061993",
            "thumbnail":{"source":"https://upload.wikimedia.org/wikipedia/commons/thumb/4/45/Tj%C3%B6rnarp_station.jpg/330px-Tj%C3%B6rnarp_station.jpg"},
            "content_urls":{"desktop":{"page":"https://sv.wikipedia.org/wiki/%C3%96resundst%C3%A5g"}}}"#;
        let s = parse_summary(body).unwrap();
        assert_eq!(s.title, "Öresundståg");
        assert_eq!(s.item.as_deref(), Some("Q1061993"));
        assert!(s.image.unwrap().ends_with("/640px-Tj%C3%B6rnarp_station.jpg"));
        assert!(parse_summary(r#"{"type":"disambiguation","title":"SJ"}"#).is_none());
    }

    #[test]
    fn logos_come_from_p154() {
        let body = r#"{"entities":{"Q8301325":{"claims":{"P154":[{"mainsnak":{"datavalue":{"value":"SJ logo 2022.svg"}}}]}}}}"#;
        assert_eq!(
            parse_logo(body, "Q8301325").as_deref(),
            Some("https://commons.wikimedia.org/wiki/Special:FilePath/SJ_logo_2022.svg?width=240")
        );
        assert_eq!(parse_logo(r#"{"entities":{"Q1":{"claims":{}}}}"#, "Q1"), None);
    }

    #[test]
    fn names_are_encoded_for_urls() {
        assert_eq!(encode("SJ Snabbtåg"), "SJ_Snabbt%C3%A5g");
        assert!(!is_photo("https://x/SJ_logo.svg/640px-SJ_logo.svg.png"));
        assert!(is_photo("https://x/640px-X2000_Stockholm.jpg"));
    }
}
