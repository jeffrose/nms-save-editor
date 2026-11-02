# Finding the save-key mapping in the game itself

The older editor's issue tracker is mostly one problem in two shapes: *"name
mapping not found"* after every game update, and *"why does this item show as a
code?"*. Its mapping is a hand-maintained list, so it trails the game until
someone rebuilds it. This note records how the mapping can instead be **derived
from the player's own installation**, and what was measured to prove it.

## The key is a hash of the name

A save stores field names obfuscated — the platform word is `"8>q"`, the save
name `"Pk4"`, the unit balance `"wGS"` — and the readable names are nowhere in
the save. They are in the game's executable, and the key is a pure function of
the name:

```text
hash = SpookyHash128(name, 8268756125562466087, 8268756125562466087)
key  = three 21-bit slices of hash, each modulo 68, through the alphabet
       "0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxy"
       (values are written as '0' + v, with v + '0' > 'Z' shifted up by six)
```

SpookyHash V2 is Bob Jenkins' public-domain hash; the executable is hashed by
name only, with no salt, no length prefix and no type information.

### How it was verified

* **The reference implementation agrees.** Compiled Bob Jenkins' `spooky.c` and
  compared hashes for messages of 1..40 bytes: identical, including the 16-byte
  case that is the first to exercise `ShortMix` rather than just the tail.
  (`crates/nms-data/src/mapping.rs` has both sets of vectors as tests.)
* **All 1,505 entries of the published MBINCompiler mapping reproduce** — key
  for key, name for name, with no exceptions. That file is generated from the
  game by a community tool; reproducing it exactly from the algorithm alone is
  what makes this a fact rather than a guess.
* **The saves agree with the names.** For fields whose names are 16 characters
  or longer the hash path differs from the short one, so they are a useful
  independent check: `tSD` (`RevealBlackHoles`) holds `true`, `nkF`
  (`TradingSupplyDataIndex`) holds a number, `wFm` (`ChestMagic2Layout`) holds a
  layout object — the shapes match the names.

## Reading the names out of the executable

`NMS.exe` carries the game's serialization tables: NUL-terminated identifiers,
one table per structure, with the structure's own type name (`cGcScanEventData`)
after its fields. A scan for identifier-shaped NUL-terminated strings reads
**76,550 names** from the 89 MB binary.

Two names can hash to the same key — the published mapping has nine such pairs —
so a name is kept only when its key is **unique** among everything extracted:
**59,907 keys**, with **7,985 keys dropped as ambiguous**. A dropped key shows
its id, which is worse than a name but better than a wrong one.

## What the generator is good for, and what it is not

`nmsedit data mapping` writes a mapping from the local installation, merging any
file already at `~/.config/nmsedit/mapping.json` (the existing entries win).

| | measured on a 7.x install and one 300-hour save |
|---|---|
| community names reproduced by the extraction | 1,142 of 1,496 (76%) |
| keys of the save named by the extraction alone | 714 of 926 (77%) |
| of those, names that disagree with the community mapping | **0** |
| community entries surviving the merge | 1,496 of 1,496 (100%) |

The gaps are names that are not standalone identifiers in the executable. The
clearest case is `MultiTool`, which appears only inside longer identifiers such
as `ConditionHasMultiTool`; `DiscoveryData-v1` contains a `-`, so it is not an
identifier at all. Both are entries the community file adds by hand. When the
true name is missing, a *longer* name that shares its key can take the key —
which is why an existing community mapping is merged in and wins, and why the
command prints how many entries the extraction reproduced and how many differ
(one, on the machine measured here).

A second extraction rule was measured as well: requiring a name to sit in an
8-byte-padded slot (the layout of the game's per-structure field tables) covers
more of a save's keys — 734 against 714 — but produces one name that disagrees
with the community mapping where the stricter scan produces none. The stricter
scan is the one implemented, and the difference is left to the community file,
which wins on merge anyway.

So: the generator is a fallback that needs no third-party file, and a way to
name fields the community file has not caught up with. It is not a replacement
for a maintained mapping where one exists.

## Why the mapping is generated, not shipped with the editor

The pairs are not personal to anyone — they are facts about the game — so it is
fair to ask why they are not simply committed to this repository. Three reasons,
in order of weight:

1. **They are derived from Hello Games' binary.** The names are the game's own
   field names; this project's rule is that nothing derived from the game's
   content is committed — no text tables, no icons, and no name lists. The pack
   of 84,965 strings is regenerated locally for the same reason, and it is 8.5 MB
   of derived data the player builds in a tenth of a second.
2. **The community file is a third party's artifact.** `mapping.json` ships with
   MBINCompiler under its own licence; copying it into an MIT/Apache-2.0
   repository would be a licence problem regardless of the first point.
3. **A shipped list is the bug, not the fix.** Every "name mapping not found"
   issue in the older editor's tracker is a shipped list meeting a game update.
   A file the player regenerates is never behind the game they are running.

What that means in practice is *not* that the user installs a file. `nmsedit data
refresh` — the one command the README asks for after installing the editor —
derives the text pack, the item names **and** the mapping from the local
installation:

```console
$ nmsedit data refresh
...
items           7957 named (5373 derived from text keys)
wrote 84965 strings (english) to ~/.config/nmsedit/strings.json

mapping         derived from the game's executable
names read      76550
keys resolved   59907 (7985 keys dropped as ambiguous)
wrote           60260 entries to ~/.config/nmsedit/mapping.json
```

The file lives next to the pack because it is the same kind of thing: derived
data, regenerated at will, never committed. The ~20 pairs compiled into the
editor (`KeyMap::builtin`) stay, because a save must show its version, platform,
save name and play time even on a machine with no game installed; everything
beyond that is derived.

## Reproducing it

```sh
nmsedit data mapping --out /tmp/mapping.json          # no merge, pure extraction
nmsedit data mapping                                  # merge over the installed file
nmsedit data mapping --out /tmp/m.json --install /path/to/No\ Man\'s\ Sky
```

Nothing from the game is written into the repository: the mapping names fields,
it does not carry game text.
