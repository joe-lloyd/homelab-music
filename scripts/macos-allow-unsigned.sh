#!/usr/bin/env bash
#
# Make an unsigned Homelab Music build launchable on macOS.
#
# There is no Apple Developer ID behind this app and there is not going to be
# one -- it is a personal tray app for one person's music library. macOS
# therefore refuses it twice, for two different reasons, and both have to be
# answered:
#
#   1. "Homelab Music is damaged and can't be opened."
#      Not damage. The download carried com.apple.quarantine, and Gatekeeper
#      will not evaluate an unsigned app that has it.
#
#   2. On Apple Silicon, an arm64 binary with NO signature at all is killed by
#      the kernel before Gatekeeper is even consulted. Ad-hoc signing (`-s -`)
#      satisfies that: it is a real signature, just not one tied to a
#      developer identity. It proves nothing about origin, which is fine --
#      you built it.
#
# This only ever touches the one bundle you name. It does not disable
# Gatekeeper, does not touch `spctl --master-disable`, and does not change any
# system-wide policy. Undo is simply deleting the app.
#
# Usage:
#   ./scripts/macos-allow-unsigned.sh                       # /Applications/Homelab Music.app
#   ./scripts/macos-allow-unsigned.sh path/to/Some.app
#   ./scripts/macos-allow-unsigned.sh ~/Downloads/*.dmg     # a mounted dmg's app

set -euo pipefail

APP="${1:-/Applications/Homelab Music.app}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This script is macOS-only; you are on $(uname -s)." >&2
  exit 1
fi

if [[ ! -d "$APP" ]]; then
  echo "No app bundle at: $APP" >&2
  echo >&2
  echo "Pass the path explicitly, e.g.:" >&2
  echo "  $0 ~/Downloads/Homelab\\ Music.app" >&2
  exit 1
fi

echo "==> Target: $APP"

# 1. Drop the quarantine flag from the bundle and everything inside it.
#    -r is what matters: the flag sits on nested binaries too, and clearing
#    only the top level leaves the app broken in a confusing, partial way.
if xattr -p com.apple.quarantine "$APP" >/dev/null 2>&1; then
  echo "==> Removing com.apple.quarantine"
  xattr -dr com.apple.quarantine "$APP"
else
  echo "==> No quarantine flag present (already cleared, or built locally)"
fi

# 2. Ad-hoc sign. Required on Apple Silicon, harmless on Intel.
#    --deep covers embedded frameworks and the sidecar binaries; --force
#    replaces any existing ad-hoc signature rather than failing on it.
echo "==> Ad-hoc signing"
codesign --force --deep --sign - "$APP"

# 3. Say plainly what the state now is. `codesign --verify` should pass;
#    `spctl --assess` will still REJECT, and that is expected and correct --
#    the app genuinely has no notarised developer identity. Launching it is
#    now a matter of double-clicking, or right-click -> Open the first time.
echo "==> Verifying"
if codesign --verify --deep --strict "$APP" 2>&1; then
  echo "    signature: ok"
fi

echo
echo "Done. Open it normally."
echo
echo "If macOS still balks on the very first launch, right-click the app and"
echo "choose Open -- that records a one-time exception for this bundle. Note"
echo "that 'spctl --assess' will keep reporting 'rejected'; that is the honest"
echo "answer to 'is this notarised', and it is not what stops the app running."
