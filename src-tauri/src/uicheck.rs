// Is the UI baked into this binary still the UI home is serving?
//
// The desktop app EMBEDS the front end (`include_dir!` over ui/public) rather
// than fetching it, which is deliberate -- see routes.rs -- but it means the
// app ships a *snapshot* of a repo that moves independently. Push to music-ui,
// deploy music-dump, and the web app has the new front end while the desktop
// keeps serving whatever it was last built with. Nothing about that is
// visible: the app works, it is just older than the server it talks to, and
// the only symptom is a fix that "did not arrive" on the desktop.
//
// So ask. music-dump exposes `/api/ui-build`, a sha256 over the files it
// serves; routes.rs computes the same hash over the files we embedded. Equal
// means the two agree. A digest rather than a version string because there is
// no release step here that could be trusted to bump a number, and bytes
// cannot lie about what was compiled in.
//
// This never blocks anything and never self-heals. It cannot: the UI is in the
// binary, so the fix is a rebuild, and the only useful thing to do at runtime
// is say so.

use std::sync::Arc;

use crate::proxy::Proxy;

/// What one check concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drift {
    /// The embedded UI matches what home is serving.
    Current,
    /// It does not. Carries both digests, short, for the log line.
    Stale { embedded: String, serving: String },
    /// Could not tell -- offline, mid-tunnel, or an older server that has no
    /// `/api/ui-build`. Explicitly not "stale": accusing the build of being
    /// out of date because the network was down would train you to ignore it.
    Unknown(String),
}

/// The first 12 hex chars, which is plenty to tell two sha256s apart by eye
/// and short enough to read in a log line.
fn short(digest: &str) -> &str {
    &digest[..digest.len().min(12)]
}

/// Compare a digest against what `/api/ui-build` reports.
///
/// Split from the fetch so the comparison is testable without a server.
pub fn compare(embedded: &str, body: &[u8]) -> Drift {
    let parsed: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return Drift::Unknown(format!("unreadable answer: {e}")),
    };
    match parsed.get("digest").and_then(|d| d.as_str()) {
        Some(serving) if serving == embedded => Drift::Current,
        Some(serving) => Drift::Stale {
            embedded: embedded.to_owned(),
            serving: serving.to_owned(),
        },
        None => Drift::Unknown("answer carried no digest".into()),
    }
}

/// Ask home which UI it is serving and compare it with ours.
pub async fn check(proxy: Arc<Proxy>, embedded: &str) -> Drift {
    let response = match proxy
        .forward(
            "GET",
            "/api/ui-build",
            &reqwest::header::HeaderMap::new(),
            Vec::new(),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => return Drift::Unknown(format!("{e:#}")),
    };
    // A server older than this feature answers 404, which is a legitimate
    // "cannot tell" and not a drift.
    if response.status() != 200 {
        return Drift::Unknown(format!("/api/ui-build answered {}", response.status()));
    }
    compare(embedded, response.body())
}

/// Run the check and say what it found, once, at startup.
pub fn report(drift: &Drift) {
    match drift {
        Drift::Current => log::info!("embedded UI matches home"),
        Drift::Stale { embedded, serving } => log::warn!(
            "embedded UI is not what home is serving (ours {}, home {}) -- \
             this build predates a music-ui change; rebuild to pick it up",
            short(embedded),
            short(serving),
        ),
        Drift::Unknown(why) => log::info!("could not check the embedded UI: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::Ui;

    #[test]
    fn an_identical_digest_is_current() {
        assert_eq!(compare("abc123", br#"{"digest":"abc123"}"#), Drift::Current);
    }

    #[test]
    fn a_different_digest_is_stale_and_keeps_both_sides() {
        let drift = compare("aaaa", br#"{"digest":"bbbb"}"#);
        assert_eq!(
            drift,
            Drift::Stale {
                embedded: "aaaa".into(),
                serving: "bbbb".into()
            }
        );
    }

    #[test]
    fn a_server_that_cannot_answer_is_unknown_rather_than_stale() {
        // The distinction that matters: an old server, or a garbled answer,
        // must never be reported as "your build is out of date".
        assert!(matches!(compare("aaaa", b"not json"), Drift::Unknown(_)));
        assert!(matches!(compare("aaaa", b"{}"), Drift::Unknown(_)));
    }

    #[test]
    fn the_digest_is_stable_and_covers_every_file_the_manifest_names() {
        let ui = Ui::load().unwrap();
        let first = ui.digest();
        assert_eq!(first, ui.digest(), "digest must not vary between calls");
        assert_eq!(first.len(), 64, "sha256 renders as 64 hex chars");
        // A digest over an empty file set would also be stable, and useless.
        assert_ne!(
            first,
            format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(b"")),
            "digest is the hash of nothing -- no files were fed in"
        );
    }

    #[test]
    fn short_does_not_panic_on_a_digest_shorter_than_the_window() {
        assert_eq!(short("abc"), "abc");
    }
}

#[cfg(test)]
mod crosscheck {
    use crate::routes::Ui;

    /// The digest is a CROSS-LANGUAGE contract with music-dump's `uiDigest()`,
    /// so what has to stay fixed is the RULE: sha256 over each file's name
    /// then its bytes, in name order.
    ///
    /// Pinned against a fixture rather than against the real bundle. The old
    /// version asserted the digest of whatever `ui/` currently held, which
    /// meant every submodule bump broke it -- and those bumps are opened
    /// automatically, so the failing check was guaranteed and therefore
    /// worthless. A fixture pins the same rule and never goes stale.
    ///
    /// The expected value is what music-dump's implementation produces for
    /// this input:
    ///
    ///     const h = createHash('sha256');
    ///     for (const [n, b] of [['a.css','one'], ['b.js','two']].sort())
    ///       { h.update(n); h.update(Buffer.from(b)); }
    ///     h.digest('hex');
    #[test]
    fn the_hashing_rule_matches_the_other_implementation() {
        let files: Vec<(&str, &[u8])> = vec![("b.js", b"two"), ("a.css", b"one")];
        assert_eq!(
            crate::routes::digest_files(files.into_iter()),
            "8d99b1fe80afd0d2ace636d8e4dc04d9f9ec27ff6804f833ae4416e00e749626",
        );
    }

    /// Name order, not insertion order. Two bundles with the same files must
    /// hash the same however the manifest happened to list them.
    #[test]
    fn ordering_is_by_name_not_by_insertion() {
        let one: Vec<(&str, &[u8])> = vec![("a.css", b"one"), ("b.js", b"two")];
        let other: Vec<(&str, &[u8])> = vec![("b.js", b"two"), ("a.css", b"one")];
        assert_eq!(
            crate::routes::digest_files(one.into_iter()),
            crate::routes::digest_files(other.into_iter()),
        );
    }

    /// The name is mixed in, not just the bytes. Renaming a file must change
    /// the digest, or a bundle that moved app.js to app2.js would read as
    /// current.
    #[test]
    fn the_file_name_is_part_of_the_hash() {
        let before: Vec<(&str, &[u8])> = vec![("a.css", b"one")];
        let after: Vec<(&str, &[u8])> = vec![("renamed.css", b"one")];
        assert_ne!(
            crate::routes::digest_files(before.into_iter()),
            crate::routes::digest_files(after.into_iter()),
        );
    }

    /// The contract is with a server written in another language, so the only
    /// test that really proves it is one that asks the real one. Ignored by
    /// default like the proxy's own live test; on the home LAN:
    ///
    ///     cargo test -- --ignored --nocapture
    ///
    /// A `Stale` here is a true finding, not a broken test: it means this
    /// checkout's ui/ submodule is behind what pi-server is serving.
    #[tokio::test]
    #[ignore = "needs the home LAN"]
    async fn agrees_with_the_live_server_about_the_current_bundle() {
        use crate::netpath::Path;
        use crate::proxy::Proxy;
        use std::sync::Arc;

        let proxy = Arc::new(Proxy::new(Path::Lan, None).expect("client builds"));
        let drift = super::check(proxy, &Ui::load().unwrap().digest()).await;
        assert_eq!(
            drift,
            super::Drift::Current,
            "the embedded bundle and pi-server's disagree"
        );
    }
}
