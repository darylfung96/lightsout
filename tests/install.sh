#!/bin/sh
# tests/install.sh — exercise install.sh against a local fake release.
#
# No network: the release assets live in a temporary directory and are served
# through curl's file:// transport. The test proves the checksum gate, the
# install path, the optional workspace initialization and the dry run.
set -eu

REPO=$(cd "$(dirname "$0")/.." && pwd)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/lightsout-installer-test.XXXXXX") || exit 1
trap 'rm -rf "$TMP"' EXIT HUP INT TERM
failures=0

check() { # check <label> <want> <got>
  if [ "$2" = "$3" ]; then
    printf 'ok   %s\n' "$1"
  else
    printf 'FAIL %s (want %s, got %s)\n' "$1" "$2" "$3"
    failures=$((failures + 1))
  fi
}

case "$(uname -s):$(uname -m)" in
  Darwin:arm64)              target=aarch64-apple-darwin ;;
  Darwin:x86_64)             target=x86_64-apple-darwin ;;
  Linux:x86_64|Linux:amd64)  target=x86_64-unknown-linux-musl ;;
  Linux:aarch64|Linux:arm64) target=aarch64-unknown-linux-gnu ;;
  *) echo "tests/install.sh: unsupported host"; exit 1 ;;
esac
asset="lightsout-${target}.tar.gz"

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

make_release() { # make_release <dir> <asset-bytes>
  mkdir -p "$1"
  tar -czf "$1/$asset" -C "$2" lightsout
}

mkdir -p "$TMP/build" "$TMP/release" "$TMP/release-bad"
cat > "$TMP/build/lightsout" <<'SH'
#!/bin/sh
case "${1:-}" in
  --version) echo "lightsout 9.9.9" ;;
  init)
    mkdir -p "$2"
    printf 'schema = 1\nmanager = "9.9.9"\n' > "$2/lightsout.toml"
    echo "initialized workspace at $2"
    ;;
  *) exit 0 ;;
esac
SH
chmod +x "$TMP/build/lightsout"

make_release "$TMP/release" "$TMP/build"
hash=$(sha256 "$TMP/release/$asset")
printf '%s  %s\n' "$hash" "$asset" > "$TMP/release/$asset.sha256"

LIGHTSOUT_BASE_URL="file://$TMP/release" LIGHTSOUT_VERSION=9.9.9 \
  sh "$REPO/install.sh" --prefix "$TMP/prefix" --workspace "$TMP/office" \
  > "$TMP/install.log" 2>&1
check "install exits 0" 0 "$?"
check "binary installed" 0 "$([ -x "$TMP/prefix/bin/lightsout" ] && echo 0 || echo 1)"
check "workspace initialized" 0 "$([ -f "$TMP/office/lightsout.toml" ] && echo 0 || echo 1)"
check "installed binary runs" "lightsout 9.9.9" "$("$TMP/prefix/bin/lightsout" --version)"

# A tampered asset must be refused before anything is installed.
cp "$TMP/release/$asset" "$TMP/release-bad/$asset"
printf 'tampered\n' >> "$TMP/release-bad/$asset"
cp "$TMP/release/$asset.sha256" "$TMP/release-bad/$asset.sha256"
if LIGHTSOUT_BASE_URL="file://$TMP/release-bad" LIGHTSOUT_VERSION=9.9.9 \
  sh "$REPO/install.sh" --prefix "$TMP/bad-prefix" > "$TMP/bad.log" 2>&1; then
  check "tampered asset is refused" 1 0
else
  check "tampered asset is refused" 1 1
fi
check "refused install writes no binary" 1 \
  "$([ -e "$TMP/bad-prefix/bin/lightsout" ] && echo 0 || echo 1)"
grep -q "checksum mismatch" "$TMP/bad.log"
check "refusal names the checksum" 0 "$?"

LIGHTSOUT_BASE_URL="file://$TMP/release" LIGHTSOUT_VERSION=9.9.9 \
  sh "$REPO/install.sh" --prefix "$TMP/dry-prefix" --dry-run > "$TMP/dry.log" 2>&1
check "dry run exits 0" 0 "$?"
check "dry run writes nothing" 1 "$([ -e "$TMP/dry-prefix" ] && echo 0 || echo 1)"

# An out-of-band pinned hash skips the downloaded checksum and still verifies.
mkdir -p "$TMP/release-pinned"
cp "$TMP/release/$asset" "$TMP/release-pinned/$asset"
LIGHTSOUT_SHA256="$hash" LIGHTSOUT_BASE_URL="file://$TMP/release-pinned" \
  LIGHTSOUT_VERSION=9.9.9 sh "$REPO/install.sh" --prefix "$TMP/pinned-prefix" \
  > "$TMP/pinned.log" 2>&1
check "pinned hash installs" 0 "$?"
grep -q "verified sha256" "$TMP/pinned.log"
check "pinned hash verifies" 0 "$?"

echo
if [ "$failures" -eq 0 ]; then
  echo "all installer checks passed"
else
  echo "$failures check(s) failed"
  exit 1
fi
