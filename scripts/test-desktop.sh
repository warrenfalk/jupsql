#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
pnpm desktop:build --debug
test_root="$(mktemp -d /tmp/jupsql-desktop.XXXXXX)"
cleanup() {
  pg_ctl -D "$test_root/postgres" -m immediate -w stop > /dev/null 2>&1 || true
  rm -rf "$test_root"
}
trap cleanup EXIT
mkdir -p "$test_root/socket" .cache/validation
initdb -D "$test_root/postgres" -U jupsql --auth-local=trust --auth-host=trust --no-locale --encoding=UTF8 > "$test_root/init.log"
pg_ctl -D "$test_root/postgres" -l "$test_root/postgres.log" -o "-k '$test_root/socket' -h 127.0.0.1 -p 55434" -w start
createdb -h "$test_root/socket" -p 55434 -U jupsql jupsql_demo
export JUPSQL_TEST_PORT=55434
export JUPSQL_DATA_DIR="$test_root/app"
export TAURI_WEBVIEW_AUTOMATION=true
export GDK_BACKEND=x11
if ! xvfb-run -a -s '-screen 0 1440x1100x24' dbus-run-session -- node scripts/desktop-smoke.mjs 2> .cache/validation/desktop.log; then
  cat .cache/validation/desktop.log
  exit 1
fi
