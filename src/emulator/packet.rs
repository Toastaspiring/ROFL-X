//! Decoded packet structs returned by the emulator.
//!
//! Ported from Mowokuma's `src/emulator/packet.rs`. The `PathPacket::parse`
//! waypoint-decode routine is a near-verbatim port of the game client's
//! own decompiled code (preserved as she wrote it, including the Ghidra-
//! style variable names that made the correspondence with the decompiler
//! auditable). See CREDITS.md.

use crate::error::{Result, RoflError};

/// Integer-coordinate key for matching ward spawn and destruction events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PosKey {
    pub x: i32,
    pub y: i32,
}

impl PosKey {
    pub fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// One decoded ward-spawn packet.
///
/// The same opcode (ward-spawn netid) carries both placement events
/// (name = `"YellowTrinket"` / `"SightWard"` / `"JammerDevice"`) and
/// destruction events (name contains `"Corpse"`). The caller distinguishes
/// by looking at `name` and matches corpses to placements by `(x, y)`.
#[derive(Debug, Clone)]
pub struct WardSpawnPacket {
    pub timestamp: f32,
    pub name: String,
    pub id: u32,
    pub owner_id: u32,
    pub x: i32,
    pub y: i32,
}

/// One decoded movement packet: an entity id, its current speed, and a
/// waypoint chain in map coordinates.
#[derive(Debug, Clone)]
pub struct PathPacket {
    pub timestamp: f32,
    pub id: u32,
    pub speed: f32,
    pub waypoints: Vec<(f32, f32)>,
}

impl PathPacket {
    /// Build a `PathPacket` from the inline-float layout used on patch 16.9+.
    ///
    /// On those patches the position decoder writes `x` and `y` as two
    /// `f32`s directly into the output struct at known offsets, with no
    /// separate buffer. The waypoint list lives in a vector at different
    /// struct offsets that we don't yet decode (it would require driving
    /// the per-element vtable thunks the decoder calls), so this packet
    /// carries a single point in `waypoints`.
    ///
    /// `id` is `0` because the inline-float decoder we've identified
    /// (`fb4070` on 16.9) doesn't write the entity id into its output
    /// struct — that field is set by an enclosing dispatcher we haven't
    /// reverse-engineered yet. Callers that filter by `player_id_start`
    /// will see no matches.
    pub fn from_inline_floats(timestamp: f32, x: f32, y: f32) -> Self {
        Self {
            timestamp,
            id: 0,
            speed: 0.0,
            waypoints: vec![(x, y)],
        }
    }

    /// Parse a movement-packet body (post-decryption) into waypoints.
    ///
    /// Verbatim port of Mowokuma's `PathPacket::parse`. The unusual
    /// `v10/v13/v14/...` variable names are intentional: they match the
    /// decompiler output she traced the logic from, so future readers can
    /// diff against the game's own function if Riot changes it.
    pub fn parse(timestamp: f32, payload: Vec<u8>) -> Result<Self> {
        let mut payload_iter = payload.into_iter();

        let parsing_type = next_u16(&mut payload_iter)?;
        let ent_id = next_u32(&mut payload_iter)?;
        let ent_speed = next_f32(&mut payload_iter)?;

        if (parsing_type as u8 & 1) != 0 {
            payload_iter
                .next()
                .ok_or_else(|| mov_err("truncated after parsing_type"))?;
        }

        let temp_arr = payload_iter.clone().collect::<Vec<u8>>();

        let mut encoded_coords: Vec<u16> = Vec::new();

        let unk = (parsing_type as u8 >> 1) as u32;
        if unk == 0 {
            return Err(mov_err("invalid parsing_type (unk == 0)"));
        } else if unk > 1 {
            let unk2 = ((unk - 2) >> 2) + 1;
            payload_iter
                .nth(unk2 as usize - 1)
                .ok_or_else(|| mov_err("truncated consuming bitmap prefix"))?;
        }

        let mut v10: u32 = 0;
        let mut v13: i32 = 0;
        let mut y_coord: u16 = 0;
        let mut x_coord: u16 = 0;
        loop {
            let mut v14: i8 = 2;
            let mut v15: i8 = 2;
            if v10 != 0 {
                let mut v16 = v13;
                let mut v17: i8 = (v13 & 7) as i8;
                if v13 < 0 {
                    v16 = v13 + 7;
                    v17 -= 8;
                }
                let v18 = temp_arr[(v16 as usize) >> 3];
                let mut v19 = v13 + 1;
                let v20: i8 = -(((1i8 << v17) & v18 as i8) as i8);
                let mut v21: i8 = ((v13 + 1) & 7) as i8;
                v14 = 2 - (v20 != 0) as i8;
                if v19 < 0 {
                    v19 = v13 + 8;
                    v21 -= 8;
                }
                v15 = 2
                    - (((1i8 << v21) & temp_arr[(v19 as usize) >> 3] as i8) != 0) as i8;
                v13 += 2;
            }

            if v14 == 1 {
                x_coord = x_coord.wrapping_add(
                    payload_iter
                        .next()
                        .ok_or_else(|| mov_err("truncated at x-delta"))?
                        as u16,
                );
            } else {
                x_coord = next_u16(&mut payload_iter)?;
            }

            if v15 == 1 {
                y_coord = y_coord.wrapping_add(
                    payload_iter
                        .next()
                        .ok_or_else(|| mov_err("truncated at y-delta"))?
                        as u16,
                );
            } else {
                y_coord = next_u16(&mut payload_iter)?;
            }

            encoded_coords.push(x_coord);
            encoded_coords.push(y_coord);

            v10 += 1;
            if v10 >= unk {
                break;
            }
        }

        let mut path: Vec<(f32, f32)> = Vec::with_capacity(encoded_coords.len() / 2);
        let mut i = 0;
        while i + 1 < encoded_coords.len() {
            let x = (sign_extend_16(encoded_coords[i] as i16, 16) as f32 * 2.0) + 7358.0;
            let y = (sign_extend_16(encoded_coords[i + 1] as i16, 16) as f32 * 2.0) + 7412.0;
            path.push((x, y));
            i += 2;
        }

        Ok(Self {
            timestamp,
            id: ent_id,
            speed: ent_speed,
            waypoints: path,
        })
    }

    /// Interpolate the packet's position at a later `query_time`,
    /// walking along the waypoint chain using `speed` to convert segment
    /// length to time.
    pub fn get_pos(&self, query_time: f32) -> (f32, f32) {
        if self.waypoints.is_empty() {
            return (0.0, 0.0);
        }
        if self.waypoints.len() == 1 {
            return *self.waypoints.first().unwrap();
        }

        let delta = query_time - self.timestamp;
        if delta <= 1.0 {
            return *self.waypoints.first().unwrap();
        }

        let mut remaining = delta;
        for pair in self.waypoints.windows(2) {
            let (x1, y1) = pair[0];
            let (x2, y2) = pair[1];
            let dist = ((x1 - x2).powi(2) + (y1 - y2).powi(2)).sqrt();
            let dt = if self.speed > 0.0 { dist / self.speed } else { 0.0 };
            if remaining <= dt {
                let t = if dt > 0.0 { remaining / dt } else { 0.0 };
                return (x1 + (x2 - x1) * t, y1 + (y2 - y1) * t);
            }
            remaining -= dt;
        }

        *self.waypoints.last().unwrap()
    }
}

fn next_u16(it: &mut impl Iterator<Item = u8>) -> Result<u16> {
    let a = it
        .next()
        .ok_or_else(|| mov_err("truncated at u16 byte 0"))?;
    let b = it
        .next()
        .ok_or_else(|| mov_err("truncated at u16 byte 1"))?;
    Ok(u16::from_le_bytes([a, b]))
}

fn next_u32(it: &mut impl Iterator<Item = u8>) -> Result<u32> {
    let mut bs = [0u8; 4];
    for b in bs.iter_mut() {
        *b = it
            .next()
            .ok_or_else(|| mov_err("truncated at u32"))?;
    }
    Ok(u32::from_le_bytes(bs))
}

fn next_f32(it: &mut impl Iterator<Item = u8>) -> Result<f32> {
    Ok(f32::from_bits(next_u32(it)?))
}

fn sign_extend_16(num: i16, bits: u32) -> i16 {
    let shift = i16::BITS - bits;
    (num << shift) >> shift
}

fn mov_err(msg: &str) -> RoflError {
    RoflError::BlockFraming {
        offset: 0,
        message: format!("movement packet: {msg}"),
    }
}
