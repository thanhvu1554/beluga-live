#!/usr/bin/env bash
# Record a short h264 video of the app running the mock lesson end to end (-mock 1 -mockMode live, no scene):
# idle, listening, thinking, photo, step-by-step lesson, "your turn", the answer, done.
# Usage: record-demo.sh <path/to/BelugaLive.app> <outdir>
set -euo pipefail
APP="${1:?path to BelugaLive.app}"; OUT="${2:-video}"
BUNDLE_ID="${BUNDLE_ID:-com.thanhvu.belugalive}"
SECONDS_TO_RECORD="${VIDEO_SECONDS:-32}"
mkdir -p "$OUT"

UDID=$(xcrun simctl list devices available -j | jq -r '
  [.devices | to_entries[] | select(.key | test("iOS")) | .value[]
   | select(.name | test("^iPhone 1[78]( Pro)?$"))] | sort_by(.name) | .[0].udid')
[ -n "$UDID" ] && [ "$UDID" != "null" ] || { echo "No iPhone 17/18 simulator found"; exit 1; }
xcrun simctl boot "$UDID" 2>/dev/null || true
xcrun simctl bootstatus "$UDID" -b
xcrun simctl status_bar "$UDID" override --time 9:41 --batteryState charged --batteryLevel 100 \
  --cellularMode active --cellularBars 4 --wifiBars 3 || true
xcrun simctl install "$UDID" "$APP" 2>/dev/null || true
xcrun simctl ui "$UDID" appearance "${DEMO_APPEARANCE:-light}"
xcrun simctl terminate "$UDID" "$BUNDLE_ID" >/dev/null 2>&1 || true

# DEMO_NAME / DEMO_ARGS / DEMO_APPEARANCE / VIDEO_SECONDS choose another run, e.g. the chat shell tour:
#   DEMO_NAME=tour DEMO_ARGS="-mock 1 -mockTour 1" VIDEO_SECONDS=44 record-demo.sh ...
NAME="${DEMO_NAME:-demo}"
ARGS="${DEMO_ARGS:--mock 1 -mockMode live}"
FILE="$OUT/$NAME.mp4"
rm -f "$FILE"
# Background jobs of a non-interactive shell ignore SIGINT; reset it so the recorder can finish the file.
perl -e '$SIG{INT} = "DEFAULT"; exec @ARGV' -- xcrun simctl io "$UDID" recordVideo --codec=h264 --force "$FILE" &
REC=$!
sleep 2
# shellcheck disable=SC2086
xcrun simctl launch "$UDID" "$BUNDLE_ID" $ARGS >/dev/null
sleep "$SECONDS_TO_RECORD"
kill -INT "$REC" 2>/dev/null || true
for _ in $(seq 1 30); do kill -0 "$REC" 2>/dev/null || break; sleep 1; done
kill -9 "$REC" 2>/dev/null || true
wait "$REC" 2>/dev/null || true

[ -s "$FILE" ] || { echo "video missing or empty"; exit 1; }
echo "video: $(du -h "$FILE" | cut -f1)"
