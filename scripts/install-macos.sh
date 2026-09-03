#!/usr/bin/env bash
#
# Install Homelab Music on macOS, from the latest GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/joe-lloyd/homelab-music/main/scripts/install-macos.sh | bash
#
# Picks the right build for your CPU, then does the two things macOS requires of
# an app with no Apple Developer ID behind it:
#
#   1. strips com.apple.quarantine -- otherwise Gatekeeper reports the app as
#      "damaged", which it is not;
#   2. applies an ad-hoc signature -- Apple Silicon will not run an unsigned
#      arm64 binary at all, regardless of Gatekeeper.
#
# It touches only this one app. Gatekeeper stays on, and no system-wide policy
# is changed. Uninstall is `rm -rf "/Applications/Homelab Music.app"`.
#
# After this, the app updates itself from the tray -- you should not need to run
# this script twice.

set -euo pipefail

REPO="joe-lloyd/homelab-music"
APP="Homelab Music.app"
DEST="${INSTALL_DIR:-/Applications}"

case "$(uname -m)" in
  arm64) ARCH="aarch64" ;;
  x86_64) ARCH="x64" ;;
  *) echo "Unsupported CPU: $(uname -m)" >&2; exit 1 ;;
esac

echo "==> Looking up the latest release"
# The .app.tar.gz is the updater artifact and is simply a tarred bundle, so it
# installs without mounting a disk image -- fewer moving parts in a piped script.
URL=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
  | grep -o "https://[^\"]*\.app\.tar\.gz" \
  | grep -- "$ARCH" \
  | head -1)

if [[ -z "${URL:-}" ]]; then
  echo "No macOS $ARCH build in the latest release." >&2
  echo "Releases: https://github.com/$REPO/releases" >&2
  exit 1
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "==> Downloading $(basename "$URL")"
curl -fsSL "$URL" -o "$TMP/app.tar.gz"

echo "==> Unpacking"
tar -xzf "$TMP/app.tar.gz" -C "$TMP"
[[ -d "$TMP/$APP" ]] || { echo "Archive did not contain $APP" >&2; exit 1; }

# Replace rather than merge: leaving an old bundle's files behind is how you get
# an app that launches with half of the previous version still inside it.
if [[ -d "$DEST/$APP" ]]; then
  echo "==> Removing the previous install"
  rm -rf "$DEST/$APP"
fi

echo "==> Installing to $DEST"
mv "$TMP/$APP" "$DEST/"

echo "==> Clearing quarantine"
xattr -dr com.apple.quarantine "$DEST/$APP" 2>/dev/null || true

echo "==> Ad-hoc signing"
codesign --force --deep --sign - "$DEST/$APP"

echo
echo "Installed: $DEST/$APP"
echo "Open it from Launchpad, or:  open \"$DEST/$APP\""
echo
echo "Note: 'spctl --assess' will still say rejected. That is the honest answer"
echo "to \"is this notarised\", and it is not what stops the app running."
