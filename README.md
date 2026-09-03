# homelab-music

A tray-resident desktop player for the home music library, on macOS and
Windows, that carries **its own WireGuard tunnel** so it works from anywhere
without putting the whole machine on the VPN.

The music player itself already existed at `https://music.home.arpa`. Two things
it could not be as a browser tab:

1. **It wasn't there when you wanted it.** No tray icon, no media keys, no
   now-playing. You had to go find the tab.
2. **It only worked at home.** Off the LAN you had to bring up WireGuard for the
   entire machine, which is a heavy thing to do just to hear an album.

## How it works, and why it has to work this way

```
WebView ──► homelab://…  ─┬─► embedded UI bytes (index.html, app.css, player.js)
                          │
                          └─► reverse proxy ──┬─► 192.168.2.23:443   (at home)
                              pins homelab CA │
                              Host/SNI =      └─► userspace WireGuard (away)
                              music.home.arpa      → vpn.jia-lab.cc:51820
```

A system WebView gives no per-app proxy hook — there is no `session.setProxy()`
for WKWebView or WebView2 — so we cannot simply point it at `music.home.arpa`
and expect its traffic to enter a userspace tunnel. **So the Rust side is the
app's entire network stack.** The WebView only ever talks to the `homelab://`
scheme; this process answers it, either from bytes baked into the binary or by
fetching from home itself.

That works without touching the player at all, because every request
`player.js` makes is **root-relative** (`/api/player/resolve`,
`/api/player/stream`, …). Those resolve against whatever origin served the
page, so pointing that origin at our handler is invisible to the UI.

Three things fall out of it for free:

- **No certificate installation.** `music.home.arpa` is signed by the Caddy
  Local Authority. The app bundles that root and pins it — neither macOS nor
  Windows has to trust anything, and only our CA is accepted, so a captive
  portal can't MITM the connection even with a cert the OS *would* accept.
- **Seeking works.** `Range` headers pass straight through, which is what
  scrubbing and the player's ~20-second gapless prefetch depend on.
- **No reliance on `192.168.2.23:8091`.** That port serves the whole app
  unauthenticated on the LAN and is a known bug; this goes through Caddy on 443
  like everything else.

## The UI is not in this repo

It lives in [`music-ui`](https://github.com/joe-lloyd/music-ui), vendored here
as the `ui/` submodule, and is embedded into the binary at compile time. The web
app at `music.home.arpa` serves the identical package. One copy, pinned by
commit on each side, so a fix to the lyric scroll can't land in one and be
forgotten in the other.

`ui/routes.json` decides what serves at which URL. `src-tauri/src/routes.rs`
reads that manifest rather than restating it — that manifest is JSON rather than
JavaScript specifically so this Rust can read it.

```sh
git clone --recurse-submodules https://github.com/joe-lloyd/homelab-music.git
```

`--recurse-submodules` is not optional; the build embeds `ui/public` and fails
without it.

## Home vs away

On startup and on every network change the app opens a TCP connection to
`192.168.2.23:443`. Reachable means we're home — go direct, over gigabit
ethernet. Not reachable means bring up the tunnel.

This is a TCP connect rather than a ping or a DNS lookup on purpose: DNS for
`.home.arpa` resolves through AdGuard *on pi-server*, so a name resolving
proves nothing about whether the box is reachable. Opening the socket we
actually intend to use is the only probe that can't lie.

Worth knowing: measured throughput over `wg0` is about **7.9 Mbit/s**, and
`/api/player/stream` serves the original file (`?static=true`, never
transcoded). A FLAC stream is roughly 1 Mbit/s, so it fits — with the prefetch,
not by a lot. Taking the tunnel when you didn't have to would be audible.

## Building

Needs Rust and, on Windows, the MSVC C++ build tools.

```sh
cargo build --manifest-path src-tauri/Cargo.toml            # dev
cargo tauri build                                           # installers
python scripts/make-icons.py                                # after a ui/ bump
```

macOS binaries cannot be cross-compiled from Windows or Linux — they're built
by CI on a `macos-latest` runner, or on the Mac itself.

## Running an unsigned build on macOS

There's no Apple Developer ID behind this and there isn't going to be one — it's
a personal app for one person's music. macOS will refuse it twice, for two
different reasons. `scripts/macos-allow-unsigned.sh` answers both:

```sh
./scripts/macos-allow-unsigned.sh                 # /Applications/Homelab Music.app
./scripts/macos-allow-unsigned.sh ~/Downloads/Homelab\ Music.app
```

It strips `com.apple.quarantine` (the cause of the misleading *"is damaged and
can't be opened"*) and applies an **ad-hoc signature**, which Apple Silicon
requires before the kernel will run an arm64 binary at all. It touches only the
bundle you name — it does not disable Gatekeeper or change any system-wide
policy. `spctl --assess` will still say "rejected" afterwards; that's the honest
answer to *"is this notarised"*, and it isn't what stops the app running.

## Status

| | |
|---|---|
| Tray, window, close-to-tray | done |
| Embedded UI from the shared package | done |
| Reverse proxy, CA pinning, Range passthrough | done |
| Home-vs-away detection | done |
| Userspace WireGuard tunnel | **next** |
| Media keys, now-playing, notifications | **next** |

The tunnel is the interesting remaining piece: `onetun`-style userspace
WireGuard (`boringtun` + `smoltcp`), no TUN device, no driver, no admin rights,
reusing the **existing** `wg0` server on pi-server and the `vpn.jia-lab.cc:51820`
endpoint. The peer is provisioned with `add-client.sh` on pi-server like any
other, and its private key goes into the OS keychain — never onto disk, never
into this repo.

Media keys are the shakiest item: `tauri-plugin-global-shortcut` doesn't support
`MediaPlayPause` (it panics on the keycode), so the plan is `tauri-plugin-media`
with a plain configurable shortcut as the fallback.
