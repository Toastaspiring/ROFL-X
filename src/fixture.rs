//! Pull raw packet payloads from a `.rofl` file by netid, write them as
//! fixture files for handler tests. Companion to the per-decoder workflow
//! in `docs/RE_PATCH.md`.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::error::Result;
use crate::replay_info::blocks_with_netid;
use crate::Replay;

/// One extracted payload, with the timestamp from its enclosing block.
///
/// `entity_id` is the block's `param` field, which for many netids carries
/// the receiving entity id (verified empirically on heartbeat/replication
/// netids). Handler tests can use it to anchor fixture-driven correlations.
pub struct Fixture {
    pub timestamp: f32,
    pub entity_id: u32,
    pub payload: Vec<u8>,
}

/// Pull the first `count` blocks with `netid` from the replay.
///
/// The full set is collected first, then truncated, so the chosen samples
/// are the *earliest* in the game. That biases towards tutorial / lane-phase
/// payloads, which tend to exercise the simpler code paths in a decoder.
pub fn collect(replay: &Replay<'_>, netid: u16, count: usize) -> Result<Vec<Fixture>> {
    let hits = blocks_with_netid(replay, netid)?;
    Ok(hits
        .into_iter()
        .take(count)
        .map(|(timestamp, payload, entity_id)| Fixture {
            timestamp,
            entity_id,
            payload,
        })
        .collect())
}

/// Write each fixture as `<out_dir>/<name>_<i>.bin` and a sidecar
/// `<out_dir>/<name>.json` describing the source replay, netid, and per-
/// fixture timestamps.
pub fn write_fixtures(
    out_dir: &Path,
    name: &str,
    fixtures: &[Fixture],
    source_replay: &Path,
    netid: u16,
    patch_tag: &str,
) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(out_dir)?;

    let mut written = Vec::with_capacity(fixtures.len());
    for (i, f) in fixtures.iter().enumerate() {
        let path = out_dir.join(format!("{name}_{i}.bin"));
        std::fs::write(&path, &f.payload)?;
        written.push(path);
    }

    let sidecar = json!({
        "name": name,
        "netid": netid,
        "patch": patch_tag,
        "source_replay": source_replay.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<unknown>"),
        "fixtures": fixtures.iter().enumerate().map(|(i, f)| json!({
            "file": format!("{name}_{i}.bin"),
            "timestamp_seconds": f.timestamp,
            "entity_id": f.entity_id,
            "entity_id_hex": format!("0x{:x}", f.entity_id),
            "size_bytes": f.payload.len(),
        })).collect::<Vec<_>>(),
    });

    let sidecar_path = out_dir.join(format!("{name}.json"));
    std::fs::write(&sidecar_path, serde_json::to_string_pretty(&sidecar)?)?;
    written.push(sidecar_path);

    Ok(written)
}
