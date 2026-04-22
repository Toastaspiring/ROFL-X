//! Transport layer: file bytes to `Replay`, `ChunkRecord`s, `Block`s.
//!
//! Intentionally contains zero game-specific knowledge. The semantic layer
//! (packet decoding, entity tracking) lives under `crate::packet` (and its
//! emulator backend under `crate::emulator`).

pub mod block;
pub mod chunk;
pub mod decompress;
pub mod header;
pub mod metadata;
pub mod signature;
pub mod stream;

use crate::error::{Result, RoflError};

pub use header::FileHeader;
pub use metadata::{Metadata, Player, Role, Team};
pub use signature::{Signature, SIGNATURE_SIZE};

/// Size of the trailing u32 that gives the metadata-JSON length.
pub const METADATA_LEN_TRAILER_SIZE: usize = 4;

/// A successfully-parsed ROFL file.
///
/// Holds a borrow of the original byte buffer so chunks and blocks can be
/// sub-sliced without copying. The lifetime `'a` ties every downstream
/// iterator to the buffer's lifetime.
pub struct Replay<'a> {
    pub buffer: &'a [u8],
    pub header: FileHeader,
    pub metadata: Metadata,
    pub signature: Signature,
    /// Byte offset at which the chunks region begins (right after the header).
    pub chunks_start: usize,
    /// Byte offset at which the chunks region ends (right before the signature).
    pub chunks_end: usize,
}

impl<'a> Replay<'a> {
    /// Parse a whole-file byte buffer into a `Replay`.
    ///
    /// Parse order: header from the front, then metadata from the tail
    /// (using the trailing u32 length), then the signature, then use those
    /// bounds to compute the chunks region. This matches the structural
    /// dependencies documented in `docs/ROFL_FORMAT.md`.
    pub fn parse(buffer: &'a [u8]) -> Result<Self> {
        let header = FileHeader::parse(buffer)?;
        let chunks_start = header.size;

        if buffer.len() < METADATA_LEN_TRAILER_SIZE {
            return Err(RoflError::FileTooShort {
                got: buffer.len(),
                need: METADATA_LEN_TRAILER_SIZE,
            });
        }
        let meta_len_off = buffer.len() - METADATA_LEN_TRAILER_SIZE;
        let meta_len = u32::from_le_bytes([
            buffer[meta_len_off],
            buffer[meta_len_off + 1],
            buffer[meta_len_off + 2],
            buffer[meta_len_off + 3],
        ]) as usize;

        let need = chunks_start + SIGNATURE_SIZE + meta_len + METADATA_LEN_TRAILER_SIZE;
        if buffer.len() < need {
            return Err(RoflError::FileTooShort {
                got: buffer.len(),
                need,
            });
        }

        let meta_start = meta_len_off - meta_len;
        let metadata = Metadata::parse(&buffer[meta_start..meta_len_off])?;

        let sig_end = meta_start;
        let sig_start = sig_end - SIGNATURE_SIZE;
        let signature = Signature::from_slice(&buffer[sig_start..sig_end])
            .expect("signature size already validated above");

        Ok(Self {
            buffer,
            header,
            metadata,
            signature,
            chunks_start,
            chunks_end: sig_start,
        })
    }

    /// Iterator over `ChunkRecord`s in file order, across all streams.
    pub fn chunks(&self) -> chunk::ChunkIterator<'a> {
        chunk::ChunkIterator::new(self.buffer, self.chunks_start, self.chunks_end)
    }
}
