//! File-only replay analysis: everything a `.rofl` yields with no game
//! binary and no emulation.
//!
//! This is the right path for patches whose client binary ships packed
//! (16.10+, see `docs/COMPATIBILITY.md`): we never touch the game, we read
//! the file. Two things come out, both plaintext in the file:
//!
//! - **The end-of-game scoreboard.** Riot stores it as `statsJson` in the
//!   metadata trailer: one object per player, a few hundred fields each
//!   (damage breakdowns, gold, CS, vision, objectives, pings, mission
//!   trackers, ...). No decoding needed; it is JSON.
//! - **The transport activity profile.** Every block's framing
//!   (`timestamp`, `netid`, `param`, length) is plaintext even though its
//!   payload is obfuscated. That gives per-netid, per-entity and
//!   per-minute block counts: the shape of the game's traffic without
//!   reading a single payload.
//!
//! Identity fields (Riot ID, PUUID, summoner name) are redacted to their
//! length unless `include_identities` is set; these are the user's own
//! replays, but the default keeps committed or shared output clean.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::error::Result;
use crate::replay_info::{role_label, team_label};
use crate::rofl::block::BlockIterator;
use crate::rofl::decompress::decompress_chunk;
use crate::rofl::stream::StreamTag;
use crate::rofl::Replay;

/// A `param` at or above this value is a runtime entity id (champions,
/// minions, wards, ...); below it the field is a spell/buff/other id.
const ENTITY_ID_BASE: u32 = 0x4000_0000;
const TOP_ENTITIES: usize = 40;

/// Build the file-only summary JSON for an already-parsed replay.
pub fn summary(replay: &Replay<'_>, include_identities: bool) -> Result<Value> {
    Ok(json!({
        "metadata": metadata(replay),
        "players": players(replay, include_identities),
        "transport": transport(replay)?,
    }))
}

fn metadata(replay: &Replay<'_>) -> Value {
    let m = &replay.metadata;
    let mut game = 0usize;
    let mut keyframe = 0usize;
    for chunk in replay.chunks().flatten() {
        match chunk.stream_tag {
            StreamTag::GameChunk => game += 1,
            StreamTag::Keyframe => keyframe += 1,
            _ => {}
        }
    }
    json!({
        "patch": replay.header.patch(),
        "build": replay.header.version,
        "format_version": replay.header.format_version,
        "game_length_ms": m.game_length_ms,
        "game_length_s": (m.game_length_ms as f64 / 1000.0 * 10.0).round() / 10.0,
        "winning_team": team_label(m.winning_team),
        "chunks": {"game": game, "keyframe": keyframe},
    })
}

fn players(replay: &Replay<'_>, include_identities: bool) -> Value {
    let stats: Vec<Value> =
        serde_json::from_str(&replay.metadata.stats_json_raw).unwrap_or_default();
    let players: Vec<Value> = replay
        .metadata
        .players
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let raw = stats.get(i).and_then(Value::as_object);
            json!({
                "index": i,
                "champion": p.skin,
                "team": team_label(p.team),
                "role": role_label(p.role),
                "win": p.win,
                "stats": raw.map(|o| redact(o, include_identities)).unwrap_or(Value::Null),
            })
        })
        .collect();
    Value::Array(players)
}

/// A statsJson object, with identity fields reduced to `"<N chars>"`
/// unless `include_identities`.
fn redact(obj: &Map<String, Value>, include_identities: bool) -> Value {
    let out: Map<String, Value> = obj
        .iter()
        .map(|(k, v)| {
            if !include_identities && is_identity_key(k) {
                let len = v.as_str().map_or(0, str::len);
                (k.clone(), Value::String(format!("<{len} chars>")))
            } else {
                (k.clone(), v.clone())
            }
        })
        .collect();
    Value::Object(out)
}

fn is_identity_key(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    k == "NAME"
        || k.contains("PUUID")
        || k.contains("RIOT_ID")
        || k.contains("SUMMONER_NAME")
        || k.contains("DISPLAY_NAME")
}

fn transport(replay: &Replay<'_>) -> Result<Value> {
    let mut netids: BTreeMap<u16, u64> = BTreeMap::new();
    let mut entities: BTreeMap<u32, u64> = BTreeMap::new();
    let mut per_minute: Vec<u64> = Vec::new();
    let mut total: u64 = 0;

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
            total += 1;
            *netids.entry(block.packet_id).or_default() += 1;
            if block.param >= ENTITY_ID_BASE {
                *entities.entry(block.param).or_default() += 1;
            }
            let minute = (block.timestamp / 60.0).max(0.0) as usize;
            if per_minute.len() <= minute {
                per_minute.resize(minute + 1, 0);
            }
            per_minute[minute] += 1;
        }
    }

    let mut netid_rows: Vec<Value> = netids
        .iter()
        .map(|(id, n)| json!({"netid": id, "blocks": n}))
        .collect();
    netid_rows.sort_by_key(|r| std::cmp::Reverse(r["blocks"].as_u64().unwrap_or(0)));

    let mut entity_rows: Vec<(u32, u64)> = entities.into_iter().collect();
    entity_rows.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let top_entities: Vec<Value> = entity_rows
        .iter()
        .take(TOP_ENTITIES)
        .map(|(id, n)| json!({"entity": format!("0x{id:08x}"), "blocks": n}))
        .collect();

    Ok(json!({
        "total_blocks": total,
        "distinct_netids": netid_rows.len(),
        "distinct_entities": entity_rows.len(),
        "netids": netid_rows,
        "top_entities": top_entities,
        "blocks_per_minute": per_minute,
        "note": "Block framing (netid, param, timestamp, length) is plaintext; \
                 payloads are obfuscated and are not decoded here. For payload \
                 decoding, see `rofl-x file` with a patch archive (docs/DECODING.md).",
    }))
}
