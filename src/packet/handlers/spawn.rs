//! Unit spawn (Zhu's `SpawnMinion`): lane minions, summons, plants, camp
//! markers, wards and ward corpses. No Riot class name is registered for
//! it, so it is recognised by shape.
//!
//! This is the class Mowokuma decoded on 15.x as `ward_spawn_decrypt`
//! (`WARD_SPAWN_OR_DESTROY`), and the ward lifecycle below is her idea:
//! a placement and its `*Corpse` are the same ward, so pairing them
//! gives a duration. Our one change is how they are paired — she
//! matches on an exact position, and on 16.9 the corpse can land ~100
//! units away, so we match the owner's nearest live ward instead.
//!
//! | offset | type        | field                                   |
//! |--------|-------------|-----------------------------------------|
//! | +0x0c  | u32         | new unit net id                         |
//! | +0x10  | u32         | team (1 order/blue, 2 chaos/red, 3 neutral) |
//! | +0x14  | f32         | x                                       |
//! | +0x18  | f32         | height                                  |
//! | +0x1c  | f32         | z                                       |
//! | +0x28  | f32         | spawn time, seconds                     |
//! | +0x30  | u32         | owner net id (0 when unowned)           |
//! | +0x38  | Riot string | object name, e.g. `YellowTrinket`       |
//! | +0x58  | Riot string | skin name, e.g. `WardCorpse`            |
//!
//! Verified on 16.9 (netid 214): ward spawns per champion equal statsJson
//! `WARD_PLACED`; a ward's `*WardCorpse` has the same owner and spawns
//! where the ward ended up (up to ~100 units from the placement point);
//! the spawn time equals the block timestamp.

use serde::Serialize;

use super::SENDER;
use crate::emulator::DecodedPacket;
use crate::packet::riot_string;

#[derive(Debug, Clone, Serialize)]
pub struct Spawn {
    pub unit: u32,
    pub team: u32,
    pub owner: u32,
    pub name: String,
    pub skin: String,
    pub x: f32,
    pub y: f32,
    pub time: f32,
}

pub fn read(p: &DecodedPacket) -> Option<Spawn> {
    let team = p.u32(0x10)?;
    if !(1..=3).contains(&team) {
        return None;
    }
    let name = riot_string(p, 0x38, 128)?;
    let skin = riot_string(p, 0x58, 128)?;
    let (x, z) = (p.f32(0x14)?, p.f32(0x1C)?);
    if !on_map(x) || !on_map(z) {
        return None;
    }
    Some(Spawn {
        unit: p.u32(SENDER)?,
        team,
        owner: p.u32(0x30)?,
        name,
        skin,
        x,
        y: z,
        time: p.f32(0x28)?,
    })
}

fn on_map(v: f32) -> bool {
    (-1000.0..16000.0).contains(&v)
}
