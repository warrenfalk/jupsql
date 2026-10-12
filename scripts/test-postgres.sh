#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
test_root="$(mktemp -d /tmp/jupsql-postgres.XXXXXX)"
cleanup() {
  pg_ctl -D "$test_root/data" -m immediate -w stop > /dev/null 2>&1 || true
  rm -rf "$test_root"
}
trap cleanup EXIT
mkdir -p "$test_root/socket"
initdb -D "$test_root/data" -U jupsql --auth-local=trust --auth-host=reject --no-locale --encoding=UTF8 > "$test_root/init.log"
pg_ctl -D "$test_root/data" -l "$test_root/server.log" -o "-k '$test_root/socket' -h '' -p 55433" -w start
export JUPSQL_TEST_HOST="$test_root/socket"
cargo test --manifest-path src-tauri/Cargo.toml --test postgres -- --ignored --nocapture
