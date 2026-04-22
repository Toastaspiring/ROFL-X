use crate::error::{Result, RoflError};

/// Marker bit: when set, timestamp is a `u8` millisecond delta.
pub const MARKER_TS_RELATIVE: u8 = 0x80;
/// Marker bit: when set, `packet_id` is reused from the previous block.
pub const MARKER_REUSE_PACKET_ID: u8 = 0x40;
/// Marker bit: when set, `param` is a `u8` delta added to the previous param.
pub const MARKER_PARAM_RELATIVE: u8 = 0x20;
/// Marker bit: when set, `length` is a `u8`; otherwise a `u32`.
pub const MARKER_LEN_U8: u8 = 0x10;

/// One packet within a decompressed chunk.
#[derive(Debug, Clone)]
pub struct Block<'a> {
    /// Accumulated timestamp in seconds.
    pub timestamp: f32,
    pub length: u32,
    pub packet_id: u16,
    pub param: u32,
    pub payload: &'a [u8],
    /// Byte offset of this block's start within the decompressed chunk.
    pub offset: usize,
}

/// Iterator over blocks inside a single decompressed chunk body.
///
/// State carried between blocks: the time accumulator, previous packet id,
/// previous param. All initialised to zero at the start of each chunk.
pub struct BlockIterator<'a> {
    buffer: &'a [u8],
    cursor: usize,
    acc_time: f32,
    prev_packet_id: u16,
    prev_param: u32,
}

impl<'a> BlockIterator<'a> {
    pub fn new(buffer: &'a [u8]) -> Self {
        Self {
            buffer,
            cursor: 0,
            acc_time: 0.0,
            prev_packet_id: 0,
            prev_param: 0,
        }
    }

    fn need(&self, bytes: usize) -> Result<()> {
        if self.cursor + bytes > self.buffer.len() {
            Err(RoflError::BlockFraming {
                offset: self.cursor,
                message: format!(
                    "need {} more bytes, only {} available",
                    bytes,
                    self.buffer.len() - self.cursor
                ),
            })
        } else {
            Ok(())
        }
    }

    fn read_u8(&mut self) -> Result<u8> {
        self.need(1)?;
        let b = self.buffer[self.cursor];
        self.cursor += 1;
        Ok(b)
    }

    fn read_u16(&mut self) -> Result<u16> {
        self.need(2)?;
        let r = u16::from_le_bytes([self.buffer[self.cursor], self.buffer[self.cursor + 1]]);
        self.cursor += 2;
        Ok(r)
    }

    fn read_u32(&mut self) -> Result<u32> {
        self.need(4)?;
        let r = u32::from_le_bytes([
            self.buffer[self.cursor],
            self.buffer[self.cursor + 1],
            self.buffer[self.cursor + 2],
            self.buffer[self.cursor + 3],
        ]);
        self.cursor += 4;
        Ok(r)
    }

    fn read_f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.read_u32()?))
    }

    fn next_block(&mut self) -> Result<Block<'a>> {
        let offset = self.cursor;
        let marker = self.read_u8()?;

        let timestamp = if marker & MARKER_TS_RELATIVE != 0 {
            let d = self.read_u8()?;
            self.acc_time += (d as f32) * 0.001;
            self.acc_time
        } else {
            let t = self.read_f32()?;
            self.acc_time = t;
            t
        };

        let length = if marker & MARKER_LEN_U8 != 0 {
            self.read_u8()? as u32
        } else {
            self.read_u32()?
        };

        let packet_id = if marker & MARKER_REUSE_PACKET_ID != 0 {
            self.prev_packet_id
        } else {
            self.read_u16()?
        };

        let param = if marker & MARKER_PARAM_RELATIVE != 0 {
            let d = self.read_u8()?;
            self.prev_param.wrapping_add(d as u32)
        } else {
            self.read_u32()?
        };

        if self.cursor + (length as usize) > self.buffer.len() {
            return Err(RoflError::BlockFraming {
                offset,
                message: format!(
                    "payload length {} exceeds {} remaining bytes",
                    length,
                    self.buffer.len() - self.cursor
                ),
            });
        }
        let payload = &self.buffer[self.cursor..self.cursor + length as usize];
        self.cursor += length as usize;

        self.prev_packet_id = packet_id;
        self.prev_param = param;

        Ok(Block {
            timestamp,
            length,
            packet_id,
            param,
            payload,
            offset,
        })
    }
}

impl<'a> Iterator for BlockIterator<'a> {
    type Item = Result<Block<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.buffer.len() {
            return None;
        }
        Some(self.next_block())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_absolute_block() {
        // marker 0x00: absolute ts, u32 length, fresh u16 packet_id, u32 param
        let mut buf = vec![0x00u8]; // marker
        buf.extend_from_slice(&0f32.to_le_bytes()); // timestamp = 0.0
        buf.extend_from_slice(&5u32.to_le_bytes()); // length = 5
        buf.extend_from_slice(&0x0478u16.to_le_bytes()); // packet_id = 0x0478
        buf.extend_from_slice(&0u32.to_le_bytes()); // param = 0
        buf.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x42]); // payload

        let mut it = BlockIterator::new(&buf);
        let b = it.next().unwrap().unwrap();
        assert_eq!(b.timestamp, 0.0);
        assert_eq!(b.length, 5);
        assert_eq!(b.packet_id, 0x0478);
        assert_eq!(b.param, 0);
        assert_eq!(b.payload, &[0xDE, 0xAD, 0xBE, 0xEF, 0x42]);
        assert!(it.next().is_none());
    }

    #[test]
    fn accumulates_time_across_relative_blocks() {
        // Block 1: absolute ts 1.5, len 0, pid 0x100, param 0.
        // Block 2: relative-everything with delta bits, reuse pid.
        let mut buf = vec![0x00u8];
        buf.extend_from_slice(&1.5f32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0x0100u16.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());

        // marker = 0x80 | 0x40 | 0x20 | 0x10 = 0xF0
        // ts-delta = 250 (-> +0.25s), len-u8 = 0, reuse pid, param-delta = 7.
        buf.push(0xF0);
        buf.push(250); // ts delta
        buf.push(0); // len
        buf.push(7); // param delta

        let mut it = BlockIterator::new(&buf);
        let b1 = it.next().unwrap().unwrap();
        assert_eq!(b1.timestamp, 1.5);
        let b2 = it.next().unwrap().unwrap();
        assert!((b2.timestamp - 1.75).abs() < 1e-5);
        assert_eq!(b2.packet_id, 0x0100);
        assert_eq!(b2.param, 7);
        assert!(it.next().is_none());
    }

    #[test]
    fn flags_truncated_payload() {
        let mut buf = vec![0x00u8];
        buf.extend_from_slice(&0f32.to_le_bytes());
        buf.extend_from_slice(&10u32.to_le_bytes()); // length = 10 but no payload follows
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());

        let mut it = BlockIterator::new(&buf);
        let err = it.next().unwrap().unwrap_err();
        matches!(err, RoflError::BlockFraming { .. });
    }
}
