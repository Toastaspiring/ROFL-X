//! `PKT_UnitApplyDamage_s`: damage landed on a unit (after mitigation).
//!
//! | offset | type | field                                          |
//! |--------|------|------------------------------------------------|
//! | +0x14  | u8   | hit type (5 for nearly all champion damage)    |
//! | +0x18  | u32  | target net id                                  |
//! | +0x1c  | u32  | source effect hash (0 for plain attacks)       |
//! | +0x24  | u32  | source net id                                  |
//! | +0x28  | u16  | damage flags                                   |
//! | +0x2c  | f32  | amount                                         |
//!
//! Verified on 16.9: summing `amount` over champion-to-enemy-champion
//! packets reproduces statsJson `TOTAL_DAMAGE_DEALT_TO_CHAMPIONS` to
//! within one point (float rounding) for every player of every 16.9 replay
//! we hold. Hit type and flags are recorded, not interpreted.

use serde::Serialize;

use crate::emulator::DecodedPacket;

pub const CLASS: &str = "PKT_UnitApplyDamage_s";

#[derive(Debug, Clone, Serialize)]
pub struct Damage {
    pub source: u32,
    pub target: u32,
    pub amount: f32,
    pub hit_type: u8,
    pub flags: u16,
    pub effect_hash: u32,
}

pub fn read(p: &DecodedPacket) -> Option<Damage> {
    Some(Damage {
        source: p.u32(0x24)?,
        target: p.u32(0x18)?,
        amount: p.f32(0x2C)?,
        hit_type: p.u8(0x14)?,
        flags: p.u16(0x28)?,
        effect_hash: p.u32(0x1C)?,
    })
}
