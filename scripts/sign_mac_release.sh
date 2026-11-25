#!/usr/bin/env bash
# Signs the macOS runtimes of a published release with the Developer ID certificate, notarises
# them with Apple, staples the tickets and puts the re-packed archives back on the release.
# CI ad-hoc signs its macOS bundles, which is enough for `npx deskhatch` (that download carries
# no quarantine attribute) but makes Gatekeeper stop a browser download. This closes that gap
# without keeping the certificate in a GitHub secret.
#
# Needs, once, on the machine that holds the certificate:
#   security find-identity -v -p codesigning            # the Developer ID Application identity
#   xcrun notarytool store-credentials deskhatch --apple-id <Apple ID> --team-id <TEAMID>
#
# Usage: SIGN_IDENTITY="Developer ID Application: Name (TEAMID)" scripts/sign_mac_release.sh v0.2.0
set -euo pipefail
TAG="${1:?usage: sign_mac_release.sh <tag, e.g. v0.2.0>}"
V="${TAG#v}"
REPO="${REPO:-beskvladimir-create/deskhatch}"
IDENTITY="${SIGN_IDENTITY:?set SIGN_IDENTITY to the Developer ID Application identity}"
PROFILE="${NOTARY_PROFILE:-deskhatch}"
WAIT="${NOTARY_TIMEOUT:-40m}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

ASSETS=("deskhatch-v${V}-darwin-arm64.tar.gz" "deskhatch-v${V}-darwin-x64.tar.gz")
gh release download "$TAG" --repo "$REPO" --dir "$WORK" --pattern "SHA256SUMS.txt"
for a in "${ASSETS[@]}"; do
  gh release download "$TAG" --repo "$REPO" --dir "$WORK" --pattern "$a"
done

for arch in arm64 x64; do
  asset="deskhatch-v${V}-darwin-${arch}.tar.gz"
  out="$WORK/$arch"
  mkdir -p "$out"
  tar -xzf "$WORK/$asset" -C "$out"
  app="$out/Deskhatch.app"

  echo "== signing $arch"
  rm -rf "$app/Contents/_CodeSignature"
  codesign --force --timestamp --options runtime --sign "$IDENTITY" "$app"
  codesign --verify --deep --strict "$app"

  echo "== notarising $arch (minutes, occasionally much longer)"
  ditto -c -k --keepParent "$app" "$out/upload.zip"
  xcrun notarytool submit "$out/upload.zip" --keychain-profile "$PROFILE" --wait --timeout "$WAIT"
  xcrun stapler staple "$app"
  spctl -a -vvv -t exec "$app"

  rm -f "$WORK/$asset"
  tar -C "$out" -czf "$WORK/$asset" Deskhatch.app
done

# Only the two macOS lines change; the Linux hashes stay as CI computed them.
cd "$WORK"
for a in "${ASSETS[@]}"; do
  sum="$(shasum -a 256 "$a" | awk '{print $1}')"
  awk -v f="$a" -v s="$sum" '{ if ($2 == f) print s "  " f; else print }' SHA256SUMS.txt > SHA256SUMS.new
  mv SHA256SUMS.new SHA256SUMS.txt
done
cat SHA256SUMS.txt

gh release upload "$TAG" --repo "$REPO" --clobber "${ASSETS[@]}" SHA256SUMS.txt
echo "signed, notarised and uploaded for $TAG"
