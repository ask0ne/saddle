#!/usr/bin/env bash
# Fails if the app version differs between package.json, Cargo.toml and (when
# given) a release tag. Cargo.toml is the single source of truth: tauri.conf.json
# deliberately has no "version", so Tauri falls back to it.
#   usage: scripts/check-version.sh [vX.Y.Z[-pre]]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cargo_v="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/app/src-tauri/Cargo.toml" | head -1)"
npm_v="$(jq -r .version "$ROOT/app/package.json")"
lock_v="$(jq -r '.version' "$ROOT/app/package-lock.json")"
conf_v="$(jq -r '.version // empty' "$ROOT/app/src-tauri/tauri.conf.json")"

fail=0
[ "$cargo_v" = "$npm_v" ]  || { echo "version drift: Cargo.toml=$cargo_v package.json=$npm_v"; fail=1; }
[ "$cargo_v" = "$lock_v" ] || { echo "version drift: Cargo.toml=$cargo_v package-lock.json=$lock_v"; fail=1; }
[ -z "$conf_v" ]           || { echo "tauri.conf.json must not set \"version\" (found $conf_v); Cargo.toml is the source"; fail=1; }
if [ $# -ge 1 ]; then
  [ "v$cargo_v" = "$1" ] || { echo "tag $1 does not match Cargo.toml version v$cargo_v"; fail=1; }
fi
[ $fail -eq 0 ] && echo "version $cargo_v consistent"
exit $fail
