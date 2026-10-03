//! `PKT_NPC_Hero_Die_s`: a champion died.
//!
//! | offset | type | field                                         |
//! |--------|------|-----------------------------------------------|
//! | +0x0c  | u32  | victim net id                                 |
//! | +0x34  | f32  | x                                             |
//! | +0x38  | f32  | height                                        |
//! | +0x3c  | f32  | z (the map's second axis)                     |
//! | +0x54  | f32  | death timer, seconds                          |
//! | +0x58  | u32  | killer net id (0 when no unit is credited)    |
//!
//! Verified on 16.9: per-victim death counts equal statsJson `NUM_DEATHS`
//! for every player of every 16.9 replay we hold; the killer field agrees
//! with the kill counter in `PKT_Update_HeroStats_KDA_s`.

use serde::Serialize;

use super::SENDER;
use crate::emulator::DecodedPacket;

pub const CLASS: &str = "PKT_NPC_Hero_Die_s";

#[derive(Debug, Clone, Serialize)]
pub struct HeroDie {
    pub victim: u32,
    pub killer: u32,
    pub x: f32,
    pub y: f32,
    pub death_timer: f32,
}

pub fn read(p: &DecodedPacket) -> Option<HeroDie> {
    Some(HeroDie {
        victim: p.u32(SENDER)?,
        killer: p.u32(0x58)?,
        x: p.f32(0x34)?,
        y: p.f32(0x3C)?,
        death_timer: p.f32(0x54)?,
    })
}
