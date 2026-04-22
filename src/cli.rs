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
    /// Parse one replay and emit Mowokuma-compatible JSON. Requires the
    /// `emulator` feature and a per-patch `.patch` archive.
    File {
        /// Path to a `.rofl` file.
        #[arg(short, long)]
        replay: PathBuf,

        /// Output JSON path.
        #[arg(short, long)]
        output: PathBuf,

        /// Directory holding per-patch emulator configs. Files must be
        /// named `<patch>.patch` (e.g. `15-5.patch` or Mowokuma's
        /// `5-5.patch`). Defaults to `./patch`.
        #[arg(long, default_value = "./patch")]
        patch_dir: PathBuf,
    },
    /// Parse a single replay and print a transport-layer summary.
    ///
    /// Does not decode packet semantics; prints file header, metadata,
    /// chunk distribution, and optionally a block histogram.
    Inspect {
        /// Path to a `.rofl` file.
        #[arg(short, long)]
        replay: PathBuf,

        /// Also decompress chunks and print an opcode histogram.
        #[arg(long)]
        histogram: bool,
    },
    /// Extract `.text`/`.data`/`.rdata` from a League binary and write
    /// a `.patch` archive skeleton. RVAs inside `result.json` are left
    /// as `NEEDS_RE` placeholders; see `docs/RE_PATCH.md` for how to
    /// fill them in.
    ExtractPatch {
        /// Path to `League of Legends.exe` for the target patch.
        #[arg(short, long)]
        binary: PathBuf,

        /// Output `.patch` archive path.
        #[arg(short, long)]
        output: PathBuf,
    },
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::File {
            replay,
            output,
            patch_dir,
        } => file(replay, output, patch_dir),
        Command::Inspect { replay, histogram } => inspect(replay, histogram),
        Command::ExtractPatch { binary, output } => extract_patch(binary, output),
    }
}

fn extract_patch(binary: PathBuf, output: PathBuf) -> Result<()> {
    use crate::extract_patch::extract_skeleton;
    let pe = extract_skeleton(&binary, &output)?;
    eprintln!("parsed PE: machine=0x{:04x}, sections:", pe.machine);
    for s in &pe.sections {
        eprintln!(
            "  {:10} rva=0x{:08x} virt_size={:>10} raw_size={:>10}",
            s.name, s.rva, s.virt_size, s.raw_size
        );
    }
    eprintln!("wrote {}", output.display());
    eprintln!("Next step: fill in the RVAs in result.json by reverse-");
    eprintln!("engineering the binary. See docs/RE_PATCH.md.");
    Ok(())
}

#[cfg(feature = "emulator")]
fn file(replay: PathBuf, output: PathBuf, patch_dir: PathBuf) -> Result<()> {
    use crate::emulator::config::Config;
    use crate::replay_info::parse_and_decode;
    use crate::RoflError;

    let bytes = std::fs::read(&replay)?;
    let parsed = Replay::parse(&bytes)?;

    let patch_tag = parsed.header.patch();
    let patch_file = Config::resolve_patch_file(&patch_dir, patch_tag).ok_or_else(|| {
        RoflError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "no patch archive for {patch_tag:?} under {}",
                patch_dir.display()
            ),
        ))
    })?;
    eprintln!(
        "using patch config: {} (for patch {})",
        patch_file.display(),
        patch_tag
    );
    let config = Config::parse(&patch_file)?;

    let game = parse_and_decode(&parsed, &config)?;
    std::fs::write(&output, game.to_string().as_bytes())?;
    eprintln!("wrote {}", output.display());
    Ok(())
}

#[cfg(not(feature = "emulator"))]
fn file(_replay: PathBuf, _output: PathBuf, _patch_dir: PathBuf) -> Result<()> {
    Err(crate::RoflError::Io(std::io::Error::other(
        "file subcommand requires the `emulator` feature at build time",
    )))
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
