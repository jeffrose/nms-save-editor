# Verifying an edit in game

The format layer is verified offline against real saves, but one thing cannot be
checked without the game: whether the game *accepts* a save this editor wrote.
That test needs a real game install, and on PC the saves sit inside the Steam
Cloud-synced Proton prefix, where a careless test propagates to every machine on
the account.

## Current rule: live folders are off-limits

Until the user says otherwise, no test, script or agent touches the real save
folder (`~/.steam/.../compatdata/275850/...`). Every check goes through the
sandbox below, which runs on copies. The edit layer enforces this in code:
a path that looks like a live save location is refused unless
`--confirm-live` is passed, and a directory tree that carries a
`.nmsedit-sandbox` marker is exempt so saved copies stay easy to work with.

## Rules

1. **Never test in the live save folder.** Steam Cloud syncs
   `steamapps/compatdata/275850/pfx/.../HelloGames/NMS`, so a test save there is
   a test save everywhere after the next sync. The editors that got this wrong
   are the reason "it corrupted my save on all my machines" appears in the issue
   tracker.
2. **Take a hash manifest before anything else.** `sha1sum *` in the save folder
   is the receipt that proves what changed and what did not.
3. **Test in an isolated prefix.** Steam Cloud tracks the app's compatdata path,
   not the prefix a process happens to use, so running the game against a
   throwaway prefix leaves the cloud state untouched. This is what
   `tools/sandbox-test.sh` does.
4. **Restore, then re-verify hashes.** If a test ever does touch the live folder,
   restoring the exact bytes is what keeps Steam from re-uploading: Cloud
   compares the SHA-1 recorded in `userdata/<id>/275850/remotecache.vdf`, so
   byte-identical files are a no-op.

## Sandbox procedure

```sh
cargo build --release -p nms-cli
tools/sandbox-test.sh prepare          # copy saves into a throwaway prefix, edit one slot
tools/sandbox-test.sh run              # launch the game against that prefix
#   in game: the slot list should show the edited name with its original
#   location text (metadata mirroring), the slot should load, the HUD should
#   show the edited currency, then save once and quit
tools/sandbox-test.sh inspect          # diff what the game wrote against pristine
tools/sandbox-test.sh clean            # delete the sandbox
```

`prepare` writes a pristine reference copy first and rebuilds the sandbox from it
on every run, so repeated tests never stack edits.

## What `inspect` gives us

The game rewrites metadata on every save, so after one in-game save we get
ground truth for the format that no amount of offline analysis can produce:

* which words the game changes on a save (timestamp, sizes, version);
* whether the game keeps our mirrored save name and location;
* whether the file layout the editor produces is one the game then rewrites
  rather than rejects.

Differences are printed word by word against the pristine copy, ready to fold
back into `docs/format/metadata.md`.

## If a test has to use the live folder

In order:

1. disable Steam Cloud for No Man's Sky (Steam → properties → general);
2. quit Steam completely, so no sync is in flight;
3. record `sha1sum` for every file;
4. make the change and test;
5. restore from the backup and re-verify the hashes;
6. re-enable Steam Cloud — Steam sees unchanged hashes and does nothing.
