//! Semantic layer: typed views over packet objects decoded by
//! `emulator::PacketDecoder`.
//!
//! Handlers are keyed by Riot's class name (`PKT_NPC_Hero_Die_s`, recovered
//! from the handler registrations in the binary), which survives patches;
//! netids do not. Two classes the replay needs have no registered name and
//! are recognised by the shape of their decoded object instead
//! (`handlers::waypoints`, `handlers::spawn`).
//!
//! Every field offset below is listed in `docs/PACKETS.md` with the patch
//! it was verified on and the check that verified it.

pub mod handlers;

use serde::Serialize;

use crate::emulator::DecodedPacket;

pub use handlers::damage::Damage;
pub use handlers::hero_die::HeroDie;
pub use handlers::items::{BuyItem, RemoveItem, SwapItem, UseItem};
pub use handlers::kda::KdaUpdate;
pub use handlers::level_up::LevelUp;
pub use handlers::npc_die::{BuildingDie, NpcDie};
pub use handlers::reincarnate::HeroReincarnate;
pub use handlers::spawn::Spawn;
pub use handlers::waypoints::{Path, WaypointGroup};

/// One typed packet.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GamePacket {
    HeroDie(HeroDie),
    HeroReincarnate(HeroReincarnate),
    Kda(KdaUpdate),
    LevelUp(LevelUp),
    Damage(Damage),
    BuyItem(BuyItem),
    RemoveItem(RemoveItem),
    SwapItem(SwapItem),
    UseItem(UseItem),
    NpcDie(NpcDie),
    BuildingDie(BuildingDie),
    Spawn(Spawn),
    Waypoints(WaypointGroup),
}

/// Typed read of a packet whose class has a registered Riot name.
pub fn interpret(class: &str, p: &DecodedPacket) -> Option<GamePacket> {
    use handlers::*;
    match class {
        hero_die::CLASS => hero_die::read(p).map(GamePacket::HeroDie),
        reincarnate::CLASS => reincarnate::read(p).map(GamePacket::HeroReincarnate),
        kda::CLASS => kda::read(p).map(GamePacket::Kda),
        level_up::CLASS => level_up::read(p).map(GamePacket::LevelUp),
        damage::CLASS => damage::read(p).map(GamePacket::Damage),
        items::BUY_CLASS => items::read_buy(p).map(GamePacket::BuyItem),
        items::REMOVE_CLASS => items::read_remove(p).map(GamePacket::RemoveItem),
        items::SWAP_CLASS => items::read_swap(p).map(GamePacket::SwapItem),
        items::USE_CLASS => items::read_use(p).map(GamePacket::UseItem),
        npc_die::NPC_CLASS => npc_die::read_npc(p).map(GamePacket::NpcDie),
        npc_die::BUILDING_CLASS => npc_die::read_building(p).map(GamePacket::BuildingDie),
        _ => None,
    }
}

/// Classes recognised by the shape of their decoded object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Shape {
    Waypoints,
    Spawn,
}

impl Shape {
    pub fn label(self) -> &'static str {
        match self {
            Shape::Waypoints => "WaypointGroup",
            Shape::Spawn => "SpawnMinion",
        }
    }
}

/// Typed read of an unnamed class, if its object has one of the known
/// shapes. Callers vote per netid (`game.rs`) before trusting the result:
/// one packet matching a shape proves little, a netid whose every packet
/// matches it proves a lot.
pub fn recognise(p: &DecodedPacket) -> Option<(Shape, GamePacket)> {
    if let Some(w) = handlers::waypoints::read(p) {
        return Some((Shape::Waypoints, GamePacket::Waypoints(w)));
    }
    if let Some(s) = handlers::spawn::read(p) {
        return Some((Shape::Spawn, GamePacket::Spawn(s)));
    }
    None
}

/// Riot's string as laid out inside packet objects: `{ char* data; u32
/// size; u32 capacity }`, data on the heap.
pub(crate) fn riot_string(p: &DecodedPacket, off: u64, max: u32) -> Option<String> {
    let ptr = p.u64(off)?;
    let size = p.u32(off + 8)?;
    let cap = p.u32(off + 12)?;
    if size == 0 || size > max || cap < size {
        return None;
    }
    let bytes = p.memory.bytes(ptr, size as usize)?;
    if !bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}
