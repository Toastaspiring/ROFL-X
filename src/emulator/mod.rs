//! Per-patch emulator that decodes packet payloads by calling the game
//! client's own code inside a Unicorn x86-64 VM.
//!
//! Two archive formats:
//!
//! - **Format 1** (`config.rs`, `unicorn.rs`, `packet.rs`): Mowokuma's
//!   `.patch` archives. Two hand-reverse-engineered decoder functions
//!   (movement, ward spawn) and the struct offsets to read back. The
//!   approach, the archive format, the allocator shellcode and the 15.x
//!   RVAs are all her work (see `CREDITS.md`); this is a port.
//! - **Format 2** (`archive.rs`, `machine.rs`, `decoder.rs`): written by
//!   `scripts/build_patch.py`. Runs every packet class through the game's
//!   own netid factory and deserializer; see `docs/DECODING.md`.

pub mod archive;
pub mod config;
pub mod packet;

#[cfg(feature = "emulator")]
pub mod decoder;
#[cfg(feature = "emulator")]
pub mod machine;
#[cfg(feature = "emulator")]
pub mod unicorn;

pub use archive::{PacketName, PatchArchive};
pub use config::{Config, MovDecrypt, Section, WardSpawnDecrypt};
pub use packet::{PathPacket, PosKey, WardSpawnPacket};

#[cfg(feature = "emulator")]
pub use decoder::{DecodeStatus, DecodedPacket, PacketDecoder, PlainMemory};
#[cfg(feature = "emulator")]
pub use unicorn::StubEmulator;
