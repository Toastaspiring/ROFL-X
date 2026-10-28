use crate::error::{Result, RoflError};
use crate::rofl::stream::StreamTag;

/// Size of a `ChunkRecord` header in bytes, through 16.20.
pub const CHUNK_HEADER_SIZE: usize = 0x11; // 17

/// Record header sizes seen in the wild, in the order they are tried.
///
/// Through 16.20 a record is 17 bytes: id, type, id2, uncompressed
/// length, compressed length, then the body. **16.21 inserts one more
/// byte between the compressed length and the body**, carrying `0x04` in
/// every chunk of every replay measured — 1,018 of them across ten
/// games. Nothing in the file announces the change, and the five fields
/// before it still read correctly, so a parser that does not know about
/// it reads the first record plausibly and then lands in the middle of a
/// zstd frame: the chunk ids it reports next run to hundreds of
/// millions.
pub const CHUNK_HEADER_SIZES: [usize; 2] = [0x11, 0x12];

/// The magic a zstd frame starts with, which a compressed body carries.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// Which record size the chunks region is actually written with.
///
/// Decided by walking rather than by patch number, because the patch is
/// a label and the layout is a fact. The right size consumes the region
/// exactly; a wrong one fails almost at once, since a record claiming
/// compression whose body does not begin with the zstd magic cannot be
/// a record. Returns `None` when neither size works, which is a file
/// this parser does not understand rather than a file to guess at.
pub fn detect_record_size(buffer: &[u8], start: usize, end: usize) -> Option<usize> {
    CHUNK_HEADER_SIZES
        .into_iter()
        .find(|size| walks_exactly(buffer, start, end, *size))
}

/// Does every record from `start` to `end` read as one at this size?
fn walks_exactly(buffer: &[u8], start: usize, end: usize, size: usize) -> bool {
    let mut cursor = start;
    let mut seen = 0usize;
    while cursor < end {
        if cursor + size > end {
            return false;
        }
        let h = &buffer[cursor..cursor + size];
        let uncompressed = u32::from_le_bytes([h[9], h[10], h[11], h[12]]) as usize;
        let compressed = u32::from_le_bytes([h[13], h[14], h[15], h[16]]) as usize;
        let body_len = if compressed > 0 { compressed } else { uncompressed };
        let body_start = cursor + size;
        let Some(body_end) = body_start.checked_add(body_len) else {
            return false;
        };
        if body_end > end {
            return false;
        }
        // A compressed body is a zstd frame, so it says so.
        if compressed > 0
            && buffer
                .get(body_start..body_start + ZSTD_MAGIC.len())
                .is_none_or(|m| m != ZSTD_MAGIC)
        {
            return false;
        }
        seen += 1;
        cursor = body_end;
    }
    seen > 0 && cursor == end
}

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
    /// Bytes of record header, which 16.21 made one longer: see
    /// [`CHUNK_HEADER_SIZES`].
    record_size: usize,
}

impl<'a> ChunkIterator<'a> {
    pub fn new(buffer: &'a [u8], start: usize, end: usize, record_size: usize) -> Self {
        debug_assert!(start <= end);
        debug_assert!(end <= buffer.len());
        Self {
            buffer,
            cursor: start,
            end,
            record_size,
        }
    }
}

impl<'a> Iterator for ChunkIterator<'a> {
    type Item = Result<ChunkRecord<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.end {
            return None;
        }
        if self.cursor + self.record_size > self.end {
            return Some(Err(RoflError::ChunkBoundsExceeded { chunk_id: 0 }));
        }

        let h = &self.buffer[self.cursor..self.cursor + self.record_size];
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
        let body_start = self.cursor + self.record_size;
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

    /// A record size is measured, not assumed, because 16.21 made the
    /// header one byte longer and the file says so nowhere.
    ///
    /// The point of the test is that the wrong size is *rejected*, not
    /// merely that the right one works: the five fields before the new
    /// byte still read correctly at 17, so a parser that guesses reads
    /// the first record plausibly and then lands inside a zstd frame.
    #[test]
    fn the_record_size_is_measured_not_assumed() {
        /// One record: id, type, id2, uncompressed, compressed, then an
        /// `extra`-byte gap, then a body that looks like a zstd frame.
        fn record(id: u32, body: &[u8], extra: usize) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&id.to_le_bytes());
            v.push(2);
            v.extend_from_slice(&0x0300_0000u32.to_le_bytes());
            v.extend_from_slice(&999u32.to_le_bytes());
            v.extend_from_slice(&(body.len() as u32).to_le_bytes());
            v.extend(std::iter::repeat_n(0x04u8, extra));
            v.extend_from_slice(body);
            v
        }
        let frame = [0x28u8, 0xB5, 0x2F, 0xFD, 0x11, 0x22, 0x33];

        let short: Vec<u8> = (1..=3).flat_map(|i| record(i, &frame, 0)).collect();
        assert_eq!(
            detect_record_size(&short, 0, short.len()),
            Some(0x11),
            "a 17-byte region must be read as 17"
        );

        let long: Vec<u8> = (1..=3).flat_map(|i| record(i, &frame, 1)).collect();
        assert_eq!(
            detect_record_size(&long, 0, long.len()),
            Some(0x12),
            "16.21 inserts one byte before the body"
        );

        // Neither size can consume a region that is simply not records.
        let junk = vec![0x5Cu8; 64];
        assert_eq!(detect_record_size(&junk, 0, junk.len()), None);
    }

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

        let mut it = ChunkIterator::new(&buf, 0, buf.len(), CHUNK_HEADER_SIZE);
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

        let mut it = ChunkIterator::new(&buf, 0, buf.len(), CHUNK_HEADER_SIZE);
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
        let mut it = ChunkIterator::new(&buf, 0, buf.len(), CHUNK_HEADER_SIZE);
        let err = it.next().unwrap().unwrap_err();
        matches!(err, RoflError::ChunkBoundsExceeded { .. });
    }
}
