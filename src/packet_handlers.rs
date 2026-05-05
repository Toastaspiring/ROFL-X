//! Typed handlers for the most-frequent packet classes.
//!
//! Each handler turns the emulator's raw `ExtraDecoded` (a struct-bytes
//! blob plus a list of writes) into a Rust struct with named fields. The
//! struct layouts come from `scripts/semantic_field_names.json` — they're
//! shape-anchored guesses rather than RTTI-proven, so the type tags
//! (`u32_or_f32`) get split into separate `_u32` / `_f32` fields and the
//! caller picks the interpretation that makes sense for their use.
//!
//! Coverage on a 27-min EUW replay (rough proportions of total events):
//!
//!   Class                                        share  notes
//!   PKT_S2C_ReplicateFields_s / ReplicateField_s   83%  generic property
//!                                                       replication, no
//!                                                       typed handler yet
//!   PKT_DirectInputMovementDriverServerTurnData_s   8%  via mov decoder
//!   PKT_Basic_Attack_Pos_s                          3%  handled here
//!   PKT_OnLeaveVisibilityClient_s                   2%  handled here
//!   PKT_CHAR_SetCooldown_Broadcast_s                1%  handled here
//!   PKT_S2C_AddBuffModifier_s                       1%  handled here
//!   PKT_NPC_BuffAdd2_s                              1%  handled here
//!
//! The replication classes are intentionally skipped: their payload is a
//! property-id + a free-form value, and the property table isn't decoded
//! yet. They'd give one typed event per replication but no semantic gain.
//!
//! ## Field-value caveats
//!
//! Field offsets come from `scripts/semantic_field_names.json`, which
//! is shape-anchored: an offset is named based on what *kind* of value
//! the decoder typically writes there (u32 vs f32, position-shaped vs
//! id-shaped). Two consequences:
//!
//! 1. Some offsets are correctly named but the decoder doesn't always
//!    write them — variable-length-decoded fields can be skipped per
//!    payload. The handler returns `None` for that whole packet when
//!    any required offset is missing. Empirically: ~40% of
//!    `BasicAttackPos` samples on EUW1-7841558966 produce a typed
//!    event; the rest hit a missing-write somewhere in the struct.
//! 2. A few offsets may be misaligned. The catalog naming was
//!    cross-validated via shape-match, not RTTI, so individual fields
//!    can be off by a slot. When a typed handler shows values that
//!    consistently look wrong (positions outside [0, 16000], spell
//!    hashes that don't look like 64-bit Murmur), the offset is the
//!    suspect — not the reader. Fix in the catalog and rebuild.

use serde::Serialize;

use crate::emulator::unicorn::ExtraDecoded;

/// A handler-decoded packet. One typed event ready for consumers.
///
/// The variant name matches the riot_class root (without the `PKT_` /
/// `_s` decoration), so external consumers can match by string or
/// pattern as they prefer.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum TypedPacket {
    BasicAttackPos(BasicAttackPos),
    BasicAttack(BasicAttack),
    OnLeaveVisibility(OnLeaveVisibility),
    SetCooldown(SetCooldown),
    AddBuffModifier(AddBuffModifier),
    NpcBuffAdd(NpcBuffAdd),
}

/// Basic attack landed: caster, target, end-position, spell hash,
/// windup, cooldown, mana cost, ability slot.
///
/// Decoder: 0x1074580 — PKT_Basic_Attack_Pos_s. Field offsets:
/// 0x18 caster, 0x20 source_xy, 0x30 target, 0x38 target_xy,
/// 0x40 target_end_xy, 0x48 spell_param_a, 0x50 spell_hash, 0x58 windup,
/// 0x5c cooldown, 0x60 mana, 0x64 slot.
#[derive(Debug, Clone, Serialize)]
pub struct BasicAttackPos {
    pub timestamp: f32,
    pub entity_id: u32,
    pub caster_or_source_id: u32,
    pub source_position_x: f32,
    pub source_position_y: f32,
    pub target_id: u32,
    pub target_position_x: f32,
    pub target_position_y: f32,
    pub target_end_position_x: f32,
    pub target_end_position_y: f32,
    pub spell_or_attack_param_a: f32,
    pub spell_hash: u64,
    pub windup_time_ms: f32,
    pub cooldown_ms: f32,
    pub mana_cost: f32,
    pub level_or_slot: u32,
}

/// Smaller variant of BasicAttackPos used for ~3K events on the
/// benchmark replay (vs ~22K for BasicAttackPos). Lacks the windup /
/// cooldown / spell-hash fields — likely the "auto-attack already
/// resolved" packet vs BasicAttackPos's "spell starting" packet.
///
/// Decoder: 0xf9bae0 — PKT_Basic_Attack_s.
#[derive(Debug, Clone, Serialize)]
pub struct BasicAttack {
    pub timestamp: f32,
    pub entity_id: u32,
    pub source_or_caster_id: u32,
    pub position_a_x: f32,
    pub position_a_y: f32,
    pub position_b_x: f32,
    pub position_b_y: f32,
    pub scalar_a_u32: u32,
    pub scalar_a_f32: f32,
    pub scalar_b_u32: u32,
    pub scalar_b_f32: f32,
    pub scalar_c_u32: u32,
    pub scalar_c_f32: f32,
    pub trailing_id_or_flag: u32,
    pub trailing_value: u32,
    pub trailing_value_b: u32,
}

/// Entity left visibility (fog of war). Pairs with EnterVisibility
/// (handled in a different decoder). The `visibility_flag_or_subtype`
/// distinguishes "left because dead" vs "left because fogged".
///
/// Decoder: 0xfaffc0 — PKT_OnLeaveVisibilityClient_s.
#[derive(Debug, Clone, Serialize)]
pub struct OnLeaveVisibility {
    pub timestamp: f32,
    pub entity_id: u32,
    pub leaver_net_id: u32,
    pub visibility_flag_or_subtype: u32,
}

/// Spell or summoner cooldown set on a target. `spell_slot` is the
/// ability index (Q=0/1, W, E, R, summoner1, summoner2 — exact
/// numbering depends on champion).
///
/// Decoder: 0xfdb200 — PKT_CHAR_SetCooldown_Broadcast_s.
#[derive(Debug, Clone, Serialize)]
pub struct SetCooldown {
    pub timestamp: f32,
    pub entity_id: u32,
    pub spell_slot: u8,
    pub cooldown_remaining_ms: f32,
    pub cooldown_total_ms: f32,
    pub target_net_id: u32,
    pub display_cooldown_ms: f32,
}

/// Add a buff modifier (stack, duration adjustment) to an existing buff
/// instance. Distinct from BuffAdd which creates a new buff.
///
/// Decoder: 0x107c470 — PKT_S2C_AddBuffModifier_s. Field types are
/// shape-anchored guesses, so each ambiguous slot is exposed as both
/// u32 and f32 — pick whichever matches the field's units.
#[derive(Debug, Clone, Serialize)]
pub struct AddBuffModifier {
    pub timestamp: f32,
    pub entity_id: u32,
    pub primary_id_or_value: u32,
    pub scalar_a_u32: u32,
    pub scalar_a_f32: f32,
    pub scalar_b_u32: u32,
    pub scalar_b_f32: f32,
    pub field_c: u32,
    pub field_d: u32,
    pub field_e: f32,
    pub field_f_u32: u32,
    pub field_f_f32: f32,
}

/// New buff added on an NPC (minion / monster / champion). Used for
/// items, dragon stacks, blue/red buffs, ability buffs, etc.
///
/// Decoder: 0xfcd970 — PKT_NPC_BuffAdd2_s.
///
/// Field semantics nailed down by the cross-replay validator:
///
///   offset  validator data (40K samples)             our name
///   ------  ---------------------------------------  -----------------
///   0x18    only 12% entity-shape (catalog mis-name) misc_id_at_0x18
///   0x2c    98% entity-shape, 1.4% zero              buff_target
///   0x34    86% entity-shape, 14% zero               caster_or_source
///
/// The catalog labels offset 0x18 as `entity_net_id`, which the
/// validator showed is wrong: it almost always reads as something
/// else (a buff/property/spell id). The real receiving entity is at
/// 0x2c, and the buff source / caster is at 0x34. Both renamed
/// here; the catalog remains untouched (it's regenerated from the
/// emulator and re-introducing a manual edit there would get clobbered).
#[derive(Debug, Clone, Serialize)]
pub struct NpcBuffAdd {
    pub timestamp: f32,
    pub entity_id: u32,
    /// Misnomer in the catalog. Empirically not the buffed entity —
    /// reads as a small id ~88% of the time. Kept in the struct for
    /// completeness; consumers should use `buff_target` instead when
    /// they want the entity receiving the buff.
    pub misc_id_at_0x18: u32,
    pub field_b_u32: u32,
    pub field_b_f32: f32,
    pub field_c_u32: u32,
    pub field_c_f32: f32,
    /// The entity receiving the buff. 98% entity-shape across 40K
    /// samples on the benchmark replay.
    pub buff_target: u32,
    pub field_d_f32: f32,
    /// The caster / source applying the buff. 86% entity-shape;
    /// 14% zero (system buffs, e.g. dragon soul applied without
    /// a champion source).
    pub caster_or_source: u32,
    pub field_e_f32: f32,
    pub field_f: f32,
    pub field_g_u32: u32,
    pub field_g_f32: f32,
    pub field_h: f32,
}

impl TypedPacket {
    /// Try to decode an `ExtraDecoded` blob as the typed packet matching
    /// `decoder_rva`. Returns `None` if the RVA isn't recognized (the
    /// other 90% of decoders).
    ///
    /// IMPORTANT: this reads the **plaintext** value from the writes log,
    /// not the post-obfuscation `struct_bytes`. Riot's decoders write the
    /// real value as a single atomic 4-byte (or 8-byte) `mov` first, then
    /// run a byte-by-byte XOR loop that overwrites the plaintext with the
    /// encrypted form. Reading struct_bytes returns the encrypted bytes;
    /// we want the first wide write per offset.
    ///
    /// The dispatch keys off the literal RVA strings from the semantic
    /// catalog. Other patches will need their own RVAs added — there's
    /// no automatic class-name resolution at runtime; that's the
    /// catalog's job, and the catalog is patch-specific.
    pub fn from_extra(
        decoder_rva: &str,
        timestamp: f32,
        entity_id: u32,
        decoded: &ExtraDecoded,
    ) -> Option<Self> {
        let s = PlaintextStruct::from(decoded);
        match decoder_rva {
            "0x1074580" => Some(TypedPacket::BasicAttackPos(BasicAttackPos {
                timestamp,
                entity_id,
                caster_or_source_id: s.u32_at(0x18)?,
                source_position_x: s.f32_at(0x20)?,
                source_position_y: s.f32_at(0x24)?,
                target_id: s.u32_at(0x30)?,
                target_position_x: s.f32_at(0x38)?,
                target_position_y: s.f32_at(0x3c)?,
                target_end_position_x: s.f32_at(0x40)?,
                target_end_position_y: s.f32_at(0x44)?,
                spell_or_attack_param_a: s.f32_at(0x48)?,
                spell_hash: s.u64_split_at(0x50)?,
                windup_time_ms: s.f32_at(0x58)?,
                cooldown_ms: s.f32_at(0x5c)?,
                mana_cost: s.f32_at(0x60)?,
                level_or_slot: s.u32_at(0x64)?,
            })),
            "0xf9bae0" => Some(TypedPacket::BasicAttack(BasicAttack {
                timestamp,
                entity_id,
                source_or_caster_id: s.u32_at(0x10)?,
                position_a_x: s.f32_at(0x18)?,
                position_a_y: s.f32_at(0x1c)?,
                position_b_x: s.f32_at(0x20)?,
                position_b_y: s.f32_at(0x24)?,
                scalar_a_u32: s.u32_at(0x2c)?,
                scalar_a_f32: s.f32_at(0x2c)?,
                scalar_b_u32: s.u32_at(0x30)?,
                scalar_b_f32: s.f32_at(0x30)?,
                scalar_c_u32: s.u32_at(0x34)?,
                scalar_c_f32: s.f32_at(0x34)?,
                trailing_id_or_flag: s.u32_at(0x40)?,
                trailing_value: s.u32_at(0x48)?,
                trailing_value_b: s.u32_at(0x4c)?,
            })),
            "0xfaffc0" => Some(TypedPacket::OnLeaveVisibility(OnLeaveVisibility {
                timestamp,
                entity_id,
                leaver_net_id: s.u32_at(0x10)?,
                visibility_flag_or_subtype: s.u32_at(0x14)?,
            })),
            "0xfdb200" => Some(TypedPacket::SetCooldown(SetCooldown {
                timestamp,
                entity_id,
                spell_slot: s.u8_at(0x0c)?,
                cooldown_remaining_ms: s.f32_at(0x10)?,
                cooldown_total_ms: s.f32_at(0x18)?,
                target_net_id: s.u32_at(0x1c)?,
                display_cooldown_ms: s.f32_at(0x24)?,
            })),
            "0x107c470" => Some(TypedPacket::AddBuffModifier(AddBuffModifier {
                timestamp,
                entity_id,
                primary_id_or_value: s.u32_at(0x14)?,
                scalar_a_u32: s.u32_at(0x18)?,
                scalar_a_f32: s.f32_at(0x18)?,
                scalar_b_u32: s.u32_at(0x1c)?,
                scalar_b_f32: s.f32_at(0x1c)?,
                field_c: s.u32_at(0x28)?,
                field_d: s.u32_at(0x38)?,
                field_e: s.f32_at(0x40)?,
                field_f_u32: s.u32_at(0x44)?,
                field_f_f32: s.f32_at(0x44)?,
            })),
            "0xfcd970" => Some(TypedPacket::NpcBuffAdd(NpcBuffAdd {
                timestamp,
                entity_id,
                misc_id_at_0x18: s.u32_at(0x18)?,
                field_b_u32: s.u32_at(0x1c)?,
                field_b_f32: s.f32_at(0x1c)?,
                field_c_u32: s.u32_at(0x20)?,
                field_c_f32: s.f32_at(0x20)?,
                buff_target: s.u32_at(0x2c)?,
                field_d_f32: s.f32_at(0x2c)?,
                caster_or_source: s.u32_at(0x34)?,
                field_e_f32: s.f32_at(0x34)?,
                field_f: s.f32_at(0x38)?,
                field_g_u32: s.u32_at(0x3c)?,
                field_g_f32: s.f32_at(0x3c)?,
                field_h: s.f32_at(0x40)?,
            })),
            _ => None,
        }
    }

    /// Class label that consumers can match for switch-style code. Same
    /// string as `riot_class_primary` from the catalog.
    pub fn class_label(&self) -> &'static str {
        match self {
            TypedPacket::BasicAttackPos(_) => "PKT_Basic_Attack_Pos_s",
            TypedPacket::BasicAttack(_) => "PKT_Basic_Attack_s",
            TypedPacket::OnLeaveVisibility(_) => "PKT_OnLeaveVisibilityClient_s",
            TypedPacket::SetCooldown(_) => "PKT_CHAR_SetCooldown_Broadcast_s",
            TypedPacket::AddBuffModifier(_) => "PKT_S2C_AddBuffModifier_s",
            TypedPacket::NpcBuffAdd(_) => "PKT_NPC_BuffAdd2_s",
        }
    }

    /// Timestamp (seconds from game start) for sorting / windowing.
    pub fn timestamp(&self) -> f32 {
        match self {
            TypedPacket::BasicAttackPos(p) => p.timestamp,
            TypedPacket::BasicAttack(p) => p.timestamp,
            TypedPacket::OnLeaveVisibility(p) => p.timestamp,
            TypedPacket::SetCooldown(p) => p.timestamp,
            TypedPacket::AddBuffModifier(p) => p.timestamp,
            TypedPacket::NpcBuffAdd(p) => p.timestamp,
        }
    }

    /// Receiving entity (block-level `param`). May be 0 when `param`
    /// carries a non-entity field for that netid.
    pub fn entity_id(&self) -> u32 {
        match self {
            TypedPacket::BasicAttackPos(p) => p.entity_id,
            TypedPacket::BasicAttack(p) => p.entity_id,
            TypedPacket::OnLeaveVisibility(p) => p.entity_id,
            TypedPacket::SetCooldown(p) => p.entity_id,
            TypedPacket::AddBuffModifier(p) => p.entity_id,
            TypedPacket::NpcBuffAdd(p) => p.entity_id,
        }
    }
}

/// Reconstructed plaintext view of the packet struct.
///
/// Riot's decoders write the real plaintext value first as a single wide
/// `mov`, then a byte-by-byte XOR pass overwrites it with the encrypted
/// form. The decoder's final `struct_bytes` is therefore POST-encryption
/// — useless. To get plaintext we walk the writes log and keep the FIRST
/// atomic 4-byte (or wider) write per offset.
///
/// This mirrors the logic in `replay_info.rs` that builds the
/// `decoded_fields` JSON; both consumers want the same reconstruction.
struct PlaintextStruct {
    /// Maps byte_offset -> plaintext byte. Byte-level (size 1, 2)
    /// writes are filtered out — those are the obfuscation pass and
    /// never carry the real value.
    bytes: std::collections::BTreeMap<u16, u8>,
}

impl PlaintextStruct {
    fn from(decoded: &ExtraDecoded) -> Self {
        // Last wide write wins: for variable-length-decoded fields the
        // initial inline-default constant gets overwritten by a callee
        // store with the real value. For constant-only branches the
        // last write equals the first. Either way, we want what was
        // there *before* the byte-XOR obfuscation pass — and the XOR
        // pass uses size-1 writes, which we exclude.
        let mut last_wide: std::collections::BTreeMap<u16, (u8, u64)> = Default::default();
        for (off, sz, val) in &decoded.writes {
            if *sz < 4 {
                continue;
            }
            last_wide.insert(*off, (*sz, *val));
        }
        let mut bytes: std::collections::BTreeMap<u16, u8> = Default::default();
        for (off, (sz, val)) in &last_wide {
            let val_le = val.to_le_bytes();
            for i in 0..*sz {
                bytes.insert(off + i as u16, val_le[i as usize]);
            }
        }
        Self { bytes }
    }

    fn u8_at(&self, off: usize) -> Option<u8> {
        self.bytes.get(&(off as u16)).copied()
    }

    fn u32_at(&self, off: usize) -> Option<u32> {
        let mut buf = [0u8; 4];
        for i in 0..4 {
            buf[i] = *self.bytes.get(&((off + i) as u16))?;
        }
        Some(u32::from_le_bytes(buf))
    }

    fn f32_at(&self, off: usize) -> Option<f32> {
        Some(f32::from_bits(self.u32_at(off)?))
    }

    /// 64-bit value stored as two atomic 4-byte halves at `off` (low)
    /// and `off+4` (high). Used for `spell_hash` and similar fields
    /// the decoder splits across two `mov dword ptr [...], imm32`.
    fn u64_split_at(&self, off: usize) -> Option<u64> {
        let lo = self.u32_at(off)? as u64;
        let hi = self.u32_at(off + 4)? as u64;
        Some(lo | (hi << 32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a writes log from a list of `(offset, value, size)` tuples
    /// — convenience for the tests, which think in terms of "write
    /// this u32 at offset 0x18" rather than the raw 3-tuple shape.
    fn writes_log(entries: &[(u16, u64, u8)]) -> Vec<(u16, u8, u64)> {
        entries.iter().map(|(o, v, s)| (*o, *s, *v)).collect()
    }

    fn extra_with(writes: Vec<(u16, u8, u64)>) -> ExtraDecoded {
        ExtraDecoded {
            writes,
            // Deliberately filled with 0xff so a handler that mistakenly
            // reads struct_bytes (post-obfuscation) instead of the
            // writes log (plaintext) blows up the assertions.
            struct_bytes: vec![0xff; 256],
            heap_writes: vec![],
            final_regs: Default::default(),
        }
    }

    #[test]
    fn basic_attack_pos_round_trip() {
        let extra = extra_with(writes_log(&[
            (0x18, 123, 4),
            (0x20, 7000.0f32.to_bits() as u64, 4),
            (0x24, 8000.0f32.to_bits() as u64, 4),
            (0x30, 456, 4),
            (0x38, 9000.0f32.to_bits() as u64, 4),
            (0x3c, 10000.0f32.to_bits() as u64, 4),
            (0x40, 11000.0f32.to_bits() as u64, 4),
            (0x44, 12000.0f32.to_bits() as u64, 4),
            (0x48, 1.5f32.to_bits() as u64, 4),
            (0x50, 0xdead_beef, 4),
            (0x54, 0xcafe_babe, 4),
            (0x58, 250.0f32.to_bits() as u64, 4),
            (0x5c, 7.5f32.to_bits() as u64, 4),
            (0x60, 30.0f32.to_bits() as u64, 4),
            (0x64, 3, 4),
        ]));
        let p = TypedPacket::from_extra("0x1074580", 12.5, 0x40000099, &extra).unwrap();
        if let TypedPacket::BasicAttackPos(p) = p {
            assert_eq!(p.entity_id, 0x40000099);
            assert_eq!(p.caster_or_source_id, 123);
            assert_eq!(p.target_id, 456);
            assert_eq!(p.spell_hash, 0xcafe_babe_dead_beef);
            assert_eq!(p.level_or_slot, 3);
            assert_eq!(p.cooldown_ms, 7.5);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn set_cooldown_round_trip() {
        let extra = extra_with(writes_log(&[
            // spell_slot is u8; emulator will still record it as 1-byte
            // and we should not look at it for plaintext (handler
            // documents this as one of the rare u8 fields, but reading
            // it from a 1-byte write would be wrong because byte writes
            // are the obfuscation pass — exclude). Test the realistic
            // case where the emulator did a 4-byte write that included
            // the slot byte.
            (0x0c, 2, 4),
            (0x10, 500.0f32.to_bits() as u64, 4),
            (0x18, 1000.0f32.to_bits() as u64, 4),
            (0x1c, 0x40000099, 4),
            (0x24, 450.0f32.to_bits() as u64, 4),
        ]));
        let p = TypedPacket::from_extra("0xfdb200", 99.0, 0x40000099, &extra).unwrap();
        if let TypedPacket::SetCooldown(p) = p {
            assert_eq!(p.spell_slot, 2);
            assert_eq!(p.cooldown_remaining_ms, 500.0);
            assert_eq!(p.cooldown_total_ms, 1000.0);
            assert_eq!(p.target_net_id, 0x40000099);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn obfuscation_bytes_are_ignored() {
        // The byte-level XOR pass writes 1-byte garbage. Plaintext came
        // from a 4-byte write at the same offset. The handler must
        // return the plaintext, not the byte-overwritten value.
        let mut writes = writes_log(&[(0x10, 500.0f32.to_bits() as u64, 4)]);
        // Add post-obfuscation byte writes that would corrupt the f32.
        writes.push((0x10, 1, 0x99));
        writes.push((0x11, 1, 0xab));
        writes.push((0x12, 1, 0xcd));
        writes.push((0x13, 1, 0xef));
        let extra = extra_with(writes);
        // Round-trip through SetCooldown — needs the rest of its fields:
        let mut writes = extra.writes.clone();
        writes.extend(writes_log(&[
            (0x0c, 2, 4),
            (0x18, 1000.0f32.to_bits() as u64, 4),
            (0x1c, 0x40000099, 4),
            (0x24, 450.0f32.to_bits() as u64, 4),
        ]));
        let extra = extra_with(writes);
        let p = TypedPacket::from_extra("0xfdb200", 0.0, 0, &extra).unwrap();
        if let TypedPacket::SetCooldown(p) = p {
            assert_eq!(p.cooldown_remaining_ms, 500.0, "byte writes leaked through");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn unknown_decoder_returns_none() {
        let extra = extra_with(vec![]);
        assert!(TypedPacket::from_extra("0xdeadbeef", 0.0, 0, &extra).is_none());
    }

    #[test]
    fn missing_writes_return_none() {
        // Empty writes log => no plaintext bytes => every field read
        // fails. The handler returns None on the first missing offset.
        let extra = extra_with(vec![]);
        assert!(TypedPacket::from_extra("0x1074580", 0.0, 0, &extra).is_none());
    }
}
