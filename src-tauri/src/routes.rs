// The UI, embedded, and the map of what it serves.
//
// Both come from the ui/ submodule -- the same package the web server reads --
// so the desktop app cannot serve a different front end than music.home.arpa
// does. routes.json is deliberately JSON rather than JavaScript precisely so
// this file can read it; see ui/README.md.

use std::collections::HashMap;

use include_dir::{include_dir, Dir};
use serde::Deserialize;

/// The UI assets, baked into the binary. Nothing is read from disk at runtime,
/// so a user cannot end up with a half-updated app after moving files around.
static UI: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../ui/public");

const MANIFEST: &str = include_str!("../../ui/routes.json");

#[derive(Deserialize)]
struct RawEntry {
    file: String,
    #[serde(rename = "type")]
    content_type: String,
    #[serde(rename = "cacheControl")]
    cache_control: String,
}

#[derive(Deserialize)]
struct RawDocument {
    urls: Vec<String>,
    file: String,
    #[serde(rename = "type")]
    content_type: String,
    #[serde(rename = "cacheControl")]
    cache_control: String,
}

#[derive(Deserialize)]
struct RawManifest {
    document: RawDocument,
    #[serde(rename = "static")]
    statics: HashMap<String, RawEntry>,
}

/// A resolved asset: the bytes to send and the headers to send them with.
pub struct Asset {
    pub body: &'static [u8],
    pub content_type: String,
    pub cache_control: String,
}

pub struct Ui {
    document_urls: Vec<String>,
    assets: HashMap<String, Asset>,
    document: Asset,
}

impl Ui {
    /// Parse the shared manifest and bind every route to embedded bytes.
    ///
    /// A route naming a file that is not in the bundle is a build-time
    /// packaging mistake, not a runtime condition, so this returns an error
    /// rather than quietly serving a 404 later.
    pub fn load() -> anyhow::Result<Self> {
        let raw: RawManifest = serde_json::from_str(MANIFEST)?;

        // Paths in routes.json are relative to the package root ("public/x"),
        // while the embedded Dir is rooted *at* public/. Strip the prefix.
        let bytes = |rel: &str| -> anyhow::Result<&'static [u8]> {
            let name = rel.strip_prefix("public/").unwrap_or(rel);
            UI.get_file(name).map(|f| f.contents()).ok_or_else(|| {
                anyhow::anyhow!("routes.json names {rel}, which is not in the bundle")
            })
        };

        let document = Asset {
            body: bytes(&raw.document.file)?,
            content_type: raw.document.content_type,
            cache_control: raw.document.cache_control,
        };

        let mut assets = HashMap::new();
        for (url, entry) in raw.statics {
            assets.insert(
                url,
                Asset {
                    body: bytes(&entry.file)?,
                    content_type: entry.content_type,
                    cache_control: entry.cache_control,
                },
            );
        }

        Ok(Self {
            document_urls: raw.document.urls,
            assets,
            document,
        })
    }

    /// The asset for a path, if this path is part of the UI at all.
    /// Anything that returns None belongs to the server and gets proxied.
    pub fn resolve(&self, path: &str) -> Option<&Asset> {
        if self.document_urls.iter().any(|u| u == path) {
            return Some(&self.document);
        }
        self.assets.get(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_and_every_route_has_bytes() {
        let ui = Ui::load().expect("routes.json should parse and resolve");
        for path in ["/", "/index.html", "/app.css", "/player.js", "/icon.svg"] {
            let asset = ui
                .resolve(path)
                .unwrap_or_else(|| panic!("no asset for {path}"));
            assert!(!asset.body.is_empty(), "{path} resolved to zero bytes");
        }
    }

    #[test]
    fn api_paths_are_not_ui_and_must_be_proxied() {
        let ui = Ui::load().unwrap();
        assert!(ui.resolve("/api/player/resolve").is_none());
        assert!(ui.resolve("/img/folder").is_none());
    }
}
