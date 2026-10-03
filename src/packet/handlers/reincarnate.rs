//! `PKT_HeroReincarnateAlive_s`: a champion respawned.
//!
//! | offset | type | field            |
//! |--------|------|------------------|
//! | +0x0c  | u32  | hero net id      |
//! | +0x10  | f32  | x                |
//! | +0x14  | f32  | z                |
//!
//! Verified on 16.9: positions land on the team's fountain (≈394, 461 for
//! blue; ≈14340, 14391 for red) and follow each `PKT_NPC_Hero_Die_s` of
//! the same hero by its death timer.

use serde::Serialize;

use super::SENDER;
use crate::emulator::DecodedPacket;

pub const CLASS: &str = "PKT_HeroReincarnateAlive_s";

#[derive(Debug, Clone, Serialize)]
pub struct HeroReincarnate {
    pub hero: u32,
    pub x: f32,
    pub y: f32,
}

pub fn read(p: &DecodedPacket) -> Option<HeroReincarnate> {
    Some(HeroReincarnate {
        hero: p.u32(SENDER)?,
        x: p.f32(0x10)?,
        y: p.f32(0x14)?,
    })
}
