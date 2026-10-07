#!/usr/bin/env bash
# Run one page walk from deploy/browser against a console (#58):
#
#   deploy/browser/run.sh <work dir> <walk.cjs> <console url> [ENV=value…]
#
# Installs Playwright and a headless Chromium into <work dir>/pw once (about
# 15 s, no root), copies the walks there and runs the one named. Exits with
# the walk's status. Screenshots land in <work dir>/shots.
set -euo pipefail
W=$1; WALK=$2; CONSOLE=$3; shift 3
HERE=$(cd "$(dirname "$0")" && pwd)
if [ ! -d "$W/pw/node_modules/playwright" ]; then
  mkdir -p "$W/pw"
  (cd "$W/pw" && npm init -y >/dev/null && npm i --no-audit --no-fund playwright@1 >/dev/null 2>&1 \
    && npx playwright install chromium-headless-shell >/dev/null 2>&1)
fi
cp "$HERE"/*.cjs "$W/pw/"
mkdir -p "$W/shots"
(cd "$W/pw" && env CONSOLE="$CONSOLE" SHOTS="$W/shots" "$@" node "$WALK")
