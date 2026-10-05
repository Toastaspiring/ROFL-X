//! End-to-end regression test for format-2 decoding.
//!
//! For every replay in `ROFL_X_SAMPLES_DIR` whose patch has a format-2
//! archive in `ROFL_X_PATCH_DIR` (default `./patch`), decode every block
//! and assert:
//!   - every block is a clean decode (the game's deserializer returned true
//!     and consumed exactly the payload);
//!   - every self-check against statsJson passes (kills / deaths /
//!     assists, kill events, level, final items, damage to champions,
//!     wards placed);
//!   - the output keeps its top-level keys;
//!   - the file-only path (`explore`, no binary and no deserializer)
//!     reaches the same per-player vision time as the decoder does.
//!
//! Skipped when the samples directory is unset or no replay has an
//! archive, so CI without game data still passes. Replays on patches with
//! no archive (e.g. packed binaries) are skipped individually.

use std::fs;
use std::path::{Path, PathBuf};

use rofl_x::emulator::config::Config;
use rofl_x::emulator::PatchArchive;
use rofl_x::Replay;

/// The file-only path has to land where the decoder lands.
///
/// `explore` reads the visibility classes without the game's code at
/// all: it recovers what the payload bytes mean from the champions'
/// known teams, then builds the same timeline. Run on a patch we can
/// decode, it has to agree with the decoder, or the method is not sound
/// enough to use on a patch we cannot.
fn file_only_vision_agrees(replay: &Replay<'_>, decoded: &serde_json::Value, path: &Path) {
    use rofl_x::explore;

    let f = explore::framing(replay).expect("framing");
    let Some(run) = f.champion_run(replay.metadata.players.len()) else {
        panic!("{}: file-only path found no champion run", path.display());
    };
    let ids: Vec<String> = run.iter().map(|id| format!("0x{id:08x}")).collect();
    let decoded_ids: Vec<String> = decoded["players"]
        .as_array()
        .expect("players")
        .iter()
        .map(|p| p["net_id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(ids, decoded_ids, "{}: champion run differs", path.display());

    let teams = explore::roster_teams(replay);
    let solved = explore::solve_visibility(replay, &f, &run, &teams).expect("solve");
    assert_eq!(solved.len(), 2, "{}: visibility pair not solved", path.display());
    for s in &solved {
        assert_eq!(s.exceptions, 0, "{}: netid {} has exceptions", path.display(), s.netid);
    }
    let windows = explore::vision_windows(replay, &solved, &run).expect("windows");
    for (i, w) in windows.iter().enumerate() {
        let mine: f32 = w.iter().map(|(a, b)| b - a).sum();
        let theirs = decoded["vision"][i]["seen_by_enemy_s"].as_f64().unwrap_or(-1.0);
        assert!(
            (f64::from(mine) - theirs).abs() <= 1.0,
            "{}: player {i} vision {mine} file-only vs {theirs} decoded",
            path.display()
        );
    }
}

fn rofls(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .map(|it| it.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    out.retain(|p| p.extension().and_then(|s| s.to_str()) == Some("rofl"));
    out.sort();
    out
}

#[test]
fn every_block_clean_and_every_check_passes() {
    let Some(dir) = std::env::var_os("ROFL_X_SAMPLES_DIR").map(PathBuf::from) else {
        eprintln!("ROFL_X_SAMPLES_DIR not set; skipping");
        return;
    };
    let patch_dir = PathBuf::from(std::env::var("ROFL_X_PATCH_DIR").unwrap_or_else(|_| "patch".into()));
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());

    let mut tested = 0usize;
    for path in rofls(&dir) {
        let bytes = fs::read(&path).expect("read replay");
        let replay = Replay::parse(&bytes).expect("parse replay");
        let Some(archive_path) = Config::resolve_patch_file(&patch_dir, replay.header.patch()) else {
            continue;
        };
        if !PatchArchive::is_format2(&archive_path).expect("read archive") {
            continue;
        }
        let archive = PatchArchive::load(&archive_path).expect("load archive");
        let (typed, summary) =
            rofl_x::game::decode_replay(&replay, &archive, threads).expect("decode replay");
        let clean = summary.status.get("clean").copied().unwrap_or(0);
        assert_eq!(
            clean,
            summary.blocks,
            "{}: not every block decoded clean: {:?}",
            path.display(),
            summary.status
        );

        let out = rofl_x::game::assemble(&replay, &typed, &summary);
        for key in ["metadata", "players", "events", "players_state", "wards", "checks", "decode"] {
            assert!(out.get(key).is_some(), "{}: output lost key {key:?}", path.display());
        }
        let checks = out["checks"]["summary"].as_object().expect("checks.summary");
        for (field, score) in checks {
            let score = score.as_str().unwrap_or("");
            let (ok, all) = score.split_once('/').expect("ok/all");
            assert_eq!(ok, all, "{}: self-check {field} = {score}", path.display());
        }
        file_only_vision_agrees(&replay, &out, &path);
        eprintln!(
            "{}: {} blocks clean, checks {:?}",
            path.file_name().unwrap().to_string_lossy(),
            summary.blocks,
            checks
        );
        tested += 1;
    }
    if tested == 0 {
        eprintln!("no replay in {} has a format-2 archive; skipping", dir.display());
    }
}
