//! ROFL-X: documented parser for League of Legends replay (`.rofl`) files.
//!
//! - `rofl`: transport layer. File bytes to header, metadata, chunks and
//!   blocks, with no game knowledge.
//! - `emulator`: runs the game's own code over packet payloads inside a
//!   Unicorn VM, from a per-patch `.patch` archive (Mowokuma's format 1,
//!   or format 2 for every packet class).
//! - `packet`: typed views over decoded packet objects, keyed by Riot's
//!   class names.
//! - `fields`: measured field layout of every class, for the ones no
//!   handler types yet.
//! - `summary`, `explore`: file-only analysis for patches whose client
//!   ships packed. No binary, no payloads, framing and metadata alone.
//! - `game`: whole-replay assembly (roster, events, positions, self-check).
//!
//! See `docs/DOMAIN.md`, `docs/MODULE_LAYOUT.md` and `docs/DECODING.md`.

pub mod cli;
pub mod emulator;
pub mod explore;
pub mod error;
pub mod replay_info;
pub mod rofl;
pub mod summary;

#[cfg(feature = "emulator")]
pub mod fields;
#[cfg(feature = "emulator")]
pub mod game;
#[cfg(feature = "emulator")]
pub mod packet;

pub use error::{Result, RoflError};
pub use rofl::Replay;
