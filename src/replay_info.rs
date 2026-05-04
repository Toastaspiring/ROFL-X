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
        // For inline-floats movement packets (16.9+), entity id is not
        // written by the decoder we've identified, so `players_state` stays
        // empty. The raw (timestamp, x, y) tuples land here so downstream
        // consumers can at least see decoded position data.
        "raw_positions": [],
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
        } else if packet.id == 0 {
            // Inline-floats packet (16.9+ partial decoder). Land it in
            // raw_positions so the data is visible even without entity
            // attribution.
            if let Some(&(x, y)) = packet.waypoints.first() {
                game["raw_positions"].as_array_mut().unwrap().push(json!({
                    "timestamp": packet.timestamp,
                    "x": x,
                    "y": y,
                }));
            }
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

/// Format a contiguous byte run captured at the FIRST write per offset
/// into a JSON object that lets a downstream consumer read the value
/// as the most likely concrete type.
///
/// We don't know which interpretation is right per offset (it depends
/// on the packet class), so we surface all of them — `u32_le`, `i32_le`,
/// `f32_le`, `u64_le` — and let the consumer pick.
fn field_summary(offset: u16, bytes: &[u8]) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("offset".to_string(), json!(offset));
    out.insert(
        "hex".to_string(),
        json!(bytes
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>()),
    );
    if bytes.len() >= 4 {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(&bytes[..4]);
        let u = u32::from_le_bytes(buf);
        out.insert("u32_le".to_string(), json!(u));
        out.insert("i32_le".to_string(), json!(u as i32));
        out.insert(
            "f32_le".to_string(),
            json!(f32::from_le_bytes(buf) as f64),
        );
    }
    if bytes.len() >= 8 {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&bytes[..8]);
        out.insert("u64_le".to_string(), json!(u64::from_le_bytes(buf)));
    }
    Value::Object(out)
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
            if let Ok(p) = emu.call_decrypt_ward_spawn_packet(
                config.ward_spawn_decrypt.rva,
                config.ward_spawn_decrypt.end_rva,
                *timestamp,
            ) {
                ward_packets.push(p);
            }
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

    // Run any extra-decoders declared in the patch archive. These are
    // long-tail packet classes whose struct layout we haven't yet RE'd:
    // we capture the raw struct writes per packet and surface them under
    // the `extra_decoders` field of the output JSON for downstream
    // inspection. This is the long-term path to "every packet decoded":
    // archive entries here grow as more decoders get RE'd.
    let mut extras: BTreeMap<String, Vec<(f32, crate::emulator::unicorn::ExtraDecoded)>> =
        BTreeMap::new();
    for ed in &config.extra_decoders {
        let hits = blocks_with_netid(replay, ed.netid as u16)?;
        if hits.is_empty() {
            continue;
        }
        let mut entries: Vec<(f32, crate::emulator::unicorn::ExtraDecoded)> = Vec::new();
        // Cap per-class samples so we don't blow the JSON up to gigabytes
        // on heartbeat-class netids that fire millions of times.
        const MAX_SAMPLES_PER_EXTRA: usize = 30;
        for batch in hits.chunks(BATCH_SIZE).take(
            (MAX_SAMPLES_PER_EXTRA + BATCH_SIZE - 1) / BATCH_SIZE,
        ) {
            let mut emu = StubEmulator::new(config.clone());
            emu.setup()?;
            for (timestamp, payload) in batch.iter().take(MAX_SAMPLES_PER_EXTRA - entries.len()) {
                emu.setup_args(payload)?;
                if let Ok(decoded) = emu.call_decrypt_extra(ed.rva_start, ed.rva_end, ed.struct_size)
                {
                    entries.push((*timestamp, decoded));
                }
                emu.reset()?;
                if entries.len() >= MAX_SAMPLES_PER_EXTRA {
                    break;
                }
            }
        }
        let label = format!("{}_netid{}", ed.name, ed.netid);
        extras.insert(label, entries);
    }

    let mut json = build_output_json(
        &replay.header,
        &replay.metadata,
        ward_packets,
        path_packets,
        config.player_id_start,
    );

    // Attach extra-decoder dumps. Layout:
    //   "extra_decoders": {
    //     "<name>_netid<N>": [
    //       { "timestamp": 12.34, "writes": [[off, sz, val], ...], "struct_hex": "..." }
    //     ]
    //   }
    if !extras.is_empty() {
        let mut by_name = serde_json::Map::new();
        for (label, entries) in extras {
            let arr: Vec<Value> = entries
                .iter()
                .map(|(ts, decoded)| {
                    let writes: Vec<Value> = decoded
                        .writes
                        .iter()
                        .map(|(off, sz, val)| json!([*off, *sz, *val]))
                        .collect();
                    let struct_hex: String = decoded
                        .struct_bytes
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect();

                    // Pre-obfuscation field extraction.
                    //
                    // Riot's decoders typically write a field as a single
                    // atomic 4-byte (or 8-byte) `mov` first — that's the
                    // plaintext value — then run a byte-by-byte obfuscation
                    // loop that overwrites with the encrypted form. Same for
                    // values stored as inline constants (an `f32 const` like
                    // -1.0 or 2.0) and for variable-length-decoded values
                    // (a callee writes the result as one wide store).
                    //
                    // We split the writes into:
                    //   - "atomic" writes (size >= 4): the function's
                    //     intentional field stores. We keep these.
                    //   - "byte" writes (size 1, 2): typically the
                    //     obfuscation transform. We ignore these.
                    //
                    // For each offset we keep the FIRST atomic write — that's
                    // the pre-obfuscation plaintext value of the field.
                    // Track the LAST atomic write per offset, with size >= 4.
                    // For a constant-only branch this is identical to the
                    // first write. For variable-length-decoded fields, the
                    // initial constant gets overwritten by the variable
                    // value (often by a callee writing a wide store), so
                    // last-write captures the actually-decoded payload.
                    // Byte-level transforms (size 1) are excluded because
                    // they're the obfuscation pass.
                    let mut first_writes: BTreeMap<u16, (u8, u64)> = BTreeMap::new();
                    for (off, sz, val) in &decoded.writes {
                        if *sz < 4 {
                            continue;
                        }
                        first_writes.insert(*off, (*sz, *val));
                    }
                    // Walk first_writes and reconstruct contiguous runs as
                    // possible field values.
                    let mut decoded_fields: Vec<Value> = Vec::new();
                    let mut bytes_at_offset: BTreeMap<u16, u8> = BTreeMap::new();
                    for (off, (sz, val)) in &first_writes {
                        let val_le = val.to_le_bytes();
                        for i in 0..*sz {
                            let byte_off = off + i as u16;
                            // Only set if not already (first write wins).
                            bytes_at_offset.entry(byte_off).or_insert(val_le[i as usize]);
                        }
                    }
                    // Group contiguous bytes into 4-byte aligned spans where
                    // possible.
                    let mut group_off: Option<u16> = None;
                    let mut group_bytes: Vec<u8> = Vec::new();
                    let offsets: Vec<u16> = bytes_at_offset.keys().copied().collect();
                    let mut prev: Option<u16> = None;
                    for o in &offsets {
                        if let Some(p) = prev {
                            if *o != p + 1 || group_bytes.len() >= 4 {
                                if let Some(g) = group_off {
                                    decoded_fields.push(field_summary(g, &group_bytes));
                                }
                                group_off = Some(*o);
                                group_bytes.clear();
                            }
                        } else {
                            group_off = Some(*o);
                        }
                        group_bytes.push(bytes_at_offset[o]);
                        prev = Some(*o);
                    }
                    if let Some(g) = group_off {
                        decoded_fields.push(field_summary(g, &group_bytes));
                    }

                    json!({
                        "timestamp": ts,
                        "writes": writes,
                        "struct_hex": struct_hex,
                        "decoded_fields": decoded_fields,
                    })
                })
                .collect();
            by_name.insert(label, Value::Array(arr));
        }
        json["extra_decoders"] = Value::Object(by_name);
    }

    Ok(json)
}
