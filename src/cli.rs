use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
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
            find,
        } => decode(&replay, &patch_dir, netid, limit, jsonl, find),
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
            output,
        } => match dump_netid {
            Some(netid) => self::dump_netid(&replays, netid, &dump),
            None => explore(&replays, &reference, &patch_dir, self_test, output.as_deref()),
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
                    "payload": hex(block.payload),
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

fn explore(
    replays: &[PathBuf],
    reference: &[PathBuf],
    patch_dir: &Path,
    self_test: bool,
    output: Option<&Path>,
) -> Result<()> {
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

/// Read the killer field of the champion-death class, then use it.
///
/// The death class is already identified: it is the netid that fires
/// exactly `NUM_DEATHS` times per champion. One of its payload bytes
/// has to name whoever got the kill, and across a whole game that byte
/// must name each player exactly `CHAMPIONS_KILLED` times. That is what
/// finds it, and what checks it afterwards.
fn solve_killer(replays: &[PathBuf], netid: u16) -> Result<Option<Value>> {
    use crate::explore::{
        blocks_of, framing, kills, solve_netid_field, DeathBlocks, FieldInput,
    };

    let mut inputs: Vec<FieldInput> = Vec::new();
    let mut games: Vec<(DeathBlocks, Vec<u32>)> = Vec::new();
    for path in replays {
        let bytes = std::fs::read(path)?;
        let replay = Replay::parse(&bytes)?;
        let f = framing(&replay)?;
        let Some(run) = f.champion_run(replay.metadata.players.len()) else {
            continue;
        };
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
    for (rows, run) in &games {
        let ks = kills(rows, &field, run);
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
    out
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
