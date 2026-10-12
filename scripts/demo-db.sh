#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
demo_root="$PWD/.cache/demo-postgres"
mkdir -p "$demo_root/socket"
if [[ ! -f "$demo_root/data/PG_VERSION" ]]; then
  initdb -D "$demo_root/data" -U jupsql --auth-local=trust --auth-host=trust --no-locale --encoding=UTF8 > "$demo_root/init.log"
fi
if ! pg_ctl -D "$demo_root/data" status > /dev/null 2>&1; then
  pg_ctl -D "$demo_root/data" -l "$demo_root/server.log" -o "-k '$demo_root/socket' -h 127.0.0.1 -p 55432" -w start
fi
if [[ "$(psql -h "$demo_root/socket" -p 55432 -U jupsql -d postgres -tAc "SELECT 1 FROM pg_database WHERE datname = 'jupsql_demo'")" != 1 ]]; then
  createdb -h "$demo_root/socket" -p 55432 -U jupsql jupsql_demo
fi
printf 'Demo ready: host=127.0.0.1 port=55432 database=jupsql_demo user=jupsql (no password, TLS off)\n'
printf 'Stop with: pg_ctl -D .cache/demo-postgres/data stop\n'
