//! Deep file-only exploration: everything a `.rofl` gives up from block
//! framing alone, with no game binary and no payload decoding.
//!
//! This is the path for patches whose client ships packed (16.10+, see
//! `docs/COMPATIBILITY.md`). `summary.rs` reports the scoreboard and a
//! flat activity profile; this goes further, and the extra reach all
//! comes from one observation: a block's `timestamp`, `netid`, `param`
//! and length are plaintext, so the *behaviour* of every packet class and
//! every entity is measurable even when the payload is not.
//!
//! Three things come out:
//!
//! - **Who the champions are.** Entity ids are handed out in order as the
//!   game creates objects, and the ten champions are created together.
//!   They are also the entities the server has the most different things
//!   to say about. Taking the run of ten consecutive ids with the highest
//!   netid variety finds them without decoding anything; on 16.9, where
//!   the decoder knows the answer independently, it finds exactly the
//!   right ten in every replay.
//! - **A per-champion timeline.** Blocks per minute per champion: how
//!   busy each player's champion was, minute by minute, for the whole
//!   game.
//! - **A fingerprint per packet class.** Share of blocks, payload length,
//!   whether the class addresses an entity or the game, how many distinct
//!   entities it ever named, and when in the game it fires. Netids are
//!   renumbered every patch, so this is what a class looks like from the
//!   outside, and it is the raw material for matching a packed patch's
//!   netids against a patch we can decode (`match_patches`).

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;
use serde_json::{json, Value};

use crate::error::Result;
use crate::replay_info::{role_label, team_label};
use crate::rofl::block::BlockIterator;
use crate::rofl::decompress::decompress_chunk;
use crate::rofl::stream::StreamTag;
use crate::rofl::metadata::Team;
use crate::rofl::Replay;

/// A match this far ahead of the runner-up, at this score, is reported
/// as confident.
///
/// Chosen from [`self_test`] on the five 16.9 replays, not by taste.
/// Across the grid it scans, the margin is what carries the signal and
/// the score barely moves precision: at a margin of 0.05 every score
/// cut-off lands near 88 %, at 0.10 near 94 %, at 0.15 near 95 %. This
/// pair called 72 of 254 netids and was right on 69 of them, 95.8 %.
/// Raising it further buys nothing and costs calls.
///
/// That number is the ceiling, not the expectation. The self-test
/// renumbers nothing and changes no class: it only asks whether a
/// fingerprint taken from two games identifies the same class in three
/// others. A real patch gap also moves the classes themselves.
pub const CONFIDENT_MARGIN: f64 = 0.15;
pub const CONFIDENT_SCORE: f64 = 0.85;

/// Team ids as the game numbers them, the same on every patch measured.
pub const TEAM_ORDER: u8 = 1;
pub const TEAM_CHAOS: u8 = 2;

/// A `param` at or above this is a runtime entity id.
const ENTITY_ID_BASE: u32 = 0x4000_0000;
/// An entity seen in the first and last of these many seconds of the
/// game counts as present for the whole of it.
const EDGE_WINDOW: f32 = 90.0;
/// Different values a statsJson field must take across the ten players
/// before a match against it means anything.
const MIN_SPREAD: usize = 3;
/// Largest constant difference accepted between a block count and a
/// statsJson counter. One extra block per player is ordinary; ten is a
/// coincidence dressed up as a match.
const MAX_OFFSET: i64 = 2;
/// Replays that must be able to test an identity before it is reported.
const MIN_TESTS: usize = 3;
/// Distinct payload lengths kept per netid before we call it variable.
const LENGTH_CAP: usize = 8;

#[derive(Debug, Default, Clone)]
struct Entity {
    blocks: u64,
    first: f32,
    last: f32,
    netids: HashSet<u16>,
    per_minute: Vec<u64>,
}

#[derive(Debug, Default, Clone)]
struct NetId {
    blocks: u64,
    /// Blocks whose `param` is not an entity id: the class is about the
    /// game, not about a unit.
    global: u64,
    params: HashSet<u32>,
    /// Blocks per entity, for the champions only: the full table would
    /// be thousands of entries per netid and nothing reads it.
    per_param: HashMap<u32, u64>,
    /// How often each byte value occurs anywhere in this class's
    /// payloads.
    byte_counts: Vec<u64>,
    lengths: BTreeMap<usize, u64>,
    first: f32,
    last: f32,
    per_minute: Vec<u64>,
}

/// Everything measured from framing, before it is interpreted.
#[derive(Debug, Default)]
pub struct Framing {
    netids: BTreeMap<u16, NetId>,
    entities: BTreeMap<u32, Entity>,
    pub blocks: u64,
    pub minutes: usize,
}

fn bump(v: &mut Vec<u64>, minute: usize) {
    if v.len() <= minute {
        v.resize(minute + 1, 0);
    }
    v[minute] += 1;
}

/// Walk every game-chunk block and record what its framing says.
pub fn framing(replay: &Replay<'_>) -> Result<Framing> {
    let mut f = Framing::default();
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
            let t = block.timestamp.max(0.0);
            let minute = (t / 60.0) as usize;
            f.blocks += 1;
            f.minutes = f.minutes.max(minute + 1);

            let n = f.netids.entry(block.packet_id).or_default();
            if n.blocks == 0 {
                n.first = t;
            }
            n.blocks += 1;
            n.last = t;
            n.params.insert(block.param);
            *n.per_param.entry(block.param).or_default() += 1;
            if n.byte_counts.is_empty() {
                n.byte_counts = vec![0; 256];
            }
            for b in block.payload {
                n.byte_counts[*b as usize] += 1;
            }
            if n.lengths.len() < LENGTH_CAP || n.lengths.contains_key(&block.payload.len()) {
                *n.lengths.entry(block.payload.len()).or_default() += 1;
            }
            bump(&mut n.per_minute, minute);

            if block.param < ENTITY_ID_BASE {
                n.global += 1;
                continue;
            }
            let e = f.entities.entry(block.param).or_default();
            if e.blocks == 0 {
                e.first = t;
            }
            e.blocks += 1;
            e.last = t;
            e.netids.insert(block.packet_id);
            bump(&mut e.per_minute, minute);
        }
    }
    Ok(f)
}

impl Framing {
    /// The champions' entity ids, in creation order.
    ///
    /// Two properties pick them out, neither of which needs a payload.
    /// They are created together, so their ids are `count` consecutive
    /// values; and the server has more different things to say about a
    /// champion than about any other unit, so they carry the most
    /// distinct netids. Scoring every consecutive run and taking the best
    /// gets it right on 16.9 against the decoder's own answer.
    pub fn champion_run(&self, count: usize) -> Option<Vec<u32>> {
        if count == 0 {
            return None;
        }
        let ids: Vec<u32> = self.entities.keys().copied().collect();
        let end = self.minutes as f32 * 60.0;
        let mut best: Option<(usize, u32)> = None;
        for w in ids.windows(count) {
            if w[count - 1] - w[0] != count as u32 - 1 {
                continue;
            }
            let spans = w.iter().all(|id| {
                let e = &self.entities[id];
                e.first <= EDGE_WINDOW && e.last >= end - EDGE_WINDOW
            });
            if !spans {
                continue;
            }
            let score: usize = w.iter().map(|id| self.entities[id].netids.len()).sum();
            if best.map_or(true, |(b, _)| score > b) {
                best = Some((score, w[0]));
            }
        }
        best.map(|(_, start)| (0..count as u32).map(|i| start + i).collect())
    }

    /// Per-netid behaviour, busiest first. This is what a packet class
    /// looks like from outside its payload.
    ///
    /// `players` is the roster size, used to find the champions so that
    /// each class can report how much of its traffic is about them. That
    /// one number separates the classes that only ever concern champions
    /// (items, levels, scores) from the ones that sweep every unit.
    pub fn fingerprints(&self, players: usize) -> Vec<Fingerprint> {
        let champions: HashSet<u32> = self
            .champion_run(players)
            .unwrap_or_default()
            .into_iter()
            .collect();
        let total = self.blocks.max(1) as f64;
        let mut out: Vec<Fingerprint> = self
            .netids
            .iter()
            .map(|(netid, n)| {
                let entities = n.params.iter().filter(|p| **p >= ENTITY_ID_BASE).count();
                let champion = n.params.iter().filter(|p| champions.contains(p)).count();
                let (top1, top8) = byte_concentration(&n.byte_counts);
                Fingerprint {
                    netid: *netid,
                    blocks: n.blocks,
                    share: n.blocks as f64 / total,
                    top1,
                    top8,
                    length: Length::of(n),
                    global_share: n.global as f64 / n.blocks.max(1) as f64,
                    entities,
                    champion_share: champion as f64 / entities.max(1) as f64,
                    first: n.first,
                    last: n.last,
                    shape: shape(&n.per_minute),
                }
            })
            .collect();
        out.sort_by(|a, b| b.blocks.cmp(&a.blocks).then(a.netid.cmp(&b.netid)));
        out
    }
}

/// What a class's payload length looks like across the replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Length {
    /// Every block the same size.
    Fixed(usize),
    /// A handful of sizes, listed.
    Few(Vec<usize>),
    /// Many sizes: min and max.
    Variable(usize, usize),
}

impl Length {
    /// The sizes themselves, for comparing two classes.
    fn sizes(&self) -> Vec<usize> {
        match self {
            Length::Fixed(n) => vec![*n],
            Length::Few(v) => v.clone(),
            Length::Variable(a, b) => vec![*a, *b],
        }
    }

    fn of(n: &NetId) -> Length {
        let mut sizes: Vec<usize> = n.lengths.keys().copied().collect();
        sizes.sort_unstable();
        match sizes.len() {
            0 => Length::Fixed(0),
            1 => Length::Fixed(sizes[0]),
            _ if sizes.len() < LENGTH_CAP => Length::Few(sizes),
            _ => Length::Variable(sizes[0], *sizes.last().unwrap_or(&0)),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Length::Fixed(n) => format!("{n}"),
            Length::Few(v) => v.iter().map(usize::to_string).collect::<Vec<_>>().join("/"),
            Length::Variable(a, b) => format!("{a}-{b}"),
        }
    }
}

/// One packet class as framing sees it: no payload, no class name.
#[derive(Debug, Clone)]
pub struct Fingerprint {
    pub netid: u16,
    pub blocks: u64,
    pub share: f64,
    /// Share of all this class's payload bytes taken by its single
    /// commonest byte value, and by its commonest eight.
    ///
    /// This says how a class encodes, not what it means. A class that
    /// delta-codes paths spends most of its payload on small signed
    /// numbers that are usually the same one, so a single byte value
    /// covers a fifth of everything it ever sends; a class carrying
    /// floats and hashes spreads out. Measured: the waypoint class is
    /// 20.3 % on 16.9 and its 16.19 counterpart 23.8 %, against 6.0 %
    /// for the death class.
    pub top1: f64,
    pub top8: f64,
    pub length: Length,
    /// Share of blocks whose `param` is not an entity: the class is
    /// addressed to the game rather than to a unit.
    pub global_share: f64,
    pub entities: usize,
    /// Of the entities this class ever named, the share that were
    /// champions. 1.0 means it only ever talks about champions.
    pub champion_share: f64,
    pub first: f32,
    pub last: f32,
    pub shape: &'static str,
}

/// How a class's traffic is spread over the game.
fn shape(per_minute: &[u64]) -> &'static str {
    let live: Vec<u64> = per_minute.iter().copied().filter(|n| *n > 0).collect();
    if live.len() <= 1 {
        return "one minute";
    }
    let total: u64 = live.iter().sum();
    let mean = total as f64 / per_minute.len() as f64;
    let peak = *live.iter().max().unwrap_or(&0) as f64;
    let covered = live.len() as f64 / per_minute.len() as f64;
    match () {
        _ if covered < 0.3 => "bursts",
        _ if peak > mean * 4.0 => "spiky",
        _ if covered > 0.9 => "steady",
        _ => "uneven",
    }
}

/// Build the exploration JSON for one replay.
/// Team ids of the roster, in order.
pub fn roster_teams(replay: &Replay<'_>) -> Vec<u8> {
    replay
        .metadata
        .players
        .iter()
        .map(|p| match p.team {
            Team::Blue => TEAM_ORDER,
            Team::Red => TEAM_CHAOS,
        })
        .collect()
}

pub fn explore(replay: &Replay<'_>) -> Result<Value> {
    let f = framing(replay)?;
    let meta = &replay.metadata;
    let run = f.champion_run(meta.players.len());

    let teams = roster_teams(replay);
    let solved = match &run {
        Some(run) => solve_visibility(replay, &f, run, &teams)?,
        None => Vec::new(),
    };
    let windows = match &run {
        Some(run) if !solved.is_empty() => vision_windows(replay, &solved, run)?,
        _ => Vec::new(),
    };
    let game_len = (meta.game_length_ms as f32 / 1000.0).max(1.0);

    let players: Vec<Value> = meta
        .players
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let id = run.as_ref().and_then(|r| r.get(i).copied());
            let e = id.and_then(|id| f.entities.get(&id));
            let w = windows.get(i);
            let seen: f32 = w.map_or(0.0, |w| w.iter().map(|(a, b)| b - a).sum());
            json!({
                "index": i,
                "champion": p.skin,
                "team": team_label(p.team),
                "role": role_label(p.role),
                "win": p.win,
                "net_id": id.map(|id| format!("0x{id:08x}")),
                "blocks": e.map(|e| e.blocks),
                "distinct_netids": e.map(|e| e.netids.len()),
                "blocks_per_minute": e.map(|e| padded(&e.per_minute, f.minutes)),
                "seen_by_enemy_s": w.map(|_| (f64::from(seen) * 10.0).round() / 10.0),
                "seen_by_enemy_share": w.map(|_| (f64::from(seen / game_len) * 1000.0).round() / 1000.0),
                "vision_windows": w.map(Vec::len),
            })
        })
        .collect();

    let netids: Vec<Value> = f
        .fingerprints(meta.players.len())
        .into_iter()
        .map(|p| {
            json!({
                "netid": p.netid,
                "blocks": p.blocks,
                "share": (p.share * 10_000.0).round() / 10_000.0,
                "payload_bytes": p.length.label(),
                "commonest_byte_share": (p.top1 * 1000.0).round() / 1000.0,
                "commonest_8_bytes_share": (p.top8 * 1000.0).round() / 1000.0,
                "addresses": if p.global_share > 0.5 { "the game" } else { "a unit" },
                "distinct_entities": p.entities,
                "first_s": (f64::from(p.first) * 10.0).round() / 10.0,
                "last_s": (f64::from(p.last) * 10.0).round() / 10.0,
                "shape": p.shape,
            })
        })
        .collect();

    Ok(json!({
        "patch": replay.header.patch(),
        "game_length_s": (meta.game_length_ms as f64 / 1000.0 * 10.0).round() / 10.0,
        "blocks": f.blocks,
        "minutes": f.minutes,
        "entities": {
            "total": f.entities.len(),
            "champions": run.as_ref().map(|r| {
                json!({
                    "first": format!("0x{:08x}", r[0]),
                    "last": format!("0x{:08x}", r[r.len() - 1]),
                    "found_by": "longest-lived consecutive run with the most distinct netids",
                })
            }),
        },
        "players": players,
        "decoded_without_the_binary": solved.iter().map(|s| json!({
            "netid": s.netid,
            "field": "team that gained or lost sight",
            "role": if s.is_enter { "enter" } else { "leave" },
            "payload_to_team": s.mapping.iter()
                .map(|(k, v)| (k.iter().map(|b| format!("{b:02x}")).collect::<String>(), *v))
                .collect::<BTreeMap<_, _>>(),
            "labelled_blocks": s.samples,
            "exceptions": s.exceptions,
        })).collect::<Vec<_>>(),
        "netids": netids,
        "note": "Framing only: no payload is decoded and no game binary is read. \
                 Every number here comes from a block's timestamp, netid, param \
                 and length, all of which are plaintext.",
    }))
}

fn padded(v: &[u64], minutes: usize) -> Vec<u64> {
    let mut out = v.to_vec();
    out.resize(minutes, 0);
    out
}

/// Propose a netid mapping from one patch to another by fingerprint.
///
/// Netids are renumbered every patch, so a class we named on a patch we
/// can decode has a different number on one we cannot. What does not
/// change much is how the class behaves: how often it fires, how long
/// its payload is, whether it addresses a unit or the game, and when in
/// the game it runs. Matching on that gives candidates, not facts, and
/// the score says how much of a candidate each one is.
pub fn match_patches(target: &[Fingerprint], reference: &[Fingerprint]) -> Vec<Match> {
    // Score every pair, then take them best first. Walking the target
    // list in order instead lets an early class take a reference that a
    // later one needed far more.
    let mut pairs: Vec<(f64, u16, u16)> = Vec::with_capacity(target.len() * reference.len());
    for t in target {
        for r in reference {
            pairs.push((similarity(t, r), t.netid, r.netid));
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));

    // The margin is against the best *other* reference for the same
    // class, whether or not that one is still free: two classes that
    // look alike should both come out unconfident, not just the loser.
    let mut best_two: HashMap<u16, (f64, f64)> = HashMap::new();
    for (score, t, _) in &pairs {
        let e = best_two.entry(*t).or_insert((f64::MIN, f64::MIN));
        if *score > e.0 {
            e.1 = e.0;
            e.0 = *score;
        } else if *score > e.1 {
            e.1 = *score;
        }
    }

    let mut taken_t: HashSet<u16> = HashSet::new();
    let mut taken_r: HashSet<u16> = HashSet::new();
    let mut out = Vec::new();
    for (score, t, r) in pairs {
        if !taken_t.insert(t) {
            continue;
        }
        if !taken_r.insert(r) {
            taken_t.remove(&t);
            continue;
        }
        let (first, second) = best_two.get(&t).copied().unwrap_or((score, 0.0));
        let margin = first - second.max(0.0);
        out.push(Match {
            netid: t,
            reference_netid: r,
            score,
            margin,
            confident: score >= CONFIDENT_SCORE
                && margin >= CONFIDENT_MARGIN
                && (score - first).abs() < 1e-9,
        });
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out
}

/// One proposed netid correspondence.
#[derive(Debug, Clone)]
pub struct Match {
    pub netid: u16,
    pub reference_netid: u16,
    pub score: f64,
    /// How far ahead of the next candidate this one was. A match with no
    /// margin is a coin toss between two classes that look alike.
    pub margin: f64,
    pub confident: bool,
}

/// How alike two classes look from outside their payloads, in [0, 1].
///
/// Payload length does most of the work. The set of lengths a class
/// emits is the shape of its fields: one that is 2 bytes on every block
/// has a single small field, one that emits 5/6/7/10/11 has a fixed head
/// and a short optional tail. Riot renumbers netids every patch but
/// rarely rewrites a class, so that set survives where the number does
/// not.
fn similarity(a: &Fingerprint, b: &Fingerprint) -> f64 {
    let length = jaccard(&a.length.sizes(), &b.length.sizes());
    let kind = f64::from(std::mem::discriminant(&a.length) == std::mem::discriminant(&b.length));
    let share = 1.0 - ratio_gap(a.share, b.share);
    let champions = 1.0 - (a.champion_share - b.champion_share).abs();
    let addressing = 1.0 - (a.global_share - b.global_share).abs();
    let entities = 1.0 - ratio_gap(a.entities as f64, b.entities as f64);
    let shape = f64::from(a.shape == b.shape);
    let bytes = 1.0 - ((a.top1 - b.top1).abs() + (a.top8 - b.top8).abs()) / 2.0;
    0.34 * length
        + 0.04 * kind
        + 0.17 * share
        + 0.13 * champions
        + 0.09 * addressing
        + 0.04 * entities
        + 0.04 * shape
        + 0.15 * bytes
}

/// Share of payload bytes taken by the commonest value, and by the
/// commonest eight.
fn byte_concentration(counts: &[u64]) -> (f64, f64) {
    let total: u64 = counts.iter().sum();
    if total == 0 {
        return (0.0, 0.0);
    }
    let mut sorted: Vec<u64> = counts.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let top1 = sorted[0] as f64 / total as f64;
    let top8 = sorted.iter().take(8).sum::<u64>() as f64 / total as f64;
    (top1, top8)
}

/// Overlap of two sets of payload lengths, in [0, 1].
fn jaccard(a: &[usize], b: &[usize]) -> f64 {
    let a: HashSet<usize> = a.iter().copied().collect();
    let b: HashSet<usize> = b.iter().copied().collect();
    let union = a.union(&b).count();
    if union == 0 {
        return 1.0;
    }
    a.intersection(&b).count() as f64 / union as f64
}

/// Distance between two positive quantities on a log scale, clamped to
/// [0, 1]: 0 when equal, 1 when they differ by 10x or more.
fn ratio_gap(a: f64, b: f64) -> f64 {
    let (a, b) = (a.max(1e-9), b.max(1e-9));
    ((a / b).log10().abs()).min(1.0)
}

/// Recover part of a class's substitution table from a byte that moves
/// slowly through the game.
///
/// The movement class carries each unit's position as two 16-bit
/// values, and a unit crosses a 256-unit boundary rarely — measured on
/// 16.19, the high byte of a champion's coordinate is unchanged between
/// consecutive packets 92 % of the time. So when it *does* change it
/// almost always changes by one, and the pairs of cipher bytes seen
/// changing into one another lay the plaintext alphabet out in order.
///
/// Walking that chain from a known entry recovers its neighbours, then
/// theirs. The anchor is `T(0x00)`, which the class gives up for free
/// in three places (a coordinate's high byte when the unit is mid-map,
/// and two always-zero bytes of the unit net id). The direction is
/// fixed by `T(0x01)`, and checked against the flags byte, which the
/// format allows to be only 0 or 2.
///
/// What this does *not* give is the low byte of a coordinate. A unit
/// moves a few hundred units between packets, which is most of a low
/// byte, so consecutive low bytes are not neighbours and no chain
/// forms. Positions therefore come out to within a high byte — about
/// 512 map units — and no closer.
#[derive(Debug, Clone)]
pub struct ByteChain {
    /// Plaintext byte to payload byte, for the stretch of the alphabet
    /// the chain reached.
    pub table: BTreeMap<u8, u8>,
    /// Consecutive pairs the walk was built from.
    pub observations: u64,
    /// Share of consecutive pairs where the byte did not change at all.
    pub unchanged: f64,
}

/// One observation: the same slowly-moving byte at two consecutive
/// moments, for the same unit.
pub type BytePair = (u8, u8);

/// A unit's slowly-moving bytes over time, keyed by replay and by the
/// cipher of the unit's net id — which is enough to group a unit's own
/// packets together without being able to read the id.
pub type UnitTracks = HashMap<(usize, u8, u8), Vec<(f32, u8, u8)>>;

/// Build the chain outward from `anchor`, whose plaintext is 0, with
/// `step` as the cipher for plaintext 1.
pub fn byte_chain(pairs: &[BytePair], anchor: u8, step: u8) -> ByteChain {
    let mut adjacent: HashMap<u8, HashMap<u8, u64>> = HashMap::new();
    let mut unchanged = 0u64;
    for (a, b) in pairs {
        if a == b {
            unchanged += 1;
            continue;
        }
        *adjacent.entry(*a).or_default().entry(*b).or_default() += 1;
        *adjacent.entry(*b).or_default().entry(*a).or_default() += 1;
    }
    let mut table: BTreeMap<u8, u8> = BTreeMap::from([(0, anchor)]);
    let mut taken: HashSet<u8> = HashSet::from([anchor]);

    // Walk up from the anchor, then down, always taking the strongest
    // neighbour we have not used. An edge seen only a handful of times
    // is as likely to be a unit teleporting as a step of one.
    for (first, up) in [(Some(step), true), (None, false)] {
        let strong = |from: u8, to: u8| {
            adjacent.get(&from).and_then(|m| m.get(&to)).copied().unwrap_or(0) >= CHAIN_MIN
        };
        let mut cur = match first {
            // Even the caller's own direction hint has to be attested.
            Some(c) if strong(anchor, c) => c,
            Some(_) => continue,
            None => {
                let best = adjacent
                    .get(&anchor)
                    .into_iter()
                    .flatten()
                    .filter(|(c, n)| !taken.contains(*c) && **n >= CHAIN_MIN)
                    .max_by_key(|(_, n)| **n);
                match best {
                    Some((c, _)) => *c,
                    None => continue,
                }
            }
        };
        let mut prev = anchor;
        let mut value: i32 = if up { 1 } else { -1 };
        loop {
            if !taken.insert(cur) {
                break;
            }
            table.insert((value & 0xFF) as u8, cur);
            let next = adjacent
                .get(&cur)
                .into_iter()
                .flatten()
                .filter(|(c, n)| **c != prev && !taken.contains(*c) && **n >= CHAIN_MIN)
                .max_by_key(|(_, n)| **n)
                .map(|(c, _)| *c);
            match next {
                Some(c) => {
                    prev = cur;
                    cur = c;
                    value += if up { 1 } else { -1 };
                }
                None => break,
            }
        }
    }
    let total = pairs.len().max(1) as f64;
    ByteChain {
        table,
        observations: pairs.len() as u64,
        unchanged: unchanged as f64 / total,
    }
}

/// Times a pair of cipher bytes must be seen changing into one another
/// before the step is believed to be one.
const CHAIN_MIN: u64 = 15;

/// A class whose payload we can read on a patch we cannot decode.
///
/// The payload encoding is not a cipher. Measured on 16.9, where both
/// the raw payload and the decoded object are in hand, a class's
/// payload is its fields run through one 256-entry byte substitution:
/// the same plaintext byte always becomes the same payload byte, at any
/// position, in every block of every game, and sibling classes share
/// the table. `PKT_S2C_OnEnterTeamVisibility_s` and its leave twin emit
/// exactly two payloads on 16.9, `c719` and `c7c1`, for team 1 and team
/// 2.
///
/// So a class whose plaintext we can pin down from outside is readable
/// without the game's code. The visibility pair is the clean case: the
/// payload is two fixed bytes, the only varying field is the team, and
/// the game guarantees which team it is, because a unit is always
/// visible to its own side and a visibility change can therefore only
/// ever name the other one. The champions' teams are known from the
/// metadata, so every block whose `param` is a champion is a labelled
/// sample, and one block landing on the wrong side falsifies the whole
/// mapping.
#[derive(Debug, Clone)]
pub struct Solved {
    pub netid: u16,
    /// Payload bytes to the team they mean.
    pub mapping: BTreeMap<Vec<u8>, u8>,
    /// Labelled champion blocks the mapping was read off and checked on.
    pub samples: u64,
    /// Labelled blocks that contradict it. Anything but 0 is a refusal.
    pub exceptions: u64,
    /// True for the class that opens a visibility window.
    pub is_enter: bool,
}

/// Recover the team byte of the visibility classes from the file alone.
///
/// Candidates are the classes with a fixed 2-byte payload that address
/// units and name many of them. For each, every block whose `param` is
/// a champion votes: this payload means the team opposite that
/// champion's. A payload that ever votes both ways is thrown out, and
/// so is the whole netid.
///
/// Which of the pair opens a window is settled by the first visibility
/// event each champion ever gets: you cannot leave a vision you never
/// entered.
pub fn solve_visibility(
    replay: &Replay<'_>,
    f: &Framing,
    champions: &[u32],
    teams: &[u8],
) -> Result<Vec<Solved>> {
    let candidates: Vec<u16> = f
        .netids
        .iter()
        .filter(|(_, n)| {
            matches!(Length::of(n), Length::Fixed(2))
                && n.global * 2 < n.blocks
                && n.params.len() > champions.len()
        })
        .map(|(netid, _)| *netid)
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let team_of: HashMap<u32, u8> = champions.iter().copied().zip(teams.iter().copied()).collect();
    let mut votes: HashMap<(u16, Vec<u8>), HashMap<u8, u64>> = HashMap::new();
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
            if !candidates.contains(&block.packet_id) {
                continue;
            }
            let Some(own) = team_of.get(&block.param) else {
                continue;
            };
            // The team that gained or lost sight is the other one.
            let other = if *own == TEAM_ORDER { TEAM_CHAOS } else { TEAM_ORDER };
            *votes
                .entry((block.packet_id, block.payload.to_vec()))
                .or_default()
                .entry(other)
                .or_default() += 1;
        }
    }

    // Which of the pair opens a window. Every object that becomes
    // visible emits an enter, but a great many are destroyed while
    // still visible and never emit a leave, so across all entities
    // enter outnumbers leave heavily -- on 16.9, 3,091,816 against
    // 42,014. Timestamps do not settle it: both classes fire at t=0
    // and the champion's earliest event splits 6-4 between them.
    let enter = candidates
        .iter()
        .max_by_key(|n| f.netids.get(n).map_or(0, |n| n.blocks))
        .copied();

    let mut out = Vec::new();
    for netid in candidates {
        let mut mapping = BTreeMap::new();
        let mut samples = 0u64;
        let mut exceptions = 0u64;
        for ((n, payload), by_team) in &votes {
            if *n != netid {
                continue;
            }
            let total: u64 = by_team.values().sum();
            let (team, best) = by_team.iter().max_by_key(|(_, n)| **n).unwrap();
            samples += total;
            exceptions += total - best;
            mapping.insert(payload.clone(), *team);
        }
        // Two payloads for two teams, every labelled block agreeing.
        let teams_named: HashSet<u8> = mapping.values().copied().collect();
        if mapping.len() < 2 || teams_named.len() != mapping.len() || samples < MIN_LABELLED {
            continue;
        }
        out.push(Solved {
            netid,
            mapping,
            samples,
            exceptions,
            is_enter: Some(netid) == enter,
        });
    }
    out.retain(|s| s.exceptions == 0);
    out.sort_by_key(|s| s.netid);
    Ok(out)
}

/// Labelled champion blocks a solved mapping must rest on.
const MIN_LABELLED: u64 = 200;

/// Per champion, the stretches spent inside the other team's vision,
/// read off a solved visibility pair.
///
/// This is the same timeline `game.rs` builds on a patch it can decode,
/// built here with no binary and no deserializer: the entity is the
/// block's `param`, and the team comes from the solved payload.
pub fn vision_windows(
    replay: &Replay<'_>,
    solved: &[Solved],
    champions: &[u32],
) -> Result<Vec<Vec<(f32, f32)>>> {
    let enter: HashSet<u16> = solved.iter().filter(|s| s.is_enter).map(|s| s.netid).collect();
    let leave: HashSet<u16> = solved.iter().filter(|s| !s.is_enter).map(|s| s.netid).collect();
    if enter.is_empty() || leave.is_empty() {
        return Ok(Vec::new());
    }
    let index: HashMap<u32, usize> = champions.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut open: Vec<Option<f32>> = vec![None; champions.len()];
    let mut out: Vec<Vec<(f32, f32)>> = vec![Vec::new(); champions.len()];
    let mut last = 0.0f32;
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
            last = last.max(block.timestamp);
            let Some(i) = index.get(&block.param).copied() else {
                continue;
            };
            if enter.contains(&block.packet_id) {
                open[i].get_or_insert(block.timestamp);
            } else if leave.contains(&block.packet_id) {
                if let Some(start) = open[i].take() {
                    out[i].push((start, block.timestamp));
                }
            }
        }
    }
    for (i, start) in open.into_iter().enumerate() {
        if let Some(start) = start {
            out[i].push((start, last));
        }
    }
    Ok(out)
}

/// One replay's input to [`solve_netid_field`]: the payloads of the
/// class in question, the champions' net ids in roster order, and the
/// statsJson counter the field is supposed to explain.
pub type FieldInput = (Vec<Vec<u8>>, Vec<u32>, Vec<i64>);

/// A net-id field inside a payload, read without the game's code.
#[derive(Debug, Clone)]
pub struct SolvedNetIdField {
    pub netid: u16,
    /// Bytes the class writes after this field. The field itself is
    /// found by counting back from the end of the payload: the net
    /// id's varint groups, then this.
    pub tail: usize,
    /// Payload byte to the varint byte it stands for. This is a slice
    /// of the class's substitution table, at the net ids the rosters
    /// happened to use.
    pub table: BTreeMap<u8, u8>,
    pub replays: usize,
    pub players_checked: usize,
    pub players_right: usize,
}

/// Net ids are stored XORed with 0x40000000 unless their low 24 bits
/// are zero.
fn mask(net_id: u32) -> u32 {
    if net_id & 0x00FF_FFFF != 0 {
        net_id ^ 0x4000_0000
    } else {
        net_id
    }
}

/// The first byte a net id's varint writes.
///
/// A varint byte is its 7-bit group plus a continuation bit when more
/// groups follow, and that whole byte is what the substitution table
/// sees. The bit matters: net id 0x34 is written as the single byte
/// 0x34, while 0xb4 is written as 0xb4 then 0x01, and the two first
/// bytes share their low 7 bits but are different plaintext. Measured
/// on 16.19, those two cases do map to different payload bytes -- 0x34
/// to 0x94, 0xb4 to 0x56 -- so the bit is in the table's key, not
/// stripped from it.
fn first_varint_byte(net_id: u32) -> u8 {
    let m = mask(net_id);
    if m >= 0x80 {
        ((m & 0x7F) | 0x80) as u8
    } else {
        m as u8
    }
}

/// How many 7-bit groups a net id's varint takes.
fn groups(net_id: u32) -> usize {
    let mut v = mask(net_id) >> 7;
    let mut n = 1;
    while v != 0 {
        v >>= 7;
        n += 1;
    }
    n
}

/// Where in the payload a roster's net ids put their first varint byte.
///
/// Net ids go into the stream as a 7-bit varint, **low group first**,
/// so a field naming a unit ends with that unit's higher groups and the
/// byte we want sits that many places from the end. A roster is not
/// always small: ten champions at 0x34..0x3d take one varint byte each
/// and the byte is last, ten at 0xae..0xb7 take two and it is
/// second-to-last, with a constant byte after it because the high group
/// of every id in that range is 1. Deriving the offset from the roster
/// beats searching for it, and the two cases cross-check, since those
/// rosters overlap at low groups 0x34..0x37.
///
/// `tail` is how many bytes the class writes after the field; 0 when
/// the field is the last thing in the payload.
fn first_byte_position(ids: &[u32], tail: usize) -> Option<i32> {
    let n = groups(*ids.first()?);
    ids.iter().all(|id| groups(*id) == n).then(|| -((n + tail) as i32))
}

/// Find the byte of a class's payload that names a champion, by making
/// it account for one of Riot's end-of-game counters.
///
/// Worked on the champion-death class: it fires once per death, and the
/// byte that says who got the kill must, across the whole game, name
/// each player exactly `CHAMPIONS_KILLED` times. Scanning byte
/// positions and keeping the one where that holds finds the field; the
/// byte values then map to the champions, which is a slice of the
/// class's substitution table.
///
/// Two things make this a proof rather than a fit. The assignment is
/// only taken from a replay where it is *forced* — every champion's
/// count matching exactly one byte value, no value serving two
/// champions. And the table learned that way is then used to *predict*
/// the counts in the other replays, including ones that were ambiguous
/// on their own.
pub fn solve_netid_field(netid: u16, per_replay: &[FieldInput]) -> Option<SolvedNetIdField> {
    let mut best: Option<SolvedNetIdField> = None;
    for tail in 0..TAIL_MAX {
        // Learn only where the assignment is forced, then feed what is
        // known back in: a replay that is ambiguous on its own often
        // has only one unsolved champion left once the others are
        // pinned, and solving that one pins a further byte. Repeat
        // until nothing new falls out.
        let mut table: BTreeMap<u8, u8> = BTreeMap::new();
        let mut known: BTreeMap<u8, u8> = BTreeMap::new();
        let mut broken = false;
        loop {
            let before = table.len();
            for (payloads, ids, counter) in per_replay {
                let Some(position) = first_byte_position(ids, tail) else {
                    continue;
                };
                let Some(pairs) = forced(payloads, ids, counter, position, &known) else {
                    continue;
                };
                for (plain, cipher) in pairs {
                    if table.get(&cipher).is_some_and(|p| *p != plain)
                        || known.get(&plain).is_some_and(|c| *c != cipher)
                    {
                        broken = true;
                        break;
                    }
                    table.insert(cipher, plain);
                    known.insert(plain, cipher);
                }
                if broken {
                    break;
                }
            }
            if broken || table.len() == before {
                break;
            }
        }
        if broken || table.is_empty() {
            continue;
        }
        // Predict every replay the table covers and score it.
        let by_plain: BTreeMap<u8, u8> = table.iter().map(|(c, p)| (*p, *c)).collect();
        let (mut checked, mut right, mut replays) = (0usize, 0usize, 0usize);
        for (payloads, ids, counter) in per_replay {
            let wanted: Option<Vec<u8>> = ids
                .iter()
                .map(|id| by_plain.get(&first_varint_byte(*id)).copied())
                .collect();
            let (Some(wanted), Some(position)) = (wanted, first_byte_position(ids, tail)) else {
                continue;
            };
            let seen = counts(payloads, position);
            replays += 1;
            for (byte, truth) in wanted.iter().zip(counter) {
                checked += 1;
                right += usize::from(i64::from(seen.get(byte).copied().unwrap_or(0)) == *truth);
            }
        }
        let score = (right, checked);
        if checked >= MIN_PREDICTIONS
            && right * 100 >= checked * FIELD_PRECISION
            && best.as_ref().map_or(true, |b| (b.players_right, b.players_checked) < score)
        {
            best = Some(SolvedNetIdField {
                netid,
                tail,
                table,
                replays,
                players_checked: checked,
                players_right: right,
            });
        }
    }
    best
}

/// How many bytes after the field we are willing to assume the class
/// writes. A payload whose length varies keeps its tail in place, not
/// its head, so everything is counted from the end.
const TAIL_MAX: usize = 8;
/// Player-games a solved field must be checked against.
const MIN_PREDICTIONS: usize = 50;
/// Percent of those it must get exactly right.
const FIELD_PRECISION: usize = 97;

fn byte_at(payload: &[u8], position: i32) -> Option<u8> {
    let i = if position < 0 {
        payload.len().checked_sub(position.unsigned_abs() as usize)?
    } else {
        position as usize
    };
    payload.get(i).copied()
}

fn counts(payloads: &[Vec<u8>], position: i32) -> HashMap<u8, u32> {
    let mut out: HashMap<u8, u32> = HashMap::new();
    for p in payloads {
        if let Some(b) = byte_at(p, position) {
            *out.entry(b).or_default() += 1;
        }
    }
    out
}

/// (masked net id, payload byte) pairs this replay leaves no choice
/// about, given what is already `known` (masked net id to payload
/// byte).
///
/// A champion already in `known` has to match its count or the whole
/// replay is rejected; it then stops competing for the remaining
/// counts, which is what lets a second pass solve the rest.
fn forced(
    payloads: &[Vec<u8>],
    ids: &[u32],
    counter: &[i64],
    position: i32,
    known: &BTreeMap<u8, u8>,
) -> Option<Vec<(u8, u8)>> {
    let seen = counts(payloads, position);
    let mut taken: HashSet<u8> = HashSet::new();
    let mut unsolved: Vec<(u8, i64)> = Vec::new();
    for (id, want) in ids.iter().zip(counter) {
        let plain = first_varint_byte(*id);
        match known.get(&plain) {
            Some(byte) => {
                if i64::from(seen.get(byte).copied().unwrap_or(0)) != *want {
                    return None;
                }
                taken.insert(*byte);
            }
            None if *want == 0 => {}
            None => unsolved.push((plain, *want)),
        }
    }
    let mut by_count: HashMap<i64, Vec<u8>> = HashMap::new();
    for (byte, n) in &seen {
        if !taken.contains(byte) {
            by_count.entry(i64::from(*n)).or_default().push(*byte);
        }
    }
    // Two players on the same kill count cannot be told apart by it.
    // Take the ones that can be, and leave the rest for a later pass:
    // once their neighbours are pinned the tie usually breaks.
    let mut pairs = Vec::new();
    for (plain, want) in unsolved {
        if let Some([byte]) = by_count.get(&want).map(Vec::as_slice) {
            pairs.push((plain, *byte));
        }
    }
    let distinct: HashSet<u8> = pairs.iter().map(|(_, b)| *b).collect();
    (!pairs.is_empty() && distinct.len() == pairs.len()).then_some(pairs)
}

/// One replay's death blocks: timestamp, victim net id, payload.
pub type DeathBlocks = Vec<(f32, u32, Vec<u8>)>;

/// Kills read off a solved killer field: who killed whom, and when.
#[derive(Debug, Clone, Serialize)]
pub struct Kill {
    pub t: f32,
    pub victim: usize,
    pub killer: Option<usize>,
}

/// The kill timeline of one replay, from the solved field.
pub fn kills(
    payloads: &DeathBlocks,
    field: &SolvedNetIdField,
    champions: &[u32],
) -> Vec<Kill> {
    let index: HashMap<u8, usize> = champions
        .iter()
        .enumerate()
        .map(|(i, id)| (first_varint_byte(*id), i))
        .collect();
    let victim_of: HashMap<u32, usize> =
        champions.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let Some(position) = first_byte_position(champions, field.tail) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (t, param, payload) in payloads {
        let Some(victim) = victim_of.get(param).copied() else {
            continue;
        };
        let killer = byte_at(payload, position)
            .and_then(|b| field.table.get(&b))
            .and_then(|plain| index.get(plain))
            .copied();
        out.push(Kill { t: *t, victim, killer });
    }
    out.sort_by(|a, b| a.t.total_cmp(&b.t));
    out
}

/// Every block of one netid, as (timestamp, param, payload). Framing
/// only, so this is available on a patch whose payloads are opaque.
pub fn blocks_of(replay: &Replay<'_>, netid: u16) -> Result<DeathBlocks> {
    let mut out = Vec::new();
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
            if block.packet_id == netid {
                out.push((block.timestamp, block.param, block.payload.to_vec()));
            }
        }
    }
    Ok(out)
}

/// Every block addressed to a champion: netid, which champion, when.
pub type ChampionBlocks = Vec<(u16, usize, f32)>;

/// One replay's timeline, for [`event_locked`].
pub struct Timeline {
    pub blocks: ChampionBlocks,
    /// Per champion, the times they died.
    pub deaths: Vec<Vec<f32>>,
    /// Per champion, the times they got a kill.
    pub kills: Vec<Vec<f32>>,
    pub length_s: f32,
}

/// Collect every block whose `param` is one of the champions.
pub fn champion_blocks(replay: &Replay<'_>, champions: &[u32]) -> Result<ChampionBlocks> {
    let index: HashMap<u32, usize> =
        champions.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut out = Vec::new();
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
            if let Some(i) = index.get(&block.param) {
                out.push((block.packet_id, *i, block.timestamp));
            }
        }
    }
    Ok(out)
}

/// A class whose traffic about a champion happens when something
/// specific happens to that champion.
#[derive(Debug, Clone)]
pub struct EventLocked {
    pub netid: u16,
    /// "their own death" or "a kill they got".
    pub event: &'static str,
    pub blocks: u64,
    pub near: u64,
    pub share: f64,
    /// How much likelier than chance. A class that fires steadily all
    /// game lands near an event sometimes; one that is *about* the
    /// event lands there every time, and the ratio says which.
    pub lift: f64,
}

/// Find the classes locked to deaths and to kills.
///
/// The kill feed (§ 3 of `docs/EXPLORE.md`) gives, for every champion,
/// when they died and when they got a kill. Any other class that only
/// ever talks about a champion at those moments is about that event —
/// a death timer, a grey screen, a score update. This needs no payload
/// from the class being identified: only when it fires, and about whom.
///
/// The comparison is against chance, not against zero. The windows
/// around a champion's deaths cover some fraction of the game, so a
/// class firing steadily will land in them that often by accident;
/// `lift` is how many times more than that it actually does.
pub fn event_locked(games: &[Timeline]) -> Vec<EventLocked> {
    let mut out = Vec::new();
    for (event, pick) in [
        ("their own death", 0usize),
        ("a kill they got", 1usize),
        ("any kill in the game", 2usize),
    ] {
        let mut blocks: HashMap<u16, u64> = HashMap::new();
        let mut near: HashMap<u16, u64> = HashMap::new();
        let mut expected: HashMap<u16, f64> = HashMap::new();
        for g in games {
            // Everything a kill touches: the victim, the killer, and
            // everyone who assisted. A class that reacts to the kill
            // rather than to one role in it fires for all of them, so
            // it only shows up against the whole game's kills.
            let all: Vec<f32> = {
                let mut v: Vec<f32> = g.deaths.iter().flatten().copied().collect();
                v.sort_by(f32::total_cmp);
                v
            };
            let per_champion: Vec<&Vec<f32>> = match pick {
                0 => g.deaths.iter().collect(),
                1 => g.kills.iter().collect(),
                _ => (0..g.deaths.len()).map(|_| &all).collect(),
            };
            // Share of the game each champion's windows cover.
            let cover: Vec<f64> = per_champion
                .iter()
                .map(|ts| window_cover(ts) / f64::from(g.length_s.max(1.0)))
                .collect();
            for (netid, champion, t) in &g.blocks {
                let Some(times) = per_champion.get(*champion) else {
                    continue;
                };
                *blocks.entry(*netid).or_default() += 1;
                *expected.entry(*netid).or_default() += cover[*champion];
                if times.iter().any(|d| (d - t).abs() <= EVENT_WINDOW) {
                    *near.entry(*netid).or_default() += 1;
                }
            }
        }
        for (netid, n) in &blocks {
            let hit = near.get(netid).copied().unwrap_or(0);
            let share = hit as f64 / *n as f64;
            let base = expected.get(netid).copied().unwrap_or(0.0) / *n as f64;
            let lift = if base > 0.0 { share / base } else { 0.0 };
            if *n >= MIN_LOCKED_BLOCKS && share >= LOCKED_SHARE && lift >= LOCKED_LIFT {
                out.push(EventLocked {
                    netid: *netid,
                    event,
                    blocks: *n,
                    near: hit,
                    share,
                    lift,
                });
            }
        }
    }
    out.sort_by(|a, b| b.lift.total_cmp(&a.lift).then(b.blocks.cmp(&a.blocks)));
    out
}

/// Seconds either side of an event a block is counted as part of it.
const EVENT_WINDOW: f32 = 1.0;
/// Champion-addressed blocks a class needs before this means anything.
const MIN_LOCKED_BLOCKS: u64 = 100;
/// Share of them that must land on an event.
const LOCKED_SHARE: f64 = 0.90;
/// And how many times likelier than chance that has to be.
const LOCKED_LIFT: f64 = 4.0;

/// Seconds covered by the windows around `times`, overlaps counted once.
fn window_cover(times: &[f32]) -> f64 {
    let mut spans: Vec<(f32, f32)> =
        times.iter().map(|t| (t - EVENT_WINDOW, t + EVENT_WINDOW)).collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut total = 0.0f64;
    let mut cur: Option<(f32, f32)> = None;
    for (a, b) in spans {
        match cur {
            Some((s, e)) if a <= e => cur = Some((s, e.max(b))),
            Some((s, e)) => {
                total += f64::from(e - s);
                cur = Some((a, b));
            }
            None => cur = Some((a, b)),
        }
    }
    if let Some((s, e)) = cur {
        total += f64::from(e - s);
    }
    total
}

/// Blocks per champion for every netid, in roster order.
///
/// This is the quantity that can be checked against Riot's own
/// end-of-game record, which is the only ground truth a packed patch
/// offers.
pub fn champion_counts(f: &Framing, champions: &[u32]) -> BTreeMap<u16, Vec<u64>> {
    let mut out: BTreeMap<u16, Vec<u64>> = BTreeMap::new();
    for (netid, n) in &f.netids {
        let row: Vec<u64> = champions
            .iter()
            .map(|id| n.per_param.get(id).copied().unwrap_or(0))
            .collect();
        if row.iter().any(|n| *n > 0) {
            out.insert(*netid, row);
        }
    }
    out
}

/// One replay's input to [`match_stats`]: blocks per champion for each
/// netid, and Riot's end-of-game counters for each player.
pub type StatsRow = (BTreeMap<u16, Vec<u64>>, BTreeMap<String, Vec<i64>>);

/// A netid identified by Riot's own end-of-game numbers.
#[derive(Debug, Clone)]
pub struct StatsMatch {
    pub netid: u16,
    pub field: String,
    /// Blocks per champion minus the statsJson value, the same for
    /// every player. 0 is one block per counted thing; 1 usually means
    /// one extra at spawn.
    pub offset: i64,
    /// Replays where the identity held.
    pub replays: usize,
    /// Replays that could test it: the field varied between players and
    /// the netid fired.
    pub tested: usize,
    /// Distinct values the field took across the players, summed over
    /// the replays it matched. A field that is 0 for everyone matches
    /// everything and proves nothing.
    pub spread: usize,
}

/// Sums of statsJson counters that a single packet class can plausibly
/// fire once for.
///
/// A class is not always tied to one counter. The one that carries a
/// champion's running kill / death / assist totals fires whenever any
/// of the three changes, so its block count is their sum -- measured
/// exactly, on every player of every 16.9 replay, against the class
/// the decoder names `PKT_Update_HeroStats_KDA_s`. Without the sums a
/// matcher on single fields cannot see it at all.
///
/// The list is deliberately short. Every extra combination is another
/// chance for a coincidence to clear the test, so each one here has a
/// reason to exist rather than being a product of enumeration.
const DERIVED: &[(&str, &[&str])] = &[
    // One packet per change to any of the three.
    (
        "CHAMPIONS_KILLED+NUM_DEATHS+ASSISTS",
        &["CHAMPIONS_KILLED", "NUM_DEATHS", "ASSISTS"],
    ),
    // Lane and jungle creeps are both last hits.
    (
        "MINIONS_KILLED+NEUTRAL_MINIONS_KILLED",
        &["MINIONS_KILLED", "NEUTRAL_MINIONS_KILLED"],
    ),
    // A ward placed and a ward killed are both ward lifecycle events.
    ("WARD_PLACED+WARD_KILLED", &["WARD_PLACED", "WARD_KILLED"]),
    // Turrets and inhibitors are both structures.
    ("TURRETS_KILLED+BARRACKS_KILLED", &["TURRETS_KILLED", "BARRACKS_KILLED"]),
    // A cast is a cast, whichever of the four abilities it was.
    (
        "SPELL1..4_CAST",
        &["SPELL1_CAST", "SPELL2_CAST", "SPELL3_CAST", "SPELL4_CAST"],
    ),
    // Summoner spells may or may not go out the same way; try both.
    (
        "SPELL1..4_CAST+SUMMON_SPELL1..2_CAST",
        &[
            "SPELL1_CAST",
            "SPELL2_CAST",
            "SPELL3_CAST",
            "SPELL4_CAST",
            "SUMMON_SPELL1_CAST",
            "SUMMON_SPELL2_CAST",
        ],
    ),
];

/// Add [`DERIVED`] sums to a replay's counters, where every term is
/// present.
pub fn add_derived(counters: &mut BTreeMap<String, Vec<i64>>) {
    for (name, terms) in DERIVED {
        let columns: Option<Vec<&Vec<i64>>> = terms.iter().map(|t| counters.get(*t)).collect();
        let Some(columns) = columns else { continue };
        let n = columns[0].len();
        if columns.iter().any(|c| c.len() != n) {
            continue;
        }
        let sum: Vec<i64> = (0..n).map(|i| columns.iter().map(|c| c[i]).sum()).collect();
        counters.insert((*name).to_string(), sum);
    }
}

/// Netids whose per-champion block count *is* one of Riot's end-of-game
/// counters, in every replay.
///
/// A packed patch gives up no payloads, but it still carries statsJson,
/// and statsJson is hundreds of integers per player that the game
/// produced from the same events the packets describe. If a netid fires
/// exactly `NUM_DEATHS` times for every player in twenty games, that
/// netid is the death packet. No binary, no decoding, and not a
/// fingerprint guess: an identity that held ten players at a time,
/// twenty times over.
///
/// A field has to take at least [`MIN_SPREAD`] different values across
/// the ten players to count, because a column of zeroes matches any
/// netid that never fires.
pub fn match_stats(per_replay: &[StatsRow]) -> Vec<StatsMatch> {
    let mut hits: HashMap<(u16, String, i64), (usize, usize)> = HashMap::new();
    // A replay only tests a (netid, field) pair when the field actually
    // varies between players there and the netid fired at all. Scoring
    // against the replays where neither was true would punish a real
    // identity for a short game.
    let mut tested: HashMap<(u16, String), usize> = HashMap::new();
    for (counts, stats) in per_replay {
        for (netid, row) in counts {
            for (field, truth) in stats {
                if row.len() != truth.len() {
                    continue;
                }
                let spread = truth.iter().collect::<HashSet<_>>().len();
                if spread < MIN_SPREAD {
                    continue;
                }
                *tested.entry((*netid, field.clone())).or_default() += 1;
                let Some(offset) = uniform_offset(row, truth) else {
                    continue;
                };
                let e = hits.entry((*netid, field.clone(), offset)).or_default();
                e.0 += 1;
                e.1 += spread;
            }
        }
    }
    let mut out: Vec<StatsMatch> = hits
        .into_iter()
        .filter_map(|((netid, field, offset), (replays, spread))| {
            // Hold in every replay that could test it, bar one. Three
            // games is the floor: two agreeing proves little, and one
            // ragequit should not sink an identity that twenty games
            // otherwise confirm.
            let tested = tested.get(&(netid, field.clone())).copied().unwrap_or(0);
            (replays + 1 >= tested && tested >= MIN_TESTS).then_some(StatsMatch {
                netid,
                field,
                offset,
                replays,
                tested,
                spread,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.replays
            .cmp(&a.replays)
            .then(b.spread.cmp(&a.spread))
            .then(a.netid.cmp(&b.netid))
    });
    out
}

/// The single number that turns `truth` into `row` for every player, if
/// there is one.
fn uniform_offset(row: &[u64], truth: &[i64]) -> Option<i64> {
    let first = row.first()?.to_owned() as i64 - truth.first()?;
    if first.abs() > MAX_OFFSET {
        return None;
    }
    row.iter()
        .zip(truth)
        .all(|(n, t)| *n as i64 - t == first)
        .then_some(first)
}

/// How well fingerprint matching recovers a mapping it already knows.
///
/// Split one patch's replays in two, fingerprint each half from
/// different games, and match. The answer is known -- a netid is itself
/// -- so the share recovered is the method's accuracy under everything
/// except the one thing this cannot simulate, a class changing between
/// patches. Run it before believing any cross-patch proposal.
pub fn self_test(all: &[Framing], players: usize) -> SelfTest {
    let mid = all.len() / 2;
    let a = pool(&all[..mid], players);
    let b = pool(&all[mid..], players);
    let blocks: u64 = a.iter().map(|f| f.blocks).sum();
    let mut t = SelfTest {
        netids: a.len(),
        ..Default::default()
    };
    for m in match_patches(&a, &b) {
        let hit = m.netid == m.reference_netid;
        let w = a.iter().find(|f| f.netid == m.netid).map_or(0, |f| f.blocks);
        if hit {
            t.correct += 1;
            t.correct_blocks += w;
        }
        if m.confident {
            t.confident += 1;
            t.confident_correct += usize::from(hit);
        }
        t.calls.push((m.score, m.margin, hit));
    }
    t.block_share = t.correct_blocks as f64 / blocks.max(1) as f64;
    t
}

/// The result of [`self_test`].
#[derive(Debug, Default, Clone)]
pub struct SelfTest {
    pub netids: usize,
    pub correct: usize,
    pub correct_blocks: u64,
    /// Share of traffic, not of netids: getting the busiest classes
    /// right is worth more than getting a class that fires twice right.
    pub block_share: f64,
    pub confident: usize,
    pub confident_correct: usize,
    /// (score, margin, right) for every call, so a gate can be chosen
    /// from the data instead of by taste.
    pub calls: Vec<(f64, f64, bool)>,
}

impl SelfTest {
    /// Precision and reach of a grid of gates, for calibrating
    /// [`CONFIDENT_SCORE`] and [`CONFIDENT_MARGIN`].
    pub fn gates(&self) -> Vec<(f64, f64, usize, usize)> {
        let mut out = Vec::new();
        for score in [0.70, 0.80, 0.85, 0.90, 0.93, 0.95] {
            for margin in [0.0, 0.02, 0.05, 0.10, 0.15] {
                let kept: Vec<bool> = self
                    .calls
                    .iter()
                    .filter(|(s, m, _)| *s >= score && *m >= margin)
                    .map(|(_, _, hit)| *hit)
                    .collect();
                out.push((score, margin, kept.iter().filter(|h| **h).count(), kept.len()));
            }
        }
        out
    }
}

/// Pool fingerprints over several replays of one patch.
///
/// Every feature is merged across the replays, not taken from the first
/// one. Payload lengths especially: a class with an optional field only
/// shows the longer form in the games where it happened, so the union
/// over several games is the class's real shape and a single game's is
/// a sample of it.
pub fn pool(all: &[Framing], players: usize) -> Vec<Fingerprint> {
    let mut sizes: HashMap<u16, HashSet<usize>> = HashMap::new();
    let mut varied: HashSet<u16> = HashSet::new();
    let mut merged: BTreeMap<u16, (Fingerprint, u64)> = BTreeMap::new();
    let total: u64 = all.iter().map(|f| f.blocks).sum();
    for f in all {
        for p in f.fingerprints(players) {
            let set = sizes.entry(p.netid).or_default();
            set.extend(p.length.sizes());
            if matches!(p.length, Length::Variable(_, _)) {
                varied.insert(p.netid);
            }
            match merged.get_mut(&p.netid) {
                Some((have, seen)) => {
                    // Weight the proportions by traffic, keep the extremes.
                    let (w, v) = (have.blocks as f64, p.blocks as f64);
                    let mix = |a: f64, b: f64| (a * w + b * v) / (w + v).max(1.0);
                    have.global_share = mix(have.global_share, p.global_share);
                    have.champion_share = mix(have.champion_share, p.champion_share);
                    have.top1 = mix(have.top1, p.top1);
                    have.top8 = mix(have.top8, p.top8);
                    have.entities = have.entities.max(p.entities);
                    have.first = have.first.min(p.first);
                    have.last = have.last.max(p.last);
                    have.blocks += p.blocks;
                    *seen += 1;
                }
                None => {
                    merged.insert(p.netid, (p, 1));
                }
            }
        }
    }
    let mut out: Vec<Fingerprint> = merged
        .into_values()
        .map(|(mut p, _)| {
            let mut v: Vec<usize> = sizes.remove(&p.netid).unwrap_or_default().into_iter().collect();
            v.sort_unstable();
            p.length = match v.len() {
                0 => Length::Fixed(0),
                1 => Length::Fixed(v[0]),
                _ if v.len() < LENGTH_CAP && !varied.contains(&p.netid) => Length::Few(v),
                _ => Length::Variable(v[0], *v.last().unwrap_or(&0)),
            };
            p.share = p.blocks as f64 / total.max(1) as f64;
            p
        })
        .collect();
    out.sort_by(|a, b| b.blocks.cmp(&a.blocks).then(a.netid.cmp(&b.netid)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(netid: u16, blocks: u64, share: f64, length: Length) -> Fingerprint {
        Fingerprint {
            netid,
            blocks,
            share,
            length,
            global_share: 0.0,
            entities: 100,
            champion_share: 0.1,
            top1: 0.2,
            top8: 0.5,
            first: 0.0,
            last: 1000.0,
            shape: "steady",
        }
    }

    #[test]
    fn identical_classes_score_one() {
        let a = fp(1, 100, 0.4, Length::Fixed(2));
        let b = fp(2, 100, 0.4, Length::Fixed(2));
        assert!((similarity(&a, &b) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_different_fixed_length_costs_the_length_weight() {
        let a = fp(1, 100, 0.4, Length::Fixed(2));
        let b = fp(2, 100, 0.4, Length::Fixed(9));
        assert!((similarity(&a, &b) - 0.66).abs() < 1e-9);
    }

    #[test]
    fn byte_concentration_reads_the_histogram() {
        let mut counts = vec![0u64; 256];
        counts[0x42] = 50;
        for c in counts.iter_mut().take(9).skip(1) {
            *c = 5;
        }
        counts[200] = 10;
        let (top1, top8) = byte_concentration(&counts);
        assert!((top1 - 0.5).abs() < 1e-9);
        // 50, then 10, then six of the eight fives: 90 of 100 bytes.
        assert!((top8 - 0.90).abs() < 1e-9);
        assert_eq!(byte_concentration(&vec![0; 256]), (0.0, 0.0));
    }

    #[test]
    fn overlapping_length_sets_score_in_between() {
        assert!((jaccard(&[5, 6, 7], &[5, 6, 7]) - 1.0).abs() < 1e-9);
        assert!((jaccard(&[5, 6], &[5, 6, 7, 8]) - 0.5).abs() < 1e-9);
        assert_eq!(jaccard(&[1], &[9]), 0.0);
    }

    #[test]
    fn ten_times_the_share_is_the_worst_case() {
        assert!((ratio_gap(0.4, 0.04) - 1.0).abs() < 1e-9);
        assert!(ratio_gap(0.40, 0.39) < 0.02);
    }

    #[test]
    fn matching_is_one_to_one() {
        let target = vec![
            fp(10, 400, 0.4, Length::Fixed(2)),
            fp(11, 100, 0.1, Length::Fixed(2)),
        ];
        let reference = vec![
            fp(90, 100, 0.1, Length::Fixed(2)),
            fp(91, 400, 0.4, Length::Fixed(2)),
        ];
        let m = match_patches(&target, &reference);
        assert_eq!(m[0].reference_netid, 91);
        assert_eq!(m[1].reference_netid, 90);
    }

    #[test]
    fn a_chain_lays_out_the_alphabet_in_order() {
        // A toy table, and a quantity that wanders by one step at a
        // time without drifting away: a sawtooth over a small range,
        // so every neighbouring pair is seen many times over.
        let t = |b: u8| b.wrapping_mul(7).wrapping_add(0x11);
        let mut pairs = Vec::new();
        for _ in 0..40 {
            for v in -12i32..12 {
                pairs.push((t((v & 0xFF) as u8), t(((v + 1) & 0xFF) as u8)));
            }
            for v in (-12i32..12).rev() {
                pairs.push((t(((v + 1) & 0xFF) as u8), t((v & 0xFF) as u8)));
                pairs.push((t((v & 0xFF) as u8), t((v & 0xFF) as u8)));
            }
        }
        let chain = byte_chain(&pairs, t(0), t(1));
        assert!(chain.table.len() >= 24, "only got {}", chain.table.len());
        for (plain, cipher) in &chain.table {
            assert_eq!(*cipher, t(*plain), "wrong at 0x{plain:02x}");
        }
    }

    #[test]
    fn a_chain_refuses_to_walk_on_noise() {
        // Every pair seen once: nothing clears the threshold.
        let pairs: Vec<BytePair> = (0..200u32).map(|i| (i as u8, (i + 1) as u8)).collect();
        let chain = byte_chain(&pairs, 0x42, 0x07);
        assert_eq!(chain.table.len(), 1, "should have only the anchor");
    }

    #[test]
    fn overlapping_windows_are_counted_once() {
        // Two events 10 s apart: two separate 2 s windows.
        assert!((window_cover(&[10.0, 20.0]) - 4.0).abs() < 1e-6);
        // Two events 1 s apart: the windows merge into 3 s.
        assert!((window_cover(&[10.0, 11.0]) - 3.0).abs() < 1e-6);
        assert_eq!(window_cover(&[]), 0.0);
    }

    #[test]
    fn a_class_that_only_fires_on_deaths_is_locked_to_them() {
        let deaths = vec![vec![100.0f32, 200.0, 300.0]];
        let kills = vec![vec![]];
        let mut blocks: ChampionBlocks = Vec::new();
        for _ in 0..40 {
            for d in &deaths[0] {
                blocks.push((7, 0, *d));       // fires on every death
            }
        }
        // And one that just ticks all game.
        for i in 0..120 {
            blocks.push((8, 0, i as f32 * 5.0));
        }
        let got = event_locked(&[Timeline { blocks, deaths, kills, length_s: 600.0 }]);
        // The ticking class is never reported; the death class is, and
        // with one champion its deaths are also the game's only kills,
        // so it shows up under both labels.
        assert!(got.iter().all(|l| l.netid == 7));
        assert!(got.iter().any(|l| l.event == "their own death"));
        assert!(got.iter().all(|l| (l.share - 1.0).abs() < 1e-9));
    }

    #[test]
    fn derived_counters_are_added_only_when_every_term_is_there() {
        let mut c = BTreeMap::from([
            ("CHAMPIONS_KILLED".to_string(), vec![3, 1]),
            ("NUM_DEATHS".to_string(), vec![2, 5]),
            ("ASSISTS".to_string(), vec![4, 0]),
        ]);
        add_derived(&mut c);
        assert_eq!(c["CHAMPIONS_KILLED+NUM_DEATHS+ASSISTS"], vec![9, 6]);
        // CS needs both halves; only one is present, so no sum.
        assert!(!c.contains_key("MINIONS_KILLED+NEUTRAL_MINIONS_KILLED"));
    }

    #[test]
    fn a_uniform_offset_is_the_only_match() {
        assert_eq!(uniform_offset(&[3, 5, 1], &[3, 5, 1]), Some(0));
        assert_eq!(uniform_offset(&[4, 6, 2], &[3, 5, 1]), Some(1));
        assert_eq!(uniform_offset(&[4, 6, 3], &[3, 5, 1]), None);
        assert_eq!(uniform_offset(&[30, 50, 10], &[3, 5, 1]), None);
    }

    #[test]
    fn a_flat_statsjson_column_proves_nothing() {
        let counts = BTreeMap::from([(7u16, vec![0, 0, 0, 0, 0])]);
        let stats = BTreeMap::from([("ALWAYS_ZERO".to_string(), vec![0, 0, 0, 0, 0])]);
        assert!(match_stats(&[(counts, stats)]).is_empty());
    }

    #[test]
    fn a_counter_that_holds_in_every_replay_is_reported() {
        let one = |v: Vec<u64>| BTreeMap::from([(7u16, v)]);
        let truth = |v: Vec<i64>| BTreeMap::from([("NUM_DEATHS".to_string(), v)]);
        let m = match_stats(&[
            (one(vec![1, 2, 3, 4, 5]), truth(vec![1, 2, 3, 4, 5])),
            (one(vec![6, 2, 9, 4, 0]), truth(vec![6, 2, 9, 4, 0])),
            (one(vec![2, 7, 1, 8, 3]), truth(vec![2, 7, 1, 8, 3])),
        ]);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].netid, m[0].offset, m[0].replays), (7, 0, 3));
        assert_eq!(m[0].tested, 3);
    }

    #[test]
    fn two_agreeing_replays_are_not_enough() {
        let one = |v: Vec<u64>| BTreeMap::from([(7u16, v)]);
        let truth = |v: Vec<i64>| BTreeMap::from([("NUM_DEATHS".to_string(), v)]);
        let m = match_stats(&[
            (one(vec![1, 2, 3, 4, 5]), truth(vec![1, 2, 3, 4, 5])),
            (one(vec![6, 2, 9, 4, 0]), truth(vec![6, 2, 9, 4, 0])),
        ]);
        assert!(m.is_empty());
    }

    #[test]
    fn a_field_is_only_learned_where_the_counts_force_it() {
        // Two champions, two kills each: nothing to tell them apart.
        let payloads = vec![vec![0u8, 0xaa], vec![0, 0xaa], vec![0, 0xbb], vec![0, 0xbb]];
        assert!(forced(&payloads, &[0x4000_0034, 0x4000_0035], &[2, 2], -1, &BTreeMap::new()).is_none());
        // Different counts: forced.
        let payloads = vec![vec![0u8, 0xaa], vec![0, 0xbb], vec![0, 0xbb]];
        let got = forced(&payloads, &[0x40000034, 0x40000035], &[1, 2], -1, &BTreeMap::new()).unwrap();
        assert_eq!(got, vec![(0x34, 0xaa), (0x35, 0xbb)]);
    }

    #[test]
    fn a_solved_field_predicts_the_replays_it_did_not_learn_from() {
        let rows = |v: Vec<u8>| v.into_iter().map(|b| vec![0u8, b]).collect::<Vec<_>>();
        let ids = vec![0x4000_0034, 0x4000_0035];
        // One replay where the counts differ, so the assignment is forced.
        let mut input = vec![(rows(vec![0xaa, 0xbb, 0xbb]), ids.clone(), vec![1, 2])];
        // Many where they are equal, so only the learned table can
        // explain them.
        for _ in 0..(MIN_PREDICTIONS / 2) {
            input.push((rows(vec![0xaa, 0xaa, 0xbb, 0xbb]), ids.clone(), vec![2, 2]));
        }
        let f = solve_netid_field(9, &input).unwrap();
        assert_eq!(f.tail, 0);
        assert_eq!(f.players_right, f.players_checked);
        assert_eq!(f.table.get(&0xaa), Some(&0x34));
    }

    #[test]
    fn what_is_known_unlocks_what_is_not() {
        // Both champions killed twice, so the counts alone say nothing
        // -- but one of them is already pinned, so the other follows.
        let payloads = vec![vec![0u8, 0xaa], vec![0, 0xaa], vec![0, 0xbb], vec![0, 0xbb]];
        let known = BTreeMap::from([(0x34u8, 0xaau8)]);
        let got = forced(&payloads, &[0x4000_0034, 0x4000_0035], &[2, 2], -1, &known).unwrap();
        assert_eq!(got, vec![(0x35, 0xbb)]);
    }

    #[test]
    fn a_known_pair_that_does_not_match_rejects_the_replay() {
        let payloads = vec![vec![0u8, 0xaa], vec![0, 0xbb]];
        let known = BTreeMap::from([(0x34u8, 0xaau8)]);
        assert!(forced(&payloads, &[0x4000_0034, 0x4000_0035], &[9, 1], -1, &known).is_none());
    }

    #[test]
    fn the_offset_follows_the_size_of_the_roster_ids() {
        // Ten champions below 0x80: one varint byte, so the low group
        // is the last byte of the payload.
        let small: Vec<u32> = (0x34..0x3e).map(|n| 0x4000_0000 | n).collect();
        assert_eq!(first_byte_position(&small, 0), Some(-1));
        // At 0xae..0xb7 the varint needs two, and the high group is
        // written after the low one.
        let big: Vec<u32> = (0xae..0xb8).map(|n| 0x4000_0000 | n).collect();
        assert_eq!(first_byte_position(&big, 0), Some(-2));
        assert_eq!(first_byte_position(&big, 1), Some(-3));
        // A roster straddling the boundary has no single offset.
        assert_eq!(first_byte_position(&[0x4000_007f, 0x4000_0080], 0), None);
    }

    #[test]
    fn negative_positions_count_from_the_end() {
        assert_eq!(byte_at(&[1, 2, 3], -1), Some(3));
        assert_eq!(byte_at(&[1, 2, 3], -3), Some(1));
        assert_eq!(byte_at(&[1, 2, 3], -4), None);
        assert_eq!(byte_at(&[1, 2, 3], 0), Some(1));
    }

    #[test]
    fn shape_names_the_spread() {
        assert_eq!(shape(&[1, 1, 1, 1]), "steady");
        assert_eq!(shape(&[0, 0, 0, 40, 30, 0, 0, 0]), "bursts");
        assert_eq!(shape(&[1, 1, 1, 1, 1, 90]), "spiky");
        assert_eq!(shape(&[1]), "one minute");
    }
}
