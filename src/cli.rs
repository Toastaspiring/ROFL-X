use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{Result, RoflError};
use crate::rofl::{block::BlockIterator, decompress::decompress_chunk, stream::StreamTag, Replay};

#[derive(Parser, Debug)]
#[command(
    name = "rofl-x",
    about = "Documented parser for League of Legends replay files",
    version
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Decode one replay into JSON. With a format-2 archive (built by
    /// `scripts/build_patch.py`) every packet goes through the game's own
    /// deserializers and the output carries players, an event timeline,
    /// per-second positions, wards and a self-check against the replay's
    /// end-of-game stats. With one of Mowokuma's format-1 archives (15.x)
    /// the output is her JSON (positions and wards).
    File {
        /// Path to a `.rofl` file.
        #[arg(short, long)]
        replay: PathBuf,

        /// Output JSON path.
        #[arg(short, long)]
        output: PathBuf,

        /// Directory holding per-patch archives, named `<major>-<minor>.patch`
        /// (Mowokuma's `5-5.patch` style is accepted for 15.x).
        #[arg(long, default_value = "./patch")]
        patch_dir: PathBuf,

        /// Worker threads (one emulator each). Defaults to all cores.
        #[arg(long)]
        threads: Option<usize>,
    },
    /// Run every block of a replay through the game's own deserializers
    /// (format-2 archive) and report per-netid decode status. With
    /// `--jsonl`, also write one line per block: timestamp, netid, Riot
    /// class name, status and the plaintext object words.
    Decode {
        /// Path to a `.rofl` file.
        #[arg(short, long)]
        replay: PathBuf,

        /// Directory holding `<major>-<minor>.patch` archives.
        #[arg(long, default_value = "./patch")]
        patch_dir: PathBuf,

        /// Only decode this netid.
        #[arg(long)]
        netid: Option<u16>,

        /// Stop after this many blocks (0 = all).
        #[arg(long, default_value_t = 0usize)]
        limit: usize,

        /// Write decoded blocks as JSON lines here.
        #[arg(long)]
        jsonl: Option<PathBuf>,

        /// Report blocks whose decoded plaintext (object and everything it
        /// allocated) contains this ASCII text. Research aid for finding
        /// which class carries a given name.
        #[arg(long)]
        find: Option<String>,
    },
    /// Print a replay's transport-layer summary and the end-of-game
    /// scoreboard Riot stores in its metadata. Needs no patch archive, so
    /// it works on every patch.
    Inspect {
        /// Path to a `.rofl` file.
        #[arg(short, long)]
        replay: PathBuf,

        /// Also decompress chunks and print an opcode histogram.
        #[arg(long)]
        histogram: bool,
    },
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::File {
            replay,
            output,
            patch_dir,
            threads,
        } => file(&replay, &output, &patch_dir, threads),
        Command::Decode {
            replay,
            patch_dir,
            netid,
            limit,
            jsonl,
            find,
        } => decode(&replay, &patch_dir, netid, limit, jsonl, find),
        Command::Inspect { replay, histogram } => inspect(&replay, histogram),
    }
}

fn find_archive(patch_dir: &Path, replay: &Replay<'_>) -> Result<PathBuf> {
    let patch = replay.header.patch();
    crate::emulator::config::Config::resolve_patch_file(patch_dir, patch).ok_or_else(|| {
        RoflError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "no patch archive for {patch:?} under {} (see docs/COMPATIBILITY.md)",
                patch_dir.display()
            ),
        ))
    })
}

#[cfg(feature = "emulator")]
fn file(replay_path: &Path, output: &Path, patch_dir: &Path, threads: Option<usize>) -> Result<()> {
    use crate::emulator::{config::Config, PatchArchive};

    let bytes = std::fs::read(replay_path)?;
    let replay = Replay::parse(&bytes)?;
    let archive_path = find_archive(patch_dir, &replay)?;

    let json = if PatchArchive::is_format2(&archive_path)? {
        let archive = PatchArchive::load(&archive_path)?;
        let threads = threads.unwrap_or_else(|| {
            std::thread::available_parallelism().map_or(4, |n| n.get())
        });
        eprintln!(
            "using {} ({}), {threads} threads",
            archive_path.display(),
            archive.file_version.as_deref().unwrap_or("unknown build")
        );
        let started = std::time::Instant::now();
        let (typed, summary) = crate::game::decode_replay(&replay, &archive, threads)?;
        let clean = summary.status.get("clean").copied().unwrap_or(0);
        eprintln!(
            "decoded {} blocks in {:.1}s: {clean} clean ({:.3}%), {} typed",
            summary.blocks,
            started.elapsed().as_secs_f64(),
            clean as f64 * 100.0 / summary.blocks.max(1) as f64,
            summary.typed_blocks,
        );
        let json = crate::game::assemble(&replay, &typed, &summary);
        if let Some(checks) = json["checks"]["summary"].as_object() {
            let line: Vec<String> = checks
                .iter()
                .map(|(k, v)| format!("{k} {}", v.as_str().unwrap_or("?")))
                .collect();
            eprintln!("self-check against statsJson: {}", line.join(", "));
        }
        json
    } else {
        eprintln!("using {} (format 1, Mowokuma)", archive_path.display());
        let config = Config::parse(&archive_path)?;
        crate::replay_info::parse_and_decode(&replay, &config)?
    };
    std::fs::write(output, json.to_string().as_bytes())?;
    eprintln!("wrote {}", output.display());
    Ok(())
}

#[cfg(not(feature = "emulator"))]
fn file(_replay: &Path, _output: &Path, _patch_dir: &Path, _threads: Option<usize>) -> Result<()> {
    Err(RoflError::Io(std::io::Error::other(
        "file subcommand requires the `emulator` feature at build time",
    )))
}

#[cfg(feature = "emulator")]
fn decode(
    replay_path: &Path,
    patch_dir: &Path,
    only: Option<u16>,
    limit: usize,
    jsonl: Option<PathBuf>,
    find: Option<String>,
) -> Result<()> {
    use crate::emulator::{DecodeStatus, PacketDecoder, PatchArchive};
    use std::collections::HashMap;
    use std::io::Write;

    let bytes = std::fs::read(replay_path)?;
    let replay = Replay::parse(&bytes)?;
    let archive_path = find_archive(patch_dir, &replay)?;
    let archive = PatchArchive::load(&archive_path)?;
    let mut decoder = PacketDecoder::new(&archive)?;
    eprintln!(
        "using {} ({})",
        archive_path.display(),
        archive.file_version.as_deref().unwrap_or("unknown build")
    );

    let mut out = match &jsonl {
        Some(p) => Some(std::io::BufWriter::new(std::fs::File::create(p)?)),
        None => None,
    };
    let mut per_netid: BTreeMap<u16, HashMap<DecodeStatus, usize>> = BTreeMap::new();
    let mut found: BTreeMap<u16, usize> = BTreeMap::new();
    let mut total = 0usize;
    let started = std::time::Instant::now();
    'chunks: for chunk in replay.chunks() {
        let chunk = chunk?;
        if chunk.stream_tag != StreamTag::GameChunk {
            continue;
        }
        let body = decompress_chunk(&chunk)?;
        for block in BlockIterator::new(&body) {
            let block = block?;
            if only.is_some_and(|n| n != block.packet_id) {
                continue;
            }
            let pkt = decoder.decode(block.packet_id, block.param, block.payload)?;
            *per_netid
                .entry(block.packet_id)
                .or_default()
                .entry(pkt.status)
                .or_default() += 1;
            if let Some(needle) = find.as_deref() {
                if let Some(at) = pkt.memory.find(needle.as_bytes()) {
                    let n = found.entry(block.packet_id).or_default();
                    *n += 1;
                    if *n <= 3 {
                        println!(
                            "found {needle:?}: t={:.2} netid={} class={} param=0x{:08x} at object{:+#x}",
                            block.timestamp,
                            block.packet_id,
                            archive.class_of(block.packet_id).unwrap_or("-"),
                            block.param,
                            at as i64 - pkt.object as i64,
                        );
                    }
                }
            }
            if let Some(w) = out.as_mut() {
                let words: serde_json::Map<String, serde_json::Value> = pkt
                    .written_words(0x200)
                    .into_iter()
                    .map(|(off, v)| (format!("0x{off:03x}"), format!("0x{v:08x}").into()))
                    .collect();
                let line = serde_json::json!({
                    "t": block.timestamp,
                    "netid": block.packet_id,
                    "class": archive.class_of(block.packet_id),
                    "param": format!("0x{:08x}", block.param),
                    "status": pkt.status.label(),
                    "words": words,
                });
                writeln!(w, "{line}")?;
            }
            total += 1;
            if limit > 0 && total >= limit {
                break 'chunks;
            }
        }
    }
    let secs = started.elapsed().as_secs_f64();
    for (netid, n) in &found {
        println!("found in netid {netid} ({}) x{n}", archive.class_of(*netid).unwrap_or("-"));
    }

    let mut clean = 0usize;
    println!("{:>6} {:>8} {:>7}  {:<44} other", "netid", "blocks", "clean%", "class");
    let mut rows: Vec<_> = per_netid.iter().collect();
    rows.sort_by_key(|(_, s)| std::cmp::Reverse(s.values().sum::<usize>()));
    for (netid, statuses) in rows {
        let n: usize = statuses.values().sum();
        let c = statuses.get(&DecodeStatus::Clean).copied().unwrap_or(0);
        clean += c;
        let other: Vec<String> = statuses
            .iter()
            .filter(|(s, _)| **s != DecodeStatus::Clean)
            .map(|(s, k)| format!("{}={k}", s.label()))
            .collect();
        println!(
            "{:>6} {:>8} {:>6.1}%  {:<44} {}",
            netid,
            n,
            c as f64 * 100.0 / n as f64,
            archive.class_of(*netid).unwrap_or("-"),
            other.join(" ")
        );
    }
    eprintln!(
        "{total} blocks in {secs:.1}s ({:.0}/s), {clean} clean ({:.3}%)",
        total as f64 / secs,
        clean as f64 * 100.0 / total.max(1) as f64
    );
    Ok(())
}

#[cfg(not(feature = "emulator"))]
fn decode(
    _replay: &Path,
    _patch_dir: &Path,
    _only: Option<u16>,
    _limit: usize,
    _jsonl: Option<PathBuf>,
    _find: Option<String>,
) -> Result<()> {
    Err(RoflError::Io(std::io::Error::other(
        "decode requires the `emulator` feature",
    )))
}

fn inspect(path: &Path, histogram: bool) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let replay = Replay::parse(&bytes)?;

    println!("File              : {}", path.display());
    println!("Size              : {} bytes", bytes.len());
    println!("Format version    : {}", replay.header.format_version);
    println!("Patch (full)      : {}", replay.header.version);
    println!("Patch (short)     : {}", replay.header.patch());
    println!(
        "Game length       : {} ms ({:.1} min)",
        replay.metadata.game_length_ms,
        replay.metadata.game_length_ms as f64 / 60_000.0
    );
    println!("lastGameChunkId   : {}", replay.metadata.last_game_chunk_id);
    println!("lastKeyFrameId    : {}", replay.metadata.last_key_frame_id);
    println!("Winning team      : {:?}", replay.metadata.winning_team);
    print_scoreboard(&replay);

    let mut chunk_count_by_stream: BTreeMap<u8, usize> = BTreeMap::new();
    let mut chunk_bytes_by_stream: BTreeMap<u8, usize> = BTreeMap::new();
    let mut total_chunks = 0usize;
    let mut decompress_failures = 0usize;
    let mut opcodes: BTreeMap<u16, usize> = BTreeMap::new();
    let mut total_blocks = 0usize;
    let mut block_errors = 0usize;

    for c in replay.chunks() {
        let c = c?;
        total_chunks += 1;
        let tag_byte = match c.stream_tag {
            StreamTag::GameChunk => 0x01,
            StreamTag::Keyframe => 0x02,
            StreamTag::StartKeyframe => 0x03,
            StreamTag::StartSentinel => 0x04,
            StreamTag::Unknown(b) => b,
        };
        *chunk_count_by_stream.entry(tag_byte).or_insert(0) += 1;
        *chunk_bytes_by_stream.entry(tag_byte).or_insert(0) += c.body.len();

        if histogram && c.stream_tag == StreamTag::GameChunk {
            match decompress_chunk(&c) {
                Ok(body) => {
                    for b in BlockIterator::new(&body) {
                        match b {
                            Ok(block) => {
                                total_blocks += 1;
                                *opcodes.entry(block.packet_id).or_insert(0) += 1;
                            }
                            Err(_) => {
                                block_errors += 1;
                                break;
                            }
                        }
                    }
                }
                Err(_) => decompress_failures += 1,
            }
        }
    }

    println!("Chunks            : {}", total_chunks);
    for (tag, count) in &chunk_count_by_stream {
        let name = match tag {
            0x01 => "GameChunk",
            0x02 => "Keyframe",
            0x03 => "StartKeyframe",
            0x04 => "StartSentinel",
            _ => "Unknown",
        };
        let body_bytes = chunk_bytes_by_stream.get(tag).copied().unwrap_or(0);
        println!(
            "  stream 0x{:02x} {:<14} {:>4} chunks, {:>10} body bytes",
            tag, name, count, body_bytes
        );
    }

    if histogram {
        println!("Decompression     : {} failures", decompress_failures);
        println!("Blocks parsed     : {} (errors: {})", total_blocks, block_errors);
        println!("Distinct opcodes  : {}", opcodes.len());
        let mut all: Vec<(&u16, &usize)> = opcodes.iter().collect();
        all.sort_by(|a, b| b.1.cmp(a.1));
        println!("All opcodes (sorted by frequency):");
        for (op, count) in all {
            println!("  0x{:04x} ({:>5})  x{}", op, op, count);
        }
    }
    Ok(())
}

/// End-of-game numbers from statsJson. Riot IDs are summarised by length
/// only, never printed.
fn print_scoreboard(replay: &Replay<'_>) {
    let stats: Vec<serde_json::Value> =
        serde_json::from_str(&replay.metadata.stats_json_raw).unwrap_or_default();
    let s = |p: &serde_json::Value, k: &str| -> String {
        p.get(k).and_then(|v| v.as_str()).unwrap_or("-").to_string()
    };
    let n = |p: &serde_json::Value, k: &str| -> u64 {
        p.get(k)
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    println!("Players ({}):", replay.metadata.players.len());
    println!(
        "  {:<5} {:<8} {:<14} {}  {:>9} {:>3} {:>4} {:>6} {:>7}  riot id",
        "team", "role", "champion", "W", "K/D/A", "lvl", "cs", "gold", "dmg"
    );
    for (p, raw) in replay.metadata.players.iter().zip(stats.iter()) {
        let id = p
            .riot_id_game_name
            .as_deref()
            .map(|s| format!("{} chars", s.len()))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "  {:<5} {:<8} {:<14} {}  {:>9} {:>3} {:>4} {:>6} {:>7}  {}",
            format!("{:?}", p.team),
            format!("{:?}", p.role),
            p.skin,
            if p.win { "W" } else { "L" },
            format!(
                "{}/{}/{}",
                s(raw, "CHAMPIONS_KILLED"),
                s(raw, "NUM_DEATHS"),
                s(raw, "ASSISTS")
            ),
            s(raw, "LEVEL"),
            n(raw, "MINIONS_KILLED") + n(raw, "NEUTRAL_MINIONS_KILLED"),
            s(raw, "GOLD_EARNED"),
            s(raw, "TOTAL_DAMAGE_DEALT_TO_CHAMPIONS"),
            id,
        );
    }
}
