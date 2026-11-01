//! `nmsedit` — inspect, verify and edit No Man's Sky save files.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nms_edit::{CommitOptions, SaveBundle, now_unix};
use nms_format::metadata::{Metadata, MetadataPatch, Slot};
use nms_format::SaveFile;
use nms_model::{Document, KeyMap};

const USAGE: &str = "\
nmsedit — No Man's Sky save tool

USAGE:
    nmsedit info <save.hg>                 summarise a save file and its JSON payload
    nmsedit slots <save-dir>               list every slot in a save directory
    nmsedit get <save.hg> <path>           read a field by readable path
    nmsedit set <save.hg> <path> <value>   edit a field, back up, and refresh metadata
    nmsedit export <save.hg|dir> [out]     write readable JSON (mapping applied)
    nmsedit dump <save.hg> [out]           write the raw on-disk payload
    nmsedit metadata <mf_*.hg>             decrypt and print a metadata file
    nmsedit backup <save-dir>              back up every save in a directory
    nmsedit backups <save-dir>             list existing backups
    nmsedit restore <backup.zip> --to <dir>
                                           restore an archive (replaced files are
                                           themselves backed up first)
    nmsedit verify <save-dir>              round-trip every file and report fidelity
    nmsedit mapping <mapping.json>         load a community key mapping and report it

DATA (regenerated from a local game installation):
    nmsedit data refresh                 read the game's archives and write a data pack
    nmsedit data info                    show the installed data pack
    nmsedit data lookup <id> [id...]     print game text by id

PATH SYNTAX:
    Dotted, readable names; array indices in brackets.

        CommonStateData.SaveName
        PlayerStateData.SaveSummary
        PlayerStateData.DifficultyState.Preset.DifficultyPresetType
        PlayerStateData.Inventory[3].b2n

OPTIONS:
    --dry-run              plan an edit without writing anything
    --confirm-live         accept writing into a live, Steam Cloud-synced save
                           folder (the edit will sync to every machine on the
                           account); refused by default
    --backup-dir <dir>     where to write the timestamped zip backup
    --keep <n>             backups to keep per save (default 10)
    --raw                  export/dump the on-disk spelling instead of readable names
    --words                metadata: also print every non-zero word
    --install <dir>        game installation (default: found via Steam)
    --pack <file>          data pack file (default: ~/.config/nmsedit/strings.json)
    --language <code>      language to read from the game (default: english)
    --mapping <file>       key mapping file to extend the built-in map
                           (default: $NMSEDIT_MAPPING, then
                           $XDG_CONFIG_HOME/nmsedit/mapping.json)
    -h, --help             show this help
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h") | Some("--help") | None => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("info") => run(&args, cmd_info),
        Some("slots") => run(&args, cmd_slots),
        Some("get") => run(&args, cmd_get),
        Some("set") => run(&args, cmd_set),
        Some("export") => run(&args, cmd_export),
        Some("dump") => run(&args, cmd_dump),
        Some("metadata") => run(&args, cmd_metadata),
        Some("backup") => run(&args, cmd_backup),
        Some("backups") => run(&args, cmd_backups),
        Some("restore") => run(&args, cmd_restore),
        Some("verify") => run(&args, cmd_verify),
        Some("mapping") => run(&args, cmd_mapping),
        Some("data") => run(&args, cmd_data),
        Some(other) => {
            eprintln!("unknown command {other:?}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String], command: fn(&[String]) -> Result<(), String>) -> ExitCode {
    match command(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// argument helpers
// ---------------------------------------------------------------------------

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

fn option(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|arg| arg == name).and_then(|index| args.get(index + 1)).cloned()
}

/// Positional arguments, ignoring options and their values.
fn positional(args: &[String]) -> Vec<&str> {
    let valued = ["--backup-dir", "--keep", "--mapping"];
    let mut out = Vec::new();
    let mut skip = false;
    for (index, arg) in args.iter().enumerate().skip(1) {
        if skip {
            skip = false;
            continue;
        }
        if valued.contains(&arg.as_str()) {
            skip = true;
            continue;
        }
        if arg.starts_with("--") {
            continue;
        }
        let _ = index;
        out.push(arg.as_str());
    }
    out
}

/// The mapping file used when `--mapping` is not given.
///
/// The full ~1300-entry mapping is game data that is regenerated per game
/// version (see the data pipeline milestone), so it is not vendored into the
/// binary. It is picked up from `$NMSEDIT_MAPPING`, then
/// `$XDG_CONFIG_HOME/nmsedit/mapping.json` (default `~/.config/...`).
fn default_mapping_path() -> Option<PathBuf> {
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from(std::env::var("HOME").ok()?).join(".config"),
    };
    Some(base.join("nmsedit").join("mapping.json"))
}

/// Resolve the key map, reporting which sources were used.
fn keys(args: &[String]) -> Result<KeyMap, String> {
    let path = option(args, "--mapping")
        .map(PathBuf::from)
        .or_else(|| std::env::var("NMSEDIT_MAPPING").ok().map(PathBuf::from).filter(|p| p.is_file()))
        .or_else(default_mapping_path);
    match path {
        Some(path) if path.is_file() => {
            let loaded = KeyMap::load(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(KeyMap::builtin().extended_with(&loaded))
        }
        _ => Ok(KeyMap::builtin()),
    }
}

/// Write to stdout, treating a closed pipe (`| head`) as success.
fn write_stdout(bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    match stdout.write_all(bytes).and_then(|()| stdout.flush()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn list_dir(dir: &Path) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Ok(names)
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

fn cmd_info(args: &[String]) -> Result<(), String> {
    let path = PathBuf::from(positional(args).first().ok_or_else(|| format!("missing path\n\n{USAGE}"))?);
    let raw = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let save = SaveFile::from_bytes(&raw).map_err(|e| e.to_string())?;
    let doc = Document::parse(&save.payload).map_err(|e| e.to_string())?;
    let info = doc.info(&keys(args)?);

    println!("file            {}", path.display());
    println!("container       {} ({} blocks)", save.container.name(), save.blocks.len());
    println!("file size       {} bytes", raw.len());
    println!("payload         {} bytes{}", save.payload.len(), if save.trailing_nul { " (+NUL)" } else { "" });
    if let Some(largest) = save.blocks.iter().map(|b| b.decompressed).max() {
        println!(
            "block chunks    {largest} max decompressed ({} reserved words non-zero)",
            save.blocks.iter().filter(|b| b.reserved != 0).count()
        );
    }
    println!("json keys       {}", doc.key_count());
    println!("key map         {} entries{}", keys(args)?.len(), mapping_note(args));
    println!("version         {}", info.version.map(|v| v.to_string()).unwrap_or_else(|| "?".into()));
    println!("platform        {}", info.platform.as_deref().unwrap_or("?"));
    println!("active context  {}", info.active_context.as_deref().unwrap_or("?"));
    println!("save name       {}", info.save_name.as_deref().unwrap_or("(unset)"));
    println!("play time       {}", match info.play_time {
        Some(seconds) => format!("{} ({:02}:{:02}:{:02})", seconds, seconds / 3600, (seconds % 3600) / 60, seconds % 60),
        None => "?".into(),
    });
    if let Some(slot) = Slot::from_file_name(&path.file_name().unwrap_or_default().to_string_lossy()) {
        println!("storage slot    {} ({})", slot.index(), slot);
        let metadata_path = path.with_file_name(slot.metadata_file_name());
        if metadata_path.is_file() {
            if let Ok(meta) = Metadata::read(&std::fs::read(&metadata_path).unwrap_or_default(), slot) {
                println!("metadata says   name={:?} location={:?} mode={:?}", meta.save_name(), meta.location(), meta.game_mode());
            }
        }
    }
    Ok(())
}

fn cmd_get(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let field = positional.get(1).ok_or_else(|| format!("missing field path\n\n{USAGE}"))?;
    let bundle = SaveBundle::load(Path::new(path), keys(args)?).map_err(|e| e.to_string())?;
    let value = bundle.get(field).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    write_stdout(format!("{text}\n").as_bytes())
}

fn cmd_set(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let field = positional.get(1).ok_or_else(|| format!("missing field path\n\n{USAGE}"))?;
    let raw_value = positional.get(2).ok_or_else(|| format!("missing value\n\n{USAGE}"))?;

    // A value that parses as JSON is used as-is; anything else is a string.
    let value = serde_json::from_str(raw_value)
        .unwrap_or_else(|_| serde_json::Value::String((*raw_value).to_string()));

    let data_path = PathBuf::from(path);
    let mut bundle = SaveBundle::load(&data_path, keys(args)?).map_err(|e| e.to_string())?;
    bundle.set(field, value).map_err(|e| format!("{field}: {e}"))?;

    let dry_run = flag(args, "--dry-run");
    let options = CommitOptions {
        backup_root: Some(
            option(args, "--backup-dir")
                .map(PathBuf::from)
                .unwrap_or_else(|| default_backup_dir(&data_path)),
        ),
        keep_backups: option(args, "--keep").and_then(|v| v.parse().ok()).unwrap_or(10),
        timestamp: Some(now_unix()),
        dry_run,
        confirm_live: flag(args, "--confirm-live"),
    };

    if let Some(live) = nms_edit::live_save_location(&data_path) {
        if options.confirm_live {
            println!("note: {} is {}", data_path.display(), live.describe());
            println!("note: {}", live.hazard());
        }
    }

    let report = bundle.commit(&options).map_err(|e| e.to_string())?;

    println!("{} ({})", data_path.display(), bundle.slot);
    println!("{}", report.changes.field_report());
    println!("{}", report.changes.summary());
    for warning in &report.warnings {
        println!("warning: {warning}");
    }
    if dry_run {
        println!("dry run: nothing was written");
        return Ok(());
    }
    if let Some(backup) = &report.backup {
        println!("backup: {} ({} bytes)", backup.path.display(), backup.size);
    }
    match report.data {
        nms_format::WriteOutcome::Unchanged => println!("no changes to write"),
        nms_format::WriteOutcome::Written(bytes) => {
            let metadata = match report.metadata {
                nms_format::WriteOutcome::Written(bytes) => format!("{bytes} bytes"),
                nms_format::WriteOutcome::Unchanged => "unchanged".to_string(),
            };
            println!("wrote: {bytes} bytes (metadata: {metadata})");
        }
    }
    Ok(())
}

fn default_backup_dir(data_path: &Path) -> PathBuf {
    data_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("Save Backups")
}

fn cmd_export(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let target = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let out = positional.get(1).map(PathBuf::from);
    let keys = keys(args)?;
    let raw_mode = flag(args, "--raw");

    let target_path = PathBuf::from(target);
    if target_path.is_dir() {
        let dir = out.unwrap_or_else(|| target_path.clone());
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut exported = 0usize;
        for name in list_dir(&target_path)? {
            if name.starts_with("mf_") || !name.ends_with(".hg") {
                continue;
            }
            let data_path = target_path.join(&name);
            let document = SaveBundle::load(&data_path, keys.clone()).map_err(|e| e.to_string())?;
            let stem = name.trim_end_matches(".hg");
            export_document(&document, &dir.join(format!("{stem}.json")), raw_mode)?;
            exported += 1;
        }
        eprintln!("exported {exported} save(s) to {}", dir.display());
        return Ok(());
    }

    let bundle = SaveBundle::load(&target_path, keys).map_err(|e| e.to_string())?;
    match out {
        Some(path) => {
            export_document(&bundle, &path, raw_mode)?;
            eprintln!("exported {}", path.display());
        }
        None => {
            let value = if raw_mode { bundle.document.tree().clone() } else { bundle.document.readable(&bundle.keys) };
            let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
            write_stdout(format!("{text}\n").as_bytes())?;
        }
    }
    Ok(())
}

fn export_document(bundle: &SaveBundle, path: &Path, raw_mode: bool) -> Result<(), String> {
    let value = if raw_mode { bundle.document.tree().clone() } else { bundle.document.readable(&bundle.keys) };
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn cmd_dump(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = PathBuf::from(positional.first().ok_or_else(|| format!("missing path\n\n{USAGE}"))?);
    let save = SaveFile::read(&path).map_err(|e| e.to_string())?;
    match positional.get(1) {
        Some(out) => {
            std::fs::write(out, &save.payload).map_err(|e| format!("{out}: {e}"))?;
            eprintln!("wrote {} bytes to {out}", save.payload.len());
        }
        None => {
            write_stdout(&save.payload)?;
        }
    }
    Ok(())
}

fn cmd_metadata(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = PathBuf::from(positional.first().ok_or_else(|| format!("missing path\n\n{USAGE}"))?);
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let slot = Slot::from_metadata_file_name(&name)
        .ok_or_else(|| format!("cannot determine storage slot from {name:?}"))?;
    let meta = read_metadata(&path, slot)?;

    println!("file              {}", path.display());
    println!("slot              {} ({})", meta.slot.index(), meta.slot);
    println!("format            0x{:04X}", meta.format_version);
    println!("decompressed size {} bytes (including trailing NUL)", meta.decompressed_size);
    println!("compressed size   {} bytes", meta.compressed_size);
    println!("version           0x{:04X} ({})", meta.version, meta.version);
    println!("play time         {} seconds", meta.play_time);
    println!("profile hash      0x{:08X}", meta.profile_hash);
    println!("save name        {:?}", meta.save_name());
    println!("location         {:?}", meta.location());
    println!("game mode        {:?}", meta.game_mode());
    println!("timestamp         {}", meta.words.get(89).copied().unwrap_or(0));

    let listed = args.iter().any(|arg| arg == "--words");
    if listed {
        for (index, word) in meta.words.iter().enumerate() {
            if *word == 0 {
                continue;
            }
            let ascii: String = word
                .to_le_bytes()
                .iter()
                .map(|b| if (0x20..0x7F).contains(b) { *b as char } else { '.' })
                .collect();
            println!("  [{index:02}] 0x{word:08X} {word:>12}  {ascii}");
        }
    }
    Ok(())
}

fn read_metadata(path: &Path, slot: Slot) -> Result<Metadata, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Metadata::read(&bytes, slot).map_err(|e| e.to_string())
}

fn cmd_backup(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let dir = PathBuf::from(positional.first().ok_or_else(|| format!("missing directory\n\n{USAGE}"))?);
    let keep = option(args, "--keep").and_then(|v| v.parse().ok()).unwrap_or(10);
    let store = nms_edit::BackupStore::new(backup_root(args, &dir), keep);
    let mut files = Vec::new();
    for name in list_dir(&dir)? {
        if name.ends_with(".hg") {
            files.push((name.clone(), dir.join(&name)));
        }
    }
    if files.is_empty() {
        return Err(format!("no .hg files in {}", dir.display()));
    }
    let label = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "save".into());
    match store.create(&label, &files, now_unix()).map_err(|e| e.to_string())? {
        Some(entry) => {
            println!("backup: {} ({} files, {} bytes)", entry.path.display(), files.len(), entry.size);
            let kept = store.list(&label).map_err(|e| e.to_string())?;
            println!("retained {} backup(s) for {label} (limit {keep})", kept.len());
        }
        None => println!("nothing to back up"),
    }
    Ok(())
}

/// Describe which mapping file, if any, was loaded.
fn mapping_note(args: &[String]) -> String {
    if let Some(path) = option(args, "--mapping") {
        return format!(" ({path})");
    }
    if let Ok(value) = std::env::var("NMSEDIT_MAPPING") {
        return format!(" ($NMSEDIT_MAPPING={value})");
    }
    match default_mapping_path() {
        Some(path) if path.is_file() => format!(" ({})", path.display()),
        Some(path) => format!(" (built-in only; install a full mapping at {})", path.display()),
        None => " (built-in only)".to_string(),
    }
}

fn backup_root(args: &[String], dir: &Path) -> PathBuf {
    option(args, "--backup-dir").map(PathBuf::from).unwrap_or_else(|| dir.join("Save Backups"))
}

fn cmd_backups(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let dir = PathBuf::from(positional.first().ok_or_else(|| format!("missing directory\n\n{USAGE}"))?);
    let root = backup_root(args, &dir);
    let store = nms_edit::BackupStore::new(root.clone(), 10);

    let mut total = 0usize;
    let mut labels: Vec<String> = Vec::new();
    for name in list_dir(&root).unwrap_or_default() {
        if let Some(label) = name.split('_').next() {
            if !labels.contains(&label.to_string()) {
                labels.push(label.to_string());
            }
        }
    }
    for label in labels {
        let entries = store.list(&label).map_err(|e| e.to_string())?;
        for entry in &entries {
            println!(
                "{:<28} {:>10} bytes  {}",
                entry.path.file_name().unwrap_or_default().to_string_lossy(),
                entry.size,
                entry.path.display()
            );
        }
        total += entries.len();
    }
    println!("{} backup(s) in {}", total, root.display());
    Ok(())
}

fn cmd_restore(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let archive = PathBuf::from(positional.first().ok_or_else(|| format!("missing archive\n\n{USAGE}"))?);
    let destination = option(args, "--to").map(PathBuf::from);

    let files = nms_edit::backup::list_archive(&archive).map_err(|e| e.to_string())?;
    let Some(destination) = destination else {
        println!("{} contains {} file(s):", archive.display(), files.len());
        for file in &files {
            println!("  {:<16} {:>10} bytes", file.name, file.size);
        }
        println!("\npass --to <save-dir> to restore them (replaced files are backed up first)");
        return Ok(());
    };

    nms_edit::check_write(&destination, flag(args, "--confirm-live")).map_err(|e| e.to_string())?;

    let store_root = backup_root(args, &destination);
    let store = nms_edit::BackupStore::new(store_root.clone(), 10);
    let restored = nms_edit::backup::restore_archive(&archive, &destination, &store, now_unix())
        .map_err(|e| e.to_string())?;
    println!("restored {} file(s) into {}", restored.len(), destination.display());
    for name in &restored {
        println!("  {name}");
    }
    println!("previous contents archived in {}", store_root.display());
    Ok(())
}

fn cmd_slots(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let dir = PathBuf::from(positional.first().ok_or_else(|| format!("missing directory\n\n{USAGE}"))?);
    let keys = keys(args)?;
    let mut found = 0usize;

    for name in list_dir(&dir)? {
        let Some(slot) = Slot::from_file_name(&name) else { continue };
        let path = dir.join(&name);
        let Ok(raw) = std::fs::read(&path) else { continue };

        let meta = read_metadata(&dir.join(slot.metadata_file_name()), slot).ok();
        let detail = match SaveBundle::load(&path, keys.clone()) {
            Ok(bundle) => {
                let info = bundle.info();
                format!(
                    "{:<10} v{:<5} ctx={:<8} play={:<9} name={}",
                    info.platform.unwrap_or_else(|| "?".into()),
                    info.version.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                    info.active_context.unwrap_or_else(|| "?".into()),
                    info.play_time.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                    meta.as_ref().map(|m| m.save_name()).filter(|n| !n.is_empty()).unwrap_or_else(|| "-".into()),
                )
            }
            Err(e) => format!("unreadable: {e}"),
        };
        let meta_detail = match &meta {
            Some(m) => format!("meta v{} {:<8} {}", m.version, m.location(), m.game_mode()),
            None => "meta: unavailable".to_string(),
        };

        println!("{:<12} {:<12} {:>10} bytes  {}  |  {}", name, slot.to_string(), raw.len(), detail, meta_detail);
        found += 1;
    }

    if found == 0 {
        return Err(format!("no save files found in {}", dir.display()));
    }
    Ok(())
}

/// Outcome of checking one file.
struct VerifyRow {
    name: String,
    kind: String,
    payload: String,
    container: String,
    document: String,
    ok: bool,
}

fn cmd_verify(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let dir = PathBuf::from(positional.first().ok_or_else(|| format!("missing directory\n\n{USAGE}"))?);
    let mut rows = Vec::new();

    for name in list_dir(&dir)? {
        if !name.ends_with(".hg") {
            continue;
        }
        let path = dir.join(&name);
        let Ok(raw) = std::fs::read(&path) else { continue };
        rows.push(verify_one(&name, &raw));
    }

    if rows.is_empty() {
        return Err(format!("no .hg files found in {}", dir.display()));
    }

    println!(
        "{:<16} {:<11} {:>12}  {:<22} {:<10} {}",
        "file", "container", "payload", "container check", "document", "result"
    );
    let mut failures = 0usize;
    for row in &rows {
        if !row.ok {
            failures += 1;
        }
        println!(
            "{:<16} {:<11} {:>12}  {:<22} {:<10} {}",
            row.name,
            row.kind,
            row.payload,
            row.container,
            row.document,
            if row.ok { "ok" } else { "FAIL" }
        );
    }

    println!("\n{} files checked, {failures} failures", rows.len());
    println!("note: block containers are re-compressed, not byte-reproduced; the payload is what must match.");
    if failures > 0 {
        return Err("fidelity check failed".into());
    }
    Ok(())
}

/// Check one file against the guarantee that applies to its kind.
fn verify_one(name: &str, raw: &[u8]) -> VerifyRow {
    let mut row = VerifyRow {
        name: name.to_string(),
        kind: "?".into(),
        payload: "-".into(),
        container: "-".into(),
        document: "-".into(),
        ok: false,
    };

    if let Some(slot) = Slot::from_metadata_file_name(name) {
        row.kind = "metadata".into();
        match Metadata::read(raw, slot) {
            Ok(meta) => {
                row.payload = format!("{} words", meta.words.len());
                match meta.to_bytes() {
                    Ok(again) if again == raw => {
                        row.container = "byte-exact".into();
                        row.document = "n/a".into();
                        row.ok = true;
                    }
                    Ok(_) => row.container = "re-encrypt differs".into(),
                    Err(e) => row.container = format!("error: {e}"),
                }
            }
            Err(e) => row.container = format!("error: {e}"),
        }
        return row;
    }

    match SaveFile::from_bytes(raw) {
        Ok(save) => {
            row.kind = save.container.name().into();
            row.payload = save.payload.len().to_string();

            row.container = match SaveFile::from_bytes(&save.to_bytes()) {
                Ok(again) if again.stored_payload() == save.stored_payload() => {
                    if save.container == nms_format::Container::PlainJson {
                        if save.to_bytes() == raw {
                            "byte-exact".into()
                        } else {
                            "plain json differs".into()
                        }
                    } else {
                        format!("payload ok ({} B)", save.to_bytes().len())
                    }
                }
                Ok(_) => "payload changed".into(),
                Err(e) => format!("error: {e}"),
            };

            row.document = match Document::parse(&save.payload)
                .map_err(|e| e.to_string())
                .and_then(|doc| doc.payload().map_err(|e| e.to_string()).map(|p| (doc, p)))
            {
                Ok((doc, payload)) if payload == save.payload && doc.key_count() > 0 => "round-trips".into(),
                Ok(_) => "payload changed".into(),
                Err(e) => format!("error: {e}"),
            };

            row.ok = !row.container.contains("differs")
                && !row.container.contains("changed")
                && !row.container.starts_with("error")
                && row.document == "round-trips";
        }
        Err(e) => row.container = format!("error: {e}"),
    }
    row
}

/// `nmsedit data ...`
fn cmd_data(args: &[String]) -> Result<(), String> {
    match positional(args).first().copied() {
        Some("refresh") => cmd_data_refresh(args),
        Some("info") => cmd_data_info(args),
        Some("lookup") => cmd_data_lookup(args),
        Some(other) => Err(format!("unknown data command {other:?}\n\n{USAGE}")),
        None => Err(format!("missing data command (refresh, info, lookup)\n\n{USAGE}")),
    }
}

fn pack_path(args: &[String]) -> Result<PathBuf, String> {
    if let Some(path) = option(args, "--pack") {
        return Ok(PathBuf::from(path));
    }
    if let Ok(value) = std::env::var("NMSEDIT_PACK") {
        if !value.is_empty() {
            return Ok(PathBuf::from(value));
        }
    }
    nms_data::DataPack::default_path().ok_or_else(|| "cannot determine a home directory".to_string())
}

fn cmd_data_refresh(args: &[String]) -> Result<(), String> {
    let install = nms_data::Install::discover(option(args, "--install").map(PathBuf::from).as_deref())
        .map_err(|e| e.to_string())?;
    let language = option(args, "--language").unwrap_or_else(|| "english".to_string());
    let out = pack_path(args)?;

    println!("installation    {}", install.root.display());
    println!("archives        {}", install.pak_dir.display());
    let (pack, report) = nms_data::DataPack::build(&install, &language, now_unix()).map_err(|e| e.to_string())?;

    println!();
    for source in &report.sources {
        println!(
            "  {:<28} {:>4} tables {:>8} entries  {}",
            source.pak,
            source.tables,
            source.entries,
            if source.exact { "exact" } else { "INEXACT" }
        );
    }
    for skipped in &report.skipped {
        println!("  skipped: {skipped}");
    }

    pack.save(&out).map_err(|e| e.to_string())?;
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    println!();
    println!("wrote {} strings ({language}) to {}", pack.len(), out.display());
    println!("pack size       {size} bytes");
    println!("nothing from the game is redistributed: the pack is built from your own install and stays here");
    Ok(())
}

fn cmd_data_info(args: &[String]) -> Result<(), String> {
    let path = pack_path(args)?;
    let pack = nms_data::DataPack::load(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let filled = pack.strings.values().filter(|text| !text.is_empty()).count();
    println!("pack            {}", path.display());
    println!("format          {}", pack.format);
    println!("language        {}", pack.language);
    println!("game version    {}", pack.game_version.as_deref().unwrap_or("(unknown)"));
    println!("generated       {} ({})", pack.generated_at, nms_edit::format_timestamp(pack.generated_at));
    println!("strings         {} ({} with text, {} empty)", pack.len(), filled, pack.len() - filled);
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!("file size       {size} bytes");
    Ok(())
}

fn cmd_data_lookup(args: &[String]) -> Result<(), String> {
    let path = pack_path(args)?;
    let pack = nms_data::DataPack::load(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ids: Vec<&str> = positional(args).into_iter().skip(1).collect();
    if ids.is_empty() {
        return Err(format!("missing id\n\n{USAGE}"));
    }
    let mut missing = 0usize;
    for id in ids {
        match pack.lookup(id) {
            Some(text) => println!("{id:<32} {text}"),
            None => {
                println!("{id:<32} (not in the pack)");
                missing += 1;
            }
        }
    }
    if missing > 0 {
        return Err(format!("{missing} id(s) not found"));
    }
    Ok(())
}

fn cmd_mapping(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = PathBuf::from(positional.first().ok_or_else(|| format!("missing path\n\n{USAGE}"))?);
    let map = KeyMap::load(&path).map_err(|e| e.to_string())?;
    println!("{}: {} key mappings", path.display(), map.len());
    for name in ["Version", "Platform", "ActiveContext", "SaveName", "TotalPlayTime", "SaveSummary"] {
        println!("  {:<14} -> {}", name, map.candidates(name).last().unwrap_or("(unmapped)"));
    }
    let merged = KeyMap::builtin().extended_with(&map);
    println!("merged with builtin: {} entries", merged.len());
    let _ = MetadataPatch::default();
    Ok(())
}
