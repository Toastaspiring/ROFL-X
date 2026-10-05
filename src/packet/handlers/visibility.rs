//! `PKT_S2C_OnEnterTeamVisibility_s` and
//! `PKT_S2C_OnLeaveTeamVisibility_s`: a unit entered or left one team's
//! vision. Together they are 39.5 % of every block in a 16.9 replay, by
//! far the busiest thing the server sends.
//!
//! | offset | type | field                                |
//! |--------|------|--------------------------------------|
//! | +0x0c  | u32  | unit net id                          |
//! | +0x10  | u8   | the team whose vision changed, 1 or 2 |
//!
//! Both classes have the same two fields; only the direction differs.
//!
//! Verified on 16.9 by an asymmetry the game guarantees: a unit is always
//! visible to its own team, so a visibility change can only ever name the
//! other one. Over the 1,713 blocks of one replay whose unit is a
//! champion — whose team the roster gives independently — every single
//! one named the opposite team: blue champions only ever carry 2, red
//! champions only ever carry 1, which also fixes 1 = ORDER (blue) and
//! 2 = CHAOS (red). `game.rs` re-runs that check on every replay and
//! reports it as `visibility n/n`.
//!
//! The third visibility class, `PKT_OnLeaveVisibilityClient_s` (netid 733
//! on 16.9), carries the net id alone with no team and is not typed here.

use serde::Serialize;

use super::SENDER;
use crate::emulator::DecodedPacket;

pub const ENTER_CLASS: &str = "PKT_S2C_OnEnterTeamVisibility_s";
pub const LEAVE_CLASS: &str = "PKT_S2C_OnLeaveTeamVisibility_s";

/// Team ids as the game numbers them.
pub const TEAM_ORDER: u8 = 1;
pub const TEAM_CHAOS: u8 = 2;

#[derive(Debug, Clone, Serialize)]
pub struct Visibility {
    pub unit: u32,
    /// The team that gained or lost sight of `unit`.
    pub team: u8,
    /// True for enter, false for leave.
    pub visible: bool,
}

pub fn read_enter(p: &DecodedPacket) -> Option<Visibility> {
    read(p, true)
}

pub fn read_leave(p: &DecodedPacket) -> Option<Visibility> {
    read(p, false)
}

fn read(p: &DecodedPacket, visible: bool) -> Option<Visibility> {
    let team = p.u8(0x10)?;
    if team != TEAM_ORDER && team != TEAM_CHAOS {
        return None;
    }
    Some(Visibility {
        unit: p.u32(SENDER)?,
        team,
        visible,
    })
}
