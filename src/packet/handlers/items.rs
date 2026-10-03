//! Inventory answers sent to a champion: buy, remove (sell, or components
//! consumed by a recipe), swap, use.
//!
//! `PKT_BuyItemAns_s`
//!
//! | offset | type | field                                    |
//! |--------|------|------------------------------------------|
//! | +0x0c  | u32  | hero net id                              |
//! | +0x20  | u32  | item id (Data Dragon id)                 |
//! | +0x24  | u32  | inventory slot                           |
//! | +0x28  | f32  | price paid                               |
//! | +0x2c  | u32  | charges                                  |
//! | +0x8c  | f32  | gold left                                |
//!
//! `PKT_RemoveItemAns_s`: +0x11 u8 slot, +0x14 f32 gold left.
//! `PKT_SwapItemAns_s`: +0x10 u8 from slot, +0x11 u8 to slot.
//! `PKT_UseItemAns_s`: +0x10 u8 item survives (0 = used up), +0x11 u8
//! slot, +0x12 u8 charges left.
//!
//! Slots 0-5 are items, 6 the trinket, 8 the role-quest reward slot.
//! When a recipe completes, the buy arrives before the removals of its
//! components, at the same timestamp: replay removals first.
//!
//! Verified on 16.9: replaying these four packets per hero reproduces
//! statsJson `ITEM0`..`ITEM6` slot for slot, for every player of every
//! 16.9 replay we hold.

use serde::Serialize;

use super::SENDER;
use crate::emulator::DecodedPacket;

pub const BUY_CLASS: &str = "PKT_BuyItemAns_s";
pub const REMOVE_CLASS: &str = "PKT_RemoveItemAns_s";
pub const SWAP_CLASS: &str = "PKT_SwapItemAns_s";
pub const USE_CLASS: &str = "PKT_UseItemAns_s";

#[derive(Debug, Clone, Serialize)]
pub struct BuyItem {
    pub hero: u32,
    pub item: u32,
    pub slot: u8,
    pub price: f32,
    pub charges: u32,
    pub gold_left: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoveItem {
    pub hero: u32,
    pub slot: u8,
    pub gold_left: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwapItem {
    pub hero: u32,
    pub from: u8,
    pub to: u8,
}

#[derive(Debug, Clone, Serialize)]
pub struct UseItem {
    pub hero: u32,
    pub slot: u8,
    pub charges_left: u8,
    /// The item was used up and left the inventory.
    pub consumed: bool,
}

pub fn read_buy(p: &DecodedPacket) -> Option<BuyItem> {
    let slot = p.u32(0x24)?;
    Some(BuyItem {
        hero: p.u32(SENDER)?,
        item: p.u32(0x20)?,
        slot: u8::try_from(slot).ok()?,
        price: p.f32(0x28)?,
        charges: p.u32(0x2C)?,
        gold_left: p.f32(0x8C)?,
    })
}

pub fn read_remove(p: &DecodedPacket) -> Option<RemoveItem> {
    Some(RemoveItem {
        hero: p.u32(SENDER)?,
        slot: p.u8(0x11)?,
        gold_left: p.f32(0x14)?,
    })
}

pub fn read_swap(p: &DecodedPacket) -> Option<SwapItem> {
    Some(SwapItem {
        hero: p.u32(SENDER)?,
        from: p.u8(0x10)?,
        to: p.u8(0x11)?,
    })
}

pub fn read_use(p: &DecodedPacket) -> Option<UseItem> {
    let survives = p.u8(0x10)?;
    let charges_left = p.u8(0x12)?;
    Some(UseItem {
        hero: p.u32(SENDER)?,
        slot: p.u8(0x11)?,
        charges_left,
        consumed: survives == 0 && charges_left == 0,
    })
}
