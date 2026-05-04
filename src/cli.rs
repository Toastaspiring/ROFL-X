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
    /// Trace every write to the output packet struct while running a
    /// decoder function on a real payload. Used to discover the struct
    /// layout of a decoder whose offsets aren't known yet.
    TraceDecoder {
        /// Replay whose blocks supply the payload bytes.
        #[arg(short, long)]
        replay: PathBuf,

        /// The packet_id (netid) to sample from the replay.
        #[arg(short, long)]
        netid: u16,

        /// Directory holding the `.patch` archive for this replay's patch.
        #[arg(long, default_value = "./patch")]
        patch_dir: PathBuf,

        /// Hypothesised decoder start RVA in the patch binary, hex
        /// (e.g. 0xe3d7b0 for ward_spawn on 15.5).
        #[arg(long)]
        rva_start: String,

        /// Hypothesised decoder end RVA in the patch binary, hex.
        #[arg(long)]
        rva_end: String,

        /// How many packets from the replay to trace (default 5).
        #[arg(long, default_value_t = 5usize)]
        samples: usize,
    },
    /// Pull raw payload bytes for one netid out of a replay and write
    /// them as fixture files for handler tests.
    ExtractFixture {
        /// Path to a `.rofl` file.
        #[arg(short, long)]
        replay: PathBuf,

        /// Packet id (netid) to pull.
        #[arg(short, long)]
        netid: u16,

        /// Symbolic name used as the file stem (lower_snake_case),
        /// e.g. `ward_spawn` or `create_hero`.
        #[arg(long)]
        name: String,

        /// How many payloads to extract.
        #[arg(long, default_value_t = 5usize)]
        count: usize,

        /// Where to drop the `.bin` files and the sidecar `.json`.
        #[arg(long, default_value = "tests/fixtures")]
        out_dir: PathBuf,
    },
    /// Scan a target patch's `text.bin` for byte-pattern matches against
    /// known decoder functions exported from a donor patch via the Ghidra
    /// script in `scripts/ghidra/export_decoders.py`. Produces a ranked
    /// list of candidate RVA ranges per donor function.
    ScanDecoder {
        /// JSON dump of donor functions from the Ghidra exporter.
        #[arg(long)]
        donors: PathBuf,

        /// Target `.patch` archive (the one whose RVAs are unknown).
        #[arg(long)]
        target: PathBuf,

        /// How many candidates per donor to report (default 5).
        #[arg(long, default_value_t = 5usize)]
        top_k: usize,

        /// Optional: also write the full report (all donors, all
        /// candidates) as JSON for downstream scripting.
        #[arg(long)]
        report_json: Option<PathBuf>,
    },
    /// Scaffold a new packet entry: integration test stub plus a row
    /// appended to `docs/PACKETS.md`. Run after `extract-fixture` so the
    /// test can find its inputs. Does not create handler source files;
    /// the production decoder gets written by hand once the RVA range is
    /// known and `trace-decoder` has produced struct offsets.
    NewHandler {
        /// Lower_snake_case packet name, e.g. `create_hero`.
        #[arg(long)]
        name: String,

        /// Patch tag (e.g. `15.5`) the netid was observed on.
        #[arg(long)]
        patch: String,

        /// Numeric netid on that patch.
        #[arg(long)]
        netid: u16,

        /// Catalog status: DOCUMENTED, PARTIAL, OBSERVED-ONLY, or UNKNOWN.
        #[arg(long, default_value = "OBSERVED-ONLY")]
        status: String,

        /// Optional decoder start RVA, hex.
        #[arg(long)]
        rva_start: Option<String>,

        /// Optional decoder end RVA, hex.
        #[arg(long)]
        rva_end: Option<String>,

        /// One-line summary of what the packet is for.
        #[arg(long, default_value = "")]
        summary: String,

        /// Repository root. Files are written relative to this directory.
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
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
        Command::TraceDecoder {
            replay,
            netid,
            patch_dir,
            rva_start,
            rva_end,
            samples,
        } => trace_decoder(replay, netid, patch_dir, &rva_start, &rva_end, samples),
        Command::ExtractFixture {
            replay,
            netid,
            name,
            count,
            out_dir,
        } => extract_fixture(replay, netid, name, count, out_dir),
        Command::ScanDecoder {
            donors,
            target,
            top_k,
            report_json,
        } => scan_decoder(donors, target, top_k, report_json),
        Command::NewHandler {
            name,
            patch,
            netid,
            status,
            rva_start,
            rva_end,
            summary,
            repo_root,
        } => new_handler(
            repo_root, name, patch, netid, status, rva_start, rva_end, summary,
        ),
    }
}

fn extract_fixture(
    replay_path: PathBuf,
    netid: u16,
    name: String,
    count: usize,
    out_dir: PathBuf,
) -> Result<()> {
    use crate::fixture;

    let bytes = std::fs::read(&replay_path)?;
    let parsed = Replay::parse(&bytes)?;
    let patch_tag = parsed.header.patch().to_string();

    let fixtures = fixture::collect(&parsed, netid, count)?;
    if fixtures.is_empty() {
        eprintln!(
            "no blocks with netid {} found in {}",
            netid,
            replay_path.display()
        );
        return Ok(());
    }

    let written = fixture::write_fixtures(&out_dir, &name, &fixtures, &replay_path, netid, &patch_tag)?;
    eprintln!(
        "wrote {} file(s) under {} for netid {} (patch {}):",
        written.len(),
        out_dir.display(),
        netid,
        patch_tag
    );
    for p in &written {
        eprintln!("  {}", p.display());
    }
    eprintln!(
        "size summary: {} payload(s), bytes per payload = [{}]",
        fixtures.len(),
        fixtures
            .iter()
            .map(|f| f.payload.len().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

fn scan_decoder(
    donors_path: PathBuf,
    target_archive: PathBuf,
    top_k: usize,
    report_json: Option<PathBuf>,
) -> Result<()> {
    use crate::scan_decoder::{load_donors, scan_all};
    use crate::RoflError;
    use std::io::Read;

    let donors = load_donors(&donors_path)?;

    // Pull text.bin and result.json from the target archive without taking
    // a full Config dependency (the target archive's RVAs are exactly what
    // we don't yet know, so most Config fields would be NEEDS_RE).
    let zipfile = std::fs::File::open(&target_archive)?;
    let mut archive = zip::ZipArchive::new(zipfile).map_err(|e| {
        RoflError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("target archive open: {e}"),
        ))
    })?;

    let mut text_bytes = Vec::new();
    archive
        .by_name("text.bin")
        .map_err(|e| {
            RoflError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("target archive missing text.bin: {e}"),
            ))
        })?
        .read_to_end(&mut text_bytes)?;

    let mut result_bytes = Vec::new();
    archive
        .by_name("result.json")
        .map_err(|e| {
            RoflError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("target archive missing result.json: {e}"),
            ))
        })?
        .read_to_end(&mut result_bytes)?;
    let target_meta: serde_json::Value = serde_json::from_slice(&result_bytes)?;
    let text_rva_str = target_meta
        .get("text")
        .and_then(|t| t.get("rva"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            RoflError::Io(std::io::Error::other(
                "target archive result.json missing text.rva",
            ))
        })?;
    let text_rva = u64::from_str_radix(text_rva_str.trim_start_matches("0x"), 16)
        .map_err(|e| RoflError::Io(std::io::Error::other(format!("text.rva parse: {e}"))))?;

    let reports = scan_all(&donors, &text_bytes, text_rva, top_k)?;

    println!(
        "scanning {} donor function(s) from patch {} against {} ({} bytes of .text)",
        donors.functions.len(),
        donors.patch,
        target_archive.display(),
        text_bytes.len()
    );
    println!();
    for report in &reports {
        println!(
            "{} ({} anchors)  donor {} .. {}",
            report.donor_name, report.anchor_count, report.donor_rva_start, report.donor_rva_end
        );
        if report.top_candidates.is_empty() {
            println!("  no candidates above zero anchors matched");
            continue;
        }
        for (i, c) in report.top_candidates.iter().enumerate() {
            println!(
                "  #{:<2} {} .. {}   {:>3}/{:<3} anchors  score={:.2}",
                i + 1,
                c.rva_start_hex,
                c.rva_end_hex,
                c.matched_anchors,
                c.total_anchors,
                c.score
            );
        }
        println!();
    }

    if let Some(path) = report_json {
        std::fs::write(&path, serde_json::to_string_pretty(&reports)?)?;
        eprintln!("wrote full report to {}", path.display());
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn new_handler(
    repo_root: PathBuf,
    name: String,
    patch: String,
    netid: u16,
    status: String,
    rva_start: Option<String>,
    rva_end: Option<String>,
    summary: String,
) -> Result<()> {
    use crate::scaffold::{self, Scaffold, Status};

    let status = Status::parse(&status)?;
    let decoder_rva = match (rva_start, rva_end) {
        (Some(s), Some(e)) => Some((s, e)),
        (None, None) => None,
        _ => {
            return Err(crate::RoflError::Io(std::io::Error::other(
                "--rva-start and --rva-end must be passed together",
            )))
        }
    };

    let s = Scaffold {
        name: name.clone(),
        patch: patch.clone(),
        netid,
        decoder_rva,
        status,
        summary,
    };
    let written = scaffold::run(&repo_root, &s)?;

    eprintln!("scaffolded {} file(s):", written.len());
    for p in &written {
        eprintln!("  {}", p.display());
    }
    eprintln!();
    eprintln!("next steps:");
    eprintln!("  1. extract fixtures (if you haven't):");
    eprintln!(
        "       rofl-x extract-fixture --replay <.rofl> --netid {} --name {} --count 5",
        netid, name
    );
    eprintln!("  2. find the decoder RVA in the donor binary (Ghidra) and run:");
    eprintln!(
        "       rofl-x trace-decoder --replay <.rofl> --netid {} --rva-start 0x... --rva-end 0x...",
        netid
    );
    eprintln!(
        "  3. add a handler at src/packet/handlers/{}.rs once you have struct offsets;",
        name
    );
    eprintln!(
        "     follow the StubEmulator pattern in src/emulator/packet.rs."
    );
    eprintln!("  4. promote the PACKETS.md entry from the current status to DOCUMENTED.");
    Ok(())
}

#[cfg(feature = "emulator")]
fn trace_decoder(
    replay: PathBuf,
    netid: u16,
    patch_dir: PathBuf,
    rva_start: &str,
    rva_end: &str,
    samples: usize,
) -> Result<()> {
    use crate::emulator::{config::Config, StubEmulator};
    use crate::replay_info::blocks_with_netid;
    use crate::RoflError;
    use std::collections::HashMap;

    let rs = u64::from_str_radix(rva_start.trim_start_matches("0x"), 16)
        .map_err(|e| RoflError::Io(std::io::Error::other(format!("rva_start: {e}"))))?;
    let re = u64::from_str_radix(rva_end.trim_start_matches("0x"), 16)
        .map_err(|e| RoflError::Io(std::io::Error::other(format!("rva_end: {e}"))))?;

    let bytes = std::fs::read(&replay)?;
    let parsed = crate::Replay::parse(&bytes)?;
    let patch_tag = parsed.header.patch();
    let patch_file = Config::resolve_patch_file(&patch_dir, patch_tag).ok_or_else(|| {
        RoflError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no patch archive for {patch_tag:?}"),
        ))
    })?;
    let config = Config::parse(&patch_file)?;

    let hits = blocks_with_netid(&parsed, netid)?;
    if hits.is_empty() {
        eprintln!("no blocks with netid {netid} in this replay");
        return Ok(());
    }
    let to_trace = hits.iter().take(samples).collect::<Vec<_>>();
    eprintln!(
        "tracing {} sample payload(s) of netid {} through RVA 0x{:x}..0x{:x}",
        to_trace.len(),
        netid,
        rs,
        re
    );

    // Per-offset statistics: how many times written, which write-counts
    // produced which values, most-common byte-size.
    let mut offset_writes: HashMap<u16, Vec<(u8, u64)>> = HashMap::new();

    for (i, (ts, payload)) in to_trace.iter().enumerate() {
        let mut emu = StubEmulator::new(config.clone());
        emu.setup()?;
        emu.setup_args(payload)?;
        let log = emu.trace_decoder_writes(rs, re)?;
        eprintln!(
            "sample {}: timestamp={:.2}s payload_len={} total_writes={}",
            i,
            ts,
            payload.len(),
            log.len()
        );
        for (off, size, value) in log {
            offset_writes.entry(off).or_default().push((size, value));
        }
    }

    let mut offsets: Vec<u16> = offset_writes.keys().copied().collect();
    offsets.sort();
    println!();
    println!(
        "struct-layout summary ({} distinct offsets across {} samples):",
        offsets.len(),
        to_trace.len()
    );
    println!(
        "  {:>6}  {:>6}  {:>5}  writes per sample (avg)",
        "offset", "size", "count"
    );
    for off in offsets {
        let writes = &offset_writes[&off];
        let n = writes.len();
        let avg_per_sample = n as f64 / to_trace.len() as f64;
        let size = writes.iter().map(|w| w.0).max().unwrap_or(0);
        println!(
            "  0x{:04x}  {:>4}B  {:>5}  {:>5.1}",
            off, size, n, avg_per_sample
        );
    }
    Ok(())
}

#[cfg(not(feature = "emulator"))]
fn trace_decoder(
    _replay: PathBuf,
    _netid: u16,
    _patch_dir: PathBuf,
    _rva_start: &str,
    _rva_end: &str,
    _samples: usize,
) -> Result<()> {
    Err(crate::RoflError::Io(std::io::Error::other(
        "trace-decoder requires the `emulator` feature",
    )))
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
        println!("All opcodes (sorted by frequency):");
        for (op, count) in top.iter() {
            println!("  0x{:04x} ({:>5})  x{}", op, op, count);
        }
    }

    Ok(())
}
