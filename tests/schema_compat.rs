//! JSON output schema regression test.
//!
//! Pins the contract between rofl-x and downstream consumers
//! (build_event_timeline.py, readable_replay.py, validate_typed_events.py,
//! and any external tools). When this test fails, the output JSON
//! shape changed; either roll back the change or update the consumers
//! and bump the schema expectations here.
//!
//! Skipped when `ROFL_X_SAMPLES_DIR` isn't set.

use std::fs;
use std::path::PathBuf;

use rofl_x::emulator::config::Config;
use rofl_x::replay_info::parse_and_decode;
use rofl_x::Replay;
use serde_json::Value;

fn samples_dir() -> Option<PathBuf> {
    std::env::var_os("ROFL_X_SAMPLES_DIR").map(PathBuf::from)
}

fn rofls(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .ok()
        .map(|it| it.flatten().map(|e| e.path()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("rofl"))
        .collect();
    entries.sort();
    entries
}

#[test]
fn output_schema_is_stable() {
    let Some(dir) = samples_dir() else {
        eprintln!("ROFL_X_SAMPLES_DIR not set; skipping");
        return;
    };
    let candidates = rofls(&dir);
    if candidates.is_empty() {
        eprintln!("no .rofl files in {}", dir.display());
        return;
    }

    let patch_dir = std::env::var("ROFL_X_PATCH_DIR").unwrap_or_else(|_| "patch".to_string());

    // Walk replays until we find one whose patch we have a config
    // for. This makes the test robust to a samples directory that
    // mixes patches.
    let mut chosen: Option<(PathBuf, Vec<u8>, PathBuf)> = None;
    for rofl in candidates {
        let bytes = fs::read(&rofl).expect("read rofl");
        let replay = Replay::parse(&bytes).expect("parse rofl");
        if let Some(p) = Config::resolve_patch_file(
            std::path::Path::new(&patch_dir),
            replay.header.patch(),
        ) {
            chosen = Some((rofl, bytes, p));
            break;
        }
    }
    let Some((rofl, bytes, patch_file)) = chosen else {
        eprintln!("no replay matched a patch archive in {patch_dir}");
        return;
    };
    let replay = Replay::parse(&bytes).expect("parse rofl (re-parse)");
    eprintln!("schema test running on {} (patch {})",
              rofl.file_name().unwrap().to_string_lossy(),
              replay.header.patch());
    let config = Config::parse(&patch_file).expect("parse config");

    // Cap samples per class so this test runs in a few seconds rather
    // than minutes. The schema is independent of sample count.
    std::env::set_var("ROFL_X_MAX_SAMPLES", "5");
    let json = parse_and_decode(&replay, &config).expect("decode");
    std::env::remove_var("ROFL_X_MAX_SAMPLES");

    // Top-level: required keys must all be present.
    let required_top = [
        "metadata",
        "raw_positions",
        "wards",
    ];
    for key in &required_top {
        assert!(
            json.get(*key).is_some(),
            "schema regression: top-level field {key:?} missing"
        );
    }

    // metadata: required nested keys.
    let metadata = json.get("metadata").and_then(Value::as_object).expect("metadata is object");
    for k in &["players", "game_len", "winning_team"] {
        assert!(
            metadata.contains_key(*k),
            "schema regression: metadata.{k:?} missing"
        );
    }

    // wards: array; each entry has a fixed shape.
    let wards = json.get("wards").and_then(Value::as_array).expect("wards is array");
    if let Some(w) = wards.first() {
        let w = w.as_object().expect("ward is object");
        for k in &["timestamp", "name", "id", "owner_id", "x", "y"] {
            assert!(w.contains_key(*k), "schema regression: ward.{k:?} missing");
        }
    }

    // raw_positions: array; each entry is {timestamp, x, y}.
    let rp = json.get("raw_positions").and_then(Value::as_array).expect("raw_positions is array");
    if let Some(p) = rp.first() {
        let p = p.as_object().expect("raw_position is object");
        for k in &["timestamp", "x", "y"] {
            assert!(p.contains_key(*k), "schema regression: raw_position.{k:?} missing");
        }
    }

    // extra_decoders: optional but, when present, every sample has a
    // fixed envelope.
    if let Some(ed) = json.get("extra_decoders") {
        let ed = ed.as_object().expect("extra_decoders is object");
        for (label, samples) in ed {
            let arr = samples.as_array()
                .unwrap_or_else(|| panic!("extra_decoders[{label}] is not an array"));
            for (i, s) in arr.iter().enumerate() {
                let s = s.as_object()
                    .unwrap_or_else(|| panic!("extra_decoders[{label}][{i}] is not an object"));
                for k in &[
                    "timestamp", "entity_id", "entity_id_hex",
                    "writes", "struct_hex", "decoded_fields",
                    "heap_writes", "final_regs",
                ] {
                    assert!(
                        s.contains_key(*k),
                        "schema regression: extra_decoders[{label}][{i}].{k:?} missing"
                    );
                }
                let regs = s["final_regs"].as_object().expect("final_regs is object");
                for k in &["rax", "rdx", "rcx"] {
                    assert!(
                        regs.contains_key(*k),
                        "schema regression: final_regs.{k:?} missing"
                    );
                }
            }
        }
    }

    // typed_events: optional but, when present, every entry has at
    // least kind + timestamp + entity_id. Specific kinds are NOT pinned
    // here — adding a new typed handler shouldn't break this test.
    if let Some(te) = json.get("typed_events") {
        let arr = te.as_array().expect("typed_events is array");
        for (i, e) in arr.iter().enumerate() {
            let obj = e.as_object()
                .unwrap_or_else(|| panic!("typed_events[{i}] is not an object"));
            for k in &["kind", "timestamp", "entity_id"] {
                assert!(
                    obj.contains_key(*k),
                    "schema regression: typed_events[{i}].{k:?} missing"
                );
            }
        }
    }

    eprintln!(
        "schema OK: extra_decoders has {} classes, typed_events has {} samples",
        json.get("extra_decoders").and_then(|v| v.as_object()).map(|o| o.len()).unwrap_or(0),
        json.get("typed_events").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
    );
}
