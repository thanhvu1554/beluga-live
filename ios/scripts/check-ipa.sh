#!/usr/bin/env bash
# Verify the unsigned IPA layout: Payload/*.app with Info.plist, no signature, no profile.
set -euo pipefail
IPA="$1"
LIST=$(unzip -Z1 "$IPA")
APPDIR=$(echo "$LIST" | grep -E '^Payload/[^/]+\.app/$' | head -n1)
[ -n "$APPDIR" ] || { echo "missing Payload/*.app"; exit 1; }
echo "$LIST" | grep -qx "${APPDIR}Info.plist" || { echo "missing Info.plist"; exit 1; }
if echo "$LIST" | grep -qE '(_CodeSignature/|embedded\.mobileprovision)'; then
  echo "IPA contains signing artifacts"; exit 1
fi
echo "IPA OK: $APPDIR ($(du -h "$IPA" | cut -f1))"
