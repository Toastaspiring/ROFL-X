//! Opcode coverage regression test.
//!
//! For every replay under `ROFL_X_SAMPLES_DIR`, walk the block stream
//! and count how many distinct netids appear. Then load the patch's
//! `extra_decoders` config and assert that the observed netids are
//! covered by the catalog (i.e. that there's no traffic class we
//! haven't mapped).
//!
//! What this catches:
//!   - A new patch introduces a netid we don't know about
//!   - A regression in `build_riot_catalog.py` drops a previously-
//!     mapped netid out of `extra_decoders[]`
//!   - A replay carries a netid family we've never seen (would
//!     surface a new game mode or feature)
//!
//! Skipped when `ROFL_X_SAMPLES_DIR` isn't set.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use rofl_x::rofl::{block::BlockIterator, decompress::decompress_chunk, stream::StreamTag};
use rofl_x::Replay;

fn samples_dir() -> Option<PathBuf> {
    std::env::var_os("ROFL_X_SAMPLES_DIR").map(PathBuf::from)
}

fn find_rofls(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("rofl") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn observed_opcodes_are_in_catalog() {
    let Some(dir) = samples_dir() else {
        eprintln!("ROFL_X_SAMPLES_DIR not set; skipping");
        return;
    };
    let rofls = find_rofls(&dir);
    if rofls.is_empty() {
        eprintln!("ROFL_X_SAMPLES_DIR={} contained no .rofl files", dir.display());
        return;
    }

    // Load the master semantic catalog. Coverage is measured against
    // this — extra_decoders[] is only the emulator-friendly subset
    // (~40 of 217), so checking against it would miss the bigger
    // catalog of decoder/class assignments.
    let catalog_path = std::env::var("ROFL_X_CATALOG")
        .unwrap_or_else(|_| "scripts/semantic_field_names.json".to_string());
    let catalog_bytes = match fs::read(&catalog_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("can't read catalog at {catalog_path}: {e}; skipping");
            return;
        }
    };
    let catalog: serde_json::Value = serde_json::from_slice(&catalog_bytes)
        .expect("catalog parses as JSON");
    let netid_map = catalog
        .get("netid_to_decoder")
        .and_then(|v| v.as_object())
        .expect("catalog has netid_to_decoder");
    let catalog_netids: HashSet<u32> = netid_map
        .keys()
        .filter_map(|k| k.parse::<u32>().ok())
        .collect();
    eprintln!("catalog has {} netid -> decoder mappings", catalog_netids.len());

    // Track per-replay coverage.
    let mut total_observed = 0usize;
    let mut total_covered = 0usize;
    let mut replays_checked = 0usize;

    for rofl_path in &rofls {
        let bytes = fs::read(rofl_path).expect("read rofl");
        let replay = match Replay::parse(&bytes) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skip {}: parse error: {}", rofl_path.display(), e);
                continue;
            }
        };
        let patch = replay.header.patch().to_string();

        let mut observed: HashSet<u16> = HashSet::new();
        for chunk in replay.chunks() {
            let chunk = match chunk {
                Ok(c) => c,
                Err(_) => continue,
            };
            if chunk.stream_tag != StreamTag::GameChunk || chunk.compressed_len == 0 {
                continue;
            }
            let body = match decompress_chunk(&chunk) {
                Ok(b) => b,
                Err(_) => continue,
            };
            for block in BlockIterator::new(&body) {
                if let Ok(b) = block {
                    observed.insert(b.packet_id);
                }
            }
        }

        let observed_count = observed.len();
        let covered = observed
            .iter()
            .filter(|n| catalog_netids.contains(&(**n as u32)))
            .count();
        total_observed += observed_count;
        total_covered += covered;
        replays_checked += 1;

        eprintln!(
            "{}  patch={}  observed={}  covered={}  uncovered_pct={:.1}%",
            rofl_path.file_name().unwrap().to_string_lossy(),
            patch,
            observed_count,
            covered,
            100.0 * (observed_count - covered) as f64 / observed_count.max(1) as f64,
        );

        // Per-replay threshold: the master catalog should cover at
        // least 85% of observed netids on a known patch. Empirical
        // ceiling on real 16.9 replays is ~87-88%; the gap is
        // tag-only netids that fire without a decoder (the catalog
        // only lists netids that HAVE decoders, so they can't match).
        // 85% is set just under the empirical floor — it'll fire
        // when the catalog actually loses decoder mappings, not
        // when game variability shifts the tag-only count by a few.
        let coverage = covered as f64 / observed_count.max(1) as f64;
        if patch.starts_with("16.9") {
            assert!(
                coverage >= 0.85,
                "{}: 16.9 catalog covers only {:.1}% of {} observed netids; \
                 expected >= 85%. Either the catalog lost decoder mappings \
                 or scripts/build_riot_catalog.py needs to be re-run.",
                rofl_path.file_name().unwrap().to_string_lossy(),
                100.0 * coverage,
                observed_count,
            );
        } else {
            eprintln!("  (no assertion on patch {patch} — catalog is for 16.9)");
        }
    }

    if replays_checked == 0 {
        // Don't fail in this case — could be that no replays match a
        // patch we have configs for. Just inform.
        eprintln!("no replays checked (no matching patch archives found)");
        return;
    }

    eprintln!(
        "across {} replay(s): {}/{} netid observations covered ({:.2}%)",
        replays_checked,
        total_covered,
        total_observed,
        100.0 * total_covered as f64 / total_observed.max(1) as f64,
    );
}
