//! The tools the server exposes, and what each one does.
//!
//! Every write goes through [`edit_and_commit`], which is the same
//! load-edit-commit path the CLI uses: the edit runs against the in-memory
//! document, and only a document that actually changed is written, with a
//! timestamped backup first.

use std::path::{Path, PathBuf};

use nms_edit::{
    now_unix, normalize_id, BackupStore, CommitOptions, Currency, Inventory, ItemKind, KnownList, NewItem,
    SaveBundle,
};
use serde_json::{json, Value};

use crate::Server;

/// Tool arguments, as the client sent them.
type Args = serde_json::Map<String, Value>;

/// The full tool catalogue.
pub fn definitions() -> Vec<Value> {
    vec![
        tool(
            "list_slots",
            "List the saves in a folder, with the name, game mode, location and play time their \
             metadata files carry. Every game slot holds two save points — an Autosave and a \
             Restore Point (the manual save) — and each result carries save_point and \
             newest_in_slot so the pair can be told apart; the game loads the newer one by default. \
             Without a folder, the usual save locations on this machine are searched \
             (NMSEDIT_SAVE_DIR wins when set). Start here to find a save by name.",
            json!({ "folder": string_prop("A save folder; omit to search the usual locations") }),
            &[],
        ),
        tool(
            "save_info",
            "Version, platform, active context, save name, play time and save point (Autosave or \
             Restore Point) of one save file.",
            json!({ "save": save_prop() }),
            &["save"],
        ),
        tool(
            "read_currencies",
            "The three balances the game shows: units, nanites and quicksilver. They cap at \
             4294967295. To change one, use set_currency — they are not inventory items.",
            json!({ "save": save_prop() }),
            &["save"],
        ),
        tool(
            "read_suit_stats",
            "The exosuit's three stats: health, shield and energy. They are plain fields, not \
             items; to change one, use set_suit_stat.",
            json!({ "save": save_prop() }),
            &["save"],
        ),
        tool(
            "list_inventories",
            "Every inventory in the save: name, grid size, how many slots are used, and whether it \
             is player-facing or machine-owned (refiner buffers and repair grids). Use the name \
             with read_inventory, add_item and the other inventory tools.",
            json!({ "save": save_prop() }),
            &["save"],
        ),
        tool(
            "read_inventory",
            "The occupied slots of one inventory, with item ids, amounts, grid cells and damage.",
            json!({
                "save": save_prop(),
                "inventory": string_prop("Inventory name from list_inventories"),
            }),
            &["save", "inventory"],
        ),
        tool(
            "search_items",
            "Find item ids by name, for example \"carbon\" or \"warp cell\". Returns id, name and \
             kind, best matches first; pass the id to add_item.",
            json!({
                "query": string_prop("Text to look for in an item's name or id"),
                "limit": integer_prop("How many matches to return (default 25, maximum 200)"),
            }),
            &["query"],
        ),
        tool(
            "read_known_list",
            "Ids in one of the player's learned lists: products, technology, specials, base-parts \
             or refiner-recipes.",
            json!({
                "save": save_prop(),
                "list": string_prop("products, technology, specials, base-parts or refiner-recipes"),
                "query": string_prop("Only ids whose id or item name contains this text"),
                "limit": integer_prop("How many ids to return (default 100, maximum 2000)"),
            }),
            &["save", "list"],
        ),
        tool(
            "list_backups",
            "The backups the editor has written for one save, newest first, with the state each one \
             holds (when the game wrote it, the save's name, the play time), its size, and whether \
             it is pinned. restore_backup takes one of these paths.",
            json!({ "save": save_prop() }),
            &["save"],
        ),
        tool(
            "get_field",
            "Read any field by readable, dotted path, for example CommonStateData.SaveName or \
             PlayerStateData.Inventory[3].b2n. Array indices go in brackets. Readable names come \
             from the key mapping (the community file, or one derived from the game \
             with `nmsedit data refresh`); on-disk keys work too.",
            json!({
                "save": save_prop(),
                "path": string_prop("Dotted field path"),
            }),
            &["save", "path"],
        ),
        tool(
            "set_currency",
            "Set one balance and commit. A timestamped backup is written first, and a save whose \
             value did not change is not rewritten.",
            json!({
                "save": save_prop(),
                "currency": enum_prop("Which balance", &["units", "nanites", "quicksilver"]),
                "value": integer_prop("The number the game shows, 0 to 4294967295"),
            }),
            &["save", "currency", "value"],
        ),
        tool(
            "set_suit_stat",
            "Set one exosuit stat and commit. A timestamped backup is written first, and a save \
             whose value did not change is not rewritten. The game reads these as signed 32-bit \
             values, so the largest accepted value is 2147483647.",
            json!({
                "save": save_prop(),
                "stat": enum_prop("Which stat", &["health", "shield", "energy"]),
                "value": integer_prop("The new value, 0 to 2147483647"),
            }),
            &["save", "stat", "value"],
        ),
        tool(
            "add_item",
            "Add items to an inventory, filling existing stacks first and opening new slots as \
             needed, then commit. Stack sizes come from the save unless max_amount is given.",
            json!({
                "save": save_prop(),
                "inventory": string_prop("Inventory name from list_inventories"),
                "id": string_prop("Item id such as ^CARBON; search_items resolves a name to an id"),
                "amount": integer_prop("How many to add"),
                "kind": enum_prop("Item kind; from the save when omitted", &["product", "substance", "technology"]),
                "max_amount": integer_prop("Stack size to write; from the save when omitted"),
            }),
            &["save", "inventory", "id", "amount"],
        ),
        tool(
            "set_item_amount",
            "Set one slot's amount and commit. Technology uses -1 for \"not chargeable\".",
            json!({
                "save": save_prop(),
                "inventory": string_prop("Inventory name from list_inventories"),
                "slot": integer_prop("Position in the inventory's slot array, from read_inventory"),
                "amount": integer_prop("The new amount"),
            }),
            &["save", "inventory", "slot", "amount"],
        ),
        tool(
            "remove_item",
            "Remove one slot from an inventory and commit.",
            json!({
                "save": save_prop(),
                "inventory": string_prop("Inventory name from list_inventories"),
                "slot": integer_prop("Position in the inventory's slot array, from read_inventory"),
            }),
            &["save", "inventory", "slot"],
        ),
        tool(
            "repair_all",
            "Repair every damaged slot in the player's inventories and commit.",
            json!({
                "save": save_prop(),
            }),
            &["save"],
        ),
        tool(
            "known_add",
            "Mark an id as learned in one of the player's lists and commit. The list is created \
             only where the save already has it.",
            json!({
                "save": save_prop(),
                "list": string_prop("products, technology, specials, base-parts or refiner-recipes"),
                "id": string_prop("Item id such as ^UP_SCAN"),
            }),
            &["save", "list", "id"],
        ),
        tool(
            "known_remove",
            "Forget an id in one of the player's learned lists and commit.",
            json!({
                "save": save_prop(),
                "list": string_prop("products, technology, specials, base-parts or refiner-recipes"),
                "id": string_prop("Item id"),
            }),
            &["save", "list", "id"],
        ),
        tool(
            "set_field",
            "Set any field by readable, dotted path. The value is parsed as JSON when it is valid \
             JSON, and stored as a string otherwise. The path must already exist unless create is \
             true, so a typo cannot invent a field.",
            json!({
                "save": save_prop(),
                "path": string_prop("Dotted field path"),
                "value": { "description": "The new value: JSON, or plain text for a string" },
                "create": bool_prop("Allow the field to be created when it does not exist (default false)"),
            }),
            &["save", "path", "value"],
        ),
        tool(
            "pin_backup",
            "Pin a backup against pruning, or unpin it. The editor keeps the newest ten unpinned \
             backups per save; a pinned one is never removed and does not count against the limit. \
             Pin a known-good state before a risky edit.",
            json!({
                "save": save_prop(),
                "archive": string_prop("Archive path, or a file name from list_backups"),
                "pinned": bool_prop("True pins it (the default), false unpins it"),
            }),
            &["save", "archive"],
        ),
        tool(
            "delete_backup",
            "Delete one backup archive. The save itself is untouched; only the restore point goes.",
            json!({
                "save": save_prop(),
                "archive": string_prop("Archive path, or a file name from list_backups"),
            }),
            &["save", "archive"],
        ),
        tool(
            "restore_backup",
            "Replace a save with one of its backups and commit. The files being replaced are \
             archived under the pre-restore label first, so a restore is itself undoable. The \
             archive may be a full path or a file name inside the save's backup folder.",
            json!({
                "save": save_prop(),
                "archive": string_prop("Archive path, or a file name from list_backups"),
            }),
            &["save", "archive"],
        ),
    ]
}

/// Run one tool, or `None` when no such tool exists.
pub fn call(server: &Server, name: &str, args: &Args) -> Option<Result<String, String>> {
    let result = match name {
        "list_slots" => list_slots(args),
        "save_info" => save_info(server, args),
        "read_currencies" => read_currencies(server, args),
        "read_suit_stats" => read_suit_stats(server, args),
        "list_inventories" => list_inventories(server, args),
        "read_inventory" => read_inventory(server, args),
        "search_items" => search_items(server, args),
        "read_known_list" => read_known_list(server, args),
        "list_backups" => list_backups(args),
        "get_field" => get_field(server, args),
        "set_currency" => set_currency(server, args),
        "set_suit_stat" => set_suit_stat(server, args),
        "add_item" => add_item(server, args),
        "set_item_amount" => set_item_amount(server, args),
        "remove_item" => remove_item(server, args),
        "repair_all" => repair_all(server, args),
        "known_add" => known_edit(server, args, true),
        "known_remove" => known_edit(server, args, false),
        "set_field" => set_field(server, args),
        "pin_backup" => pin_backup(args),
        "delete_backup" => delete_backup(args),
        "restore_backup" => restore_backup(server, args),
        _ => return None,
    };
    Some(result)
}

// ---------------------------------------------------------------------------
// schema helpers
// ---------------------------------------------------------------------------

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
        },
    })
}

fn string_prop(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn integer_prop(description: &str) -> Value {
    json!({ "type": "integer", "description": description })
}

fn bool_prop(description: &str) -> Value {
    json!({ "type": "boolean", "description": description })
}

fn enum_prop(description: &str, values: &[&str]) -> Value {
    json!({ "type": "string", "description": description, "enum": values })
}

fn save_prop() -> Value {
    string_prop("Path to a save file (save.hg, save2.hg, ...)")
}

// ---------------------------------------------------------------------------
// argument helpers
// ---------------------------------------------------------------------------

fn arg_str(args: &Args, name: &str) -> Result<String, String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing string argument {name:?}"))
}

fn arg_opt_str(args: &Args, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

fn arg_i64(args: &Args, name: &str) -> Result<i64, String> {
    args.get(name)
        .and_then(|value| {
            value.as_i64().or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        })
        .ok_or_else(|| format!("missing integer argument {name:?}"))
}

fn arg_opt_i64(args: &Args, name: &str) -> Result<Option<i64>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        _ => arg_i64(args, name).map(Some),
    }
}

fn arg_usize(args: &Args, name: &str) -> Result<usize, String> {
    let value = args
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing whole-number argument {name:?}"))?;
    usize::try_from(value).map_err(|_| format!("{name} is too large"))
}

fn arg_opt_usize(args: &Args, name: &str, default: usize) -> Result<usize, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(default),
        _ => arg_usize(args, name),
    }
}

fn arg_u32(args: &Args, name: &str) -> Result<u32, String> {
    let value = args
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing integer argument {name:?}"))?;
    u32::try_from(value).map_err(|_| format!("{name} must be 0..={}", u32::MAX))
}

fn arg_bool(args: &Args, name: &str, default: bool) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(default)
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

fn load(server: &Server, save: &str) -> Result<SaveBundle, String> {
    let path = PathBuf::from(save);
    if !path.is_file() {
        return Err(format!("no save file at {save}"));
    }
    SaveBundle::load(&path, server.keys.clone()).map_err(|error| error.to_string())
}

/// Refuse tools that only make sense with the full key mapping.
///
/// The built-in baseline names the common fields, but these tools edit
/// balances and learned lists whose keys the derivation cannot resolve on its
/// own; without a full mapping they would fail with a much less helpful
/// message.
fn ensure_mapping(server: &Server) -> Result<(), String> {
    if server.mapping.is_some() {
        return Ok(());
    }
    Err("this needs the full key mapping: run `nmsedit data refresh` to derive one from your own \
         game, or set NMSEDIT_MAPPING (the built-in baseline does not name every field)"
        .to_string())
}

/// Load, edit and commit, returning a report an agent can read.
fn edit_and_commit(
    server: &Server,
    save: &str,
    edit: impl FnOnce(&mut SaveBundle) -> Result<String, String>,
) -> Result<String, String> {
    let path = PathBuf::from(save);
    let mut bundle = load(server, save)?;
    let summary = edit(&mut bundle)?;
    if !bundle.document.is_dirty() {
        return Ok(format!("{summary}\nnothing changed; the file was not rewritten"));
    }

    let options = CommitOptions {
        backup_root: Some(backup_root(&path)),
        keep_backups: 10,
        timestamp: Some(now_unix()),
        dry_run: false,
    };
    let report = bundle.commit(&options).map_err(|error| error.to_string())?;

    let mut out = summary;
    if !report.changes.fields_changed.is_empty() {
        out.push('\n');
        out.push_str(&report.changes.field_report());
    }
    match report.data {
        nms_format::WriteOutcome::Unchanged => out.push_str("\nnothing to write"),
        nms_format::WriteOutcome::Written(bytes) => out.push_str(&format!(
            "\nwrote {bytes} bytes (metadata: {})",
            match report.metadata {
                nms_format::WriteOutcome::Written(bytes) => format!("{bytes} bytes"),
                nms_format::WriteOutcome::Unchanged => "unchanged".to_string(),
            }
        )),
    }
    if let Some(backup) = &report.backup {
        out.push_str(&format!("\nbackup: {}", backup.path.display()));
    }
    for warning in &report.warnings {
        out.push_str(&format!("\nwarning: {warning}"));
    }
    // A write into the game's own folder syncs through Steam Cloud: the model
    // should say so, because the backup is only on this machine.
    if let Some(notice) = nms_edit::live_notice(&path) {
        out.push_str(&format!("\nnote: {notice}"));
    }
    Ok(out)
}

/// Where a save's backups live; the same folder the CLI and GUI use.
fn backup_root(data_path: &Path) -> PathBuf {
    data_path.parent().unwrap_or_else(|| Path::new(".")).join("Save Backups")
}

fn inventory_or_error(bundle: &SaveBundle, name: &str) -> Result<Inventory, String> {
    if let Some(inventory) = bundle.inventory(name) {
        return Ok(inventory);
    }
    // A longer path resolves too; see `SaveBundle::inventory`.
    let available: Vec<String> = bundle
        .inventories()
        .iter()
        .map(|inventory| inventory.path.label().to_string())
        .collect();
    if available.is_empty() {
        return Err(format!(
            "no inventory named {name:?}: this save has no readable inventories"
        ));
    }
    let shown = available.iter().take(12).cloned().collect::<Vec<_>>().join(", ");
    let more = if available.len() > 12 { ", ..." } else { "" };
    Err(format!("no inventory named {name:?}; available: {shown}{more}"))
}

fn known_list(args: &Args) -> Result<KnownList, String> {
    let name = arg_str(args, "list")?;
    KnownList::parse(&name).ok_or_else(|| {
        format!("unknown list {name:?}; one of: products, technology, specials, base-parts, refiner-recipes")
    })
}

fn json_text(value: Value) -> String {
    serde_json::to_string(&value).unwrap_or_else(|_| value.to_string())
}

fn value_text(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

// ---------------------------------------------------------------------------
// read tools
// ---------------------------------------------------------------------------

fn list_slots(args: &Args) -> Result<String, String> {
    let roots: Vec<PathBuf> = match arg_opt_str(args, "folder") {
        Some(folder) => vec![PathBuf::from(folder)],
        None => match std::env::var("NMSEDIT_SAVE_DIR") {
            Ok(dir) if !dir.is_empty() => vec![PathBuf::from(dir)],
            _ => crate::default_save_roots(),
        },
    };
    if roots.is_empty() {
        return Err("no save folder given and none of the usual locations exist; pass folder \
                    (or set NMSEDIT_SAVE_DIR)"
            .to_string());
    }

    let mut folders = Vec::new();
    let mut slots = Vec::new();
    for root in &roots {
        for dir in crate::save_dirs(root) {
            folders.push(dir.display().to_string());
            for save in crate::scan(&dir) {
                // `FoundSave` knows how it is rendered; there is one shape, in
                // nms-edit, so a new field cannot appear in one face only.
                slots.push(save.to_json());
            }
        }
    }
    Ok(json_text(json!({
        "roots": roots.iter().map(|root| root.display().to_string()).collect::<Vec<_>>(),
        "folders": folders,
        "slots": slots,
    })))
}

fn save_info(server: &Server, args: &Args) -> Result<String, String> {
    let bundle = load(server, &arg_str(args, "save")?)?;
    let info = bundle.info();
    let save_point = bundle.slot.player_number().map(nms_edit::SavePointKind::of_player_number);
    let metadata = bundle.metadata.as_ref().map(|metadata| {
        json!({
            "save_name": metadata.save_name(),
            "location": metadata.location(),
            "game_mode": metadata.game_mode(),
            "play_time_seconds": metadata.play_time,
        })
    });
    Ok(json_text(json!({
        "file": bundle.data_path.display().to_string(),
        "slot": bundle.slot.to_string(),
        "save_point": save_point.map(|kind| kind.slug()),
        "version": info.version,
        "platform": info.platform,
        "active_context": info.active_context,
        "save_name": info.save_name,
        "play_time_seconds": info.play_time,
        "metadata": metadata,
    })))
}

fn read_currencies(server: &Server, args: &Args) -> Result<String, String> {
    ensure_mapping(server)?;
    let bundle = load(server, &arg_str(args, "save")?)?;
    let currencies = bundle.currencies().map_err(|error| error.to_string())?;
    Ok(json_text(json!({
        "units": currencies.units,
        "nanites": currencies.nanites,
        "quicksilver": currencies.quicksilver,
        "cap": u32::MAX,
    })))
}

fn read_suit_stats(server: &Server, args: &Args) -> Result<String, String> {
    ensure_mapping(server)?;
    let bundle = load(server, &arg_str(args, "save")?)?;
    let stats = bundle.suit_stats().map_err(|error| error.to_string())?;
    Ok(json_text(json!({
        "health": stats.health,
        "shield": stats.shield,
        "energy": stats.energy,
        "max": nms_edit::SuitStat::MAX,
    })))
}

fn list_inventories(server: &Server, args: &Args) -> Result<String, String> {
    ensure_mapping(server)?;
    let bundle = load(server, &arg_str(args, "save")?)?;
    let inventories: Vec<Value> = bundle
        .inventories()
        .iter()
        .enumerate()
        .map(|(index, inventory)| {
            json!({
                "index": index,
                "name": inventory.path.label(),
                "role": match inventory.role {
                    nms_edit::InventoryRole::Player => "player",
                    nms_edit::InventoryRole::Auxiliary => "machine",
                },
                "width": inventory.width,
                "height": inventory.height,
                "slots_used": inventory.slots.len(),
                "cells": inventory.valid_cells.len(),
                "stack_group": inventory.stack_group,
                "class": inventory.class,
            })
        })
        .collect();
    Ok(json_text(json!({ "inventories": inventories })))
}

fn read_inventory(server: &Server, args: &Args) -> Result<String, String> {
    ensure_mapping(server)?;
    let bundle = load(server, &arg_str(args, "save")?)?;
    let inventory = inventory_or_error(&bundle, &arg_str(args, "inventory")?)?;
    let slots: Vec<Value> = inventory
        .slots
        .iter()
        .map(|slot| {
            let name = server.pack.as_ref().and_then(|pack| pack.item_name(&slot.id));
            json!({
                "slot": slot.position,
                "cell": format!("{},{}", slot.cell.x, slot.cell.y),
                "id": slot.id,
                "name": name,
                "kind": slot.kind.label().to_ascii_lowercase(),
                "amount": slot.amount,
                "max_amount": slot.max_amount,
                "damage": slot.damage,
                "installed": slot.installed,
            })
        })
        .collect();
    Ok(json_text(json!({
        "name": inventory.path.label(),
        "role": match inventory.role {
            nms_edit::InventoryRole::Player => "player",
            nms_edit::InventoryRole::Auxiliary => "machine",
        },
        "width": inventory.width,
        "height": inventory.height,
        "slots": slots,
    })))
}

fn search_items(server: &Server, args: &Args) -> Result<String, String> {
    let query = arg_str(args, "query")?;
    let limit = arg_opt_usize(args, "limit", 25)?.min(200);
    let Some(pack) = server.pack.as_ref() else {
        return Err("no data pack: run `nmsedit data refresh` to read item names from the game, \
                    or pass item ids such as ^CARBON directly"
            .to_string());
    };
    let items: Vec<Value> = pack
        .search_items(&query, limit)
        .into_iter()
        .map(|(id, name)| {
            json!({
                "id": format!("^{id}"),
                "name": name,
                "kind": pack.item_category(id).map(str::to_ascii_lowercase),
            })
        })
        .collect();

    // Units, nanites and quicksilver are balances, not items, and an agent
    // asked to "add 100,000 nanites" will search for them here first.
    let trimmed = query.trim().to_ascii_lowercase();
    let singular = trimmed.strip_suffix('s').unwrap_or(&trimmed);
    let balance = match singular {
        "unit" => Some("units"),
        "nanite" => Some("nanites"),
        "quicksilver" => Some("quicksilver"),
        _ => None,
    };
    let mut out = json!({ "query": query, "items": items });
    if let Some(balance) = balance {
        out["balance"] = json!({
            "name": balance,
            "hint": "a balance, not an inventory item: change it with set_currency",
        });
    }
    Ok(json_text(out))
}

fn read_known_list(server: &Server, args: &Args) -> Result<String, String> {
    ensure_mapping(server)?;
    let bundle = load(server, &arg_str(args, "save")?)?;
    let list = known_list(args)?;
    let all = bundle.known(list).map_err(|error| error.to_string())?;
    let query = arg_opt_str(args, "query").map(|text| text.to_ascii_lowercase());
    let limit = arg_opt_usize(args, "limit", 100)?.min(2000);

    let mut ids: Vec<String> = all
        .iter()
        .filter(|id| match &query {
            None => true,
            Some(needle) => {
                id.to_ascii_lowercase().contains(needle)
                    || server
                        .pack
                        .as_ref()
                        .and_then(|pack| pack.item_name(id))
                        .is_some_and(|name| name.to_ascii_lowercase().contains(needle))
            }
        })
        .cloned()
        .collect();
    let matched = ids.len();
    ids.truncate(limit);
    Ok(json_text(json!({
        "list": list.label(),
        "total": all.len(),
        "matched": matched,
        "ids": ids,
    })))
}

fn list_backups(args: &Args) -> Result<String, String> {
    let save = arg_str(args, "save")?;
    let path = PathBuf::from(&save);
    let label = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{save} does not look like a save file"))?;
    let root = backup_root(&path);
    let store = BackupStore::new(root.clone(), 10);
    let entries = store.list(&label).map_err(|error| error.to_string())?;
    let backups: Vec<Value> = entries
        .iter()
        .map(|entry| {
            // The metadata file inside each archive says which state it holds;
            // the save payload is not decompressed.
            let summary = store.inspect(&entry.path).ok();
            json!({
                "file": entry.path.display().to_string(),
                "name": entry.path.file_name().map(|name| name.to_string_lossy().into_owned()),
                "size": entry.size,
                "pinned": entry.kept,
                "state_from": summary.as_ref().and_then(|summary| summary.state_time()).map(nms_edit::format_utc),
                "save_name": summary.as_ref().and_then(|summary| summary.save_name()),
                "play_time_seconds": summary.as_ref().and_then(|summary| summary.play_time()),
                "modified_unix": std::fs::metadata(&entry.path)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|age| age.as_secs()),
            })
        })
        .collect();
    Ok(json_text(json!({
        "backup_dir": root.display().to_string(),
        "backups": backups,
    })))
}

fn get_field(server: &Server, args: &Args) -> Result<String, String> {
    let bundle = load(server, &arg_str(args, "save")?)?;
    let path = arg_str(args, "path")?;
    let value = bundle.get(&path).map_err(|error| format!("{path}: {error}"))?;
    Ok(json_text(json!({
        "path": path,
        "value": value,
    })))
}

// ---------------------------------------------------------------------------
// write tools
// ---------------------------------------------------------------------------

fn set_currency(server: &Server, args: &Args) -> Result<String, String> {
    ensure_mapping(server)?;
    let save = arg_str(args, "save")?;
    let name = arg_str(args, "currency")?;
    let currency = Currency::parse(&name)
        .ok_or_else(|| format!("unknown currency {name:?}; one of: units, nanites, quicksilver"))?;
    let value = arg_u32(args, "value")?;
    edit_and_commit(server, &save, |bundle| {
        let before = bundle.currencies().map_err(|error| error.to_string())?.get(currency);
        bundle.set_currency(currency, value).map_err(|error| error.to_string())?;
        Ok(format!("{}: {before} -> {value}", currency.label()))
    })
}

fn set_suit_stat(server: &Server, args: &Args) -> Result<String, String> {
    ensure_mapping(server)?;
    let save = arg_str(args, "save")?;
    let name = arg_str(args, "stat")?;
    let stat = nms_edit::SuitStat::parse(&name)
        .ok_or_else(|| format!("unknown suit stat {name:?}; one of: health, shield, energy"))?;
    let value = arg_u32(args, "value")?;
    edit_and_commit(server, &save, |bundle| {
        let before = bundle.suit_stats().map_err(|error| error.to_string())?.get(stat);
        bundle.set_suit_stat(stat, value).map_err(|error| error.to_string())?;
        Ok(format!("{}: {before} -> {value}", stat.label()))
    })
}

fn add_item(server: &Server, args: &Args) -> Result<String, String> {
    ensure_mapping(server)?;
    let save = arg_str(args, "save")?;
    let inventory_name = arg_str(args, "inventory")?;
    let id = arg_str(args, "id")?;
    let amount = arg_i64(args, "amount")?;
    let kind = match arg_opt_str(args, "kind") {
        Some(text) => Some(
            ItemKind::parse(&text)
                .ok_or_else(|| format!("unknown kind {text:?}; one of: product, substance, technology"))?,
        ),
        None => None,
    };
    let max_amount = arg_opt_i64(args, "max_amount")?;
    edit_and_commit(server, &save, |bundle| {
        let inventory = inventory_or_error(bundle, &inventory_name)?;
        let mut item = NewItem::new(id.trim(), amount);
        item.kind = kind;
        item.max_amount = max_amount;
        let slots = bundle.add_item(&inventory.path, &item).map_err(|error| error.to_string())?;
        let id = normalize_id(id.trim());
        let cells: Vec<String> = slots.iter().map(|slot| slot.cell.to_string()).collect();
        Ok(format!(
            "added {amount} x {id} to {} ({} slot(s): {})",
            inventory.path.label(),
            slots.len(),
            cells.join(", ")
        ))
    })
}

fn set_item_amount(server: &Server, args: &Args) -> Result<String, String> {
    ensure_mapping(server)?;
    let save = arg_str(args, "save")?;
    let inventory_name = arg_str(args, "inventory")?;
    let index = arg_usize(args, "slot")?;
    let amount = arg_i64(args, "amount")?;
    edit_and_commit(server, &save, |bundle| {
        let inventory = inventory_or_error(bundle, &inventory_name)?;
        let slot = bundle.set_amount(&inventory.path, index, amount).map_err(|error| error.to_string())?;
        Ok(format!("{}[{}]: amount set to {amount} ({})", inventory.path.label(), index, slot.id))
    })
}

fn remove_item(server: &Server, args: &Args) -> Result<String, String> {
    ensure_mapping(server)?;
    let save = arg_str(args, "save")?;
    let inventory_name = arg_str(args, "inventory")?;
    let index = arg_usize(args, "slot")?;
    edit_and_commit(server, &save, |bundle| {
        let inventory = inventory_or_error(bundle, &inventory_name)?;
        let slot = bundle.remove_item(&inventory.path, index).map_err(|error| error.to_string())?;
        Ok(format!(
            "removed {} x{} from {}[{}]",
            slot.id,
            slot.amount,
            inventory.path.label(),
            index
        ))
    })
}

fn repair_all(server: &Server, args: &Args) -> Result<String, String> {
    ensure_mapping(server)?;
    let save = arg_str(args, "save")?;
    edit_and_commit(server, &save, |bundle| {
        let repaired = bundle.repair_all().map_err(|error| error.to_string())?;
        Ok(format!("repaired {repaired} damaged slot(s)"))
    })
}

fn known_edit(server: &Server, args: &Args, add: bool) -> Result<String, String> {
    ensure_mapping(server)?;
    let save = arg_str(args, "save")?;
    let list = known_list(args)?;
    let id = arg_str(args, "id")?;
    edit_and_commit(server, &save, |bundle| {
        let changed = bundle.set_known(list, &id, add).map_err(|error| error.to_string())?;
        let id = normalize_id(&id);
        let verb = if add { "added" } else { "removed" };
        let preposition = if add { "to" } else { "from" };
        Ok(if changed {
            format!("{verb} {id} {preposition} the {} list", list.label())
        } else if add {
            format!("{id} is already in the {} list", list.label())
        } else {
            format!("{id} is not in the {} list", list.label())
        })
    })
}

fn set_field(server: &Server, args: &Args) -> Result<String, String> {
    let save = arg_str(args, "save")?;
    let path = arg_str(args, "path")?;
    let raw = args
        .get("value")
        .cloned()
        .ok_or_else(|| "missing argument \"value\"".to_string())?;
    // The CLI's rule: a value that parses as JSON is used as-is, anything else
    // is a string.
    let value = match &raw {
        Value::String(text) => serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.clone())),
        other => other.clone(),
    };
    let create = arg_bool(args, "create", false);
    let shown = value_text(&value);
    edit_and_commit(server, &save, |bundle| {
        let before = match bundle.get(&path) {
            Ok(existing) => Some(value_text(existing)),
            Err(_) if create => None,
            Err(_) => {
                return Err(format!(
                    "{path}: no such field (pass create=true to add it; without that a typo would \
                     invent a field)"
                ))
            }
        };
        bundle.set(&path, value.clone()).map_err(|error| format!("{path}: {error}"))?;
        Ok(match before {
            Some(before) => format!("{path}: {before} -> {shown}"),
            None => format!("{path}: created as {shown}"),
        })
    })
}

fn pin_backup(args: &Args) -> Result<String, String> {
    let save = arg_str(args, "save")?;
    let archive = resolve_archive(&save, &arg_str(args, "archive")?)?;
    let pinned = arg_bool(args, "pinned", true);
    let store = BackupStore::new(backup_root(&PathBuf::from(&save)), 10);
    let moved = store.set_kept(&archive, pinned).map_err(|error| error.to_string())?;
    let name = moved.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out = format!(
        "{} {name}\na pinned archive is never pruned and does not count against the retention limit",
        if pinned { "pinned" } else { "unpinned" }
    );
    if let Some(notice) = nms_edit::live_notice(&moved) {
        out.push_str(&format!("\nnote: {notice}"));
    }
    Ok(out)
}

fn delete_backup(args: &Args) -> Result<String, String> {
    let save = arg_str(args, "save")?;
    let archive = resolve_archive(&save, &arg_str(args, "archive")?)?;
    let store = BackupStore::new(backup_root(&PathBuf::from(&save)), 10);
    store.delete(&archive).map_err(|error| error.to_string())?;
    let name = archive.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out = format!("deleted {name}\nthe save itself is untouched; only the restore point is gone");
    if let Some(notice) = nms_edit::live_notice(&archive) {
        out.push_str(&format!("\nnote: {notice}"));
    }
    Ok(out)
}

/// Resolve an archive argument: a path, or a file name inside the save's
/// backup folder.
fn resolve_archive(save: &str, archive: &str) -> Result<PathBuf, String> {
    let given = PathBuf::from(archive);
    let path = if given.is_file() { given } else { backup_root(&PathBuf::from(save)).join(archive) };
    if !path.is_file() {
        return Err(format!("no archive at {}; list_backups shows what exists", path.display()));
    }
    Ok(path)
}

fn restore_backup(server: &Server, args: &Args) -> Result<String, String> {
    let _ = server;
    let save = arg_str(args, "save")?;
    let save_path = PathBuf::from(&save);
    let folder = save_path
        .parent()
        .ok_or_else(|| format!("{save} has no folder"))?
        .to_path_buf();
    let archive_path = resolve_archive(&save, &arg_str(args, "archive")?)?;

    let root = backup_root(&save_path);
    let store = BackupStore::new(root.clone(), 10);
    let restored = nms_edit::backup::restore_archive(&archive_path, &folder, &store, now_unix())
        .map_err(|error| error.to_string())?;
    let mut out = format!(
        "restored {} file(s) into {} from {}\nprevious contents archived in {}",
        restored.len(),
        folder.display(),
        archive_path.display(),
        root.display()
    );
    if let Some(notice) = nms_edit::live_notice(&folder) {
        out.push_str(&format!("\nnote: {notice}"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nms_model::KeyMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    const SAVE: &str = r#"{
        "Version": 4743,
        "Platform": "Win|Final",
        "ActiveContext": "Main",
        "BaseContext": {
            "PlayerStateData": {
                "Units": 1200,
                "Nanites": 40,
                "Specials": 7,
                "Health": 180,
                "Shield": 100,
                "Energy": 76,
                "Inventory": {
                    "Slots": [
                        {"Type":{"InventoryType":"Substance"},"Id":"^CARBON","Amount":10,"MaxAmount":20,"DamageFactor":0.0,"FullyInstalled":true,"AddedAutomatically":false,"Index":{"X":0,"Y":0}}
                    ],
                    "ValidSlotIndices": [{"X":0,"Y":0},{"X":1,"Y":0}],
                    "Class": {"InventoryClass": "C"},
                    "StackSizeGroup": {"InventoryStackSizeGroup": "Personal"},
                    "Width": 2,
                    "Height": 1
                },
                "KnownProducts": ["^A"],
                "KnownTech": [],
                "KnownSpecials": [],
                "SeenBaseBuildingObjects": [],
                "KnownRefinerRecipes": []
            }
        }
    }"#;

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A scratch directory removed when the guard is dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("nms-mcp-{}-{}-{unique}", std::process::id(), name));
            std::fs::create_dir_all(&path).expect("create scratch directory");
            Scratch(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(&path, contents).expect("write scratch file");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn call_ok(server: &Server, name: &str, args: Value) -> String {
        let args = args.as_object().expect("object arguments").clone();
        call(server, name, &args).expect("a known tool").expect("the tool succeeds")
    }

    fn call_err(server: &Server, name: &str, args: Value) -> String {
        let args = args.as_object().expect("object arguments").clone();
        call(server, name, &args).expect("a known tool").expect_err("the tool fails")
    }

    #[test]
    fn a_set_currency_call_reads_edits_and_reports() {
        let scratch = Scratch::new("currency");
        let save = scratch.write("save.hg", SAVE);
        let server = mapped_server(scratch.path());
        let save = save.display().to_string();

        let read = call_ok(&server, "read_currencies", json!({ "save": save }));
        assert!(read.contains("\"units\":1200"), "{read}");
        assert!(read.contains("\"cap\":4294967295"), "{read}");

        let report = call_ok(
            &server,
            "set_currency",
            json!({ "save": save, "currency": "nanites", "value": 100040 }),
        );
        assert!(report.contains("nanites: 40 -> 100040"), "{report}");
        assert!(report.contains("backup:"), "{report}");
        assert!(report.contains("wrote"), "{report}");

        let read = call_ok(&server, "read_currencies", json!({ "save": save }));
        assert!(read.contains("\"nanites\":100040"), "{read}");

        // A second identical call changes nothing and does not rewrite the file.
        let again = call_ok(
            &server,
            "set_currency",
            json!({ "save": save, "currency": "nanites", "value": 100040 }),
        );
        assert!(again.contains("nothing changed"), "{again}");
    }

    #[test]
    fn a_set_suit_stat_call_reads_edits_and_reports() {
        let scratch = Scratch::new("suit");
        let save = scratch.write("save.hg", SAVE);
        let server = mapped_server(scratch.path());
        let save = save.display().to_string();

        let read = call_ok(&server, "read_suit_stats", json!({ "save": save }));
        assert!(read.contains("\"health\":180"), "{read}");
        assert!(read.contains("\"energy\":76"), "{read}");
        assert!(read.contains("\"max\":2147483647"), "{read}");

        let report = call_ok(
            &server,
            "set_suit_stat",
            json!({ "save": save, "stat": "health", "value": 999 }),
        );
        assert!(report.contains("health: 180 -> 999"), "{report}");
        assert!(report.contains("backup:"), "{report}");

        let read = call_ok(&server, "read_suit_stats", json!({ "save": save }));
        assert!(read.contains("\"health\":999"), "{read}");

        // A value the game's signed 32-bit field cannot hold is refused, and
        // nothing is written.
        let error = call_err(
            &server,
            "set_suit_stat",
            json!({ "save": save, "stat": "shield", "value": 3_000_000_000u64 }),
        );
        assert!(error.contains("between 0 and 2147483647"), "{error}");
    }

    #[test]
    fn a_live_folder_is_written_and_flagged() {
        let scratch = Scratch::new("live");
        let live = scratch.path().join(
            "steamapps/compatdata/275850/pfx/drive_c/users/steamuser/AppData/Roaming/HelloGames/NMS/st_1",
        );
        std::fs::create_dir_all(&live).expect("create the live-looking folder");
        let save = live.join("save.hg");
        std::fs::write(&save, SAVE).expect("write save");
        let server = mapped_server(scratch.path());
        let save = save.display().to_string();

        // The write goes through — the owner asked for live editing once every
        // write was backed up and verified — and the report says the folder is
        // the one Steam Cloud syncs.
        let report = call_ok(
            &server,
            "set_currency",
            json!({ "save": save, "currency": "units", "value": 999 }),
        );
        assert!(report.contains("units: 1200 -> 999"), "{report}");
        assert!(report.contains("note:"), "{report}");
        assert!(report.contains("Steam"), "{report}");
        assert!(report.contains("backup:"), "{report}");

        let read = call_ok(&server, "read_currencies", json!({ "save": save }));
        assert!(read.contains("\"units\":999"), "{read}");
    }

    #[test]
    fn item_tools_accept_a_unique_inventory_suffix() {
        let scratch = Scratch::new("items");
        let save = scratch.write("save.hg", SAVE);
        let server = mapped_server(scratch.path());
        let save = save.display().to_string();

        let listed = call_ok(&server, "list_inventories", json!({ "save": save }));
        assert!(listed.contains("\"name\":\"Inventory\""), "{listed}");
        assert!(listed.contains("\"slots_used\":1"), "{listed}");

        let report = call_ok(
            &server,
            "add_item",
            json!({ "save": save, "inventory": "Inventory", "id": "^IRON", "amount": 5 }),
        );
        assert!(report.contains("added 5 x ^IRON"), "{report}");
        // A full path copied out of the JSON tree resolves too, via the unique
        // suffix fallback.
        let slots = call_ok(
            &server,
            "read_inventory",
            json!({ "save": save, "inventory": "BaseContext.PlayerStateData.Inventory" }),
        );
        assert!(slots.contains("\"id\":\"^IRON\""), "{slots}");
        assert!(slots.contains("\"slot\":1"), "{slots}");

        let unknown = call_err(&server, "read_inventory", json!({ "save": save, "inventory": "Suit" }));
        assert!(unknown.contains("available:"), "{unknown}");
    }

    #[test]
    fn set_field_refuses_a_typo_unless_create_is_true() {
        let scratch = Scratch::new("field");
        let save = scratch.write("save.hg", SAVE);
        let server = mapped_server(scratch.path());
        let save = save.display().to_string();

        let typo = call_err(
            &server,
            "set_field",
            json!({ "save": save, "path": "BaseContext.PlayerStateData.Unitz", "value": 5 }),
        );
        assert!(typo.contains("create=true"), "{typo}");
        // Nothing was written: the file still has the original text.
        assert_eq!(std::fs::read_to_string(scratch.path().join("save.hg")).unwrap(), SAVE);

        let created = call_ok(
            &server,
            "set_field",
            json!({ "save": save, "path": "BaseContext.PlayerStateData.Unitz", "value": 5, "create": true }),
        );
        assert!(created.contains("created as 5"), "{created}");

        // Follow the CLI's rule: text that parses as JSON is stored as JSON, so
        // a string that looks like a number needs the quotes in the text.
        let report = call_ok(
            &server,
            "set_field",
            json!({ "save": save, "path": "BaseContext.PlayerStateData.Unitz", "value": "\"5\"" }),
        );
        assert!(report.contains("-> \"5\""), "{report}");
    }

    #[test]
    fn a_balance_search_says_to_use_set_currency() {
        let scratch = Scratch::new("balance");
        let pack_path = scratch.write("strings.json", r#"{"format":2,"language":"english","game_version":null,"generated_at":0,"notes":"t","strings":{},"items":{"TECHFRAG":"NANITE CLUSTER"},"categories":{}}"#);
        let pack = nms_data::DataPack::load(&pack_path).expect("load pack");
        let server = Server::new(KeyMap::builtin(), Some(pack), None);

        // "nanites" names items too (the cluster), so the answer explains both.
        let found = call_ok(&server, "search_items", json!({ "query": "nanite" }));
        assert!(found.contains("\"balance\""), "{found}");
        assert!(found.contains("set_currency"), "{found}");
        assert!(found.contains("^TECHFRAG"), "{found}");

        let found = call_ok(&server, "search_items", json!({ "query": "carbon" }));
        assert!(!found.contains("\"balance\""), "{found}");
    }

    #[test]
    fn backups_can_be_listed_pinned_and_deleted() {
        let scratch = Scratch::new("backups");
        let save = scratch.write("save.hg", SAVE);
        let server = mapped_server(scratch.path());
        let save = save.display().to_string();

        // A write, so there is something to manage.
        call_ok(&server, "set_currency", json!({ "save": save, "currency": "units", "value": 4321 }));
        let listed = call_ok(&server, "list_backups", json!({ "save": save }));
        let listing: Value = serde_json::from_str(&listed).expect("json");
        let backups = listing["backups"].as_array().expect("backups");
        assert_eq!(backups.len(), 1, "{listed}");
        assert_eq!(backups[0]["pinned"], false);
        let name = backups[0]["name"].as_str().expect("a name").to_string();
        assert!(name.starts_with("save_"), "{name}");

        // Pinning renames it and the listing says so.
        let report = call_ok(&server, "pin_backup", json!({ "save": save, "archive": name }));
        assert!(report.contains("pinned"), "{report}");
        let listed = call_ok(&server, "list_backups", json!({ "save": save }));
        let listing: Value = serde_json::from_str(&listed).expect("json");
        let backups = listing["backups"].as_array().expect("backups");
        assert_eq!(backups[0]["pinned"], true, "{listed}");
        let pinned_name = backups[0]["name"].as_str().expect("a name").to_string();
        assert!(pinned_name.ends_with(".keep.zip"), "{pinned_name}");

        // Unpinning, then deleting, both work by name.
        call_ok(&server, "pin_backup", json!({ "save": save, "archive": pinned_name, "pinned": false }));
        let report = call_ok(&server, "delete_backup", json!({ "save": save, "archive": name }));
        assert!(report.contains("deleted"), "{report}");
        let listed = call_ok(&server, "list_backups", json!({ "save": save }));
        let listing: Value = serde_json::from_str(&listed).expect("json");
        assert!(listing["backups"].as_array().expect("backups").is_empty(), "{listed}");

        // An archive that is not there is an error, not a panic.
        let missing = call_err(&server, "delete_backup", json!({ "save": save, "archive": "nope.zip" }));
        assert!(missing.contains("no archive at"), "{missing}");
    }

    #[test]
    fn search_items_needs_a_data_pack_and_says_so() {
        let server = Server::new(KeyMap::builtin(), None, None);
        let error = call_err(&server, "search_items", json!({ "query": "carbon" }));
        assert!(error.contains("data refresh"), "{error}");
    }

    #[test]
    fn named_field_tools_explain_a_missing_mapping() {
        let scratch = Scratch::new("mapping");
        let save = scratch.write("save.hg", SAVE);
        let server = Server::new(KeyMap::builtin(), None, None);
        let error = call_err(
            &server,
            "read_currencies",
            json!({ "save": save.display().to_string() }),
        );
        assert!(error.contains("data refresh"), "{error}");
    }

    #[test]
    fn list_slots_finds_the_saves_in_a_folder() {
        let scratch = Scratch::new("slots");
        scratch.write("save.hg", SAVE);
        scratch.write("save3.hg", SAVE);

        let listed = call_ok(&server_without_mapping(), "list_slots", json!({ "folder": scratch.path() }));
        assert!(listed.contains("save.hg"), "{listed}");
        assert!(listed.contains("save3.hg"), "{listed}");
        assert!(listed.contains("\"slot\":\"PlayerState3\""), "{listed}");
        assert!(listed.contains("\"play_time_seconds\":0"), "{listed}");
    }

    fn server_without_mapping() -> Server {
        Server::new(KeyMap::builtin(), None, None)
    }

    /// A server with a real mapping file on disk, for the tools that need one.
    fn mapped_server(dir: &Path) -> Server {
        let mapping = dir.join("mapping.json");
        std::fs::write(&mapping, "{}").expect("write mapping");
        Server::new(KeyMap::builtin(), None, Some(mapping))
    }
}
