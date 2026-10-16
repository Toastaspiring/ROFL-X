//! Whole-replay decode for format-2 archives.
//!
//! Every game-chunk block goes through `PacketDecoder` (one per worker
//! thread), is typed by `packet`, and the typed stream is assembled into:
//!
//! - a roster (champion net id per statsJson player),
//! - an event timeline (kills with assists, respawns, level-ups, item
//!   buys / removals / swaps / uses, ward placements, buildings),
//! - per-second champion positions and ward lifecycles in Mowokuma's JSON
//!   shape (`players_state`, `wards`),
//! - a self-check against the end-of-game stats Riot stores in the same
//!   file (statsJson): kills, deaths, assists, level, final items, damage
//!   to champions, wards placed. Every number the output claims to decode
//!   is compared against Riot's own.

use std::collections::{BTreeMap, HashMap, HashSet};

use rayon::prelude::*;
use serde_json::{json, Value};

use crate::emulator::{DecodeStatus, PacketDecoder, PatchArchive};
use crate::error::{Result, RoflError};
use crate::packet::handlers::visibility::{TEAM_CHAOS, TEAM_ORDER};
use crate::packet::{self, GamePacket, Path, Shape};
use crate::replay_info::{metadata_to_json, role_label, team_label};
use crate::rofl::block::BlockIterator;
use crate::rofl::decompress::decompress_chunk;
use crate::rofl::stream::StreamTag;
use crate::rofl::metadata::Team;
use crate::rofl::Replay;

/// A shape-recognised class is trusted for a netid only when at least
/// this share of its clean packets matched, over at least `SHAPE_MIN`.
const SHAPE_VOTE: f64 = 0.95;
const SHAPE_MIN: usize = 5;

/// How far a ward's corpse may spawn from where the ward was placed.
const WARD_CORPSE_RADIUS: f32 = 200.0;

/// Ward names placed by champions (trinkets, wards bought, rune wards).
const WARD_NAMES: &[&str] = &[
    "YellowTrinket",
    "SightWard",
    "JammerDevice",
    "BlueTrinket",
    "VisionWard",
    "PerksZombieWard",
];

/// A placed ward, until its corpse spawns.
struct WardRecord {
    placed: f32,
    name: String,
    owner: u32,
    pos: [f32; 2],
    died: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct TypedBlock {
    pub t: f32,
    pub netid: u16,
    pub packet: GamePacket,
}

/// Decode-level counters, reported in the output's `decode` section.
#[derive(Debug, Default, Clone)]
pub struct DecodeSummary {
    pub blocks: usize,
    pub status: BTreeMap<&'static str, usize>,
    /// Blocks whose netid has a registered Riot class name.
    pub named_blocks: usize,
    pub typed_blocks: usize,
    /// Netids identified by shape, after the vote.
    pub shapes: BTreeMap<u16, Shape>,
    pub threads: usize,
}

#[derive(Default)]
struct ChunkOut {
    blocks: usize,
    status: HashMap<DecodeStatus, usize>,
    named: usize,
    typed: Vec<TypedBlock>,
    unnamed_clean: HashMap<u16, usize>,
    shape_hits: HashMap<(u16, Shape), usize>,
}

/// Decode every block of the game-chunk stream and keep the typed ones,
/// in replay order.
pub fn decode_replay(
    replay: &Replay<'_>,
    archive: &PatchArchive,
    threads: usize,
) -> Result<(Vec<TypedBlock>, DecodeSummary)> {
    let mut records = Vec::new();
    for chunk in replay.chunks() {
        let chunk = chunk?;
        if chunk.stream_tag == StreamTag::GameChunk {
            records.push(chunk);
        }
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(|e| RoflError::Emulator(format!("thread pool: {e}")))?;
    let outs: Vec<Result<ChunkOut>> = pool.install(|| {
        records
            .par_iter()
            .map_init(
                || PacketDecoder::new(archive).map_err(|e| e.to_string()),
                |decoder, record| {
                    let decoder = decoder.as_mut().map_err(|e| RoflError::Emulator(e.clone()))?;
                    decode_chunk(decoder, archive, &decompress_chunk(record)?)
                },
            )
            .collect()
    });

    let mut summary = DecodeSummary {
        threads,
        ..Default::default()
    };
    let mut unnamed_clean: HashMap<u16, usize> = HashMap::new();
    let mut shape_hits: HashMap<(u16, Shape), usize> = HashMap::new();
    let mut typed = Vec::new();
    for out in outs {
        let out = out?;
        summary.blocks += out.blocks;
        summary.named_blocks += out.named;
        for (status, n) in out.status {
            *summary.status.entry(status.label()).or_default() += n;
        }
        for (k, n) in out.unnamed_clean {
            *unnamed_clean.entry(k).or_default() += n;
        }
        for (k, n) in out.shape_hits {
            *shape_hits.entry(k).or_default() += n;
        }
        typed.extend(out.typed);
    }
    for ((netid, shape), hits) in &shape_hits {
        let clean = unnamed_clean.get(netid).copied().unwrap_or(0);
        if clean >= SHAPE_MIN && *hits as f64 >= SHAPE_VOTE * clean as f64 {
            summary.shapes.insert(*netid, *shape);
        }
    }
    typed.retain(|b| match &b.packet {
        GamePacket::Waypoints(_) => summary.shapes.get(&b.netid) == Some(&Shape::Waypoints),
        GamePacket::Spawn(_) => summary.shapes.get(&b.netid) == Some(&Shape::Spawn),
        _ => true,
    });
    summary.typed_blocks = typed.len();
    Ok((typed, summary))
}

fn decode_chunk(decoder: &mut PacketDecoder, archive: &PatchArchive, body: &[u8]) -> Result<ChunkOut> {
    let mut out = ChunkOut::default();
    for block in BlockIterator::new(body) {
        let block = block?;
        let pkt = decoder.decode(block.packet_id, block.param, block.payload)?;
        out.blocks += 1;
        *out.status.entry(pkt.status).or_default() += 1;
        let class = archive.class_of(block.packet_id);
        if class.is_some() {
            out.named += 1;
        }
        if pkt.status != DecodeStatus::Clean {
            continue;
        }
        let typed = match class {
            Some(class) => packet::interpret(class, &pkt),
            None => {
                *out.unnamed_clean.entry(block.packet_id).or_default() += 1;
                packet::recognise(&pkt).map(|(shape, p)| {
                    *out.shape_hits.entry((block.packet_id, shape)).or_default() += 1;
                    p
                })
            }
        };
        if let Some(packet) = typed {
            out.typed.push(TypedBlock {
                t: block.timestamp,
                netid: block.packet_id,
                packet,
            });
        }
    }
    Ok(out)
}

// --------------------------------------------------------------------------
// Assembly
// --------------------------------------------------------------------------

/// Champion net ids, in statsJson order.
struct Roster {
    ids: Vec<u32>,
}

impl Roster {
    /// Champion net ids are contiguous and assigned in statsJson order
    /// (verified on 16.9: kills / deaths / items / damage line up with
    /// statsJson player by player). Seeded from hero-only packet classes.
    fn infer(typed: &[TypedBlock], players: usize) -> Option<Self> {
        let mut seen: HashSet<u32> = HashSet::new();
        for b in typed {
            match &b.packet {
                GamePacket::Kda(k) => seen.insert(k.hero),
                GamePacket::BuyItem(x) => seen.insert(x.hero),
                GamePacket::HeroDie(d) => seen.insert(d.victim),
                GamePacket::HeroReincarnate(r) => seen.insert(r.hero),
                _ => false,
            };
        }
        let first = *seen.iter().min()?;
        let last = *seen.iter().max()?;
        if players == 0 || (last - first) as usize >= players {
            return None;
        }
        Some(Roster {
            ids: (0..players as u32).map(|i| first + i).collect(),
        })
    }

    fn index(&self, net_id: u32) -> Option<usize> {
        self.ids.iter().position(|id| *id == net_id)
    }
}

/// Where a champion is: following a path, or standing still.
#[derive(Clone)]
enum Motion {
    Path { since: f32, path: Path },
    Still { x: f32, y: f32, alive: bool },
}

impl Motion {
    fn at(&self, t: f32) -> ([f32; 2], bool) {
        match self {
            Motion::Still { x, y, alive } => ([*x, *y], *alive),
            Motion::Path { since, path } => {
                let mut left = (t - since).max(0.0) * path.speed;
                for w in path.points.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                    if left <= len {
                        let u = if len > 0.0 { left / len } else { 0.0 };
                        return ([a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u], true);
                    }
                    left -= len;
                }
                (*path.points.last().unwrap_or(&[0.0, 0.0]), true)
            }
        }
    }
}

/// Assemble the format-2 output JSON.
pub fn assemble(replay: &Replay<'_>, typed: &[TypedBlock], summary: &DecodeSummary) -> Value {
    let meta = &replay.metadata;
    let n = meta.players.len();
    let roster = Roster::infer(typed, n);
    let player_of = |id: u32| roster.as_ref().and_then(|r| r.index(id));
    let who = |id: u32| match player_of(id) {
        Some(i) => json!(i),
        None if id == 0 => Value::Null,
        None => json!(format!("0x{id:08x}")),
    };

    let mut events: Vec<Value> = Vec::new();
    // Every unit's movement orders, champions included, keyed by net id.
    let mut tracks: std::collections::BTreeMap<u32, Vec<Value>> =
        std::collections::BTreeMap::new();
    let mut inventory: Vec<[u32; 9]> = vec![[0; 9]; n];
    let mut kda: Vec<[u32; 3]> = vec![[0; 3]; n];
    let mut event_kills = vec![0u32; n];
    let mut event_assists = vec![0u32; n];
    let mut level: Vec<u8> = vec![1; n];
    let mut damage_to_champions = vec![0f64; n];
    let mut wards_placed = vec![0u32; n];
    let mut vision: Vec<Vision> = vec![Vision::default(); n];
    let mut motion: Vec<Option<Motion>> = vec![None; n];
    let mut players_state: Vec<Value> = Vec::new();
    let mut ward_list: Vec<WardRecord> = Vec::new();
    let mut open_wards: Vec<usize> = Vec::new();
    let mut next_tick = 1.0f32;
    let assists_of = assign_assists(typed, &roster, meta);
    // A recipe's result can land in a slot still holding one of its
    // components; that component's removal then arrives after the buy, at
    // the same timestamp. Remember the slot so the removal does not take
    // the new item with it.
    let mut displaced: HashMap<(usize, u8), f32> = HashMap::new();

    for (i, b) in typed.iter().enumerate() {
        while b.t >= next_tick {
            players_state.push(state_at(next_tick, &motion, meta));
            next_tick += 1.0;
        }
        match &b.packet {
            GamePacket::HeroDie(d) => {
                let Some(v) = player_of(d.victim) else { continue };
                motion[v] = Some(Motion::Still { x: d.x, y: d.y, alive: false });
                let assists = assists_of.get(&i).cloned().unwrap_or_default();
                if let Some(k) = player_of(d.killer) {
                    event_kills[k] += 1;
                }
                for a in &assists {
                    event_assists[*a] += 1;
                }
                events.push(json!({
                    "t": b.t, "type": "champion_kill",
                    "victim": v, "killer": who(d.killer), "assists": assists,
                    "x": d.x, "y": d.y, "death_timer": d.death_timer,
                }));
            }
            GamePacket::HeroReincarnate(r) => {
                let Some(p) = player_of(r.hero) else { continue };
                motion[p] = Some(Motion::Still { x: r.x, y: r.y, alive: true });
                events.push(json!({"t": b.t, "type": "respawn", "player": p, "x": r.x, "y": r.y}));
            }
            GamePacket::Kda(k) => {
                if let Some(p) = player_of(k.hero) {
                    kda[p] = [k.kills, k.deaths, k.assists];
                }
            }
            GamePacket::LevelUp(l) => {
                let Some(p) = player_of(l.unit) else { continue };
                level[p] = level[p].max(l.level);
                events.push(json!({"t": b.t, "type": "level_up", "player": p, "level": l.level}));
            }
            GamePacket::Damage(d) => {
                if let (Some(s), Some(t)) = (player_of(d.source), player_of(d.target)) {
                    if meta.players[s].team != meta.players[t].team {
                        damage_to_champions[s] += f64::from(d.amount);
                    }
                }
                // The class was only ever counted, never reported, so every
                // blow a champion struck or took was decoded and dropped.
                // Emitted where a champion is on one end: minion trading
                // minion is most of the traffic and none of the story.
                //
                // `hit` is the game's hit result, not the kind of damage.
                // 3 is a critical strike: its largest value per player
                // equals the replay's own LARGEST_CRITICAL_STRIKE for nine
                // of ten champions, and reads zero for the seven who never
                // crit. `effect` names the particular ability or attack
                // that did it, one hash per champion per source -- which is
                // how an auto can be told from an ability, since the hash
                // whose largest value matches LARGEST_ATTACK_DAMAGE is that
                // champion's basic attack.
                if player_of(d.source).is_some() || player_of(d.target).is_some() {
                    events.push(json!({
                        "t": b.t, "type": "damage",
                        "source": who(d.source), "target": who(d.target),
                        "amount": d.amount,
                        "hit": d.hit_type,
                        "critical": d.hit_type == 3,
                        "effect": format!("0x{:08x}", d.effect_hash),
                    }));
                }
            }
            GamePacket::BuyItem(x) => {
                let Some(p) = player_of(x.hero) else { continue };
                if let Some(slot) = inventory[p].get_mut(usize::from(x.slot)) {
                    if *slot != 0 {
                        displaced.insert((p, x.slot), b.t);
                    }
                    *slot = x.item;
                }
                events.push(json!({
                    "t": b.t, "type": "item_purchase", "player": p, "item": x.item,
                    "slot": x.slot, "price": x.price, "gold_left": x.gold_left,
                }));
            }
            GamePacket::RemoveItem(x) => {
                let Some(p) = player_of(x.hero) else { continue };
                let Some(slot) = inventory[p].get_mut(usize::from(x.slot)) else { continue };
                if displaced.remove(&(p, x.slot)) == Some(b.t) {
                    continue;
                }
                let item = std::mem::take(slot);
                events.push(json!({
                    "t": b.t, "type": "item_removed", "player": p, "item": item,
                    "slot": x.slot, "gold_left": x.gold_left,
                }));
            }
            GamePacket::SwapItem(x) => {
                let Some(p) = player_of(x.hero) else { continue };
                let (a, c) = (usize::from(x.from), usize::from(x.to));
                if a < 9 && c < 9 {
                    inventory[p].swap(a, c);
                }
            }
            GamePacket::UseItem(x) => {
                let Some(p) = player_of(x.hero) else { continue };
                let Some(slot) = inventory[p].get_mut(usize::from(x.slot)) else { continue };
                let item = *slot;
                if x.consumed {
                    *slot = 0;
                }
                events.push(json!({
                    "t": b.t, "type": "item_use", "player": p, "item": item,
                    "slot": x.slot, "charges_left": x.charges_left,
                }));
            }
            GamePacket::BuildingDie(x) => {
                events.push(json!({
                    "t": b.t, "type": "building_destroyed",
                    "building": format!("0x{:08x}", x.building), "killer": who(x.killer),
                }));
            }
            GamePacket::Spawn(s) => {
                if s.name.contains("Corpse") {
                    // The corpse spawns where the ward ended up, which can be
                    // ~100 units from where it was placed: match the owner's
                    // nearest live ward. Each death also spawns an unowned
                    // corpse twin, skipped by the owner match.
                    let nearest = open_wards
                        .iter()
                        .enumerate()
                        .filter(|(_, &w)| ward_list[w].owner == s.owner)
                        .map(|(i, &w)| (i, distance(ward_list[w].pos, [s.x, s.y])))
                        .filter(|(_, d)| *d <= WARD_CORPSE_RADIUS)
                        .min_by(|a, b| a.1.total_cmp(&b.1));
                    if let Some((i, _)) = nearest {
                        let w = open_wards.swap_remove(i);
                        ward_list[w].died = Some(b.t);
                    }
                } else if WARD_NAMES.contains(&s.name.as_str()) {
                    let Some(p) = player_of(s.owner) else { continue };
                    wards_placed[p] += 1;
                    open_wards.push(ward_list.len());
                    ward_list.push(WardRecord {
                        placed: b.t,
                        name: s.name.clone(),
                        owner: s.owner,
                        pos: [s.x, s.y],
                        died: None,
                    });
                    events.push(json!({
                        "t": b.t, "type": "ward_placed", "player": p,
                        "ward": s.name, "x": s.x, "y": s.y,
                    }));
                }
            }
            GamePacket::Waypoints(g) => {
                for path in &g.paths {
                    if let Some(p) = player_of(path.unit) {
                        motion[p] = Some(Motion::Path { since: b.t, path: path.clone() });
                    }
                    // Everything that is not a champion was being thrown
                    // away: minions, monsters, pets and turret shots all
                    // move through this class and none of them reached the
                    // output. They are kept as the orders themselves --
                    // when, how fast, and along which path -- rather than
                    // as positions sampled on some grid, because a packet
                    // is an order to walk and sampling it decides for the
                    // reader what resolution they are allowed.
                    tracks
                        .entry(path.unit)
                        .or_insert_with(Vec::new)
                        .push(json!({
                            "t": b.t,
                            "speed": path.speed,
                            "path": path.points,
                        }));
                }
            }
            GamePacket::SpellCast(c) => {
                let Some(p) = player_of(c.champion) else { continue };
                let Some(slot) = packet::handlers::spell_cast::slot_name(c.slot) else { continue };
                events.push(json!({
                    "t": b.t, "type": "spell_cast", "player": p, "slot": slot,
                    "spell_id": format!("0x{:08x}", c.spell_id),
                    "x": c.target_x, "y": c.target_y,
                    "mana_cost": c.mana_cost, "cooldown": c.cooldown,
                }));
            }
            GamePacket::Visibility(v) => {
                let Some(p) = player_of(v.unit) else { continue };
                let own = match meta.players[p].team {
                    Team::Blue => TEAM_ORDER,
                    Team::Red => TEAM_CHAOS,
                };
                let vis = &mut vision[p];
                vis.events += 1;
                if v.team == own {
                    vis.own_team += 1;
                    continue;
                }
                match (v.visible, vis.since.take()) {
                    (true, None) => vis.since = Some(b.t),
                    (true, Some(open)) => vis.since = Some(open),
                    (false, Some(open)) => vis.windows.push((open, b.t)),
                    (false, None) => {}
                }
            }
            GamePacket::NpcDie(_) => {}
        }
    }
    let game_len = meta.game_length_ms as f32 / 1000.0;
    for vis in &mut vision {
        if let Some(open) = vis.since.take() {
            vis.windows.push((open, game_len));
        }
    }
    while next_tick <= game_len {
        players_state.push(state_at(next_tick, &motion, meta));
        next_tick += 1.0;
    }

    let wards: Vec<Value> = ward_list
        .iter()
        .filter_map(|w| {
            let player = &meta.players[player_of(w.owner)?];
            Some(json!({
                "duration": w.died.map(|d| d - w.placed),
                "name": w.name,
                "owner": {"name": "", "role": role_label(player.role), "team": team_label(player.team)},
                "pos": [w.pos[0] as i32, w.pos[1] as i32],
                "team": team_label(player.team),
                "timestamp": w.placed,
            }))
        })
        .collect();

    let players: Vec<Value> = meta
        .players
        .iter()
        .enumerate()
        .map(|(i, p)| {
            json!({
                "index": i,
                "net_id": roster.as_ref().map(|r| format!("0x{:08x}", r.ids[i])),
                "champion": p.skin,
                "team": team_label(p.team),
                "role": role_label(p.role),
                "win": p.win,
            })
        })
        .collect();

    let checks = self_check(
        replay,
        &Decoded {
            kda: &kda,
            event_kills: &event_kills,
            event_assists: &event_assists,
            level: &level,
            inventory: &inventory,
            damage: &damage_to_champions,
            wards: &wards_placed,
            vision: &vision,
        },
    );
    let vision: Vec<Value> = vision
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let seen: f32 = v.windows.iter().map(|(a, b)| b - a).sum();
            json!({
                "player": i,
                "seen_by_enemy_s": tenths(seen),
                "share_of_game": (f64::from(seen / game_len.max(1.0)) * 1000.0).round() / 1000.0,
                "windows": v.windows.len(),
                "intervals": v.windows.iter()
                    .map(|(a, b)| json!([tenths(*a), tenths(*b)]))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut metadata = metadata_to_json(&replay.header, meta);
    metadata["patch"] = json!(replay.header.patch());
    metadata["build"] = json!(replay.header.version);

    json!({
        "metadata": metadata,
        "players": players,
        "events": events,
        // Orders, not samples: each is a moment, a speed and the path
        // asked for, which is what the class actually carries. Walking
        // them reconstructs a position at any instant.
        "tracks": tracks.into_iter()
            .map(|(id, orders)| json!({
                "unit": format!("0x{id:08x}"),
                "player": who(id),
                "orders": orders,
            }))
            .collect::<Vec<_>>(),
        "players_state": players_state,
        "wards": wards,
        "vision": vision,
        "checks": checks,
        "decode": {
            "blocks": summary.blocks,
            "status": summary.status,
            "named_blocks": summary.named_blocks,
            "typed_blocks": summary.typed_blocks,
            "shape_netids": summary.shapes.iter()
                .map(|(k, v)| (k.to_string(), json!(v.label())))
                .collect::<serde_json::Map<_, _>>(),
            "roster": roster.is_some(),
        },
    })
}

/// Assists per champion death (keyed by the death's index in `typed`).
///
/// `PKT_NPC_Hero_Die_s` names the killer but not the assisters; the
/// assist counter in `PKT_Update_HeroStats_KDA_s` does count them, exactly
/// (it matches statsJson). Each counter step is one assist: it goes to
/// the nearest death in time (within `WINDOW`) of an enemy of the
/// assister that the assister did not kill and is not already credited
/// on. Totals are exact; in a teamfight with several deaths in the same
/// second the split between those deaths is a best match.
fn assign_assists(
    typed: &[TypedBlock],
    roster: &Option<Roster>,
    meta: &crate::rofl::Metadata,
) -> HashMap<usize, Vec<usize>> {
    const WINDOW: f32 = 1.0;
    let mut out: HashMap<usize, Vec<usize>> = HashMap::new();
    let Some(roster) = roster else { return out };
    let mut deaths: Vec<(usize, f32, usize, Option<usize>)> = Vec::new();
    let mut steps: Vec<(f32, usize)> = Vec::new();
    let mut last = vec![0u32; roster.ids.len()];
    for (i, b) in typed.iter().enumerate() {
        match &b.packet {
            GamePacket::HeroDie(d) => {
                if let Some(v) = roster.index(d.victim) {
                    deaths.push((i, b.t, v, roster.index(d.killer)));
                }
            }
            GamePacket::Kda(k) => {
                if let Some(p) = roster.index(k.hero) {
                    for _ in last[p]..k.assists {
                        steps.push((b.t, p));
                    }
                    last[p] = last[p].max(k.assists);
                }
            }
            _ => {}
        }
    }
    for (t, p) in steps {
        let best = deaths
            .iter()
            .filter(|(i, dt, v, killer)| {
                (dt - t).abs() <= WINDOW
                    && meta.players[*v].team != meta.players[p].team
                    && *killer != Some(p)
                    && !out.get(i).is_some_and(|a| a.contains(&p))
            })
            .min_by(|a, b| (a.1 - t).abs().total_cmp(&(b.1 - t).abs()));
        if let Some((i, ..)) = best {
            out.entry(*i).or_default().push(p);
        }
    }
    out
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn state_at(t: f32, motion: &[Option<Motion>], meta: &crate::rofl::Metadata) -> Value {
    let players: Vec<Value> = motion
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let (pos, alive) = m.as_ref()?.at(t);
            let p = &meta.players[i];
            Some(json!({
                "alive": alive,
                "champ": p.skin,
                "name": "",
                "pos": pos,
                "role": role_label(p.role),
                "team": team_label(p.team),
            }))
        })
        .collect();
    json!({"players": players, "timestamp": t})
}

/// Per-player totals rebuilt from the decoded packets.
struct Decoded<'a> {
    /// Last `PKT_Update_HeroStats_KDA_s`: kills, deaths, assists.
    kda: &'a [[u32; 3]],
    /// Kills / assists counted from champion_kill events instead.
    event_kills: &'a [u32],
    event_assists: &'a [u32],
    level: &'a [u8],
    inventory: &'a [[u32; 9]],
    damage: &'a [f64],
    wards: &'a [u32],
    vision: &'a [Vision],
}

/// Seconds to one decimal. An `f32` second rounded in `f32` still
/// serialises as 45.400001525878906.
fn tenths(t: f32) -> f64 {
    (f64::from(t) * 10.0).round() / 10.0
}

/// How long one champion spent inside the enemy team's vision, built
/// from `PKT_S2C_On{Enter,Leave}TeamVisibility_s`.
#[derive(Debug, Default, Clone)]
struct Vision {
    /// Closed intervals of being visible to the other team.
    windows: Vec<(f32, f32)>,
    /// Start of the interval still open, if any.
    since: Option<f32>,
    events: u32,
    /// Visibility changes that named the champion's own team. A unit is
    /// always visible to its own team, so this must stay 0.
    own_team: u32,
}

/// Compare decoded per-player totals with statsJson, Riot's own record.
fn self_check(replay: &Replay<'_>, d: &Decoded<'_>) -> Value {
    let stats: Vec<Value> = serde_json::from_str(&replay.metadata.stats_json_raw).unwrap_or_default();
    let num = |p: &Value, k: &str| p.get(k).and_then(Value::as_str).and_then(|s| s.parse::<f64>().ok());
    let mut rows = Vec::new();
    let mut tally: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut mark = |field: &'static str, ok: bool| {
        let e = tally.entry(field).or_default();
        e.0 += ok as usize;
        e.1 += 1;
        ok
    };
    for (i, p) in stats.iter().enumerate().take(d.kda.len()) {
        let kills = num(p, "CHAMPIONS_KILLED");
        let assists = num(p, "ASSISTS");
        let truth_kda = [kills, num(p, "NUM_DEATHS"), assists];
        let items: Vec<u32> = (0..7).map(|s| num(p, &format!("ITEM{s}")).unwrap_or(0.0) as u32).collect();
        let dmg_truth = num(p, "TOTAL_DAMAGE_DEALT_TO_CHAMPIONS");
        let wards_truth = num(p, "WARD_PLACED");
        let is = |truth: Option<f64>, v: u32| truth == Some(f64::from(v));
        let kda_ok = truth_kda.iter().zip(d.kda[i]).all(|(t, v)| is(*t, v));
        let events_ok = is(kills, d.event_kills[i]) && is(assists, d.event_assists[i]);
        let level_ok = is(num(p, "LEVEL"), u32::from(d.level[i]));
        let items_ok = items == d.inventory[i][..7];
        let dmg_ok = dmg_truth.is_some_and(|t| (t - d.damage[i]).abs() <= 2.0);
        let wards_ok = is(wards_truth, d.wards[i]);
        rows.push(json!({
            "player": i,
            "kda": {"decoded": d.kda[i], "stats": truth_kda, "ok": mark("kda", kda_ok)},
            "kill_events": {
                "decoded": [d.event_kills[i], d.event_assists[i]], "stats": [kills, assists],
                "ok": mark("kill_events", events_ok),
            },
            "level": {"decoded": d.level[i], "stats": num(p, "LEVEL"), "ok": mark("level", level_ok)},
            "items": {"decoded": &d.inventory[i][..7], "stats": items, "ok": mark("items", items_ok)},
            "damage_to_champions": {
                "decoded": d.damage[i].round(), "stats": dmg_truth,
                "ok": mark("damage_to_champions", dmg_ok),
            },
            "wards_placed": {"decoded": d.wards[i], "stats": wards_truth, "ok": mark("wards_placed", wards_ok)},
            "visibility": {
                "events": d.vision[i].events,
                "named_own_team": d.vision[i].own_team,
                "ok": mark("visibility", d.vision[i].events > 0 && d.vision[i].own_team == 0),
            },
        }));
    }
    let summary: serde_json::Map<String, Value> = tally
        .into_iter()
        .map(|(k, (ok, all))| (k.to_string(), json!(format!("{ok}/{all}"))))
        .collect();
    json!({"summary": summary, "players": rows})
}
