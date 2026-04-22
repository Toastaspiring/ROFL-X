//! ROFL-X: documented parser for League of Legends replay (`.rofl`) files.
//!
//! The crate is split into a transport layer (`rofl`) that reads bytes with
//! no game knowledge, and (forthcoming) a semantic layer (`packet`) that
//! decodes per-opcode payloads via an emulator backend. See
//! `docs/DOMAIN.md` and `docs/MODULE_LAYOUT.md` for the full picture.

pub mod cli;
pub mod error;
pub mod rofl;

pub use error::{Result, RoflError};
pub use rofl::Replay;
