#!/bin/sh
# A stack's whole life against a real Docker, through the command line:
# found on disk, up, listed with its link, down, still listed, gone.
set -eu
BIN=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
TMP=$(mktemp -d)
export HOME="$TMP/home" XDG_CONFIG_HOME="$TMP/config" XDG_STATE_HOME="$TMP/state" DOCKER_STACKS_DIR="$TMP/stacks"
mkdir -p "$HOME" "$DOCKER_STACKS_DIR/citest"
cleanup() { docker compose -p dsci -f "$DOCKER_STACKS_DIR/citest/compose.yml" down -v >/dev/null 2>&1 || :; rm -rf "$TMP"; }
trap cleanup EXIT
cat > "$DOCKER_STACKS_DIR/citest/compose.yml" <<'Y'
name: dsci
services:
  web:
    image: nginx:alpine
    ports: ["127.0.0.1:18089:80"]
  job:
    image: busybox
    command: ["true"]
Y
"$BIN" list | grep -q '^dsci .* down ' || { echo "FAIL: not found on disk"; "$BIN" list; exit 1; }
"$BIN" up dsci >/dev/null 2>&1
sleep 2
"$BIN" list | grep -q '^dsci .* running .*http://127.0.0.1:18089' || { echo "FAIL: not running with its link"; "$BIN" list; exit 1; }
"$BIN" links dsci | grep -q 'http://127.0.0.1:18089' || { echo "FAIL: links"; exit 1; }
"$BIN" list --json | grep -q '"state": "running"' || { echo "FAIL: json"; exit 1; }
"$BIN" down dsci >/dev/null 2>&1
"$BIN" list | grep -q '^dsci .* down ' || { echo "FAIL: not down"; exit 1; }
# A project whose files are gone and that has no containers drops out.
mv "$DOCKER_STACKS_DIR/citest" "$TMP/elsewhere"
"$BIN" list | grep -q '^dsci' && { echo "FAIL: a vanished project is still listed"; exit 1; }
echo "docker-stacks docker tests: PASS"
