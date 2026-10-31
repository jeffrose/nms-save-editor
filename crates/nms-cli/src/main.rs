//! `nmsedit` — inspect, verify, and (soon) edit No Man's Sky save files.
//!
//! This first milestone is deliberately read-only plus verification: every
//! command below proves something about the on-disk data without changing it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nms_format::metadata::{Metadata, Slot};
use nms_format::SaveFile;
use nms_model::{ByteDiff, Document, KeyMap};

const USAGE: &str = "\
nmsedit — No Man's Sky save tool

USAGE:
    nmsedit info <save.hg>          summarise a save file and its JSON payload
    nmsedit dump <save.hg> [out]    write the decompressed payload (default: stdout)
    nmsedit metadata <mf_*.hg>      decrypt and print a metadata file
    nmsedit slots <save-dir>        list every slot in a save directory
    nmsedit verify <save-dir>       round-trip every file and report fidelity
    nmsedit mapping <mapping.json>  load a community key mapping and report it

OPTIONS:
    -h, --help                      show this help
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h") | Some("--help") | None => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("info") => run(&args, cmd_info),
        Some("dump") => run(&args, cmd_dump),
        Some("metadata") => run(&args, cmd_metadata),
        Some("slots") => run(&args, cmd_slots),
        Some("verify") => run(&args, cmd_verify),
        Some("mapping") => run(&args, cmd_mapping),
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

fn require_path(args: &[String], index: usize) -> Result<PathBuf, String> {
    args.get(index)
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing path argument\n\n{USAGE}"))
}

fn cmd_info(args: &[String]) -> Result<(), String> {
    let path = require_path(args, 1)?;
    let raw = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let save = SaveFile::from_bytes(&raw).map_err(|e| e.to_string())?;
    let doc = Document::parse(&save.payload).map_err(|e| e.to_string())?;
    let keys = KeyMap::builtin();
    let info = doc.info(&keys);

    println!("file            {}", path.display());
    println!("container       {} ({} blocks)", save.container.name(), save.blocks.len());
    println!("file size       {} bytes", raw.len());
    println!("payload         {} bytes{}", save.payload.len(), if save.trailing_nul { " (+NUL)" } else { "" });
    if let Some(layout) = save.blocks.first() {
        let largest = save.blocks.iter().map(|b| b.decompressed).max().unwrap_or(0);
        println!("block chunks    {largest} max decompressed ({} reserved words non-zero)",
            save.blocks.iter().filter(|b| b.reserved != 0).count());
        let _ = layout;
    }
    println!("json keys       {}", doc.key_count());
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
    }
    Ok(())
}

fn cmd_dump(args: &[String]) -> Result<(), String> {
    let path = require_path(args, 1)?;
    let save = SaveFile::read(&path).map_err(|e| e.to_string())?;
    match args.get(2) {
        Some(out) => {
            std::fs::write(out, &save.payload).map_err(|e| format!("{out}: {e}"))?;
            eprintln!("wrote {} bytes to {out}", save.payload.len());
        }
        None => {
            use std::io::Write;
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(&save.payload).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn cmd_metadata(args: &[String]) -> Result<(), String> {
    let path = require_path(args, 1)?;
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
    println!("words             {}", meta.words.len());
    let unknown: Vec<String> = meta
        .words
        .iter()
        .enumerate()
        .filter(|(i, w)| **w != 0 && !matches!(i, 0 | 1 | 14 | 15 | 16 | 17 | 18))
        .map(|(i, w)| format!("[{i}]=0x{w:08X}"))
        .collect();
    println!("other non-zero    {}", if unknown.is_empty() { "none".to_string() } else { unknown.join(" ") });

    let strings = readable_strings(&meta.words);
    if !strings.is_empty() {
        println!("strings           {}", strings.join(" | "));
    }
    Ok(())
}

/// Extract runs of printable ASCII from decrypted metadata words.
///
/// Metadata carries human-readable text (the current location name, and the
/// game mode such as `Normal`), which is useful for slot listings.
fn readable_strings(words: &[u32]) -> Vec<String> {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut out = Vec::new();
    let mut current = Vec::new();
    for byte in bytes {
        if (0x20..0x7F).contains(&byte) {
            current.push(byte);
        } else {
            if current.len() >= 3 {
                out.push(String::from_utf8_lossy(&current).into_owned());
            }
            current.clear();
        }
    }
    if current.len() >= 3 {
        out.push(String::from_utf8_lossy(&current).into_owned());
    }
    out
}

fn read_metadata(path: &Path, slot: Slot) -> Result<Metadata, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Metadata::read(&bytes, slot).map_err(|e| e.to_string())
}

fn cmd_slots(args: &[String]) -> Result<(), String> {
    let dir = require_path(args, 1)?;
    let entries = list_dir(&dir)?;
    let mut found = 0usize;

    for name in entries {
        let Some(slot) = Slot::from_file_name(&name) else { continue };
        let path = dir.join(&name);
        let Ok(raw) = std::fs::read(&path) else { continue };

        let meta_name = slot.metadata_file_name();
        let meta = read_metadata(&dir.join(&meta_name), slot).ok();

        let detail = match SaveFile::from_bytes(&raw).and_then(|s| Ok(Document::parse(&s.payload).map_err(|e| {
            nms_format::FormatError::Decompress { offset: 0, message: e.to_string() }
        })?)) {
            Ok(doc) => {
                let info = doc.info(&KeyMap::builtin());
                format!(
                    "{:<10} v{:<5} ctx={:<8} play={:<9} name={}",
                    info.platform.unwrap_or_else(|| "?".into()),
                    info.version.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                    info.active_context.unwrap_or_else(|| "?".into()),
                    info.play_time.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
                    info.save_name.unwrap_or_else(|| "-".into()),
                )
            }
            Err(e) => format!("unreadable: {e}"),
        };

        let meta_detail = match meta {
            Some(m) => format!("meta v{} {} bytes", m.version, m.decompressed_size),
            None => "meta: unavailable".to_string(),
        };

        println!("{:<14} {:<12} {:>10} bytes  {}  |  {}", name, slot.to_string(), raw.len(), detail, meta_detail);
        found += 1;
    }

    if found == 0 {
        return Err(format!("no save files found in {}", dir.display()));
    }
    Ok(())
}

fn cmd_verify(args: &[String]) -> Result<(), String> {
    let dir = require_path(args, 1)?;
    let entries = list_dir(&dir)?;
    let mut checked = 0usize;
    let mut failures = 0usize;

    println!("{:<16} {:>12} {:>12}  {}", "file", "bytes", "re-encoded", "result");

    for name in entries.iter().filter(|n| n.ends_with(".hg")) {
        let path = dir.join(name);
        let Ok(raw) = std::fs::read(&path) else { continue };

        let result = if let Some(slot) = Slot::from_metadata_file_name(name) {
            match Metadata::read(&raw, slot) {
                Ok(meta) => match meta.to_bytes() {
                    Ok(again) => ByteDiff::compute(&raw, &again),
                    Err(e) => {
                        println!("{name:<16} {:>12} {:>12}  ERROR {e}", raw.len(), "-");
                        failures += 1;
                        continue;
                    }
                },
                Err(e) => {
                    println!("{name:<16} {:>12} {:>12}  ERROR {e}", raw.len(), "-");
                    failures += 1;
                    continue;
                }
            }
        } else {
            match SaveFile::from_bytes(&raw) {
                Ok(save) => {
                    let again = save.to_bytes();
                    ByteDiff::compute(&raw, &again)
                }
                Err(e) => {
                    println!("{name:<16} {:>12} {:>12}  ERROR {e}", raw.len(), "-");
                    failures += 1;
                    continue;
                }
            }
        };

        // For a save with edits, round-tripping the document must reproduce the
        // payload exactly when nothing was changed.
        let document_result = if Slot::from_metadata_file_name(name).is_some() {
            None
        } else {
            match SaveFile::from_bytes(&raw).map_err(|e| e.to_string()).and_then(|s| {
                Document::parse(&s.payload).map_err(|e| e.to_string()).and_then(|doc| {
                    doc.payload().map_err(|e| e.to_string()).map(|p| (s, doc, p))
                })
            }) {
                Ok((s, doc, payload)) => Some(ByteDiff::compute(&s.payload, &payload).is_identical() && doc.key_count() > 0),
                Err(_) => Some(false),
            }
        };

        let ok = result.is_identical() && document_result.unwrap_or(true);
        let note = if ok {
            "ok".to_string()
        } else {
            failures += 1;
            format!("FAIL {}", result.summary())
        };
        println!("{name:<16} {:>12} {:>12}  {}", raw.len(), result.len_b, note);
        checked += 1;
    }

    if checked == 0 {
        return Err(format!("no .hg files found in {}", dir.display()));
    }
    println!("\n{checked} files checked, {failures} failures");
    if failures > 0 {
        return Err("round-trip fidelity check failed".into());
    }
    Ok(())
}

fn cmd_mapping(args: &[String]) -> Result<(), String> {
    let path = require_path(args, 1)?;
    let map = KeyMap::load(&path).map_err(|e| e.to_string())?;
    println!("{}: {} key mappings", path.display(), map.len());
    for name in ["Version", "Platform", "ActiveContext", "SaveName", "TotalPlayTime"] {
        println!(
            "  {:<14} -> {}",
            name,
            map.candidates(name).last().unwrap_or("(unmapped)")
        );
    }
    let merged = KeyMap::builtin().extended_with(&map);
    println!("merged with builtin: {} entries", merged.len());
    Ok(())
}

fn list_dir(dir: &Path) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Ok(names)
}
