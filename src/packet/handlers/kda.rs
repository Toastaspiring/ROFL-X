//! `PKT_Update_HeroStats_KDA_s`: a champion's running kill / death /
//! assist totals, sent whenever one of them changes.
//!
//! | offset | type | field                 |
//! |--------|------|-----------------------|
//! | +0x0c  | u32  | hero net id           |
//! | +0x10  | u32  | deaths (cumulative)   |
//! | +0x14  | u32  | assists (cumulative)  |
//! | +0x18  | u32  | kills (cumulative)    |
//!
//! Verified on 16.9: each hero's last update equals statsJson
//! `NUM_DEATHS` / `ASSISTS` / `CHAMPIONS_KILLED` exactly, for every player
//! of every 16.9 replay we hold. Note the field order is D, A, K.

use serde::Serialize;

use super::SENDER;
use crate::emulator::DecodedPacket;

pub const CLASS: &str = "PKT_Update_HeroStats_KDA_s";

#[derive(Debug, Clone, Serialize)]
pub struct KdaUpdate {
    pub hero: u32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
}

pub fn read(p: &DecodedPacket) -> Option<KdaUpdate> {
    Some(KdaUpdate {
        hero: p.u32(SENDER)?,
        deaths: p.u32(0x10)?,
        assists: p.u32(0x14)?,
        kills: p.u32(0x18)?,
    })
}
