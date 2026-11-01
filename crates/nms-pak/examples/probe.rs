//! Probe a pak: list entries, or extract one, without loading the archive.
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: probe <pak> [name-filter] [extract-out]")?;
    let filter = args.next();
    let out = args.next();

    let pak = nms_pak::Pak::open(Path::new(&path))?;
    println!("{path}: {} entries, {} chunks", pak.entries().len(), pak.chunk_count());

    match (&filter, &out) {
        (Some(name), Some(out)) => {
            let bytes = pak.extract_name(name)?;
            std::fs::write(out, &bytes)?;
            println!("extracted {name} -> {out} ({} bytes)", bytes.len());
        }
        (Some(needle), None) => {
            let hits = pak.find(needle);
            println!("{} entries matching {needle:?}:", hits.len());
            for entry in hits.iter().take(20) {
                println!("  {:<60} {:>10} bytes", entry.name, entry.size);
            }
        }
        _ => {
            for entry in pak.entries().iter().take(10) {
                println!("  {:<60} {:>10} bytes", entry.name, entry.size);
            }
        }
    }
    Ok(())
}
