#!/usr/bin/env bash
# Copy a No Man's Sky save directory into fixtures/ for local testing.
#
# Fixtures contain personal data (player names, account IDs, GUIDs) and are
# git-ignored: only fixtures/README.md is tracked. Pass either a directory
# containing save*.hg files, or a .zip of one.
#
#   tools/fetch-fixtures.sh ~/Downloads/nms-saves.zip
#   tools/fetch-fixtures.sh "/path/to/HelloGames/NMS"
#
# Override the destination with NMS_FIXTURES_DEST (default: fixtures/steam).
set -euo pipefail

src="${1:-}"
if [[ -z "$src" ]]; then
  echo "usage: $0 <save-dir-or-zip>" >&2
  exit 2
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="${NMS_FIXTURES_DEST:-$root/fixtures/steam}"

if [[ ! -e "$src" ]]; then
  echo "error: $src does not exist" >&2
  exit 1
fi

mkdir -p "$dest"

if [[ -d "$src" ]]; then
  # Copy the .hg files (and the autocloud marker, which identifies the account).
  find "$src" -maxdepth 1 -type f \( -name '*.hg' -o -name 'steam_autocloud.vdf' \) -exec cp -f {} "$dest/" \;
elif [[ -f "$src" ]]; then
  case "$src" in
    *.zip) unzip -oq "$src" -d "$dest" ;;
    *) echo "error: expected a directory or a .zip archive" >&2; exit 1 ;;
  esac
else
  echo "error: $src is neither a file nor a directory" >&2
  exit 1
fi

# Normalise: if the archive unpacked into a single subdirectory, move it up.
mapfile -t dirs < <(find "$dest" -mindepth 1 -maxdepth 1 -type d)
if [[ ${#dirs[@]} -eq 1 && ! -e "${dirs[0]}/save.hg" ]]; then
  mv "${dirs[0]}"/* "$dest"/ 2>/dev/null || true
  rmdir "${dirs[0]}" 2>/dev/null || true
fi

count=$(find "$dest" -name '*.hg' | wc -l)
echo "fixtures: $count .hg files in $dest"
find "$dest" -name '*.hg' | sort | head -5
