#!/bin/sh
# install.sh — install the lightsout binary from a GitHub release.
#
# The default version is pinned; set LIGHTSOUT_VERSION=latest or pass
# --version latest when you deliberately want the newest release. Downloads are
# checksum-verified before anything is installed. No sudo is used: the default
# prefix is $HOME/.local, and --prefix chooses another writable directory.
#
#   sh install.sh [--version V] [--prefix DIR] [--workspace DIR] [--dry-run]
#
# Environment:
#   LIGHTSOUT_VERSION    version to install (default 0.2.0; "latest" allowed)
#   LIGHTSOUT_PREFIX     install prefix (default $HOME/.local)
#   LIGHTSOUT_BASE_URL   override the release asset directory (mirrors/tests)
#   LIGHTSOUT_SHA256     expected SHA-256 obtained out-of-band; skips the
#                        checksum download and pins the exact archive
#
# Exit: 0 installed (and optionally initialized), 64 usage, 1 a download,
#       checksum, install or initialization failure.

set -eu

VERSION="${LIGHTSOUT_VERSION:-0.2.0}"
PREFIX="${LIGHTSOUT_PREFIX:-${PREFIX:-$HOME/.local}}"
BASE_URL="${LIGHTSOUT_BASE_URL:-}"
PINNED_SHA="${LIGHTSOUT_SHA256:-}"
WORKSPACE=""
DRY_RUN=0

usage() {
  cat <<'TXT'
usage: install.sh [options]

  --version V       install this version (default 0.2.0; "latest" allowed)
  --prefix DIR      install under DIR/bin (default $HOME/.local)
  --workspace DIR   initialize a workspace after installing
  --dry-run         print what would happen; download and write nothing
  --help            show this help

Environment: LIGHTSOUT_VERSION, LIGHTSOUT_PREFIX, LIGHTSOUT_BASE_URL,
             LIGHTSOUT_SHA256
TXT
}

say() { printf '%s\n' "$*"; }
fail() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)     [ "$#" -ge 2 ] || fail "--version needs a value"; VERSION="$2"; shift 2 ;;
    --version=*)   VERSION="${1#*=}"; shift ;;
    --prefix)      [ "$#" -ge 2 ] || fail "--prefix needs a value"; PREFIX="$2"; shift 2 ;;
    --prefix=*)    PREFIX="${1#*=}"; shift ;;
    --workspace)   [ "$#" -ge 2 ] || fail "--workspace needs a value"; WORKSPACE="$2"; shift 2 ;;
    --workspace=*) WORKSPACE="${1#*=}"; shift ;;
    --dry-run)     DRY_RUN=1; shift ;;
    -h|--help)     usage; exit 0 ;;
    *)             fail "unknown option: $1 (try --help)" ;;
  esac
done

case "$VERSION" in
  v*) VERSION="${VERSION#v}" ;;
esac

OS=$(uname -s 2>/dev/null || true)
ARCH=$(uname -m 2>/dev/null || true)
case "$OS:$ARCH" in
  Darwin:arm64)          TARGET="aarch64-apple-darwin" ;;
  Darwin:x86_64)         TARGET="x86_64-apple-darwin" ;;
  Linux:x86_64|Linux:amd64) TARGET="x86_64-unknown-linux-musl" ;;
  Linux:aarch64|Linux:arm64) TARGET="aarch64-unknown-linux-gnu" ;;
  *) fail "no release asset for $OS/$ARCH (supported: macOS arm64/x86_64, Linux x86_64/aarch64)" ;;
esac

ASSET="lightsout-${TARGET}.tar.gz"
if [ -z "$BASE_URL" ]; then
  if [ "$VERSION" = "latest" ]; then
    BASE_URL="https://github.com/darylfung96/lightsout/releases/latest/download"
  else
    BASE_URL="https://github.com/darylfung96/lightsout/releases/download/v${VERSION}"
  fi
fi
ASSET_URL="${BASE_URL%/}/${ASSET}"
CHECKSUM_URL="${ASSET_URL}.sha256"
INSTALL_DIR="${PREFIX%/}/bin"
INSTALLED="${INSTALL_DIR}/lightsout"

say "lightsout installer"
say "  version  $VERSION"
say "  target   $TARGET"
say "  prefix   $PREFIX"
say "  asset    $ASSET_URL"
if [ -n "$WORKSPACE" ]; then
  say "  workspace $WORKSPACE"
fi

if [ "$DRY_RUN" = 1 ]; then
  say "dry run: would install $INSTALLED"
  if [ -n "$WORKSPACE" ]; then
    say "dry run: would run $INSTALLED init $WORKSPACE"
  else
    say "dry run: next step would be: lightsout init <dir>"
  fi
  exit 0
fi

for tool in curl tar; do
  command -v "$tool" >/dev/null 2>&1 || fail "$tool is required"
done
if command -v sha256sum >/dev/null 2>&1; then
  CHECK_SHA=sha256sum
elif command -v shasum >/dev/null 2>&1; then
  CHECK_SHA=shasum
else
  fail "sha256sum or shasum is required"
fi
if command -v install >/dev/null 2>&1; then
  INSTALL_BIN=install
else
  INSTALL_BIN=""
fi

tmp=$(mktemp -d "${TMPDIR:-/tmp}/lightsout-install.XXXXXX") || fail "cannot create a temporary directory"
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

say "downloading $ASSET"
curl -fsSL --retry 2 "$ASSET_URL" -o "$tmp/$ASSET" || fail "download failed: $ASSET_URL"
if [ -n "$PINNED_SHA" ]; then
  expected="$PINNED_SHA"
else
  curl -fsSL --retry 2 "$CHECKSUM_URL" -o "$tmp/$ASSET.sha256" || fail "checksum download failed: $CHECKSUM_URL"
  expected=$(awk 'NR==1 {print $1}' "$tmp/$ASSET.sha256")
fi
case "$expected" in
  ''|*[!0-9a-fA-F]*) fail "checksum file does not start with a SHA-256 value" ;;
esac
if [ "${#expected}" -ne 64 ]; then
  fail "checksum file does not contain a 64-character SHA-256 value"
fi
if [ "$CHECK_SHA" = "sha256sum" ]; then
  actual=$(sha256sum "$tmp/$ASSET" | awk '{print $1}')
else
  actual=$(shasum -a 256 "$tmp/$ASSET" | awk '{print $1}')
fi
if [ "$actual" != "$expected" ]; then
  fail "checksum mismatch for $ASSET (expected $expected, got $actual)"
fi
say "verified sha256 $actual"

tar -xzf "$tmp/$ASSET" -C "$tmp" || fail "cannot extract $ASSET"
[ -f "$tmp/lightsout" ] || fail "the archive does not contain a lightsout binary"
chmod +x "$tmp/lightsout"

mkdir -p "$INSTALL_DIR" || fail "cannot create $INSTALL_DIR"
if [ -n "$INSTALL_BIN" ]; then
  "$INSTALL_BIN" -m 0755 "$tmp/lightsout" "$INSTALLED" || fail "cannot install $INSTALLED"
else
  cp "$tmp/lightsout" "$INSTALLED" && chmod 0755 "$INSTALLED" || fail "cannot install $INSTALLED"
fi
"$INSTALLED" --version >/dev/null 2>&1 || fail "installed binary does not run: $INSTALLED"
say "installed $INSTALLED"

case ":${PATH:-}:" in
  *":$INSTALL_DIR:"*) ;;
  *) say "note: add $INSTALL_DIR to PATH to run lightsout without a full path" ;;
esac

if [ -n "$WORKSPACE" ]; then
  "$INSTALLED" init "$WORKSPACE" || fail "installed, but workspace init failed: $WORKSPACE"
  say "workspace ready: $WORKSPACE"
  say "  cd $WORKSPACE && lightsout doctor && lightsout run noop"
else
  say "next: lightsout init <dir>"
  say "      cd <dir> && lightsout doctor && lightsout run noop"
fi
