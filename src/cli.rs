use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::error::Result;
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
    /// Parse a single replay and print a transport-layer summary.
    ///
    /// Does not decode packet semantics (no emulator involved); prints
    /// file header, metadata, chunk distribution, and optionally a block
    /// histogram.
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
        Command::Inspect { replay, histogram } => inspect(replay, histogram),
    }
}

fn inspect(path: PathBuf, histogram: bool) -> Result<()> {
    let bytes = std::fs::read(&path)?;
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
    println!(
        "lastGameChunkId   : {}",
        replay.metadata.last_game_chunk_id
    );
    println!(
        "lastKeyFrameId    : {}",
        replay.metadata.last_key_frame_id
    );
    println!("Winning team      : {:?}", replay.metadata.winning_team);
    println!("Players ({}):", replay.metadata.players.len());
    for p in &replay.metadata.players {
        let id_display = p
            .riot_id_game_name
            .as_deref()
            .map(|s| format!("RIOT_ID({} chars)", s.len()))
            .unwrap_or_else(|| "<no RIOT_ID>".to_string());
        println!(
            "  {:<5?} {:<8?} {:<20} {} {}",
            p.team,
            p.role,
            p.skin,
            if p.win { "W" } else { "L" },
            id_display,
        );
    }

    // Walk chunks.
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
        println!(
            "Blocks parsed     : {} (errors: {})",
            total_blocks, block_errors
        );
        println!("Distinct opcodes  : {}", opcodes.len());
        let mut top: Vec<(&u16, &usize)> = opcodes.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1));
        println!("Top 20 opcodes    :");
        for (op, count) in top.iter().take(20) {
            println!("  0x{:04x} ({:>5})  x{}", op, op, count);
        }
    }

    Ok(())
}
