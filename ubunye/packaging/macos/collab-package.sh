#!/usr/bin/env bash
# Ubunye Film for macOS with ThabangVision collaboration (kopanod) inside the app:
# Ubunye Film.app/Contents/MacOS/kopanod, with filmcraft-cli beside it (kopanod makes footage
# proxies with it), both signed with the app (inside out) by packaging/macos/package.sh.
# On first launch Film makes the device key, adds a LaunchAgent (start at sign-in) and starts
# it (apps/filmcraft/src/ubunye_collab.rs).
#
# Usage: ubunye/packaging/macos/collab-package.sh <kopano lab dir> [package.sh arguments...]
# Signing and notarization: the same environment as packaging/macos/package.sh (none = ad-hoc).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
LAB="${1:?kopano lab dir}"
shift
ARCH=universal
args=("$@")
for i in "${!args[@]}"; do
  if [ "${args[$i]}" = --arch ]; then ARCH="${args[$((i + 1))]}"; fi
done
OUT="${KOPANO_TARGET_DIR:-$LAB/target}/film-collab/macos-$ARCH"
rm -rf "$OUT"
"$ROOT/ubunye/packaging/build-kopanod.sh" "$LAB" "$OUT" --arch "$ARCH"
UBUNYE_EXTRA_BIN_DIR="$OUT" "$ROOT/packaging/macos/package.sh" "$@"
