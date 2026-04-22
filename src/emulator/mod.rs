//! Per-patch emulator that decodes packet payloads by calling the game
//! client's own decoder functions inside a Unicorn x86-64 VM.
//!
//! The approach, the per-patch config format, the x86-64 shellcode that
//! stubs the client's allocator, and the specific RVAs and struct offsets
//! for the two decoded packet types are all Mowokuma's work (see
//! `CREDITS.md`). This module is a port of her `src/emulator/` tree.

pub mod config;
pub mod packet;

#[cfg(feature = "emulator")]
pub mod unicorn;

pub use config::{Config, MovDecrypt, Section, WardSpawnDecrypt};
pub use packet::{PathPacket, PosKey, WardSpawnPacket};

#[cfg(feature = "emulator")]
pub use unicorn::StubEmulator;
