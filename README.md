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

## Installing

One line per platform. Each script pulls the latest release, so it is the same
command whether this is a first install or a reinstall.

**macOS** — Apple Silicon or Intel, detected for you:

```sh
curl -fsSL https://raw.githubusercontent.com/joe-lloyd/homelab-music/main/scripts/install-macos.sh | bash
```

**Linux** — AppImage into `~/.local/bin`, plus a desktop entry. No root:

```sh
curl -fsSL https://raw.githubusercontent.com/joe-lloyd/homelab-music/main/scripts/install-linux.sh | bash
```

**Windows** — per-user install, no admin:

```powershell
irm https://raw.githubusercontent.com/joe-lloyd/homelab-music/main/scripts/install-windows.ps1 | iex
```

### Why the macOS script does two things

There is no Apple Developer ID behind this and there is not going to be one —
it is a personal app for one person's music. macOS therefore refuses it twice,
for two unrelated reasons, and the installer answers both:

- it strips `com.apple.quarantine`, the cause of the misleading *"is damaged and
  can't be opened"*;
- it applies an **ad-hoc signature**, because Apple Silicon will not run an
  unsigned arm64 binary at all, regardless of Gatekeeper.

It touches only the one bundle. Gatekeeper stays on and no system-wide policy
changes. `spctl --assess` will still report "rejected" afterwards — that is the
honest answer to *"is this notarised"*, and it is not what stops the app running.

`scripts/macos-allow-unsigned.sh` still exists for a build you obtained some
other way; the installer just folds the same steps in.

## Updating

The app updates itself. It checks once at startup and never installs on its
own — finding an update posts a notification, and the tray installs it when you
choose to. That is deliberate: installing means relaunching, and relaunching
mid-album to save thirty seconds is a bad trade.

A failed check stays quiet unless you asked for it from the tray. Away from home
and outside the tunnel the app genuinely cannot reach GitHub, and that is an
ordinary state rather than something worth interrupting you about.

Worth knowing: Tauri's updater replaces the **whole bundle** rather than
patching it. There is no delta mechanism. The macOS and Windows bundles are
3–5 MB, so that costs less than the machinery to avoid it would. The Linux
AppImage is ~79 MB because it carries its own WebKit; if that becomes annoying,
the `.deb` is 5 MB and uses the system one.

## Releasing

```sh
git tag v0.1.0 && git push origin v0.1.0
```

That builds macOS (both architectures), Linux and Windows, signs every bundle
with the updater key, and publishes them with `latest.json` — the file running
copies poll. The signing key lives in the repository's Actions secrets; the
public half is in `tauri.conf.json`. **Without those secrets the bundles still
build and the update path is silently dead**, which is a failure you would not
notice until the second release.

## Status

| | |
|---|---|
| Tray, window, close-to-tray | done |
| Embedded UI from the shared package | done — React + TypeScript since 0.2.0 |
| Reverse proxy, CA pinning, Range passthrough | done |
| Home-vs-away detection | done |
| Embedded-UI drift check | done — 0.3.0 |
| Userspace WireGuard tunnel | **next** |
| Media keys, now-playing, notifications | **next** |

### Knowing the embedded UI has gone stale

The UI is compiled in, which is deliberate (see above) and has one cost: this
app ships a *snapshot* of a repo that moves on its own. Push to music-ui,
deploy music-dump, and the web app has the new front end while this one keeps
serving whatever it was last built with. There is no symptom — the app works,
it is just older than the server it is talking to.

So it asks. music-dump serves `GET /api/ui-build`, a sha256 over the files it
serves; `Ui::digest()` computes the same hash over the files embedded here. At
startup, once the network path is settled, `uicheck` compares them and logs the
answer; a genuine mismatch also raises one notification, because the fix is a
rebuild and nothing at runtime can do it.

Being unable to check is reported as `Unknown`, never as stale — offline, mid
tunnel, or an older server with no such endpoint are all "cannot tell", and
crying stale over a dropped network would train you to ignore the one that
matters.

The hash rule is a cross-language contract: file name then bytes, in basename
order. `uicheck`'s tests pin the digest of the current bundle so a change to
the rule fails here rather than silently reporting every build as out of date,
and a `--ignored` test checks it against the live server on the home LAN.

Bumping the `ui/` submodule is automated from the other side: music-ui's
`bump-consumers` workflow opens a PR here on every push that changes what this
app vendors. Merging it is not shipping it — the UI is in the binary, so a
release build has to follow.

The tunnel is the interesting remaining piece: `onetun`-style userspace
WireGuard (`boringtun` + `smoltcp`), no TUN device, no driver, no admin rights,
reusing the **existing** `wg0` server on pi-server and the `vpn.jia-lab.cc:51820`
endpoint. The peer is provisioned with `add-client.sh` on pi-server like any
other, and its private key goes into the OS keychain — never onto disk, never
into this repo.

Media keys are the shakiest item: `tauri-plugin-global-shortcut` doesn't support
`MediaPlayPause` (it panics on the keycode), so the plan is `tauri-plugin-media`
with a plain configurable shortcut as the fallback.
