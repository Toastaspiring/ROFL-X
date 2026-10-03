//! `PKT_NPC_CastSpellAns_s`: a unit started casting a spell (abilities,
//! summoner spells, item actives, trinket, recall, and some basic attacks).
//!
//! | offset | type | field                                                  |
//! |--------|------|--------------------------------------------------------|
//! | +0x0c  | u32  | casting unit (the champion, or an object it controls)  |
//! | +0x28  | u32  | spell instance net id                                  |
//! | +0x80  | u32  | spell id: a hash, one value per champion and slot      |
//! | +0x88  | f32  | cast time                                              |
//! | +0x8c  | u32  | owning champion net id                                 |
//! | +0xb0  | f32  | target x                                               |
//! | +0xb8  | f32  | target z                                               |
//! | +0xc0  | f32  | mana cost                                              |
//! | +0xc4  | u32  | spell slot (see `slot_name`)                           |
//! | +0xd8  | f32  | cooldown started, seconds (after ability haste)        |
//!
//! Verified on 16.9: slots follow the game's convention (0-3 Q/W/E/R,
//! 4/5 summoners, 6-11 item actives, 12 trinket, 13 recall, 46+ basic
//! attacks); trinket-slot casts equal the player's trinket-ward spawns
//! except for players who switched to a lens; summoner cooldowns read
//! 300 s for Flash, 300 / 1.1 and 300 / 1.18 with summoner haste, 15 s for
//! Smite, 240 s for Ghost; ultimates cost 100 mana.

use serde::Serialize;

use super::SENDER;
use crate::emulator::DecodedPacket;

pub const CLASS: &str = "PKT_NPC_CastSpellAns_s";

#[derive(Debug, Clone, Serialize)]
pub struct SpellCast {
    pub caster: u32,
    pub champion: u32,
    pub slot: u32,
    pub spell_id: u32,
    pub target_x: f32,
    pub target_y: f32,
    pub mana_cost: f32,
    pub cooldown: f32,
}

pub fn read(p: &DecodedPacket) -> Option<SpellCast> {
    Some(SpellCast {
        caster: p.u32(SENDER)?,
        champion: p.u32(0x8C)?,
        slot: p.u32(0xC4)?,
        spell_id: p.u32(0x80)?,
        target_x: p.f32(0xB0)?,
        target_y: p.f32(0xB8)?,
        mana_cost: p.f32(0xC0)?,
        cooldown: p.f32(0xD8)?,
    })
}

/// The game's spell-slot numbering; `None` for basic attacks (46+) and
/// anything unexpected.
pub fn slot_name(slot: u32) -> Option<&'static str> {
    Some(match slot {
        0 => "Q",
        1 => "W",
        2 => "E",
        3 => "R",
        4 => "D",
        5 => "F",
        6 => "item1",
        7 => "item2",
        8 => "item3",
        9 => "item4",
        10 => "item5",
        11 => "item6",
        12 => "trinket",
        13 => "recall",
        _ => return None,
    })
}
