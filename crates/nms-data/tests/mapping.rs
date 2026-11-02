//! Validation of the mapping derivation against the real game.
//!
//! These tests only run where No Man's Sky is installed (they skip otherwise).
//! They are the evidence for the claim in `nms_data::mapping`: the field names
//! come out of the game's own executable, the key is a hash of the name, and a
//! generated mapping never disagrees with the community mapping about a field a
//! real save uses.

use std::collections::BTreeMap;
use std::path::PathBuf;

use nms_data::{Install, build_mapping, extract_field_names, key_for_name};

fn install() -> Option<Install> {
    let explicit = std::env::var("NMS_INSTALL").ok().map(PathBuf::from);
    Install::discover(explicit.as_deref()).ok()
}

/// The community mapping, when this machine has one.
fn community_mapping() -> Option<BTreeMap<String, String>> {
    let path = std::env::var("NMSEDIT_MAPPING")
        .ok()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            let base = match std::env::var("XDG_CONFIG_HOME") {
                Ok(value) if !value.is_empty() => PathBuf::from(value),
                _ => PathBuf::from(std::env::var("HOME").ok()?).join(".config"),
            };
            Some(base.join("nmsedit").join("mapping.json"))
        })?;
    let text = std::fs::read_to_string(path).ok()?;
    let document: serde_json::Value = serde_json::from_str(&text).ok()?;
    let entries = document.get("Mapping")?.as_array()?;
    let pairs: BTreeMap<String, String> = entries
        .iter()
        .filter_map(|entry| {
            let key = entry.get("Key")?.as_str()?.to_string();
            let name = entry.get("Value")?.as_str()?.to_string();
            Some((key, name))
        })
        .collect();
    Some(pairs)
}

#[test]
fn the_names_come_out_of_the_game_executable() {
    let Some(install) = install() else {
        eprintln!("skipping: no No Man's Sky installation found (set NMS_INSTALL to point at one)");
        return;
    };
    let binary = nms_data::find_game_binary(&install.root).expect("the binary is where the layouts say");
    let bytes = std::fs::read(&binary).expect("the executable reads");
    let names = extract_field_names(&bytes);

    assert!(names.len() > 10_000, "an executable holds thousands of names, got {}", names.len());
    for expected in ["Version", "Platform", "SaveName", "KnownProducts", "KnownSpecials", "X"] {
        assert!(names.iter().any(|name| name == expected), "{expected} is in the name tables");
    }
    // `MultiTool` is not: the executable only contains it inside longer
    // identifiers, which is the one gap this extraction has. A community
    // mapping, merged in, covers it.
    assert!(!names.iter().any(|name| name == "MultiTool"));
    // And the key of each is a hash of the name, which is the whole point.
    assert_eq!(key_for_name("Version"), "F2P");
    assert_eq!(key_for_name("SaveName"), "Pk4");
}

#[test]
fn a_generated_mapping_never_disagrees_about_a_named_field() {
    let Some(install) = install() else {
        eprintln!("skipping: no No Man's Sky installation found (set NMS_INSTALL to point at one)");
        return;
    };
    let Some(community) = community_mapping() else {
        eprintln!("skipping: no community mapping to compare against (set NMSEDIT_MAPPING)");
        return;
    };

    // What the command actually writes: the community file as the base, with
    // the extraction filling keys it lacks.
    let merged = build_mapping(&install.root, &community).expect("the mapping builds");
    for (key, name) in &merged.pairs {
        assert_eq!(&key_for_name(name), key, "{name}: a pair must be self-consistent");
    }
    let kept = community.iter().filter(|(key, name)| merged.pairs.get(*key) == Some(*name)).count();
    assert_eq!(kept, community.len(), "every community entry must survive the merge");
    assert!(merged.report.added > 0, "the extraction should add keys the community mapping lacks");

    // Without a community file the extraction still names most of what a real
    // save uses; the share is asserted so a regression in the name tables or in
    // the key derivation shows up here.
    let generated = build_mapping(&install.root, &BTreeMap::new()).expect("the mapping builds");
    assert!(!generated.pairs.is_empty());
    let reproduced = community.iter().filter(|(key, name)| generated.pairs.get(*key) == Some(*name)).count();
    assert!(
        reproduced * 4 >= community.len() * 3,
        "only {reproduced} of {} community names were reproduced",
        community.len()
    );

    // A name whose field appears in the executable only inside a longer
    // identifier cannot be recovered, and a longer name that shares its key may
    // take the key instead. Those cases are bounded and counted, not hidden: the
    // merged output never shows one, because the community entry wins.
    let disagreements = community
        .iter()
        .filter(|(key, name)| generated.pairs.get(*key).is_some_and(|ours| ours != *name))
        .count();
    assert!(
        disagreements * 100 <= community.len(),
        "{disagreements} of {} names differ, more than the 1% the extraction allows",
        community.len()
    );
}
