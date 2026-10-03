//! `PKT_NPC_LevelUp_s`: a unit gained a level. Champions and lane minions
//! both send it.
//!
//! | offset | type | field                    |
//! |--------|------|--------------------------|
//! | +0x0c  | u32  | unit net id              |
//! | +0x10  | u8   | new level                |
//! | +0x11  | u8   | unspent skill points     |
//!
//! Verified on 16.9: each champion's highest level equals statsJson
//! `LEVEL` for every player of every 16.9 replay we hold.

use serde::Serialize;

use super::SENDER;
use crate::emulator::DecodedPacket;

pub const CLASS: &str = "PKT_NPC_LevelUp_s";

#[derive(Debug, Clone, Serialize)]
pub struct LevelUp {
    pub unit: u32,
    pub level: u8,
    pub skill_points: u8,
}

pub fn read(p: &DecodedPacket) -> Option<LevelUp> {
    Some(LevelUp {
        unit: p.u32(SENDER)?,
        level: p.u8(0x10)?,
        skill_points: p.u8(0x11)?,
    })
}
