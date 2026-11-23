#!/usr/bin/env bash
# docker-stacks installer: one static binary in ~/.local/bin/docker-stacks.
#
#   ./install.sh             the prebuilt release binary, checked with SHA-256
#   ./install.sh --build     build this checkout with cargo
#   ./install.sh --prebuilt  use ./docker-stacks beside this script
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"
MODE=auto
for arg in "$@"; do
  case "$arg" in
    --build) MODE=build ;;
    --prebuilt) MODE=prebuilt ;;
    -y|--yes) ;;
    -h|--help) echo "usage: ./install.sh [--build|--prebuilt]"; exit 0 ;;
    *) echo "unknown option: $arg (try --help)" >&2; exit 2 ;;
  esac
done
[[ $(id -u) != 0 ]] || { echo "Run ./install.sh as your own user, not root." >&2; exit 1; }

VERSION="${DOCKER_STACKS_VERSION:-}"
[[ -z $VERSION && -f Cargo.toml ]] && VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[[ -n $VERSION ]] || { echo "Cannot tell the version (set DOCKER_STACKS_VERSION)." >&2; exit 1; }
BIN="$HOME/.local/bin"
printf '\033[1mdocker-stacks %s\033[0m\n' "$VERSION"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
verify() {
  local file=$1 sums=$2 name expected actual
  name=$(basename "$file")
  expected=$(awk -v n="$name" '$2 == n || $2 == "*"n {print $1}' "$sums" | head -1)
  [[ -n $expected ]] || { echo "SHA256SUMS has no line for $name" >&2; exit 1; }
  actual=$(sha256sum "$file" | awk '{print $1}')
  [[ $actual == "$expected" ]] || { echo "Checksum mismatch for $name: not installed." >&2; exit 1; }
}

if [[ $MODE == build ]]; then
  command -v cargo >/dev/null || { echo "Rust/cargo is needed for --build" >&2; exit 1; }
  cargo build --release --locked
  SRC=target/release/docker-stacks
elif [[ $MODE == prebuilt || ( $MODE == auto && -x ./docker-stacks ) ]]; then
  SRC=./docker-stacks
  [[ -f SHA256SUMS ]] && verify "$SRC" SHA256SUMS
else
  [[ $(uname -m) == x86_64 ]] || { echo "No prebuilt binary for $(uname -m): ./install.sh --build" >&2; exit 1; }
  URL="${DOCKER_STACKS_RELEASE_URL:-https://github.com/nirucon/docker-stacks/releases/download/v${VERSION}}"
  echo "Downloading the release binary…"
  if command -v curl >/dev/null; then
    curl -fsSL --retry 3 -o "$TMP/docker-stacks" "$URL/docker-stacks"
    curl -fsSL --retry 3 -o "$TMP/SHA256SUMS" "$URL/SHA256SUMS"
  elif command -v wget >/dev/null; then
    wget -qO "$TMP/docker-stacks" "$URL/docker-stacks"
    wget -qO "$TMP/SHA256SUMS" "$URL/SHA256SUMS"
  else
    echo "curl or wget is needed" >&2; exit 1
  fi
  verify "$TMP/docker-stacks" "$TMP/SHA256SUMS"
  SRC="$TMP/docker-stacks"
fi

mkdir -p "$BIN"
install -m 0755 "$SRC" "$BIN/docker-stacks.new.$$"
mv -f "$BIN/docker-stacks.new.$$" "$BIN/docker-stacks"
"$BIN/docker-stacks" --version
command -v docker >/dev/null || echo "Note: docker is not installed here; docker-stacks needs Docker with Compose v2."
case ":$PATH:" in *":$BIN:"*) ;; *) echo "Add $BIN to PATH to run docker-stacks." ;; esac
echo "Done. Run: docker-stacks"
