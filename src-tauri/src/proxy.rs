// The app's entire network stack.
//
// A system WebView gives no per-app proxy hook -- there is no equivalent of
// Electron's session.setProxy() for WKWebView or WebView2 -- so we cannot point
// the WebView at music.home.arpa and expect its traffic to enter a userspace
// tunnel. Instead the WebView talks only to our custom scheme, and everything
// it asks for that is not part of the embedded UI is fetched here, by us.
//
// That falls out cleanly because every request player.js makes is
// root-relative (/api/player/resolve, /api/player/stream, ...). It resolves
// against whatever origin served the page, so pointing that origin at this
// handler needs no change to the UI at all.
//
// Two things come free with it: the homelab CA is pinned here rather than
// installed into either OS trust store, and Range requests -- which seeking
// and the player's ~20s prefetch depend on -- pass straight through.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Certificate, Client, Method};

use crate::netpath::{Path, HOME_ADDR, HOME_HOST};

/// The Caddy Local Authority root. Bundled rather than installed: the app
/// trusts it, the machine does not have to.
const HOMELAB_CA: &[u8] = include_bytes!("../assets/homelab-ca.crt");

/// Long enough for the Pi to answer while its Jellyfin index rebuilds (that
/// path is documented as slow), short enough that a dead tunnel surfaces as an
/// error rather than a spinner that never resolves.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

/// Hop-by-hop headers, which are meaningless to forward and actively harmful
/// if we do -- a forwarded `Connection: keep-alive` or a second
/// `Transfer-Encoding` confuses the WebView about framing.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
];

pub struct Proxy {
    client: Client,
}

impl Proxy {
    /// Build the client for a given network path.
    ///
    /// The certificate is issued for music.home.arpa, so that name is what we
    /// send as SNI and Host on *both* paths. On the LAN we simply tell reqwest
    /// which address that name lives at, which also means we never depend on
    /// the machine's DNS agreeing with us -- away from home the resolver will
    /// know nothing about .home.arpa at all.
    pub fn new(path: Path, tunnel_addr: Option<std::net::SocketAddr>) -> Result<Self> {
        let ca =
            Certificate::from_pem(HOMELAB_CA).context("bundled homelab CA is not valid PEM")?;

        let addr = match path {
            Path::Lan => HOME_ADDR,
            // The tunnel exposes pi-server's 443 on a loopback port. Same name,
            // same certificate, different socket.
            Path::Tunnel => {
                tunnel_addr.context("tunnel path selected but the tunnel is not listening yet")?
            }
        };

        let client = Client::builder()
            .add_root_certificate(ca)
            // Only our CA is trusted. A public WiFi captive portal cannot
            // MITM this even with a certificate the OS would accept.
            .tls_built_in_root_certs(false)
            .resolve(HOME_HOST, addr)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("building the HTTP client")?;

        Ok(Self { client })
    }

    /// Forward one request to music.home.arpa and return what came back.
    ///
    /// The body is collected rather than streamed, because the custom-scheme
    /// responder we answer through takes a finished `Vec<u8>` and gives us no
    /// way to hand back a stream. So the only way to keep a response small is
    /// to make sure we never ASK for a big one -- see `cap_range`, which is
    /// what makes "collected" affordable rather than ruinous.
    pub async fn forward(
        &self,
        method: &str,
        path_and_query: &str,
        headers: &HeaderMap,
        body: Vec<u8>,
    ) -> Result<http::Response<Vec<u8>>> {
        let url = format!("https://{HOME_HOST}{path_and_query}");
        let method = Method::from_bytes(method.as_bytes()).context("bad method")?;

        let mut req = self.client.request(method, &url);
        for (name, value) in headers {
            if HOP_BY_HOP.contains(&name.as_str()) {
                continue;
            }
            req = req.header(name, value);
        }
        if !body.is_empty() {
            req = req.body(body);
        }

        let upstream = req.send().await.context("upstream request failed")?;
        let status = upstream.status();
        let upstream_headers = upstream.headers().clone();
        let bytes = upstream.bytes().await.context("reading upstream body")?;

        let mut out = http::Response::builder().status(status.as_u16());
        for (name, value) in upstream_headers.iter() {
            if HOP_BY_HOP.contains(&name.as_str()) {
                continue;
            }
            out = out.header(name, value);
        }
        // The WebView is a different origin from the server's point of view;
        // without this the fetches player.js makes are blocked before they
        // are ever sent.
        out = out.header("access-control-allow-origin", HeaderValue::from_static("*"));

        out.body(bytes.to_vec()).context("building response")
    }
}

/// Strip headers the WebView sets that would confuse the upstream server.
pub fn sanitise(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        if HOP_BY_HOP.contains(&name.as_str()) {
            continue;
        }
        if let Ok(n) = HeaderName::from_bytes(name.as_ref()) {
            out.insert(n, value.clone());
        }
    }
    cap_range(&mut out);
    out
}

/// How much of a track to fetch per upstream request.
///
/// Big enough that a FLAC does not turn into a request storm (4 MiB is the
/// better part of a minute of 24/48 audio), small enough that the first bytes
/// reach the player promptly. Nothing here depends on the exact figure.
const RANGE_CHUNK: u64 = 4 * 1024 * 1024;

/// Bound an open-ended `Range` so one response cannot be a whole album track.
///
/// WebView2 does not ask for a window, it asks for `bytes=N-` -- everything
/// from here to the end of the file. Since `forward` has to collect the whole
/// body before it can answer, an open-ended range means downloading the entire
/// remainder of a 90 MB FLAC before the player is given a single byte, and
/// re-downloading it from a new offset every time the media pipeline re-seeks.
/// Measured against the real library that was 627 MB of transfer for six
/// tracks, with 2.5-4.3 s of silence at every track boundary -- which is
/// exactly when `next()` needs the audio element to be responsive.
///
/// Turning `bytes=N-` into `bytes=N-(N+CHUNK-1)` is an ordinary, honest HTTP
/// request: the server answers 206 with a `Content-Range` naming the true
/// total size, so the player learns the real duration and simply asks for the
/// next window when it needs it. Ranges that already name an end are left
/// alone -- the caller has said what it wants -- and so is a request with no
/// `Range` at all, which is how every JSON and image fetch stays untouched.
fn cap_range(headers: &mut HeaderMap) {
    let Some(start) = headers
        .get("range")
        .and_then(|v| v.to_str().ok())
        .and_then(open_ended_start)
    else {
        return;
    };
    let end = start.saturating_add(RANGE_CHUNK - 1);
    if let Ok(value) = HeaderValue::from_str(&format!("bytes={start}-{end}")) {
        headers.insert("range", value);
    }
}

/// The start offset of a `bytes=N-` header, or None if it is anything else.
fn open_ended_start(value: &str) -> Option<u64> {
    let spec = value.trim().strip_prefix("bytes=")?;
    // A multi-range request ("bytes=0-99,200-") is a different response shape
    // (multipart/byteranges) and not something to rewrite behind the caller.
    if spec.contains(',') {
        return None;
    }
    let start = spec.strip_suffix('-')?;
    start.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_ca_is_a_usable_certificate() {
        Certificate::from_pem(HOMELAB_CA).expect("bundled CA must parse");
    }

    #[test]
    fn hop_by_hop_headers_are_dropped() {
        let mut h = HeaderMap::new();
        h.insert("connection", HeaderValue::from_static("keep-alive"));
        h.insert("host", HeaderValue::from_static("evil.example"));
        h.insert("range", HeaderValue::from_static("bytes=0-1023"));
        let out = sanitise(&h);
        assert!(!out.contains_key("connection"));
        assert!(
            !out.contains_key("host"),
            "a forwarded Host would defeat SNI pinning"
        );
        assert_eq!(
            out.get("range").map(|v| v.to_str().unwrap()),
            Some("bytes=0-1023"),
            "Range must survive -- seeking depends on it",
        );
    }

    /// The whole point of cap_range: an open-ended range is what WebView2
    /// actually sends for every audio request, and answering it literally
    /// means buffering the rest of the file before the player hears anything.
    #[test]
    fn an_open_ended_range_is_bounded_to_one_chunk() {
        let mut h = HeaderMap::new();
        h.insert("range", HeaderValue::from_static("bytes=0-"));
        let out = sanitise(&h);
        assert_eq!(
            out.get("range").unwrap().to_str().unwrap(),
            format!("bytes=0-{}", RANGE_CHUNK - 1),
            "bytes=0- must not be forwarded as-is",
        );
    }

    #[test]
    fn capping_starts_from_the_offset_the_player_asked_for() {
        let mut h = HeaderMap::new();
        h.insert("range", HeaderValue::from_static("bytes=5570560-"));
        let out = sanitise(&h);
        assert_eq!(
            out.get("range").unwrap().to_str().unwrap(),
            format!("bytes=5570560-{}", 5570560 + RANGE_CHUNK - 1),
        );
    }

    /// Seeking sends a bounded range already. Rewriting it would narrow a
    /// window the caller deliberately chose.
    #[test]
    fn a_range_that_already_names_an_end_is_left_alone() {
        let mut h = HeaderMap::new();
        h.insert("range", HeaderValue::from_static("bytes=0-1023"));
        let out = sanitise(&h);
        assert_eq!(out.get("range").unwrap().to_str().unwrap(), "bytes=0-1023");
    }

    /// Every JSON and image fetch goes through here too, and none of them
    /// should acquire a Range header they never asked for.
    #[test]
    fn a_request_with_no_range_does_not_gain_one() {
        let h = HeaderMap::new();
        assert!(sanitise(&h).get("range").is_none());
    }

    /// A multi-range request answers as multipart/byteranges. Rewriting it to
    /// a single window would change the response shape under the caller.
    #[test]
    fn multi_range_and_suffix_requests_are_not_rewritten() {
        for raw in ["bytes=0-99,200-", "bytes=-500"] {
            let mut h = HeaderMap::new();
            h.insert("range", HeaderValue::from_str(raw).unwrap());
            let out = sanitise(&h);
            assert_eq!(
                out.get("range").unwrap().to_str().unwrap(),
                raw,
                "{raw} should pass through untouched",
            );
        }
    }

    #[test]
    fn lan_client_builds() {
        Proxy::new(Path::Lan, None).expect("LAN client should build with no tunnel");
    }

    #[test]
    fn tunnel_path_without_a_tunnel_is_an_error_not_a_silent_lan_fallback() {
        assert!(Proxy::new(Path::Tunnel, None).is_err());
    }

    /// Talks to the real pi-server, so it is ignored by default and CI never
    /// runs it. Run it on the home LAN to prove the whole chain end to end:
    ///
    ///     cargo test -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "needs the home LAN"]
    async fn reaches_the_live_server_with_only_the_bundled_ca() {
        let proxy = Proxy::new(Path::Lan, None).expect("client builds");

        let overview = proxy
            .forward("GET", "/api/overview", &HeaderMap::new(), Vec::new())
            .await
            .expect("GET /api/overview should succeed against pi-server");
        assert_eq!(overview.status(), 200);
        assert!(!overview.body().is_empty(), "overview returned no data");

        // The upstream serves static assets whole -- only /api/player/stream is
        // range-aware, and that needs a resolved track id, which is too
        // stateful to assert here. What this does prove is that bytes survive
        // the round trip intact: the server's copy of app.css must be the same
        // one we embedded, since both come from the music-ui package.
        let css = proxy
            .forward("GET", "/app.css", &HeaderMap::new(), Vec::new())
            .await
            .expect("GET /app.css should succeed");
        assert_eq!(css.status(), 200);

        let embedded = crate::routes::Ui::load()
            .expect("embedded UI loads")
            .resolve("/app.css")
            .expect("app.css is a UI route")
            .body;
        assert_eq!(
            css.body().as_slice(),
            embedded,
            "the server and this binary disagree about app.css -- the ui/ \
             submodule here is at a different commit than the one deployed",
        );
    }
}
