use std::io::Read;

use crate::error::{Result, RoflError};
use crate::rofl::chunk::ChunkRecord;

/// Decompress a chunk body.
///
/// If the record is uncompressed (`compressed_len == 0`), returns the raw
/// body as an owned `Vec<u8>`. Otherwise runs a zstd frame decoder and
/// verifies the output length against the header's `uncompressed_len`.
pub fn decompress_chunk(chunk: &ChunkRecord<'_>) -> Result<Vec<u8>> {
    if !chunk.is_compressed() {
        return Ok(chunk.body.to_vec());
    }

    let mut decoder = zstd::stream::read::Decoder::new(chunk.body).map_err(|e| {
        RoflError::Decompression {
            chunk_id: chunk.chunk_id,
            message: e.to_string(),
        }
    })?;
    let mut out = Vec::with_capacity(chunk.uncompressed_len as usize);
    decoder
        .read_to_end(&mut out)
        .map_err(|e| RoflError::Decompression {
            chunk_id: chunk.chunk_id,
            message: e.to_string(),
        })?;

    if out.len() != chunk.uncompressed_len as usize {
        return Err(RoflError::DecompressedLenMismatch {
            chunk_id: chunk.chunk_id,
            expected: chunk.uncompressed_len,
            actual: out.len(),
        });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rofl::stream::StreamTag;

    #[test]
    fn roundtrips_uncompressed_body() {
        let raw = vec![1u8, 2, 3, 4, 5];
        let rec = ChunkRecord {
            chunk_id: 42,
            chunk_type: 0,
            chunk_id_2: 0,
            stream_tag: StreamTag::GameChunk,
            uncompressed_len: raw.len() as u32,
            compressed_len: 0,
            body: &raw,
            offset: 0,
        };
        assert_eq!(decompress_chunk(&rec).unwrap(), raw);
    }

    #[test]
    fn decompresses_a_zstd_frame_we_encode_ourselves() {
        // Sanity check that the crate-side encode/decode agrees. This does
        // not depend on any Riot-specific bytes.
        let raw: Vec<u8> = (0..1024u16).map(|i| (i & 0xFF) as u8).collect();
        let encoded = zstd::bulk::compress(&raw, 3).unwrap();
        let rec = ChunkRecord {
            chunk_id: 1,
            chunk_type: 0,
            chunk_id_2: 0x01000000,
            stream_tag: StreamTag::GameChunk,
            uncompressed_len: raw.len() as u32,
            compressed_len: encoded.len() as u32,
            body: &encoded,
            offset: 0,
        };
        assert_eq!(decompress_chunk(&rec).unwrap(), raw);
    }
}
