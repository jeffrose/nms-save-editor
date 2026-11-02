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

So: the generator is a fallback that needs no third-party file, and a way to
name fields the community file has not caught up with. It is not a replacement
for a maintained mapping where one exists.

## Reproducing it

```sh
nmsedit data mapping --out /tmp/mapping.json          # no merge, pure extraction
nmsedit data mapping                                  # merge over the installed file
nmsedit data mapping --out /tmp/m.json --install /path/to/No\ Man\'s\ Sky
```

Nothing from the game is written into the repository: the mapping names fields,
it does not carry game text.
