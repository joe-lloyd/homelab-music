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
    /// The body is collected rather than streamed. That is a real tradeoff and
    /// worth stating: `<audio>` fetches with Range headers, so in practice each
    /// response here is a bounded chunk, not a whole 40 MB FLAC. A request that
    /// arrives with no Range will buffer the entire file -- correct, just not
    /// free. If that shows up as a memory spike on long tracks, this is the
    /// function to make streaming.
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
    out
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
