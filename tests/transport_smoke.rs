//! Transport-layer smoke test against real `.rofl` files.
//!
//! Skipped unless `ROFL_X_SAMPLES_DIR` points at a directory of replays.
//! For each file, we:
//!   - parse the whole-file header, metadata, signature
//!   - walk every chunk
//!   - for each game-chunk stream chunk, decompress and walk every block
//! and assert zero parse errors at every stage.
//!
//! When running locally, set e.g.
//!   ROFL_X_SAMPLES_DIR="C:\Users\louis\Documents\League of Legends\replays"

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
fn walks_every_replay_cleanly() {
    let Some(dir) = samples_dir() else {
        eprintln!("ROFL_X_SAMPLES_DIR not set; skipping");
        return;
    };
    let rofls = find_rofls(&dir);
    assert!(
        !rofls.is_empty(),
        "ROFL_X_SAMPLES_DIR={} contained no .rofl files",
        dir.display()
    );

    let mut total_chunks = 0usize;
    let mut total_blocks = 0usize;
    let mut replays_seen = 0usize;

    for path in &rofls {
        let bytes = fs::read(path).expect("read replay");
        let replay =
            Replay::parse(&bytes).unwrap_or_else(|e| panic!("parse {}: {}", path.display(), e));

        assert!(
            replay.header.version.starts_with(char::is_numeric),
            "version {:?} doesn't start with a digit for {}",
            replay.header.version,
            path.display()
        );
        // Player count varies by mode: 10 for Summoner's Rift, 8 for Arena,
        // 2 for tutorials, 1 for practice tool. Just assert something sane.
        assert!(
            (1..=16).contains(&replay.metadata.players.len()),
            "{} has {} players, outside 1..=16",
            path.display(),
            replay.metadata.players.len(),
        );

        for c in replay.chunks() {
            let c = c.unwrap_or_else(|e| panic!("chunk walk {}: {}", path.display(), e));
            total_chunks += 1;
            if c.stream_tag == StreamTag::GameChunk {
                let body = decompress_chunk(&c).unwrap_or_else(|e| {
                    panic!("decompress {} chunk {}: {}", path.display(), c.chunk_id, e)
                });
                for b in BlockIterator::new(&body) {
                    let _b = b.unwrap_or_else(|e| {
                        panic!(
                            "block parse {} chunk {}: {}",
                            path.display(),
                            c.chunk_id,
                            e
                        )
                    });
                    total_blocks += 1;
                }
            }
        }
        replays_seen += 1;
    }

    eprintln!(
        "transport smoke: {} replays, {} chunks, {} blocks, 0 errors",
        replays_seen, total_chunks, total_blocks
    );
}
