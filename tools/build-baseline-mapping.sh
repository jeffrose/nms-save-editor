#!/usr/bin/env bash
# Regenerate the field-name baseline compiled into the editor.
#
# `nmsedit` ships a mapping for the keys that occur in real saves, so a machine
# with no game installed still shows field names. It is *our own* derivation
# from the game's executable — never MBINCompiler's `mapping.json`, which is
# LGPL-3.0 — scoped to the keys a set of real saves uses so the file stays
# around a thousand entries instead of sixty thousand.
#
#   tools/build-baseline-mapping.sh [fixtures-dir]
#
# The default fixtures directory is `fixtures/` (git-ignored, real saves). Run
# this after adding saves or updating the game, then run the fixture tests.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixtures="${1:-$root/fixtures}"
out="$root/crates/nms-model/data/baseline-mapping.json"

if [[ ! -d "$fixtures" ]]; then
  echo "error: $fixtures does not exist (see fixtures/README.md)" >&2
  exit 1
fi

cd "$root"
cargo run --release -p nms-cli -- data mapping --saves "$fixtures" --out "$out"
echo
echo "baseline        $(wc -l < "$out") lines, $(wc -c < "$out") bytes at $out"
echo "next:           cargo test --release -p nms-model -p nms-edit"
