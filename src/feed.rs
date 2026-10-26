use std::{io::Read, time::Duration};

use anyhow::{Result, bail};
use image::DynamicImage;
use reqwest::{Url, blocking::Client};

const MAX_IMAGE_BYTES: u64 = 15 * 1024 * 1024;

#[derive(Clone)]
pub struct Item {
    /// Unique across feeds: `<feed url>|<entry id>`.
    pub id: String,
    pub title: String,
    pub link: Option<String>,
    pub author: Option<String>,
    pub date: Option<i64>,
    pub html: String,
    pub image: Option<String>,
}

pub struct Parsed {
    pub title: Option<String>,
    pub items: Vec<Item>,
}

pub fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!("rsstui/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()?)
}

pub fn fetch(client: &Client, url: &str) -> Result<Parsed> {
    let bytes = client.get(url).send()?.error_for_status()?.bytes()?;
    let feed = feed_rs::parser::Builder::new()
        .base_uri(Some(url))
        .build()
        .parse(&bytes[..])?;
    let feed_base = Url::parse(url).ok();

    let items = feed
        .entries
        .into_iter()
        .map(|e| {
            let link = e
                .links
                .iter()
                .find(|l| l.rel.as_deref() != Some("enclosure"))
                .map(|l| l.href.clone());
            let base = link
                .as_deref()
                .and_then(|l| Url::parse(l).ok())
                .or_else(|| feed_base.clone());
            let html = e
                .content
                .as_ref()
                .and_then(|c| c.body.clone())
                .or_else(|| e.summary.as_ref().map(|s| s.content.clone()))
                .unwrap_or_default();
            let image = lead_image(&e, &html, base.as_ref());
            Item {
                id: format!("{url}|{}", e.id),
                title: e
                    .title
                    .map(|t| t.content.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| "(untitled)".into()),
                link,
                author: e
                    .authors
                    .first()
                    .and_then(|a| a.name.clone())
                    .filter(|n| !n.is_empty()),
                date: e.published.or(e.updated).map(|d| d.timestamp()),
                html,
                image,
            }
        })
        .collect();

    Ok(Parsed {
        title: feed
            .title
            .map(|t| t.content.trim().to_string())
            .filter(|t| !t.is_empty()),
        items,
    })
}

/// Pick the most deliberate image the entry offers: media thumbnail, media
/// content, image enclosure, then the first `<img>` in the body.
fn lead_image(e: &feed_rs::model::Entry, html: &str, base: Option<&Url>) -> Option<String> {
    for m in &e.media {
        if let Some(t) = m.thumbnails.first() {
            return resolve(base, &t.image.uri);
        }
        for c in &m.content {
            let is_image = c
                .content_type
                .as_ref()
                .is_some_and(|t| t.to_string().starts_with("image/"));
            if let (true, Some(u)) = (is_image, &c.url) {
                return Some(u.to_string());
            }
        }
    }
    for l in &e.links {
        if l.rel.as_deref() == Some("enclosure")
            && l.media_type
                .as_deref()
                .is_some_and(|t| t.starts_with("image/"))
        {
            return resolve(base, &l.href);
        }
    }
    first_img(html, base)
}

fn first_img(html: &str, base: Option<&Url>) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(p) = lower[from..].find("<img") {
        let start = from + p;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        let tag = &html[start..end];
        from = end;
        // 1x1 tracking pixels
        if attr(tag, "width").as_deref() == Some("1") || attr(tag, "height").as_deref() == Some("1")
        {
            continue;
        }
        if let Some(src) = attr(tag, "data-src").or_else(|| attr(tag, "src"))
            && let Some(url) = resolve(base, &src)
        {
            return Some(url);
        }
    }
    None
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{name}=");
    let mut search = 0;
    while let Some(p) = lower[search..].find(&needle) {
        let i = search + p;
        search = i + needle.len();
        if i > 0 && !lower.as_bytes()[i - 1].is_ascii_whitespace() {
            continue;
        }
        let rest = &tag[search..];
        let q = rest.chars().next()?;
        let val = if q == '"' || q == '\'' {
            rest[1..].split(q).next()?
        } else {
            rest.split(|c: char| c.is_whitespace() || c == '>').next()?
        };
        return Some(val.to_string());
    }
    None
}

fn resolve(base: Option<&Url>, src: &str) -> Option<String> {
    let src = src.trim().replace("&amp;", "&");
    if src.is_empty() || src.starts_with("data:") {
        return None;
    }
    if let Some(rest) = src.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if let Ok(u) = Url::parse(&src) {
        return matches!(u.scheme(), "http" | "https").then(|| u.to_string());
    }
    base?.join(&src).ok().map(|u| u.to_string())
}

pub fn fetch_image(client: &Client, url: &str) -> Result<DynamicImage> {
    let resp = client.get(url).send()?.error_for_status()?;
    if resp.content_length().is_some_and(|n| n > MAX_IMAGE_BYTES) {
        bail!("image too large");
    }
    let mut buf = Vec::new();
    resp.take(MAX_IMAGE_BYTES).read_to_end(&mut buf)?;
    // Pre-shrink so the render-time resize stays cheap.
    Ok(image::load_from_memory(&buf)?.thumbnail(1200, 1200))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_first_real_image_and_resolves_relative() {
        let base = Url::parse("https://example.com/posts/1").unwrap();
        let html = r#"<p>hi</p><IMG width="1" src="/pixel.gif"><img class="x" src='/a/b.png?x=1&amp;y=2' alt="z">"#;
        assert_eq!(
            first_img(html, Some(&base)).as_deref(),
            Some("https://example.com/a/b.png?x=1&y=2")
        );
    }

    #[test]
    fn skips_data_uris_and_prefers_data_src() {
        assert_eq!(
            first_img(r#"<img src="data:image/png;base64,AAA">"#, None),
            None
        );
        assert_eq!(
            first_img(
                r#"<img src="lazy.gif" data-src="https://c.io/r.jpg">"#,
                None
            )
            .as_deref(),
            Some("https://c.io/r.jpg")
        );
    }

    #[test]
    fn protocol_relative_urls_get_https() {
        assert_eq!(
            resolve(None, "//cdn.x.com/i.png").as_deref(),
            Some("https://cdn.x.com/i.png")
        );
    }
}

#[cfg(test)]
mod net_tests {
    #[test]
    #[ignore = "network"]
    fn nasa_has_images() {
        let c = super::client().unwrap();
        let p = super::fetch(&c, "https://www.nasa.gov/feeds/iotd-feed/").unwrap();
        let img = p.items[0].image.clone();
        println!("image = {img:?}");
        let i = super::fetch_image(&c, &img.unwrap()).unwrap();
        println!("{}x{}", i.width(), i.height());
    }
}
