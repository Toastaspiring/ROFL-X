//! One file per packet class. Adding a class = a new file here, a `pub
//! mod` line below, and one arm in `packet::interpret` (or a recogniser in
//! `packet::recognise` for a class without a registered name).
//!
//! Offsets are byte offsets into the decoded packet object. `+0x0c` is the
//! base-class sender net id (the block's `param`) on every class.

pub mod damage;
pub mod hero_die;
pub mod items;
pub mod kda;
pub mod level_up;
pub mod npc_die;
pub mod reincarnate;
pub mod spawn;
pub mod waypoints;

/// Base-class field: the net id of the object the packet is about.
pub const SENDER: u64 = 0x0C;
