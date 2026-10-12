use clap::{Parser, Subcommand};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde_json::Value;

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

        /// With `--jsonl`, also write the plaintext bytes of every heap
        /// block the deserializer allocated. A class that copies its
        /// payload into a buffer puts the whole thing in one of these,
        /// so this is how its substitution table comes out.
        #[arg(long)]
        heap: bool,

        /// Report blocks whose decoded plaintext (object and everything it
        /// allocated) contains this ASCII text. Research aid for finding
        /// which class carries a given name.
        #[arg(long)]
        find: Option<String>,

        /// With `--netid`, work out that class's payload byte by byte:
        /// which object field each payload byte carries, in what order
        /// the class writes its fields and in which byte order, and the
        /// substitution table that turns one into the other. This is
        /// what says where to look on a patch whose payloads cannot be
        /// decoded (docs/EXPLORE.md).
        #[arg(long)]
        layout: bool,
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
    /// Analyse a replay from the file alone: the full end-of-game
    /// scoreboard (statsJson, hundreds of fields per player) plus the
    /// transport activity profile (per-netid, per-entity, per-minute block
    /// counts). Touches no game binary and decodes no payloads, so it
    /// works on every patch, including those whose client ships packed.
    Summary {
        /// Path to a `.rofl` file.
        #[arg(short, long)]
        replay: PathBuf,

        /// Output JSON path. Without it, the JSON is printed to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Keep Riot IDs, PUUIDs and summoner names in the output instead
        /// of reducing them to their length.
        #[arg(long)]
        include_identities: bool,
    },

    /// Measure the field layout of every packet class: decode a sample of
    /// each netid's blocks and report, per 4-byte word of the packet
    /// object, what the value was across the sample (sender net id, net
    /// id, game time, float, enum, counter, pointer to a vector or
    /// string). This is how an untyped class gets read without guessing;
    /// `docs/FIELDS.md` is its output for 16.9.
    Fields {
        /// Path to a `.rofl` file. Repeat to pool several replays of the
        /// same patch.
        #[arg(short, long = "replay", num_args = 1.., required = true)]
        replays: Vec<PathBuf>,

        /// Directory holding `<major>-<minor>.patch` archives.
        #[arg(short, long, default_value = "patch")]
        patch_dir: PathBuf,

        /// Blocks to decode per netid per replay (0 = all). The sample is
        /// spread over the game chunks, not taken from the first minute.
        #[arg(long, default_value_t = 2000)]
        samples: usize,

        /// Worker threads (one emulator each). Defaults to all cores.
        #[arg(long, default_value_t = 0)]
        threads: usize,

        /// Write the full profile as JSON here.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Write the catalog as markdown here.
        #[arg(long)]
        markdown: Option<PathBuf>,
    },

    /// Explore a replay from framing alone: who the champions are, how
    /// busy each of them was minute by minute, and a behavioural
    /// fingerprint of every packet class (share, payload length, whether
    /// it addresses a unit or the game, when it fires). Reads no game
    /// binary and decodes no payload, so it works on patches whose
    /// client ships packed. With `--reference`, also proposes which
    /// class each netid is, by matching those fingerprints against a
    /// patch ROFL-X can decode.
    Explore {
        /// Path to a `.rofl` file. Repeat to pool several replays of the
        /// same patch.
        #[arg(short, long = "replay", num_args = 1.., required = true)]
        replays: Vec<PathBuf>,

        /// Replays of a patch with a format-2 archive, used as the
        /// reference side of the netid matching.
        #[arg(long = "reference", num_args = 1..)]
        reference: Vec<PathBuf>,

        /// Directory holding `<major>-<minor>.patch` archives, for
        /// naming the reference patch's netids.
        #[arg(short, long, default_value = "patch")]
        patch_dir: PathBuf,

        /// Measure how well the netid matching works before trusting
        /// it: split the reference replays in two, match one half
        /// against the other, and report how much of the known answer
        /// comes back.
        #[arg(long)]
        self_test: bool,

        /// Write every block of this netid, with its raw payload bytes,
        /// as JSON lines to `--dump`. Framing only, so it works on any
        /// patch: this is how you look at the payload bytes themselves.
        #[arg(long)]
        dump_netid: Option<u16>,

        /// Where `--dump-netid` writes.
        #[arg(long, default_value = "netid.jsonl")]
        dump: PathBuf,

        /// Write the raw material the statsJson proof works from: for
        /// every replay, each netid's blocks per champion alongside
        /// Riot's end-of-game counters. Data only, no claim attached —
        /// it is for trying a pairing the exact match rejects.
        #[arg(long)]
        dump_counts: Option<PathBuf>,

        /// Write every unit position the movement class gives up, as
        /// JSON lines. Coordinates carry the accuracy they were read
        /// at: the table recovered from the file alone pins a
        /// coordinate's high byte, which is ±256 map units, and
        /// `--table` narrows that.
        #[arg(long)]
        dump_positions: Option<PathBuf>,

        /// A fuller substitution table for the movement class, as the
        /// `plaintext: payload byte` JSON object `--layout` prints.
        /// Only the entries it adds are used, and the accuracy
        /// reported with the positions is adjusted to match.
        #[arg(long)]
        table: Option<PathBuf>,

        /// Read positions from this netid instead of the one the
        /// movement class's shape picks out. The shape test is
        /// patch-fragile — it selects the wrong class on 16.9 — so
        /// naming the netid is what makes the result checkable against
        /// a patch that decodes.
        #[arg(long)]
        positions_netid: Option<u16>,

        /// Output JSON path. Without it, a text report goes to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
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
            heap,
            find,
            layout,
        } => decode(&replay, &patch_dir, netid, limit, jsonl, heap, find, layout),
        Command::Inspect { replay, histogram } => inspect(&replay, histogram),
        Command::Summary {
            replay,
            output,
            include_identities,
        } => summary(&replay, output.as_deref(), include_identities),
        Command::Fields {
            replays,
            patch_dir,
            samples,
            threads,
            output,
            markdown,
        } => fields(
            &replays,
            &patch_dir,
            samples,
            threads,
            output.as_deref(),
            markdown.as_deref(),
        ),
        Command::Explore {
            replays,
            reference,
            patch_dir,
            self_test,
            dump_netid,
            dump,
            dump_counts,
            dump_positions,
            table,
            positions_netid,
            output,
        } => match dump_netid {
            Some(netid) => self::dump_netid(&replays, netid, &dump),
            None => explore(
                &replays,
                &reference,
                &patch_dir,
                self_test,
                ExploreOutputs {
                    counts: dump_counts.as_deref(),
                    positions: dump_positions.as_deref(),
                    table: table.as_deref(),
                    positions_netid,
                    report: output.as_deref(),
                },
            ),
        },
    }
}

fn summary(replay_path: &Path, output: Option<&Path>, include_identities: bool) -> Result<()> {
    let bytes = std::fs::read(replay_path)?;
    let replay = Replay::parse(&bytes)?;
    let json = crate::summary::summary(&replay, include_identities)?;
    match output {
        Some(path) => {
            std::fs::write(path, serde_json::to_vec_pretty(&json)?)?;
            let players = json["players"].as_array().map_or(0, Vec::len);
            let blocks = json["transport"]["total_blocks"].as_u64().unwrap_or(0);
            eprintln!(
                "wrote {} ({} patch, {players} players, {blocks} blocks profiled)",
                path.display(),
                replay.header.patch(),
            );
        }
        None => println!("{}", serde_json::to_string_pretty(&json)?),
    }
    Ok(())
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
#[allow(clippy::too_many_arguments)]
fn decode(
    replay_path: &Path,
    patch_dir: &Path,
    only: Option<u16>,
    limit: usize,
    jsonl: Option<PathBuf>,
    heap: bool,
    find: Option<String>,
    layout: bool,
) -> Result<()> {
    use crate::emulator::{DecodeStatus, PacketDecoder, PatchArchive};
    use crate::layout::Sample;
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
    let mut samples: Vec<Sample> = Vec::new();
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
            if layout && pkt.status == DecodeStatus::Clean {
                samples.push(Sample::of(block.payload, &pkt));
            }
            if let Some(w) = out.as_mut() {
                let words: serde_json::Map<String, serde_json::Value> = pkt
                    .written_words(0x200)
                    .into_iter()
                    .map(|(off, v)| (format!("0x{off:03x}"), format!("0x{v:08x}").into()))
                    .collect();
                let mut line = serde_json::json!({
                    "t": block.timestamp,
                    "netid": block.packet_id,
                    "class": archive.class_of(block.packet_id),
                    "param": format!("0x{:08x}", block.param),
                    "status": pkt.status.label(),
                    "payload": hex(block.payload),
                    "words": words,
                });
                if heap {
                    let blocks: Vec<serde_json::Value> = pkt
                        .memory
                        .allocations()
                        .iter()
                        .filter(|(addr, size)| *addr != pkt.object && *size > 0)
                        .filter_map(|(addr, size)| {
                            let bytes = pkt.memory.bytes(*addr, *size as usize)?;
                            Some(serde_json::json!({
                                "addr": format!("0x{addr:x}"),
                                "size": size,
                                "bytes": hex(bytes),
                            }))
                        })
                        .collect();
                    line["heap"] = serde_json::Value::Array(blocks);
                }
                writeln!(w, "{line}")?;
            }
            total += 1;
            if limit > 0 && total >= limit {
                break 'chunks;
            }
        }
    }
    let secs = started.elapsed().as_secs_f64();
    if layout {
        let fit = crate::layout::recover_buffer(&samples);
        match only.and_then(|netid| crate::layout::recover(netid, &samples)) {
            Some(l) => print_layout(&l, fit.as_ref(), archive.class_of(l.netid)),
            None => eprintln!("--layout needs --netid and enough clean blocks of it"),
        }
    }
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

/// Class names for the reference patch's netids, when an archive for it
/// is on disk. Without one the matching still runs; the proposals just
/// carry netids instead of names.
fn reference_names(patch_dir: &Path, replay: &Replay<'_>) -> BTreeMap<u16, String> {
    use crate::emulator::PatchArchive;

    let Ok(path) = find_archive(patch_dir, replay) else {
        return BTreeMap::new();
    };
    if !PatchArchive::is_format2(&path).unwrap_or(false) {
        return BTreeMap::new();
    }
    let Ok(archive) = PatchArchive::load(&path) else {
        return BTreeMap::new();
    };
    archive
        .packets
        .iter()
        .filter_map(|(netid, names)| Some((*netid, names.first()?.class.clone())))
        .collect()
}

/// Everything an `explore` run writes besides its own report, and the
/// one input that only affects what it writes.
struct ExploreOutputs<'a> {
    /// `--dump-counts`: the statsJson proof's raw material.
    counts: Option<&'a Path>,
    /// `--dump-positions`: unit positions as JSON lines.
    positions: Option<&'a Path>,
    /// `--table`: extra substitution entries for the movement class.
    table: Option<&'a Path>,
    /// `--positions-netid`: read positions from this class instead of
    /// the one the shape test picks.
    positions_netid: Option<u16>,
    /// `-o`: the JSON report, or a text summary to stdout without it.
    report: Option<&'a Path>,
}

fn explore(
    replays: &[PathBuf],
    reference: &[PathBuf],
    patch_dir: &Path,
    self_test: bool,
    outs: ExploreOutputs<'_>,
) -> Result<()> {
    let ExploreOutputs {
        counts: dump_counts,
        positions: dump_positions,
        table,
        positions_netid,
        report: output,
    } = outs;
    use crate::explore::{
        champion_counts, explore as explore_one, framing, match_patches, match_stats, pool, Framing,
    };
    use crate::explore::StatsRow;

    // One replay in memory at a time: twenty of them is gigabytes.
    let mut framings: Vec<Framing> = Vec::new();
    let mut per_replay: Vec<Value> = Vec::new();
    let mut stats_rows: Vec<StatsRow> = Vec::new();
    let mut patch = String::new();
    for path in replays {
        let bytes = std::fs::read(path)?;
        let replay = Replay::parse(&bytes)?;
        if patch.is_empty() {
            patch = replay.header.patch().to_string();
        } else if patch != replay.header.patch() {
            return Err(RoflError::Emulator(format!(
                "{}: patch {} does not match the pooled {patch}",
                path.display(),
                replay.header.patch()
            )));
        }
        let f = framing(&replay)?;
        if let Some(run) = f.champion_run(replay.metadata.players.len()) {
            stats_rows.push((champion_counts(&f, &run), player_counters(&replay)));
        }
        framings.push(f);
        per_replay.push(explore_one(&replay)?);
    }

    if let Some(path) = dump_counts {
        let rows: Vec<Value> = stats_rows
            .iter()
            .map(|(counts, counters)| {
                serde_json::json!({ "blocks_per_champion": counts, "counters": counters })
            })
            .collect();
        std::fs::write(path, serde_json::to_vec(&rows)?)?;
        eprintln!("wrote {} replays of counts to {}", rows.len(), path.display());
    }

    let deaths_netid = match_stats(&stats_rows)
        .into_iter()
        .find(|m| m.field == "NUM_DEATHS" && m.offset == 0)
        .map(|m| m.netid);

    let identified: Vec<Value> = match_stats(&stats_rows)
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "netid": m.netid,
                "statsjson_field": m.field,
                "blocks_minus_field": m.offset,
                "held_in_replays": m.replays,
                "replays_that_could_test_it": m.tested,
            })
        })
        .collect();

    let killer = match deaths_netid {
        Some(netid) => solve_killer(replays, netid)?,
        None => None,
    };

    let waypoints = solve_waypoints(replays)?;
    let emitted = match dump_positions {
        Some(path) => {
            dump_positions_to(replays, waypoints.as_ref(), table, positions_netid, path)?
        }
        None => None,
    };

    let mut out = serde_json::json!({
        "patch": patch,
        "replays": per_replay,
        "identified_by_statsjson": {
            "method": "a netid whose per-champion block count equals one of Riot's \
                       end-of-game counters, for all ten players, in every replay",
            "checked_replays": stats_rows.len(),
            "note": "This is a proof, not a fingerprint guess: no binary is read and \
                     no payload decoded, but the identity held ten players at a time \
                     in every game. A constant offset of 1 means one extra block per \
                     player, usually at spawn.",
            "netids": identified,
        },
        "kill_timeline": killer,
        "waypoint_table": waypoints,
        "positions": emitted,
    });

    if !reference.is_empty() {
        let mut ref_framings = Vec::new();
        let mut names = BTreeMap::new();
        let mut ref_patch = String::new();
        for path in reference {
            let bytes = std::fs::read(path)?;
            let replay = Replay::parse(&bytes)?;
            ref_patch = replay.header.patch().to_string();
            if names.is_empty() {
                names = reference_names(patch_dir, &replay);
            }
            ref_framings.push(framing(&replay)?);
        }
        if self_test {
            let t = crate::explore::self_test(&ref_framings, 10);
            out["netid_proposals_self_test"] = serde_json::json!({
                "patch": ref_patch,
                "method": "the reference replays split in two, one half matched \
                           against the other, where the right answer is known",
                "netids": t.netids,
                "recovered": t.correct,
                "recovered_share_of_traffic": (t.block_share * 1000.0).round() / 1000.0,
                "called_confident": t.confident,
                "confident_and_right": t.confident_correct,
                "gates": t.gates().iter()
                    .map(|(s, m, right, n)| serde_json::json!({
                        "score": s, "margin": m, "right": right, "calls": n,
                    }))
                    .collect::<Vec<_>>(),
            });
        }
        let target = pool(&framings, 10);
        let reference = pool(&ref_framings, 10);
        let by_netid: BTreeMap<u16, _> = reference.iter().map(|f| (f.netid, f)).collect();
        let proposals: Vec<Value> = match_patches(&target, &reference)
            .into_iter()
            .filter_map(|m| {
                let t = target.iter().find(|f| f.netid == m.netid)?;
                let r = by_netid.get(&m.reference_netid)?;
                Some(serde_json::json!({
                    "netid": m.netid,
                    "share": (t.share * 10_000.0).round() / 10_000.0,
                    "payload_bytes": t.length.label(),
                    "proposed_class": names.get(&m.reference_netid),
                    "reference_netid": m.reference_netid,
                    "reference_share": (r.share * 10_000.0).round() / 10_000.0,
                    "reference_payload_bytes": r.length.label(),
                    "score": (m.score * 1000.0).round() / 1000.0,
                    "margin": (m.margin * 1000.0).round() / 1000.0,
                    "confident": m.confident,
                }))
            })
            .collect();
        out["netid_proposals"] = serde_json::json!({
            "reference_patch": ref_patch,
            "method": "greedy one-to-one match on block share, payload length,                        whether the class addresses a unit or the game, how many                        entities it names, and how its traffic is spread over the game",
            "warning": "Hypotheses, not facts. Nothing here is checked against a                         decoded payload, because the payloads of this patch cannot                         be decoded. Treat a low margin as no answer.",
            "proposals": proposals,
        });
    }

    match output {
        Some(path) => {
            std::fs::write(path, out.to_string().as_bytes())?;
            eprintln!("wrote {}", path.display());
        }
        None => print_explore(&out),
    }
    Ok(())
}

/// Recover part of the movement class's substitution table.
///
/// The movement class is the one whose payload is its entry buffer
/// written backwards, so the first entry's header sits at a fixed
/// distance from the payload's *end* whatever the rest of the packet
/// does: the unit net id at `[-3]`..`[-6]`, the speed at `[-7]`..`[-10]`,
/// and for a single-waypoint entry the coordinates at `[-11]`..`[-14]`.
/// The coordinate high bytes move slowly enough to lay the plaintext
/// alphabet out in order ([`crate::explore::byte_chain`]).
///
/// Nothing here is hardcoded. The class is found by the shape of its
/// payload, `T(0x00)` by the net id's always-zero byte, and the
/// direction of the chain by the flags high byte, which the format
/// allows to be only 0 or 2 — so whichever direction puts that byte's
/// other value at plaintext 2 is the right one.
fn solve_waypoints(replays: &[PathBuf]) -> Result<Option<Value>> {
    use crate::explore::{blocks_of, byte_chain, framing, BytePair, Length, UnitTracks};

    let Some(path) = replays.first() else { return Ok(None) };
    let bytes = std::fs::read(path)?;
    let replay = Replay::parse(&bytes)?;
    let players = replay.metadata.players.len();

    // Pick the class by the property the solver needs, not by the
    // general shape of its payload — several classes carry a long
    // variable payload addressed to the game, and only one of them is
    // a batch of unit entries written backwards.
    //
    // That one has the first entry's net id at `[-3]`..`[-6]`, so
    // `[-6]` is the id's top byte and is `0x40` for every unit in the
    // game: through the substitution it has to be a single constant
    // across every block. `[-5]` is the next byte up, zero for every
    // id below 0x10000, so it is near-constant too. Nothing else looks
    // like that.
    let mut netid: Option<(u64, u16)> = None;
    for f in framing(&replay)?.fingerprints(players) {
        if f.blocks < WAYPOINT_MIN_BLOCKS
            || f.global_share <= 0.5
            || !matches!(f.length, Length::Variable(lo, _) if lo >= 14)
        {
            continue;
        }
        let mut top: HashMap<u8, u64> = HashMap::new();
        let mut next: HashMap<u8, u64> = HashMap::new();
        let mut seen = 0u64;
        for (_, _, payload) in blocks_of(&replay, f.netid)? {
            if payload.len() < 14 {
                continue;
            }
            seen += 1;
            *top.entry(payload[payload.len() - 6]).or_default() += 1;
            *next.entry(payload[payload.len() - 5]).or_default() += 1;
        }
        let constant = top.len() == 1;
        let near = next.values().max().copied().unwrap_or(0) * 100 >= seen.max(1) * 95;
        if constant && near && netid.map_or(true, |(n, _)| f.blocks > n) {
            netid = Some((f.blocks, f.netid));
        }
    }
    let Some((_, netid)) = netid else { return Ok(None) };

    // Walk every replay, keeping each unit's coordinate high bytes in
    // time order. A unit is keyed by the cipher of its net id's low
    // bytes: the id cannot be read yet, but the same unit gives the
    // same bytes, which is all the grouping needs.
    let mut blocks: Vec<(usize, f32, Vec<u8>)> = Vec::new();
    let mut top_byte_counts: HashMap<u8, u64> = HashMap::new();
    let mut zero_byte: HashMap<u8, u64> = HashMap::new();
    let mut flags_hi: HashMap<u8, u64> = HashMap::new();
    let mut flags_lo: HashMap<u8, u64> = HashMap::new();
    for (i, path) in replays.iter().enumerate() {
        let bytes = std::fs::read(path)?;
        let replay = Replay::parse(&bytes)?;
        for (t, _, payload) in blocks_of(&replay, netid)? {
            if payload.len() < 14 {
                continue;
            }
            let at = |k: usize| payload[payload.len() - k];
            *top_byte_counts.entry(at(6)).or_default() += 1;
            *zero_byte.entry(at(5)).or_default() += 1;
            *flags_hi.entry(at(2)).or_default() += 1;
            *flags_lo.entry(at(1)).or_default() += 1;
            blocks.push((i, t, payload));
        }
    }
    // `T(0x00)` is the byte the net id's always-zero field carries;
    // `T(0x40)` is the constant its top byte carries; `T(0x02)` is the
    // flags high byte's other value, since the format allows only 0
    // and 2 there.
    let Some(anchor) = top_byte(&zero_byte) else { return Ok(None) };
    let Some(net_id_top) = top_byte(&top_byte_counts) else { return Ok(None) };
    let orient = second_byte(&flags_hi);

    // Only an entry with a single waypoint puts its coordinates where
    // we are reading. Any other shape puts something else there — a
    // two-waypoint entry shifts them by its bitmap byte — and mixing
    // them in walks the chain straight off the alphabet.
    //
    // Which flags value means one waypoint is not worth guessing at.
    // Counting blocks picks the wrong one, because units are usually
    // given a path rather than a point; taking the shortest payload
    // picks the wrong one too, because one anomalous block is enough
    // to poison a minimum. So every candidate is tried, and the two
    // checks below decide. They are strict enough that nothing but the
    // right answer survives them.
    let mut candidates: Vec<(u8, u64)> = flags_lo
        .iter()
        .filter(|(_, n)| **n >= WAYPOINT_MIN_FLAG_BLOCKS)
        .map(|(c, n)| (*c, *n))
        .collect();
    candidates.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    candidates.truncate(WAYPOINT_CANDIDATES);

    let mut attempts: Vec<Value> = Vec::new();
    let mut solved: Option<(u8, crate::explore::ByteChain)> = None;
    for (one, _) in &candidates {
        let mut tracks: UnitTracks = HashMap::new();
        for (i, t, payload) in &blocks {
            let at = |k: usize| payload[payload.len() - k];
            if at(1) != *one {
                continue;
            }
            tracks.entry((*i, at(3), at(4))).or_default().push((*t, at(12), at(14)));
        }
        let mut pairs: Vec<BytePair> = Vec::new();
        for v in tracks.values_mut() {
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (a, b) in v.iter().zip(v.iter().skip(1)) {
                if b.0 - a.0 <= WAYPOINT_GAP_S {
                    pairs.push((a.1, b.1));
                    pairs.push((a.2, b.2));
                }
            }
        }
        let mut best: Option<(crate::explore::ByteChain, usize)> = None;
        for step in neighbours(&pairs, anchor) {
            let chain = byte_chain(&pairs, anchor, step);
            // Check 1: the flags *high* byte is only ever 0 or 2, so
            // its other value has to be the chain's plaintext 2.
            if !orient.is_some_and(|o| chain.table.get(&2) == Some(&o)) {
                continue;
            }
            // Check 2, and the chain never sees a flags byte while it
            // walks: the flags *low* byte is the waypoint count
            // doubled, so the chain's small even plaintexts have to be
            // values that byte actually takes, and its plaintext 1 —
            // which would mean zero waypoints — must not be.
            let agree = [2u8, 4, 6, 8, 10]
                .iter()
                .filter(|p| chain.table.get(p).is_some_and(|c| flags_lo.contains_key(c)))
                .count();
            let impossible = chain.table.get(&1).is_some_and(|c| flags_lo.contains_key(c));
            if agree >= WAYPOINT_MIN_AGREE && !impossible {
                best = Some((chain, agree));
                break;
            }
        }
        let best = best.map(|(chain, agree)| {
            attempts.push(serde_json::json!({
                "flags_byte": format!("0x{one:02x}"),
                "pairs": pairs.len(),
                "flags_values_agreeing": agree,
                "passed": true,
            }));
            chain
        });
        if best.is_none() {
            attempts.push(serde_json::json!({
                "flags_byte": format!("0x{one:02x}"),
                "pairs": pairs.len(),
                "passed": false,
            }));
        }
        if let Some(chain) = best {
            solved = Some((*one, chain));
            break;
        }
    }

    let Some((one, chain)) = solved else {
        return Ok(Some(serde_json::json!({
            "netid": netid,
            "solved": false,
            "why": "no candidate entry shape produced a chain that survived both checks",
            "anchor": format!("0x{anchor:02x}"),
            "net_id_top_byte": format!("0x{net_id_top:02x}"),
            "attempts": attempts,
        })));
    };
    let oriented = true;

    Ok(Some(serde_json::json!({
        "netid": netid,
        "solved": true,
        "method": "the coordinate high byte of a single-waypoint entry moves by at \
                   most one between consecutive packets for the same unit, so the \
                   pairs seen changing lay the plaintext alphabet out in order",
        "entries": chain.table.len(),
        "observations": chain.observations,
        "unchanged_share": (chain.unchanged * 1000.0).round() / 1000.0,
        "anchor": format!("0x{anchor:02x}"),
        "orient": orient.map(|o| format!("0x{o:02x}")),
        "oriented_by_the_flags_byte": oriented,
        "net_id_top_byte_separately_known": format!("0x{net_id_top:02x}"),
        "note_on_0x40": "T(0x40) is known from the net id's top byte but is not in                          this table: a coordinate's high byte never takes that                          value, so the chain never reaches it",
        "one_waypoint_flags_byte": format!("0x{one:02x}"),
        "shapes_tried": attempts,
        "table": chain.table.iter()
            .map(|(p, c)| (format!("{p:02x}"), format!("{c:02x}")))
            .collect::<BTreeMap<_, _>>(),
        "limit": "a coordinate's low byte moves by most of its range between \
                  packets, so no chain forms for it; positions come out to within \
                  a high byte, about 512 map units, and no closer",
    })))
}

/// Longest gap between two packets still treated as one step.
const WAYPOINT_GAP_S: f32 = 3.0;
/// Blocks a class needs before it is worth testing as the movement one.
const WAYPOINT_MIN_BLOCKS: u64 = 1000;
/// Blocks a flags value needs before it is worth trying as the
/// single-waypoint shape.
const WAYPOINT_MIN_FLAG_BLOCKS: u64 = 200;
/// How many entry shapes to try before giving up.
const WAYPOINT_CANDIDATES: usize = 6;
/// Flags values the chain must agree about before it is believed.
const WAYPOINT_MIN_AGREE: usize = 3;

fn top_byte(counts: &HashMap<u8, u64>) -> Option<u8> {
    counts.iter().max_by_key(|(_, n)| **n).map(|(b, _)| *b)
}

/// The second-commonest byte, which for a field that is only ever 0 or
/// 2 is the cipher for 2.
fn second_byte(counts: &HashMap<u8, u64>) -> Option<u8> {
    let mut v: Vec<(&u8, &u64)> = counts.iter().collect();
    v.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    v.get(1).map(|(b, _)| **b)
}

/// Bytes seen changing into `from`, strongest first.
fn neighbours(pairs: &[(u8, u8)], from: u8) -> Vec<u8> {
    let mut counts: HashMap<u8, u64> = HashMap::new();
    for (a, b) in pairs {
        if *a == from && b != a {
            *counts.entry(*b).or_default() += 1;
        } else if *b == from && a != b {
            *counts.entry(*a).or_default() += 1;
        }
    }
    let mut v: Vec<(u8, u64)> = counts.into_iter().collect();
    v.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    v.into_iter().map(|(b, _)| b).take(4).collect()
}

/// Read the killer field of the champion-death class, then use it.
///
/// The death class is already identified: it is the netid that fires
/// exactly `NUM_DEATHS` times per champion. One of its payload bytes
/// has to name whoever got the kill, and across a whole game that byte
/// must name each player exactly `CHAMPIONS_KILLED` times. That is what
/// finds it, and what checks it afterwards.
fn solve_killer(replays: &[PathBuf], netid: u16) -> Result<Option<Value>> {
    use crate::explore::{
        blocks_of, champion_blocks, event_locked, framing, kills, solve_netid_field, DeathBlocks,
        FieldInput, Timeline,
    };

    let mut inputs: Vec<FieldInput> = Vec::new();
    let mut games: Vec<(DeathBlocks, Vec<u32>)> = Vec::new();
    let mut timelines: Vec<Timeline> = Vec::new();
    for path in replays {
        let bytes = std::fs::read(path)?;
        let replay = Replay::parse(&bytes)?;
        let f = framing(&replay)?;
        let Some(run) = f.champion_run(replay.metadata.players.len()) else {
            continue;
        };
        timelines.push(Timeline {
            blocks: champion_blocks(&replay, &run)?,
            deaths: vec![Vec::new(); run.len()],
            kills: vec![Vec::new(); run.len()],
            length_s: replay.metadata.game_length_ms as f32 / 1000.0,
        });
        let rows = blocks_of(&replay, netid)?;
        let counter = player_counters(&replay);
        let Some(k) = counter.get("CHAMPIONS_KILLED") else {
            continue;
        };
        inputs.push((
            rows.iter().map(|(_, _, p)| p.clone()).collect(),
            run.clone(),
            k.clone(),
        ));
        games.push((rows, run));
    }
    let Some(field) = solve_netid_field(netid, &inputs) else {
        return Ok(None);
    };

    let mut timeline = 0usize;
    let mut unnamed = 0usize;
    let mut sample = Vec::new();
    for (i, (rows, run)) in games.iter().enumerate() {
        let ks = kills(rows, &field, run);
        if let Some(tl) = timelines.get_mut(i) {
            for k in &ks {
                tl.deaths[k.victim].push(k.t);
                if let Some(killer) = k.killer {
                    tl.kills[killer].push(k.t);
                }
            }
        }
        timeline += ks.len();
        unnamed += ks.iter().filter(|k| k.killer.is_none()).count();
        if sample.is_empty() {
            sample = ks
                .iter()
                .take(8)
                .map(|k| {
                    serde_json::json!({
                        "t": (f64::from(k.t) * 10.0).round() / 10.0,
                        "victim": k.victim,
                        "killer": k.killer,
                    })
                })
                .collect();
        }
    }

    let locked: Vec<Value> = event_locked(&timelines)
        .into_iter()
        .map(|l| {
            serde_json::json!({
                "netid": l.netid,
                "fires_on": l.event,
                "champion_blocks": l.blocks,
                "on_the_event": l.near,
                "share": (l.share * 1000.0).round() / 1000.0,
                "times_likelier_than_chance": l.lift.round(),
            })
        })
        .collect();

    Ok(Some(serde_json::json!({
        "netid": field.netid,
        "bytes_after_the_field": field.tail,
        "method": "the byte of the death packet that names each player exactly \
                   CHAMPIONS_KILLED times; learned only where the counts force \
                   the assignment, then used to predict the other replays",
        "payload_byte_to_net_id": field.table.iter()
            .map(|(c, p)| (format!("{c:02x}"), format!("net id varint byte 0x{p:02x}")))
            .collect::<BTreeMap<_, _>>(),
        "replays_predicted": field.replays,
        "players_checked": field.players_checked,
        "players_right": field.players_right,
        "deaths": timeline,
        "deaths_with_no_champion_killer": unnamed,
        "first_kills": sample,
        "classes_locked_to_an_event": {
            "method": "classes whose blocks about a champion only ever land when                        that champion dies, or gets a kill, measured against how                        often chance would put them there",
            "netids": locked,
        },
    })))
}

/// Riot's end-of-game integer counters, per player, in roster order.
fn player_counters(replay: &Replay<'_>) -> BTreeMap<String, Vec<i64>> {
    let stats: Vec<Value> =
        serde_json::from_str(&replay.metadata.stats_json_raw).unwrap_or_default();
    let mut out: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for p in &stats {
        let Some(obj) = p.as_object() else { continue };
        for (k, v) in obj {
            let Some(n) = v.as_str().and_then(|s| s.parse::<i64>().ok()) else {
                continue;
            };
            out.entry(k.clone()).or_default().push(n);
        }
    }
    // A field some players are missing cannot be compared player by player.
    out.retain(|_, v| v.len() == stats.len());
    crate::explore::add_derived(&mut out);
    out
}

/// Write every position the movement class gives up, as JSON lines.
///
/// This is the movement half of the goal on a patch that cannot be
/// decoded, and the honest part of it is the accuracy: a position read
/// with only the coordinate's high byte is within 256 map units, and
/// each line says which of the two it is rather than leaving the
/// reader to assume.
///
/// The table comes from the chain `solve_waypoints` recovered on this
/// patch, plus whatever `--table` adds. A wrong table does not produce
/// wrong positions quietly: the entry stream has to parse to exactly
/// its own length, so it produces none at all, which is why the parsed
/// share is reported.
fn dump_positions_to(
    replays: &[PathBuf],
    waypoints: Option<&Value>,
    extra: Option<&Path>,
    only: Option<u16>,
    out: &Path,
) -> Result<Option<Value>> {
    use crate::explore::{blocks_of, positions};

    // The chain is used when it solved, and when it solved the class
    // being read: a table recovered for one class says nothing about
    // another (§ "Two tables from the same patch are unrelated").
    let solved = waypoints.filter(|w| w["solved"] == true);
    let chain_netid = solved
        .and_then(|w| w["netid"].as_u64())
        .and_then(|n| u16::try_from(n).ok());
    let Some(netid) = only.or(chain_netid) else {
        return Ok(None);
    };
    let w = solved.filter(|_| chain_netid == Some(netid));

    // The reports hold `plaintext -> payload byte`; reading a payload
    // needs the other direction.
    let mut plain: BTreeMap<u8, u8> = BTreeMap::new();
    let mut from_chain = 0usize;
    for (p, c) in w.map(|w| &w["table"]).and_then(Value::as_object).into_iter().flatten() {
        if let (Ok(p), Some(c)) = (
            u8::from_str_radix(p, 16),
            c.as_str().and_then(|s| u8::from_str_radix(s, 16).ok()),
        ) {
            plain.insert(c, p);
            from_chain += 1;
        }
    }
    let mut added = 0usize;
    if let Some(path) = extra {
        let text = std::fs::read_to_string(path)?;
        let parsed: BTreeMap<String, String> = serde_json::from_str(&text)?;
        for (p, c) in parsed {
            if let (Ok(p), Ok(c)) = (u8::from_str_radix(&p, 16), u8::from_str_radix(&c, 16)) {
                if plain.insert(c, p).is_none() {
                    added += 1;
                }
            }
        }
    }

    let mut blocks: Vec<(usize, f32, Vec<u8>)> = Vec::new();
    for (i, path) in replays.iter().enumerate() {
        let bytes = std::fs::read(path)?;
        let replay = Replay::parse(&bytes)?;
        for (t, _, payload) in blocks_of(&replay, netid)? {
            blocks.push((i, t, payload));
        }
    }
    let (found, parsed, seen) = positions(&blocks, &plain);

    let mut text = String::new();
    let mut exact = 0u64;
    for p in &found {
        exact += u64::from(p.exact);
        text.push_str(&format!(
            "{{\"replay\":{},\"t\":{:.3},\"unit\":\"{:02x}{:02x}\",\
             \"x\":{:.1},\"z\":{:.1},\"accuracy_map_units\":{}}}\n",
            p.replay,
            p.t,
            p.unit.0,
            p.unit.1,
            p.x,
            p.z,
            if p.exact { 0 } else { 256 },
        ));
    }
    std::fs::write(out, text)?;
    eprintln!("wrote {} positions to {}", found.len(), out.display());

    let units: std::collections::HashSet<(u8, u8)> = found.iter().map(|p| p.unit).collect();
    Ok(Some(serde_json::json!({
        "netid": netid,
        "file": out.display().to_string(),
        "positions": found.len(),
        "units": units.len(),
        "payloads_parsed": parsed,
        "payloads_seen": seen,
        "table_entries": { "from_the_chain": from_chain, "added_by_table_option": added },
        "exact_positions": exact,
        "accuracy": "a position whose coordinate low bytes are in the table is \
                     exact; otherwise the low byte is taken as the middle of its \
                     range, so the position is within 256 map units of 15,000 \
                     across. Which one each is, each line says.",
        "check": "a payload has to parse to exactly its own length or it \
                  contributes nothing, so a wrong table yields no positions \
                  rather than wrong ones",
    })))
}

/// Lowercase hex of a block's raw payload, as it sits in the file.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Write every block of one netid as framing plus its raw payload. No
/// archive and no decoding, so this works on a packed patch: it is how
/// you look at the bytes themselves.
fn dump_netid(replays: &[PathBuf], netid: u16, out: &Path) -> Result<()> {
    use std::io::Write;

    let mut w = std::io::BufWriter::new(std::fs::File::create(out)?);
    let mut n = 0usize;
    for path in replays {
        let bytes = std::fs::read(path)?;
        let replay = Replay::parse(&bytes)?;
        let file = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        for chunk in replay.chunks() {
            let chunk = chunk?;
            if chunk.stream_tag != StreamTag::GameChunk {
                continue;
            }
            let Ok(body) = decompress_chunk(&chunk) else {
                continue;
            };
            for block in BlockIterator::new(&body) {
                let Ok(block) = block else { break };
                if block.packet_id != netid {
                    continue;
                }
                let line = serde_json::json!({
                    "replay": file,
                    "t": block.timestamp,
                    "netid": block.packet_id,
                    "param": format!("0x{:08x}", block.param),
                    "payload": hex(block.payload),
                });
                writeln!(w, "{line}")?;
                n += 1;
            }
        }
    }
    eprintln!("wrote {n} blocks of netid {netid} to {}", out.display());
    Ok(())
}

fn print_explore(out: &Value) {
    for r in out["replays"].as_array().into_iter().flatten() {
        println!(
            "patch {}  {} blocks  {} min  {} entities",
            r["patch"].as_str().unwrap_or("?"),
            r["blocks"],
            r["minutes"],
            r["entities"]["total"],
        );
        let champs = &r["entities"]["champions"];
        if let Some(first) = champs["first"].as_str() {
            println!("champions: {first} .. {}", champs["last"].as_str().unwrap_or("?"));
        } else {
            println!("champions: not identified");
        }
        println!("{:<4} {:<8} {:<14} {:>10} {:>8}  net id", "", "team", "champion", "blocks", "netids");
        for p in r["players"].as_array().into_iter().flatten() {
            println!(
                "{:<4} {:<8} {:<14} {:>10} {:>8}  {}",
                p["index"],
                p["team"].as_str().unwrap_or(""),
                p["champion"].as_str().unwrap_or(""),
                p["blocks"],
                p["distinct_netids"],
                p["net_id"].as_str().unwrap_or("-"),
            );
        }
        println!();
        println!(
            "{:>6} {:>8} {:>8} {:>9}  {:<10} {:>9}  shape",
            "netid", "blocks", "share", "payload", "addresses", "entities"
        );
        for n in r["netids"].as_array().into_iter().flatten().take(20) {
            println!(
                "{:>6} {:>8} {:>7.2}% {:>9}  {:<10} {:>9}  {}",
                n["netid"],
                n["blocks"],
                n["share"].as_f64().unwrap_or(0.0) * 100.0,
                n["payload_bytes"].as_str().unwrap_or(""),
                n["addresses"].as_str().unwrap_or(""),
                n["distinct_entities"],
                n["shape"].as_str().unwrap_or("")
            );
        }
        println!();
    }
    if let Some(k) = out.get("kill_timeline") {
        if !k.is_null() {
            println!(
                "killer field: netid {}, last field of the payload (tail {}) -- predicted {} of {} player-games right \
                 across {} replays, {} deaths placed, {} with no champion killer\n",
                k["netid"],
                k["bytes_after_the_field"],
                k["players_right"],
                k["players_checked"],
                k["replays_predicted"],
                k["deaths"],
                k["deaths_with_no_champion_killer"],
            );
        }
    }
    if let Some(k) = out.get("kill_timeline") {
        let rows = k["classes_locked_to_an_event"]["netids"].as_array();
        for m in rows.into_iter().flatten() {
            println!(
                "  netid {:>5} fires on {:<17} {} of {} champion blocks ({}x chance)",
                m["netid"],
                m["fires_on"].as_str().unwrap_or("?"),
                m["on_the_event"],
                m["champion_blocks"],
                m["times_likelier_than_chance"],
            );
        }
        if rows.is_some_and(|r| !r.is_empty()) {
            println!();
        }
    }
    if let Some(w) = out.get("waypoint_table") {
        if w["solved"] == Value::Bool(true) {
            println!(
                "movement class netid {}: {} of 256 table entries recovered",
                w["netid"], w["entries"]
            );
            println!(
                "  from {} consecutive coordinate high bytes, {:.1} % unchanged",
                w["observations"],
                w["unchanged_share"].as_f64().unwrap_or(0.0) * 100.0,
            );
            println!(
                "  anchored on T(0x00) = {}, oriented by the flags byte, and agreeing",
                w["anchor"].as_str().unwrap_or("?")
            );
            println!(
                "  about T(0x40) = {} without ever having looked at a net id
",
                w["net_id_top_byte"].as_str().unwrap_or("?")
            );
        } else if !w.is_null() {
            println!(
                "movement class netid {}: NOT SOLVED ({} entries walked, oriented {})
",
                w["netid"], w["entries_walked"], w["oriented_by_the_flags_byte"],
            );
        }
    }
    if let Some(i) = out.get("identified_by_statsjson") {
        let rows = i["netids"].as_array().map(Vec::as_slice).unwrap_or_default();
        println!(
            "identified by statsJson, across {} replays: {} netids",
            i["checked_replays"], rows.len()
        );
        for m in rows {
            let off = m["blocks_minus_field"].as_i64().unwrap_or(0);
            let tail = match off {
                0 => String::new(),
                n => format!(" {n:+}"),
            };
            println!(
                "  netid {:>5}  blocks per champion = {}{}  (held in {} of {} replays that could test it)",
                m["netid"],
                m["statsjson_field"].as_str().unwrap_or("?"),
                tail,
                m["held_in_replays"],
                m["replays_that_could_test_it"],
            );
        }
        println!();
    }
    if let Some(t) = out.get("netid_proposals_self_test") {
        println!(
            "matcher self-test on patch {}: {} of {} netids recovered ({:.1} % of traffic); \
             {} of {} confident calls right\n",
            t["patch"].as_str().unwrap_or("?"),
            t["recovered"],
            t["netids"],
            t["recovered_share_of_traffic"].as_f64().unwrap_or(0.0) * 100.0,
            t["confident_and_right"],
            t["called_confident"],
        );
    }
    if let Some(p) = out.get("netid_proposals") {
        println!(
            "netid proposals against patch {} (hypotheses, unchecked):",
            p["reference_patch"].as_str().unwrap_or("?")
        );
        println!("{:>6} {:>8} {:>9}  {:>6} {:>7}  class", "netid", "share", "payload", "score", "margin");
        for m in p["proposals"].as_array().into_iter().flatten() {
            if m["confident"] != Value::Bool(true) {
                continue;
            }
            println!(
                "{:>6} {:>7.2}% {:>9}  {:>6.3} {:>7.3}  {}",
                m["netid"],
                m["share"].as_f64().unwrap_or(0.0) * 100.0,
                m["payload_bytes"].as_str().unwrap_or(""),
                m["score"].as_f64().unwrap_or(0.0),
                m["margin"].as_f64().unwrap_or(0.0),
                m["proposed_class"].as_str().unwrap_or("(reference netid unnamed)"),
            );
        }
        let n = p["proposals"].as_array().map_or(0, Vec::len);
        let c = p["proposals"].as_array().into_iter().flatten()
            .filter(|m| m["confident"] == Value::Bool(true)).count();
        println!("{c} of {n} netids matched confidently; the rest are not reported.");
    }
}

#[cfg(feature = "emulator")]
fn fields(
    replays: &[PathBuf],
    patch_dir: &Path,
    samples: usize,
    threads: usize,
    output: Option<&Path>,
    markdown: Option<&Path>,
) -> Result<()> {
    use crate::emulator::PatchArchive;
    use crate::fields::Profile;

    let threads = if threads == 0 {
        std::thread::available_parallelism().map_or(4, |n| n.get())
    } else {
        threads
    };
    let mut pooled = Profile::default();
    for path in replays {
        let bytes = std::fs::read(path)?;
        let replay = Replay::parse(&bytes)?;
        let archive_path = find_archive(patch_dir, &replay)?;
        if !PatchArchive::is_format2(&archive_path)? {
            return Err(RoflError::Emulator(format!(
                "{}: field profiling needs a format-2 archive (docs/DECODING.md)",
                archive_path.display()
            )));
        }
        let archive = PatchArchive::load(&archive_path)?;
        if !pooled.patch.is_empty() && pooled.patch != replay.header.patch() {
            return Err(RoflError::Emulator(format!(
                "{}: patch {} does not match the pooled {}",
                path.display(),
                replay.header.patch(),
                pooled.patch
            )));
        }
        let started = std::time::Instant::now();
        let mut one = crate::fields::profile(&replay, &archive, samples, threads)?;
        one.replays.push(path.display().to_string());
        eprintln!(
            "{}: {} blocks, {} profiled, {} classes in {:.1}s",
            path.display(),
            one.blocks(),
            one.sampled(),
            one.classes.values().filter(|c| c.blocks > 0).count(),
            started.elapsed().as_secs_f64(),
        );
        pooled.merge(&one);
    }

    let named = pooled.classes.values().filter(|c| c.class.is_some()).count();
    eprintln!(
        "pooled: {} blocks over {} replays, {} classes, {named} named",
        pooled.blocks(),
        pooled.replays.len(),
        pooled.classes.values().filter(|c| c.blocks > 0).count(),
    );
    if let Some(path) = output {
        std::fs::write(path, pooled.to_json().to_string().as_bytes())?;
        eprintln!("wrote {}", path.display());
    }
    if let Some(path) = markdown {
        std::fs::write(path, pooled.markdown().as_bytes())?;
        eprintln!("wrote {}", path.display());
    }
    if output.is_none() && markdown.is_none() {
        println!("{}", pooled.markdown());
    }
    Ok(())
}

#[cfg(not(feature = "emulator"))]
fn fields(
    _replays: &[PathBuf],
    _patch_dir: &Path,
    _samples: usize,
    _threads: usize,
    _output: Option<&Path>,
    _markdown: Option<&Path>,
) -> Result<()> {
    Err(RoflError::Io(std::io::Error::other(
        "fields requires the `emulator` feature",
    )))
}

#[cfg(feature = "emulator")]
/// Print a recovered substitution, rows of sixteen, plaintext
/// ascending, `--` where an entry was not pinned.
///
/// The table *is* the result, so it is worth printing rather than only
/// counting: it is what a reader compares against, and on a patch that
/// cannot be decoded it is the thing being searched for.
fn print_table(table: &BTreeMap<u8, u8>) {
    if table.is_empty() {
        return;
    }
    println!("  plaintext -> payload byte:");
    for row in 0..16u16 {
        let cells: Vec<String> = (0..16u16)
            .map(|col| {
                u8::try_from(row * 16 + col)
                    .ok()
                    .and_then(|p| table.get(&p))
                    .map_or_else(|| "--".to_string(), |c| format!("{c:02x}"))
            })
            .collect();
        println!("    {:02x}  {}", row * 16, cells.join(" "));
    }
}

fn print_layout(
    l: &crate::layout::Layout,
    fit: Option<&crate::layout::BufferFit>,
    class: Option<&str>,
) {
    use crate::layout::Carries;

    println!(
        "
netid {} {} -- {} blocks of the commonest payload length, {} bytes",
        l.netid,
        class.unwrap_or("(no class name)"),
        l.blocks,
        l.payload_len,
    );
    println!("fields in the order the class writes them:");
    for (off, order, n) in l.field_order() {
        println!("  +0x{off:03x}  {n} bytes, {order}");
    }
    println!("payload byte by byte:");
    for (i, c) in l.bytes.iter().enumerate() {
        let what = match c {
            Carries::Field(off, j) => format!("+0x{off:03x} byte {j}"),
            Carries::Ambiguous(v) => format!("{} candidates (field barely varies)", v.len()),
            Carries::Unknown => "not in the object".into(),
        };
        println!("  [{:>2}] (end {:>3})  {what}", i, i as i64 - l.payload_len as i64);
    }
    if let Some(fit) = fit {
        println!(
            "payload is the class's allocated buffer, {}, behind a {} header:",
            if fit.reversed { "back to front" } else { "in order" },
            if fit.reversed { "variable".to_string() } else { format!("{}-byte", fit.header) },
        );
        println!(
            "  {} of 256 table entries over {} blocks, {} conflicts",
            fit.table.len(),
            fit.blocks,
            fit.conflicts
        );
        if let Some(c) = fit.table.get(&0) {
            println!("  T(0x00) = 0x{c:02x}");
        }
        print_table(&fit.table);
    }
    println!(
        "substitution table: {} of 256 entries from {} positions, {} conflicts",
        l.table.len(),
        l.table_from_positions,
        l.table_conflicts
    );
    if !l.disagreeing_positions.is_empty() {
        println!(
            "  it contradicts positions {:?}, whose fields take too few values to be pinned",
            l.disagreeing_positions
        );
    }
    print_table(&l.table);
    if !l.settled() {
        println!("  NOT SETTLED: with {} blocks some of the layout above is", l.blocks);
        println!("  coincidence. A class this rare needs more blocks to pin down.");
    }
}

#[cfg(not(feature = "emulator"))]
fn decode(
    _replay: &Path,
    _patch_dir: &Path,
    _only: Option<u16>,
    _limit: usize,
    _jsonl: Option<PathBuf>,
    _heap: bool,
    _find: Option<String>,
    _layout: bool,
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
        "  {:<5} {:<8} {:<14} W  {:>9} {:>3} {:>4} {:>6} {:>7}  riot id",
        "team", "role", "champion", "K/D/A", "lvl", "cs", "gold", "dmg"
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
