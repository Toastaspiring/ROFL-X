use crate::error::{Result, RoflError};
use crate::rofl::stream::StreamTag;

/// Size of a `ChunkRecord` header in bytes.
pub const CHUNK_HEADER_SIZE: usize = 0x11; // 17

/// One chunk record in the file, as observed on disk.
///
/// `body` is either the compressed zstd frame (when `compressed_len > 0`)
/// or the raw uncompressed bytes (when `compressed_len == 0`).
#[derive(Debug, Clone)]
pub struct ChunkRecord<'a> {
    pub chunk_id: u32,
    pub chunk_type: u8,
    pub chunk_id_2: u32,
    pub stream_tag: StreamTag,
    pub uncompressed_len: u32,
    pub compressed_len: u32,
    pub body: &'a [u8],
    /// Byte offset of this record's header in the whole-file buffer.
    pub offset: usize,
}

impl<'a> ChunkRecord<'a> {
    pub fn is_compressed(&self) -> bool {
        self.compressed_len > 0
    }
}

/// Iterator over `ChunkRecord`s across the chunks region.
pub struct ChunkIterator<'a> {
    buffer: &'a [u8],
    cursor: usize,
    end: usize,
}

impl<'a> ChunkIterator<'a> {
    pub fn new(buffer: &'a [u8], start: usize, end: usize) -> Self {
        debug_assert!(start <= end);
        debug_assert!(end <= buffer.len());
        Self {
            buffer,
            cursor: start,
            end,
        }
    }
}

impl<'a> Iterator for ChunkIterator<'a> {
    type Item = Result<ChunkRecord<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.end {
            return None;
        }
        if self.cursor + CHUNK_HEADER_SIZE > self.end {
            return Some(Err(RoflError::ChunkBoundsExceeded { chunk_id: 0 }));
        }

        let h = &self.buffer[self.cursor..self.cursor + CHUNK_HEADER_SIZE];
        let chunk_id = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
        let chunk_type = h[4];
        let chunk_id_2 = u32::from_le_bytes([h[5], h[6], h[7], h[8]]);
        let uncompressed_len = u32::from_le_bytes([h[9], h[10], h[11], h[12]]);
        let compressed_len = u32::from_le_bytes([h[13], h[14], h[15], h[16]]);

        let body_len = if compressed_len > 0 {
            compressed_len as usize
        } else {
            uncompressed_len as usize
        };
        let body_start = self.cursor + CHUNK_HEADER_SIZE;
        let body_end = body_start.saturating_add(body_len);
        if body_end > self.end {
            return Some(Err(RoflError::ChunkBoundsExceeded { chunk_id }));
        }

        let rec = ChunkRecord {
            chunk_id,
            chunk_type,
            chunk_id_2,
            stream_tag: StreamTag::from_chunk_id_2(chunk_id_2),
            uncompressed_len,
            compressed_len,
            body: &self.buffer[body_start..body_end],
            offset: self.cursor,
        };
        self.cursor = body_end;
        Some(Ok(rec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sample_a_first_chunk_header() {
        // Chunk 0 from sample_a (keyframe, 17-byte header + 5852-byte body).
        let mut buf = vec![0u8; CHUNK_HEADER_SIZE + 5852];
        buf[0..17].copy_from_slice(&[
            0x01, 0x00, 0x00, 0x00, // chunk_id = 1
            0x02, // chunk_type = 2
            0x00, 0x00, 0x00, 0x03, // chunk_id_2 = 0x03000000
            0xFD, 0x46, 0x00, 0x00, // uncompressed_len = 18173
            0xDC, 0x16, 0x00, 0x00, // compressed_len = 5852
        ]);

        let mut it = ChunkIterator::new(&buf, 0, buf.len());
        let rec = it.next().unwrap().unwrap();
        assert_eq!(rec.chunk_id, 1);
        assert_eq!(rec.chunk_type, 0x02);
        assert_eq!(rec.chunk_id_2, 0x03000000);
        assert_eq!(rec.stream_tag, StreamTag::StartKeyframe);
        assert_eq!(rec.uncompressed_len, 18173);
        assert_eq!(rec.compressed_len, 5852);
        assert_eq!(rec.body.len(), 5852);
        assert_eq!(rec.offset, 0);

        // No more chunks after this one fills the buffer.
        assert!(it.next().is_none());
    }

    #[test]
    fn advances_by_uncompressed_when_clen_zero() {
        // Chunk 1 from sample_a: type 0x03, ulen=17, clen=0, 17-byte raw body.
        let mut buf = vec![0u8; CHUNK_HEADER_SIZE + 17];
        buf[0..17].copy_from_slice(&[
            0x02, 0x00, 0x00, 0x00, // chunk_id = 2
            0x03, // chunk_type = 3
            0x00, 0x00, 0x00, 0x04, // chunk_id_2 = 0x04000000
            0x11, 0x00, 0x00, 0x00, // uncompressed_len = 17
            0x00, 0x00, 0x00, 0x00, // compressed_len = 0
        ]);

        let mut it = ChunkIterator::new(&buf, 0, buf.len());
        let rec = it.next().unwrap().unwrap();
        assert!(!rec.is_compressed());
        assert_eq!(rec.body.len(), 17);
        assert_eq!(rec.stream_tag, StreamTag::StartSentinel);
        assert!(it.next().is_none());
    }

    #[test]
    fn flags_body_overrun() {
        // Header claims a 1000-byte body but only 17 bytes of buffer follow.
        let mut buf = vec![0u8; CHUNK_HEADER_SIZE + 17];
        buf[0..17].copy_from_slice(&[
            0x01, 0x00, 0x00, 0x00, //
            0x00, //
            0x00, 0x00, 0x00, 0x01, //
            0xE8, 0x03, 0x00, 0x00, // ulen = 1000
            0x00, 0x00, 0x00, 0x00, // clen = 0
        ]);
        let mut it = ChunkIterator::new(&buf, 0, buf.len());
        let err = it.next().unwrap().unwrap_err();
        matches!(err, RoflError::ChunkBoundsExceeded { .. });
    }
}
