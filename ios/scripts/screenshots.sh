#!/usr/bin/env bash
# Boot a simulator, launch the app in each mock scene (light + dark) and screenshot it.
# Usage: screenshots.sh <path/to/BelugaLive.app> <outdir>
set -euo pipefail
APP="${1:-}"; OUT="${2:-screenshots}"
BUNDLE_ID="${BUNDLE_ID:-com.thanhvu.belugalive}"
SCENES="${SCENES:-idle listening thinking tool lesson ask done sessions settings offline}"
mkdir -p "$OUT"

UDID=$(xcrun simctl list devices available -j | jq -r '
  [.devices | to_entries[] | select(.key | test("iOS")) | .value[]
   | select(.name | test("^iPhone 1[78]( Pro)?$"))] | sort_by(.name) | .[0].udid')
if [ -z "$UDID" ] || [ "$UDID" = "null" ]; then
  echo "No iPhone 17/18 simulator found"; xcrun simctl list devices available; exit 1
fi
NAME=$(xcrun simctl list devices -j | jq -r --arg u "$UDID" '.devices[][] | select(.udid==$u) | .name')
echo "Using $NAME ($UDID)"
xcrun simctl boot "$UDID" 2>/dev/null || true
if [ "$APP" = "--boot-only" ]; then exit 0; fi
xcrun simctl bootstatus "$UDID" -b
xcrun simctl status_bar "$UDID" override --time 9:41 --batteryState charged --batteryLevel 100 \
  --cellularMode active --cellularBars 4 --wifiBars 3 || true
xcrun simctl install "$UDID" "$APP"

for MODE in light dark; do
  xcrun simctl ui "$UDID" appearance "$MODE"
  for SCENE in $SCENES; do
    xcrun simctl terminate "$UDID" "$BUNDLE_ID" >/dev/null 2>&1 || true
    xcrun simctl launch "$UDID" "$BUNDLE_ID" -mock 1 -mockScene "$SCENE" >/dev/null
    sleep "${SHOT_DELAY:-3}"
    xcrun simctl io "$UDID" screenshot "$OUT/${SCENE}-${MODE}.png" >/dev/null
    echo "shot $SCENE-$MODE"
  done
done
# Also capture the live mock lesson flow over time (light only).
if [ "${FLOW:-1}" = "1" ]; then
  xcrun simctl ui "$UDID" appearance light
  xcrun simctl terminate "$UDID" "$BUNDLE_ID" >/dev/null 2>&1 || true
  xcrun simctl launch "$UDID" "$BUNDLE_ID" -mock 1 >/dev/null
  for i in 1 2 3 4 5 6; do
    sleep 4
    xcrun simctl io "$UDID" screenshot "$OUT/flow-$i.png" >/dev/null
  done
fi
