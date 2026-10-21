#!/usr/bin/env bash
# macos-sign.sh — codesign a release binary and notarize it when credentials
# are present. Run on macOS only; the release workflow calls this conditionally.
#
# Required: MACOS_CERT_P12 (base64), MACOS_CERT_PASSWORD, MACOS_SIGNING_IDENTITY.
# Optional: MACOS_NOTARY_APPLE_ID, MACOS_NOTARY_TEAM_ID, MACOS_NOTARY_PASSWORD.
#
# A bare CLI binary has no bundle to staple, so notarization is submitted for
# the zip and Gatekeeper checks the ticket online. Homebrew builds from source
# and does not need this path.
set -euo pipefail

binary="${1:?usage: macos-sign.sh <binary>}"
: "${MACOS_CERT_P12:?MACOS_CERT_P12 is required}"
: "${MACOS_CERT_PASSWORD:?MACOS_CERT_PASSWORD is required}"
: "${MACOS_SIGNING_IDENTITY:?MACOS_SIGNING_IDENTITY is required}"

keychain="lightsout-signing-$$.keychain-db"
keychain_password="$(openssl rand -hex 24)"
cert="$RUNNER_TEMP/lightsout-cert.p12"
trap 'security delete-keychain "$keychain" 2>/dev/null || true; rm -f "$cert" "$RUNNER_TEMP/lightsout-notarize.zip"' EXIT

printf '%s' "$MACOS_CERT_P12" | base64 -D > "$cert"
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$cert" -k "$keychain" -P "$MACOS_CERT_PASSWORD" \
  -T /usr/bin/codesign -T /usr/bin/security
security set-key-partition-list -S apple-tool:,apple:,codesign: \
  -s -k "$keychain_password" "$keychain" >/dev/null
security list-keychains -d user -s "$keychain" $(security list-keychains -d user | tr -d '"')

codesign --force --options runtime --timestamp \
  --keychain "$keychain" --sign "$MACOS_SIGNING_IDENTITY" "$binary"
codesign --verify --strict --verbose=2 "$binary"
echo "codesigned $binary"

if [ -n "${MACOS_NOTARY_APPLE_ID:-}" ] && [ -n "${MACOS_NOTARY_TEAM_ID:-}" ] \
   && [ -n "${MACOS_NOTARY_PASSWORD:-}" ]; then
  ditto -c -k --keepParent "$binary" "$RUNNER_TEMP/lightsout-notarize.zip"
  xcrun notarytool submit "$RUNNER_TEMP/lightsout-notarize.zip" \
    --apple-id "$MACOS_NOTARY_APPLE_ID" \
    --team-id "$MACOS_NOTARY_TEAM_ID" \
    --password "$MACOS_NOTARY_PASSWORD" \
    --wait
  echo "notarized $binary (ticket checked online; no bundle to staple)"
fi
