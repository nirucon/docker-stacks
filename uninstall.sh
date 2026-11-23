#!/usr/bin/env bash
# Remove docker-stacks. The program goes; --purge also removes its
# configuration and the list of remembered projects. Database dumps in
# ~/.local/share/docker-stacks/backups are never removed by this script.
set -euo pipefail
purge=0
for arg in "$@"; do
  case "$arg" in
    --purge) purge=1 ;;
    -h|--help) echo "usage: ./uninstall.sh [--purge]"; exit 0 ;;
    *) echo "unknown option: $arg (try --help)" >&2; exit 2 ;;
  esac
done
rm -f "$HOME/.local/bin/docker-stacks"
if [[ $purge == 1 ]]; then
  rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/docker-stacks" "${XDG_STATE_HOME:-$HOME/.local/state}/docker-stacks"
fi
backups="${XDG_DATA_HOME:-$HOME/.local/share}/docker-stacks/backups"
[[ -d $backups ]] && echo "Database dumps are kept in $backups."
echo "docker-stacks removed."
