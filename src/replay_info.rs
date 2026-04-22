//! End-to-end orchestration: replay bytes in, Mowokuma-compatible JSON out.
//!
//! Ports the logic of `get_replay_info` from Mowokuma's `main.rs`
//! (commit 7181c9a): two passes over the block stream, one filtering
//! ward-spawn netid and one filtering movement netid, each batched and
//! fed through a `StubEmulator`. Ward lifecycle is reconstructed by
//! matching placement packets to "Corpse" destruction packets at the same
//! integer coordinates. Player positions are reconstructed by keeping the
//! latest `PathPacket` per player entity and stepping forward in
//! 1-second ticks, interpolating against each stored path. See CREDITS.md.

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};

use crate::emulator::config::Config;
use crate::emulator::packet::{PathPacket, PosKey, WardSpawnPacket};
use crate::error::Result;
use crate::rofl::block::BlockIterator;
use crate::rofl::decompress::decompress_chunk;
use crate::rofl::header::FileHeader;
use crate::rofl::metadata::{Metadata, Role, Team};
use crate::rofl::stream::StreamTag;
use crate::rofl::Replay;

/// Pair of `(timestamp, payload)` pulled from every block with a given
/// `packet_id` in the game-chunk stream.
type BlockHits = Vec<(f32, Vec<u8>)>;

/// Walk all game-chunk stream chunks, decompress, split into blocks, and
/// collect every block whose `packet_id` matches the target netid.
pub fn blocks_with_netid(replay: &Replay<'_>, netid: u16) -> Result<BlockHits> {
    let mut hits: BlockHits = Vec::new();
    for chunk in replay.chunks() {
        let chunk = chunk?;
        if chunk.stream_tag != StreamTag::GameChunk || chunk.compressed_len == 0 {
            continue;
        }
        let body = decompress_chunk(&chunk)?;
        for block in BlockIterator::new(&body) {
            let block = block?;
            if block.packet_id == netid {
                hits.push((block.timestamp, block.payload.to_vec()));
            }
        }
    }
    Ok(hits)
}

/// Build the Mowokuma-compatible JSON from a parsed replay plus the two
/// packet streams her binary emits.
pub fn build_output_json(
    header: &FileHeader,
    metadata: &Metadata,
    ward_packets: Vec<WardSpawnPacket>,
    mut path_packets: Vec<PathPacket>,
    player_id_start: u32,
) -> Value {
    let mut game = json!({
        "metadata": metadata_to_json(header, metadata),
        "wards": [],
        "players_state": [],
    });

    // Ward lifecycle reconstruction, same rules as Mowokuma:
    //  - placements have name in { YellowTrinket, SightWard, JammerDevice }
    //  - destructions have "Corpse" in the name, matched by (x,y) coords
    let mut placed: HashMap<u32, WardSpawnPacket> = HashMap::new();
    let mut pos_to_id: HashMap<PosKey, u32> = HashMap::new();

    for packet in ward_packets {
        let is_placement = matches!(
            packet.name.as_str(),
            "YellowTrinket" | "SightWard" | "JammerDevice"
        );
        if is_placement {
            placed.entry(packet.id).or_insert(packet.clone());
            pos_to_id
                .entry(PosKey::new(packet.x, packet.y))
                .or_insert(packet.id);
        } else if packet.name.contains("Corpse") {
            if let Some((_, id)) = pos_to_id.remove_entry(&PosKey::new(packet.x, packet.y)) {
                if let Some((_, p)) = placed.remove_entry(&id) {
                    let owner = &metadata.players[(p.owner_id - player_id_start) as usize];
                    game["wards"].as_array_mut().unwrap().push(json!({
                        "duration": packet.timestamp - p.timestamp,
                        "name": p.name,
                        "owner": json!({
                            "name": "",
                            "role": role_label(owner.role),
                            "team": team_label(owner.team),
                        }),
                        "pos": [p.x, p.y],
                        "team": team_label(owner.team),
                        "timestamp": p.timestamp,
                    }));
                }
            }
        }
    }

    // Position reconstruction: keep latest PathPacket per player entity,
    // emit a state record every time the running block timestamp crosses
    // a 1-second boundary relative to the last emitted.
    path_packets.sort_by(|a, b| {
        a.timestamp
            .partial_cmp(&b.timestamp)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // BTreeMap for deterministic iteration (sorted by entity id). Mowokuma
    // uses HashMap here, which produces output that differs run-to-run in
    // the order of `players_state[i].players[]`. Our parity test does a
    // content-level diff that normalises this.
    let mut players_path_state: BTreeMap<u32, PathPacket> = BTreeMap::new();
    let mut tick: f32 = 0.0;

    for packet in path_packets {
        if packet.id >= player_id_start && packet.id <= player_id_start + 9 {
            players_path_state.insert(packet.id, packet.clone());
        }

        if packet.timestamp - tick >= 1.0 {
            let mut state = json!({
                "players": [],
                "timestamp": tick,
            });

            for (_, path) in players_path_state.iter() {
                let (x, y) = path.get_pos(packet.timestamp);
                let player = &metadata.players[(path.id - player_id_start) as usize];
                state["players"].as_array_mut().unwrap().push(json!({
                    "champ": player.skin,
                    "name": "",
                    "pos": [x, y],
                    "role": role_label(player.role),
                    "team": team_label(player.team),
                }));
            }
            tick = packet.timestamp;
            game["players_state"].as_array_mut().unwrap().push(state);
        }
    }

    game
}

fn metadata_to_json(header: &FileHeader, m: &Metadata) -> Value {
    let players: Vec<Value> = m
        .players
        .iter()
        .map(|p| {
            json!({
                "name": "",
                "position": role_label(p.role),
                "skin": p.skin,
                "team": team_label(p.team),
            })
        })
        .collect();

    json!({
        "game_len": m.game_length_ms,
        "players": players,
        "version": mowokuma_version_string(&header.version),
        "winning_team": team_label(m.winning_team),
    })
}

/// Reproduce the exact version string Mowokuma's parser emits.
///
/// Her `Metadata::parse` reads `buffer[16..20]` and takes that as the
/// version: for patch 15.5 her output says `"5.5."` (4 chars, trailing
/// dot). Full spec in `docs/REFERENCE_MOWOKUMA.md` § Version-reading.
/// We reproduce the bug verbatim so downstream consumers of her JSON
/// schema see the same string.
fn mowokuma_version_string(full_version: &str) -> String {
    // Her reader: 4 ASCII bytes starting at offset 0x10 of the file,
    // which is the character *after* the length prefix. For a version
    // "15.5.xxx.yyyy", offset 0x10 is '5' (the leading digit), offsets
    // 0x11 / 0x12 / 0x13 are '.','5','.' respectively, so she gets "5.5.".
    // For a two-digit-minor version like "16.10.xxx.yyyy" she'd get
    // "6.10" (no trailing dot). Rebuild that.
    let bytes = full_version.as_bytes();
    if bytes.len() >= 4 {
        // Skip the first byte (leading '1') and take the next 4.
        let slice = &bytes[1..bytes.len().min(5)];
        if let Ok(s) = std::str::from_utf8(slice) {
            return s.to_string();
        }
    }
    full_version.to_string()
}

fn team_label(t: Team) -> &'static str {
    match t {
        Team::Blue => "Blue",
        Team::Red => "Red",
    }
}

fn role_label(r: Role) -> &'static str {
    match r {
        Role::Top => "Top",
        Role::Jungle => "Jungle",
        Role::Mid => "Mid",
        Role::Adc => "Adc",
        Role::Support => "Support",
    }
}

/// Convenience: runs both decode passes given a `Config`, returning the
/// finished JSON value.
///
/// This is feature-gated on `emulator` because the actual decoding calls
/// require the Unicorn backend. Without the feature, use
/// `build_output_json` directly once you've obtained the packet vectors
/// from some other source.
/// Matches Mowokuma's BATCH_SIZE in her `main.rs`. Every BATCH_SIZE packets
/// we create a fresh `StubEmulator` so heap state from earlier packets in
/// the same batch can affect decode, but not packets in later batches.
/// Keeping this equal to hers is load-bearing for byte-identical parity.
#[cfg(feature = "emulator")]
const BATCH_SIZE: usize = 100;

#[cfg(feature = "emulator")]
pub fn parse_and_decode(replay: &Replay<'_>, config: &Config) -> Result<Value> {
    use crate::emulator::StubEmulator;

    let ward_netid = config.ward_spawn_decrypt.netid as u16;
    let mov_netid = config.mov_decrypt.netid as u16;

    let ward_hits = blocks_with_netid(replay, ward_netid)?;
    let path_hits = blocks_with_netid(replay, mov_netid)?;

    let mut ward_packets: Vec<WardSpawnPacket> = Vec::with_capacity(ward_hits.len());
    for batch in ward_hits.chunks(BATCH_SIZE) {
        let mut emu = StubEmulator::new(config.clone());
        emu.setup()?;
        for (timestamp, payload) in batch {
            emu.setup_args(payload)?;
            let p = emu.call_decrypt_ward_spawn_packet(
                config.ward_spawn_decrypt.rva,
                config.ward_spawn_decrypt.end_rva,
                *timestamp,
            )?;
            ward_packets.push(p);
            emu.reset()?;
        }
    }

    let mut path_packets: Vec<PathPacket> = Vec::with_capacity(path_hits.len());
    for batch in path_hits.chunks(BATCH_SIZE) {
        let mut emu = StubEmulator::new(config.clone());
        emu.setup()?;
        for (timestamp, payload) in batch {
            emu.setup_args(payload)?;
            if let Ok(p) = emu.call_decrypt_pos_packet(
                config.mov_decrypt.rva,
                config.mov_decrypt.end_rva,
                *timestamp,
            ) {
                path_packets.push(p);
            }
            emu.reset()?;
        }
    }

    Ok(build_output_json(
        &replay.header,
        &replay.metadata,
        ward_packets,
        path_packets,
        config.player_id_start,
    ))
}
