//! `nmsedit` — inspect, verify and edit No Man's Sky save files.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nms_edit::{
    Cell, CommitOptions, Currency, Inventory, ItemKind, ItemSlot, KnownList, NewItem, SaveBundle, now_unix,
};
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

PLAYER (readable field names need a community key mapping, see --mapping):
    nmsedit currencies <save.hg>           show units, nanites and quicksilver
    nmsedit currency <save.hg> <name> <value>
                                           set one balance (units, nanites or
                                           quicksilver) and commit
    nmsedit inventories <save.hg> [--all]  list inventories and their grids
    nmsedit inventory <save.hg> <name>     list one inventory's slots
    nmsedit add-item <save.hg> <inv> <id> <amount>
                                           add items, filling stacks first
                                           [--kind product|substance|technology]
                                           [--max <n>] [--cell <x,y>]
    nmsedit remove-item <save.hg> <inv> <slot>
                                           remove the slot at that array index
    nmsedit set-amount <save.hg> <inv> <slot> <amount>
                                           set a slot's amount (-1 is \"no charge\"
                                           for technology)
    nmsedit repair <save.hg> [<inv> <slot>]
                                           repair one slot, or every damaged slot
                                           in the player's inventories
    nmsedit move-item <save.hg> <inv> <slot> <x> <y>
                                           move a slot to another grid cell
    nmsedit known <save.hg> [list]         list learned ids (products, technology,
                                           specials, base-parts, refiner-recipes)
    nmsedit known-add <save.hg> <list> <id>
                                           mark an id as learned
    nmsedit known-remove <save.hg> <list> <id>
                                           forget an id

MCP (for agents):
    nmsedit mcp                          speak the Model Context Protocol on
                                         stdin/stdout (JSON-RPC 2.0, one message
                                         per line) so an agent can read and edit
                                         saves; the mapping and data pack come
                                         from NMSEDIT_MAPPING and NMSEDIT_PACK

DATA (regenerated from a local game installation):
    nmsedit data refresh                 read the game's archives and write a data pack
    nmsedit data info                    show the installed data pack
    nmsedit data lookup <id> [id...]     print game text by id
    nmsedit data item <id> [id...]       print item names by id (^ and ! prefixes ok)

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
        Some("currencies") => run(&args, cmd_currencies),
        Some("currency") => run(&args, cmd_currency),
        Some("inventories") => run(&args, cmd_inventories),
        Some("inventory") => run(&args, cmd_inventory),
        Some("add-item") => run(&args, cmd_add_item),
        Some("remove-item") => run(&args, cmd_remove_item),
        Some("set-amount") => run(&args, cmd_set_amount),
        Some("repair") => run(&args, cmd_repair),
        Some("move-item") => run(&args, cmd_move_item),
        Some("known") => run(&args, cmd_known),
        Some("known-add") => run(&args, cmd_known_edit),
        Some("known-remove") => run(&args, cmd_known_edit),
        Some("data") => run(&args, cmd_data),
        Some("mcp") => run(&args, cmd_mcp),
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
    let valued = [
        "--backup-dir",
        "--keep",
        "--mapping",
        "--pack",
        "--install",
        "--language",
        "--to",
        "--kind",
        "--max",
        "--cell",
    ];
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
    commit_and_report(args, &mut bundle, &data_path)
}

/// Plan, commit and report an edit that has already been applied in memory.
fn commit_and_report(args: &[String], bundle: &mut SaveBundle, data_path: &Path) -> Result<(), String> {
    let dry_run = flag(args, "--dry-run");
    let options = CommitOptions {
        backup_root: Some(
            option(args, "--backup-dir")
                .map(PathBuf::from)
                .unwrap_or_else(|| default_backup_dir(data_path)),
        ),
        keep_backups: option(args, "--keep").and_then(|v| v.parse().ok()).unwrap_or(10),
        timestamp: Some(now_unix()),
        dry_run,
        confirm_live: flag(args, "--confirm-live"),
    };

    if let Some(live) = nms_edit::live_save_location(data_path) {
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


// ---------------------------------------------------------------------------
// player state: currencies, inventories, known items
// ---------------------------------------------------------------------------

/// Load a save for editing, reporting the path on failure.
fn load_bundle(args: &[String], path: &str) -> Result<SaveBundle, String> {
    SaveBundle::load(Path::new(path), keys(args)?).map_err(|e| format!("{path}: {e}"))
}

fn parse_u32(text: &str, what: &str) -> Result<u32, String> {
    let value: u64 = text
        .parse()
        .map_err(|_| format!("{what} must be a whole number from 0 to {}, got {text:?}", u32::MAX))?;
    u32::try_from(value).map_err(|_| format!("{what} must be a whole number from 0 to {}, got {value}", u32::MAX))
}

fn parse_amount(text: &str) -> Result<i64, String> {
    text.parse().map_err(|_| format!("amount must be a whole number, got {text:?}"))
}

fn parse_cell(text: &str) -> Result<Cell, String> {
    let (x, y) = text.split_once(',').ok_or_else(|| format!("cell must be written as X,Y, got {text:?}"))?;
    let x: u64 = x.trim().parse().map_err(|_| format!("cell column must be a whole number, got {x:?}"))?;
    let y: u64 = y.trim().parse().map_err(|_| format!("cell row must be a whole number, got {y:?}"))?;
    Ok(Cell::new(x, y))
}

/// Find an inventory by name, listing what exists when the name is wrong.
fn inventory_or_error(bundle: &SaveBundle, name: &str) -> Result<Inventory, String> {
    if let Some(inventory) = bundle.inventory(name) {
        return Ok(inventory);
    }
    let available: Vec<String> = bundle.inventories().iter().map(|inventory| inventory.path.label().to_string()).collect();
    if available.is_empty() {
        return Err(format!(
            "no inventory named {name:?}: this save has no readable inventories\nnote: inventory fields need the community key mapping; see --mapping"
        ));
    }
    let shown: Vec<String> = available.iter().take(20).cloned().collect();
    let more = available.len().saturating_sub(shown.len());
    Err(format!(
        "no inventory named {name:?}\n\navailable inventories:\n  {}{}",
        shown.join("\n  "),
        if more > 0 {
            format!("\n  ... and {more} more (nmsedit inventories --all)")
        } else {
            String::new()
        }
    ))
}

/// The data pack, when one has been generated. Item names are decoration for
/// the commands below; editing works without them.
fn optional_pack(args: &[String]) -> Option<nms_data::DataPack> {
    nms_data::DataPack::load(&pack_path(args).ok()?).ok()
}

fn item_name(pack: Option<&nms_data::DataPack>, id: &str) -> String {
    pack.and_then(|pack| pack.item_name(id)).unwrap_or_default()
}

fn print_slot(slot: &ItemSlot, pack: Option<&nms_data::DataPack>) {
    let mut flags = String::new();
    if slot.kind == ItemKind::Technology && slot.amount < 0 {
        flags.push_str("  no charge");
    }
    if slot.damage > 0.0 {
        flags.push_str(&format!("  damaged {:.0}%", slot.damage * 100.0));
    }
    if !slot.installed {
        flags.push_str("  not installed");
    }
    // The stack column is padded so flags line up; trim the padding on the
    // last column instead of leaving trailing spaces in terminal output.
    let line = format!(
        "[{:>3}] ({:>2},{:>2}) {:<26} {:<30} {:>7}/{:<7}{}",
        slot.position,
        slot.cell.x,
        slot.cell.y,
        slot.id,
        item_name(pack, &slot.id),
        slot.amount,
        slot.max_amount,
        flags
    );
    println!("{}", line.trim_end());
}

fn cmd_currencies(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let bundle = load_bundle(args, path)?;
    let currencies = bundle.currencies().map_err(|e| e.to_string())?;
    for currency in Currency::ALL {
        let value = currencies.get(currency);
        let note = if value == u32::MAX { "  (at the game's cap)" } else { "" };
        println!("{:<12} {:>10}{note}", currency.label(), value);
    }
    Ok(())
}

fn cmd_currency(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let name = positional
        .get(1)
        .ok_or_else(|| format!("missing currency name (units, nanites or quicksilver)\n\n{USAGE}"))?;
    let value = positional.get(2).ok_or_else(|| format!("missing balance\n\n{USAGE}"))?;
    let currency = Currency::parse(name)
        .ok_or_else(|| format!("unknown currency {name:?} (use units, nanites or quicksilver)"))?;
    let value = parse_u32(value, "the balance")?;

    let data_path = PathBuf::from(path);
    let mut bundle = load_bundle(args, path)?;
    let before = bundle.currencies().map_err(|e| e.to_string())?;
    if !bundle.set_currency(currency, value).map_err(|e| e.to_string())? {
        println!("{} already holds {value}, nothing to do", currency.label());
        return Ok(());
    }
    println!("{}: {} -> {value}", currency.label(), before.get(currency));
    commit_and_report(args, &mut bundle, &data_path)
}

fn cmd_inventories(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let bundle = load_bundle(args, path)?;
    let all = flag(args, "--all");
    let inventories = bundle.inventories();
    if inventories.is_empty() {
        return Err(
            "this save has no readable inventories\nnote: inventory fields need the community key mapping; see --mapping"
                .to_string(),
        );
    }

    println!("{:<44} {:>7} {:>12}  {:<14} {:<3} role", "inventory", "grid", "used/valid", "group", "cls");
    let mut shown = 0usize;
    for inventory in &inventories {
        if inventory.role == nms_edit::InventoryRole::Auxiliary && !all {
            continue;
        }
        shown += 1;
        let role = match inventory.role {
            nms_edit::InventoryRole::Player => "",
            nms_edit::InventoryRole::Auxiliary => "auxiliary",
        };
        println!(
            "{:<44} {:>7} {:>6}/{:<5}  {:<14} {:<3} {role}",
            inventory.path.label(),
            format!("{}x{}", inventory.width, inventory.height),
            inventory.slots.len(),
            inventory.valid_cells.len(),
            inventory.stack_group,
            inventory.class,
        );
    }
    if shown == 0 {
        println!("(no player inventories; pass --all to include maintenance and refiner containers)");
    }
    Ok(())
}

fn cmd_inventory(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let name = positional.get(1).ok_or_else(|| format!("missing inventory name\n\n{USAGE}"))?;
    let bundle = load_bundle(args, path)?;
    let inventory = inventory_or_error(&bundle, name)?;
    let pack = optional_pack(args);

    println!(
        "{}  {}x{}  {}/{} slots  group {}  class {}",
        inventory.path,
        inventory.width,
        inventory.height,
        inventory.slots.len(),
        inventory.valid_cells.len(),
        inventory.stack_group,
        inventory.class
    );
    for slot in &inventory.slots {
        print_slot(slot, pack.as_ref());
    }
    Ok(())
}

fn cmd_add_item(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let name = positional.get(1).ok_or_else(|| format!("missing inventory name\n\n{USAGE}"))?;
    let id = positional.get(2).ok_or_else(|| format!("missing item id\n\n{USAGE}"))?;
    let amount = positional.get(3).ok_or_else(|| format!("missing amount\n\n{USAGE}"))?;
    let amount = parse_amount(amount)?;

    let data_path = PathBuf::from(path);
    let mut bundle = load_bundle(args, path)?;
    let inventory = inventory_or_error(&bundle, name)?;

    let kind = match option(args, "--kind") {
        Some(text) => Some(
            ItemKind::parse(&text)
                .ok_or_else(|| format!("unknown item kind {text:?} (use product, substance or technology)"))?,
        ),
        None => None,
    }
    .or_else(|| {
        let pack = optional_pack(args)?;
        pack.item_category(id).and_then(ItemKind::parse)
    });
    let max_amount = option(args, "--max").map(|text| parse_amount(&text)).transpose()?;
    let cell = option(args, "--cell").map(|text| parse_cell(&text)).transpose()?;

    let item = NewItem { id: id.to_string(), amount, kind, max_amount, cell };
    let added = bundle.add_item(&inventory.path, &item).map_err(|e| e.to_string())?;
    let pack = optional_pack(args);
    for slot in &added {
        print_slot(slot, pack.as_ref());
    }
    commit_and_report(args, &mut bundle, &data_path)
}

fn cmd_remove_item(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let name = positional.get(1).ok_or_else(|| format!("missing inventory name\n\n{USAGE}"))?;
    let index = positional.get(2).ok_or_else(|| format!("missing slot index\n\n{USAGE}"))?;
    let index: usize = index.parse().map_err(|_| format!("slot index must be a whole number, got {index:?}"))?;

    let data_path = PathBuf::from(path);
    let mut bundle = load_bundle(args, path)?;
    let inventory = inventory_or_error(&bundle, name)?;
    let removed = bundle.remove_item(&inventory.path, index).map_err(|e| e.to_string())?;
    println!("removed {index}: {}", removed.id);
    commit_and_report(args, &mut bundle, &data_path)
}

fn cmd_set_amount(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let name = positional.get(1).ok_or_else(|| format!("missing inventory name\n\n{USAGE}"))?;
    let index = positional.get(2).ok_or_else(|| format!("missing slot index\n\n{USAGE}"))?;
    let amount = positional.get(3).ok_or_else(|| format!("missing amount\n\n{USAGE}"))?;
    let index: usize = index.parse().map_err(|_| format!("slot index must be a whole number, got {index:?}"))?;
    let amount = parse_amount(amount)?;

    let data_path = PathBuf::from(path);
    let mut bundle = load_bundle(args, path)?;
    let inventory = inventory_or_error(&bundle, name)?;
    let before = inventory
        .slots
        .iter()
        .find(|slot| slot.position == index)
        .map(|slot| slot.amount)
        .ok_or_else(|| format!("{} has no slot {index}", inventory.path))?;
    bundle.set_amount(&inventory.path, index, amount).map_err(|e| e.to_string())?;
    println!("{}[{index}]: {before} -> {amount}", inventory.path);
    commit_and_report(args, &mut bundle, &data_path)
}

fn cmd_repair(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let data_path = PathBuf::from(path);
    let mut bundle = load_bundle(args, path)?;

    match (positional.get(1), positional.get(2)) {
        (Some(name), Some(index)) => {
            let index: usize = index.parse().map_err(|_| format!("slot index must be a whole number, got {index:?}"))?;
            let inventory = inventory_or_error(&bundle, name)?;
            if !bundle.repair(&inventory.path, index).map_err(|e| e.to_string())? {
                println!("{}[{index}] is not damaged", inventory.path);
                return Ok(());
            }
            println!("repaired {}[{index}]", inventory.path);
        }
        (None, None) => {
            let repaired = bundle.repair_all().map_err(|e| e.to_string())?;
            if repaired == 0 {
                println!("nothing is damaged");
                return Ok(());
            }
            println!("repaired {repaired} damaged slot(s)");
        }
        _ => return Err(format!("pass either no inventory, or an inventory and a slot index\n\n{USAGE}")),
    }
    commit_and_report(args, &mut bundle, &data_path)
}

fn cmd_move_item(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let name = positional.get(1).ok_or_else(|| format!("missing inventory name\n\n{USAGE}"))?;
    let index = positional.get(2).ok_or_else(|| format!("missing slot index\n\n{USAGE}"))?;
    let x = positional.get(3).ok_or_else(|| format!("missing cell column\n\n{USAGE}"))?;
    let y = positional.get(4).ok_or_else(|| format!("missing cell row\n\n{USAGE}"))?;
    let index: usize = index.parse().map_err(|_| format!("slot index must be a whole number, got {index:?}"))?;
    let cell = parse_cell(&format!("{x},{y}"))?;

    let data_path = PathBuf::from(path);
    let mut bundle = load_bundle(args, path)?;
    let inventory = inventory_or_error(&bundle, name)?;
    if !bundle.move_item(&inventory.path, index, cell).map_err(|e| e.to_string())? {
        println!("{}[{index}] is already at {cell}", inventory.path);
        return Ok(());
    }
    println!("moved {}[{index}] to {cell}", inventory.path);
    commit_and_report(args, &mut bundle, &data_path)
}

fn parse_known_list(text: &str) -> Result<KnownList, String> {
    KnownList::parse(text).ok_or_else(|| {
        format!("unknown list {text:?} (use products, technology, specials, base-parts or refiner-recipes)")
    })
}

fn cmd_known(args: &[String]) -> Result<(), String> {
    let positional = positional(args);
    let path = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let bundle = load_bundle(args, path)?;

    match positional.get(1) {
        Some(name) => {
            let list = parse_known_list(name)?;
            let ids = bundle.known(list).map_err(|e| e.to_string())?;
            let pack = optional_pack(args);
            println!("{}: {} id(s)", list.label(), ids.len());
            for id in &ids {
                println!("{:<32} {}", id, item_name(pack.as_ref(), id));
            }
        }
        None => {
            for list in KnownList::ALL {
                let ids = bundle.known(list).map_err(|e| e.to_string())?;
                println!("{:<18} {:>5} id(s)", list.label(), ids.len());
            }
            println!("\npass a list name to print the ids");
        }
    }
    Ok(())
}

fn cmd_known_edit(args: &[String]) -> Result<(), String> {
    let add = args.first().map(String::as_str) == Some("known-add");
    let positional = positional(args);
    let path = positional.first().ok_or_else(|| format!("missing save path\n\n{USAGE}"))?;
    let name = positional.get(1).ok_or_else(|| format!("missing list name\n\n{USAGE}"))?;
    let id = positional.get(2).ok_or_else(|| format!("missing item id\n\n{USAGE}"))?;
    let list = parse_known_list(name)?;

    let data_path = PathBuf::from(path);
    let mut bundle = load_bundle(args, path)?;
    let changed = bundle.set_known(list, id, add).map_err(|e| e.to_string())?;
    let id = nms_edit::normalize_id(id);
    if !changed {
        if add {
            println!("{id} is already known ({})", list.label());
        } else {
            println!("{id} is not in the {} list", list.label());
        }
        return Ok(());
    }
    let (verb, preposition) = if add { ("added", "to") } else { ("removed", "from") };
    println!("{verb} {id} {preposition} the {} list", list.label());
    commit_and_report(args, &mut bundle, &data_path)
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
fn cmd_mcp(_args: &[String]) -> Result<(), String> {
    // stdout carries the protocol, so nothing else may print there; notes for
    // a human watching the log go to stderr.
    let server = nms_mcp::Server::from_config();
    eprintln!(
        "nmsedit mcp: ready (mapping: {}, data pack: {})",
        if server.mapping().is_some() { "loaded" } else { "none" },
        if server.has_pack() { "loaded" } else { "none" },
    );
    let stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    server.run(stdin, &mut stdout).map_err(|error| format!("mcp: {error}"))
}

fn cmd_data(args: &[String]) -> Result<(), String> {
    match positional(args).first().copied() {
        Some("refresh") => cmd_data_refresh(args),
        Some("info") => cmd_data_info(args),
        Some("lookup") => cmd_data_lookup(args),
        Some("item") => cmd_data_item(args),
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
    for table in &report.items.tables {
        println!(
            "  {:<34} {:>5} records, {:>5} named ({:>3}%)",
            table.table,
            table.records,
            table.resolved,
            table.resolved * 100 / table.records.max(1)
        );
    }
    println!(
        "items           {} named ({} derived from text keys)",
        pack.item_count(),
        report.items.derived
    );
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
    println!("items           {} named", pack.item_count());
    let mut by_category: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for category in pack.categories.values() {
        *by_category.entry(category.as_str()).or_default() += 1;
    }
    let summary: Vec<String> = by_category.iter().map(|(k, v)| format!("{k} {v}")).collect();
    println!("categories      {}", summary.join(", "));
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!("file size       {size} bytes");
    Ok(())
}

fn cmd_data_item(args: &[String]) -> Result<(), String> {
    let path = pack_path(args)?;
    let pack = nms_data::DataPack::load(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ids: Vec<&str> = positional(args).into_iter().skip(1).collect();
    if ids.is_empty() {
        return Err(format!("missing item id\n\n{USAGE}"));
    }
    let mut missing = 0usize;
    for id in ids {
        match pack.item_name_with_source(id) {
            Some((name, source)) => println!(
                "{id:<32} {name:<34} [{}{}]",
                source.label(),
                pack.item_category(id).map(|c| format!(", {c}")).unwrap_or_default()
            ),
            None => {
                println!("{id:<32} (no name found)");
                missing += 1;
            }
        }
    }
    if missing > 0 {
        return Err(format!("{missing} item(s) unresolved"));
    }
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
