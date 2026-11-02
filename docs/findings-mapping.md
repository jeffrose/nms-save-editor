# Finding the save-key mapping in the game itself

The older editor's issue tracker is mostly one problem in two shapes: *"name
mapping not found"* after every game update, and *"why does this item show as a
code?"*. Its mapping is a hand-maintained list, so it trails the game until
someone rebuilds it. This note records how the mapping can instead be **derived
from the game itself** — the key is a hash of the name — and what was measured
to prove it. The derivation is compiled into the editor as a baseline for the
keys real saves use, and `nmsedit data refresh` extends it from the player's own
installation.

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

| | measured on a 7.x install and 30 real saves |
|---|---|
| keys of the saves named by the extraction alone | 984 of 1,290 |
| keys named by the shipped baseline (extraction plus the hand-written fallbacks) | 990 of 1,290 |
| extraction names the community file also has, and agrees with | 1,142 of 1,143 |
| community keys surviving the merge | 1,496 of 1,496 |

The gaps are names that are not standalone identifiers in the executable. The
clearest case is `MultiTool`, which appears only inside longer identifiers such
as `ConditionHasMultiTool`; `DiscoveryData-v1` contains a `-`, so it is not an
identifier at all. Both are entries the community file adds by hand. When the
true name is missing, a *longer* name that shares its key can take the key —
which is why an existing community mapping is merged in and wins, and why the
command prints how many entries the extraction reproduced and how many differ
(one, on the machine measured here: `Ft;`, where the colliding
`FfxDofLDSNearRG` is the only standalone candidate).

A second extraction rule was measured as well: requiring a name to sit in an
8-byte-padded slot (the layout of the game's per-structure field tables) covers
more of a save's keys — 1,034 against 984 — but produces two names that disagree
with the community mapping, against one for the stricter scan. The stricter
scan is the one implemented; the disagreements are left to the community file,
which wins on merge anyway.

So: the derivation needs no third-party file and names the bulk of what real
saves use; a maintained mapping fills the gaps it cannot and wins on merge
where it has a better name.

## What is built in, and what is generated

There are two mappings, and the split is deliberate:

* **A baseline is compiled into the editor.** `crates/nms-model/data/baseline-mapping.json`
  holds the pairs for the keys real saves use — 984 entries, derived from the
  1,290 distinct keys of the 30 fixture saves — so a machine with no game
  installed still shows field names. It is this project's own derivation from
  the executable, scoped so the file stays ~65 KB rather than ~2.5 MB, and it
  is committed. `tools/build-baseline-mapping.sh` regenerates it after adding
  saves or updating the game, and the fixture tests fail if the committed file
  stops naming most of what real saves use or disagrees with an installed
  community mapping.
* **The full mapping is generated per installation.** `nmsedit data refresh` —
  the one command the README asks for — reads the player's own game and writes
  all 59,907 resolvable keys next to the text pack:

  ```console
  $ nmsedit data refresh
  ...
  mapping         derived from the game's executable
  names read      76550
  keys resolved   59907 (7985 keys dropped as ambiguous)
  wrote           60260 entries to ~/.config/nmsedit/mapping.json
  ```

  Whatever is already in that file is merged in and wins, so a maintained
  mapping is extended, never replaced.

The baseline exists because the mapping is not personal data: it is a fact
about the game, the same for every player, and "where is your game installed"
should not decide whether a save shows `TotalPlayTime` or `Lg8`. What it must
not become is a copy of the game: the committed part is bare field
identifiers, no game text and no assets, and the full extraction stays on the
machine that produced it. MBINCompiler's `mapping.json` is LGPL-3.0, so it is
never copied into this MIT/Apache-2.0 repository; a copy the player installs is
merged at run time.

The split also answers the update problem directly. A shipped *list* is what
made the older editor trail every game update; a shipped *baseline* plus a
per-install refresh means fields a new update adds are one command away, and
the baseline only has to be as current as the saves it was built from.

### Localization

None of this is localized, by construction: the mapping's right side is a
**code identifier** (`TotalPlayTime`, `KnownSpecials`), not display text. The
game has one executable for every language, the identifiers are compiled in,
and the key is a hash of the identifier — so if the names were translated, a
save written in one language would use different keys from the same save
written in another, and saves would not load across languages. They do. One
mapping serves all of them.

What *is* localized is the game text and the item names, and the game's own
file format says so: every localisation record carries one value slot per
language, and each `_<language>.mbin` fills only its own
([layout](format/localisation.md)). `nmsedit data refresh --language german`
reads the German slots; the ids and the mapping are refreshed alongside it
without changing. (Reading anything but English was broken until the slots were
worked out — the parser assumed English's slot and returned empty strings for
every other language; the installation test now parses all 153 tables in all 17
languages exactly.)

## Reproducing it

```sh
nmsedit data mapping --out /tmp/mapping.json          # pure extraction, no merge
nmsedit data mapping                                  # merge over the installed file
tools/build-baseline-mapping.sh                       # the shipped baseline
```

Nothing from the game is written into the repository except the baseline, which
names fields and carries no game text.
