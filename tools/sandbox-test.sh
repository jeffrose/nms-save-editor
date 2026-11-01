#!/usr/bin/env bash
# In-game verification for nms-save-editor, in a throwaway Proton prefix.
#
# Why a sandbox: No Man's Sky saves live inside the Steam Cloud-synced Proton
# prefix. Editing the live folder risks Steam uploading a test save to every
# machine on the account. This script runs the game against its own prefix
# instead, so the cloud-managed folder is never opened, never written, and never
# synced. Steam Cloud tracks the app's compatdata path, not whatever prefix a
# process happens to use.
#
#   tools/sandbox-test.sh prepare [slot] [units] [name]   build the sandbox and edit it
#   tools/sandbox-test.sh run                             launch the game in the sandbox
#   tools/sandbox-test.sh inspect                         show what the game wrote
#   tools/sandbox-test.sh clean                           delete the sandbox
#
# After the game exits, `inspect` compares the metadata the *game* rewrote with
# the pristine copies, which is the ground truth this project cannot get offline.
set -euo pipefail

STEAM_ROOT="${STEAM_ROOT:-$HOME/.steam/debian-installation}"
# Built outside the expansion: an apostrophe inside ${VAR:-default} unbalances quoting.
DEFAULT_GAME_DIR="$STEAM_ROOT/steamapps/common/No Man's Sky"
DEFAULT_PROTON="$STEAM_ROOT/steamapps/common/Proton - Experimental/proton"
GAME_DIR="${NMS_GAME_DIR:-$DEFAULT_GAME_DIR}"
PROTON_BIN="${NMS_PROTON:-$DEFAULT_PROTON}"
SANDBOX="${NMS_SANDBOX:-$HOME/nms-edit-sandbox}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NMSEDIT="${NMSEDIT:-$ROOT/target/release/nmsedit}"
FIXTURES="${NMS_FIXTURES:-$ROOT/fixtures/steam}"

COMPATDATA="$SANDBOX/compatdata"
SAVE_ROOT="$COMPATDATA/pfx/drive_c/users/steamuser/AppData/Roaming/HelloGames/NMS"
PRISTINE="$SANDBOX/saves-pristine"
BACKUPS="$SANDBOX/backups"

die() { echo "error: $*" >&2; exit 1; }
note() { printf '\n=== %s ===\n' "$*"; }

fixture_account_dir() {
  local found
  found="$(find "$FIXTURES" -maxdepth 3 -name save.hg -printf '%h\n' 2>/dev/null | head -1)"
  [ -n "$found" ] || die "no fixtures under $FIXTURES (run tools/fetch-fixtures.sh)"
  echo "$found"
}

account_name() { basename "$(fixture_account_dir)"; }

prepare() {
  local slot="${1:-14}" units="${2:-1234567}" name="${3:-nmsedit test}"
  [ -x "$NMSEDIT" ] || die "build the CLI first: cargo build --release -p nms-cli"
  [ -f "$GAME_DIR/Binaries/NMS.exe" ] || die "game not found at $GAME_DIR"
  [ -f "$PROTON_BIN" ] || die "proton not found at $PROTON_BIN"

  local source account
  source="$(fixture_account_dir)"
  account="$(account_name)"
  local save_dir="$SAVE_ROOT/$account"

  # The pristine copy is the reference for every later comparison.
  mkdir -p "$PRISTINE"
  if [ -z "$(ls -A "$PRISTINE")" ]; then
    cp -a "$source"/. "$PRISTINE"/
    echo "pristine reference saved from fixtures: $(ls "$PRISTINE" | wc -l) files"
  fi

  # Rebuild the sandbox save folder from pristine every time, so repeated runs
  # never stack edits on top of each other.
  rm -rf "$SAVE_ROOT"
  mkdir -p "$save_dir"
  cp -a "$PRISTINE"/. "$save_dir"/

  local data="save$slot.hg"
  [ -f "$save_dir/$data" ] || die "$data not in the fixture set"

  echo
  "$NMSEDIT" set "$save_dir/$data" BaseContext.PlayerStateData.Units "$units" --backup-dir "$BACKUPS"
  "$NMSEDIT" set "$save_dir/$data" CommonStateData.SaveName "$name" --backup-dir "$BACKUPS"

  note "sandbox ready"
  cat <<EOF
save folder   $save_dir
edited file   $data (slot $(( slot / 2 + slot % 2 )))
changes       Units -> $units, save name -> "$name"

What to check in game:
  1. the save list shows "$name" with its original location text
     (that text comes from the metadata file alone)
  2. the slot loads and the HUD shows $units units
  3. save the game once (any save point) and quit

Then run: tools/sandbox-test.sh inspect
EOF
}

run() {
  [ -d "$SAVE_ROOT" ] || die "run 'prepare' first"
  export STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_ROOT"
  export STEAM_COMPAT_DATA_PATH="$COMPATDATA"
  export SteamAppId=275850
  export SteamGameId=275850
  export PROTON_LOG="${PROTON_LOG:-0}"
  echo "launching No Man's Sky with prefix $COMPATDATA"
  echo "the cloud-managed save folder in steamapps/compatdata/275850 is not involved"
  exec "$PROTON_BIN" run "$GAME_DIR/Binaries/NMS.exe" "$@"
}

inspect() {
  local account save_dir
  account="$(account_name)"
  save_dir="$SAVE_ROOT/$account"
  [ -d "$save_dir" ] || die "no sandbox saves; run 'prepare' then 'run'"

  note "files written by the game"
  local changed=0
  for file in "$save_dir"/*; do
    local name
    name="$(basename "$file")"
    if [ ! -f "$PRISTINE/$name" ]; then
      printf '  new      %-20s %10s bytes\n' "$name" "$(stat -c%s "$file")"
      changed=$((changed + 1))
    elif ! cmp -s "$file" "$PRISTINE/$name"; then
      printf '  rewrote  %-20s %10s bytes (was %s)\n' "$name" "$(stat -c%s "$file")" "$(stat -c%s "$PRISTINE/$name")"
      changed=$((changed + 1))
    fi
  done
  [ "$changed" -eq 0 ] && echo "  nothing changed: the game did not save, or did not read this folder"

  note "what the editor says about the sandbox now"
  "$NMSEDIT" slots "$save_dir"

  note "fidelity of the files the game produced"
  "$NMSEDIT" verify "$save_dir" | tail -20

  note "metadata the game rewrote, compared with the pristine copy"
  for meta in "$save_dir"/mf_save*.hg; do
    local name
    name="$(basename "$meta")"
    [ -f "$PRISTINE/$name" ] || continue
    cmp -s "$meta" "$PRISTINE/$name" && continue
    echo "--- $name"
    diff <("$NMSEDIT" metadata "$PRISTINE/$name" --words | sed 's/^/  pristine /') \
         <("$NMSEDIT" metadata "$meta" --words | sed 's/^/  game     /') || true
  done

  note "next"
  cat <<EOF
The diff above is the game's own ground truth for the metadata layout. Send it
along and the format notes and mirror rules get corrected against it.
EOF
}

clean() {
  [ -d "$SANDBOX" ] || { echo "nothing to clean"; return; }
  echo "removing $SANDBOX"
  echo "(the game's cloud-managed save folder is untouched: $STEAM_ROOT/steamapps/compatdata/275850)"
  rm -rf "$SANDBOX"
}

case "${1:-}" in
  prepare) shift; prepare "$@" ;;
  run) shift; run "$@" ;;
  inspect) shift; inspect ;;
  clean) shift; clean ;;
  *)
    sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
