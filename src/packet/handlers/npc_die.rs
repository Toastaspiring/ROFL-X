//! Non-champion deaths.
//!
//! `PKT_NPC_Die_MapView_s`: a minion, monster, ward or summon died where
//! the map view shows it.
//!
//! | offset | type | field             |
//! |--------|------|-------------------|
//! | +0x0c  | u32  | dead unit net id  |
//! | +0x34  | f32  | x                 |
//! | +0x3c  | f32  | z                 |
//! | +0x58  | u32  | killer net id     |
//!
//! Not a CS counter: on 16.9 its per-champion kill counts come within a
//! few percent of statsJson minions + monsters for laners but miss camps
//! the map view does not show (a jungler's count runs ~25 % short).
//!
//! `PKT_Building_Die_s`: a turret, inhibitor or nexus fell.
//!
//! | offset | type | field                                  |
//! |--------|------|----------------------------------------|
//! | +0x0c  | u32  | building net id                        |
//! | +0x10  | u32  | killer net id (0 for a minion / none)  |
//! | +0x14  | u32  | last unit credited                     |

use serde::Serialize;

use super::SENDER;
use crate::emulator::DecodedPacket;

pub const NPC_CLASS: &str = "PKT_NPC_Die_MapView_s";
pub const BUILDING_CLASS: &str = "PKT_Building_Die_s";

#[derive(Debug, Clone, Serialize)]
pub struct NpcDie {
    pub unit: u32,
    pub killer: u32,
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildingDie {
    pub building: u32,
    pub killer: u32,
    pub last_credited: u32,
}

pub fn read_npc(p: &DecodedPacket) -> Option<NpcDie> {
    Some(NpcDie {
        unit: p.u32(SENDER)?,
        killer: p.u32(0x58)?,
        x: p.f32(0x34)?,
        y: p.f32(0x3C)?,
    })
}

pub fn read_building(p: &DecodedPacket) -> Option<BuildingDie> {
    Some(BuildingDie {
        building: p.u32(SENDER)?,
        killer: p.u32(0x10)?,
        last_credited: p.u32(0x14)?,
    })
}
