//! Movement paths for a batch of units: Zhu's `WaypointGroup`, the packet
//! Mowokuma's `mov_decrypt` decoded on 15.x. No Riot class name is
//! registered for it, so it is recognised by shape.
//!
//! Object:
//!
//! | offset | type | field                               |
//! |--------|------|-------------------------------------|
//! | +0x10  | u32  | number of entries                   |
//! | +0x14  | u32  | sync counter (one step per packet)  |
//! | +0x18  | ptr  | entry buffer                        |
//! | +0x20  | u32  | entry buffer length                 |
//!
//! Each entry in the buffer:
//!
//! ```text
//! u16  flags   low byte: bit 0 = one extra byte follows this header,
//!              bits 1..7 = waypoint count n; high byte: unknown (0 or 2)
//! u32  unit net id
//! f32  move speed
//! [u8]         the extra byte, when flagged
//! [u8; k]      when n > 1, k = ((n - 2) >> 2) + 1 bitmap bytes: two bits
//!              per waypoint after the first (x then z); set = the
//!              coordinate is a signed 1-byte delta, clear = an absolute u16
//! coordinates  first waypoint: two absolute u16s; the rest per bitmap
//! map value = sign_extend_16(v) * 2 + 7358 (x) / 7412 (z)
//! ```
//!
//! This is Mowokuma's `PathPacket::parse` layout (`emulator/packet.rs`),
//! with two differences: the buffer holds several entries (she read the
//! first), and deltas are signed (she adds them unsigned; signed halves
//! the median segment length and removes spurious +510-unit jumps).
//!
//! Verified on 16.9: every waypoint group of every 16.9 replay we hold
//! parses to exactly its buffer length with all points on the map.

use serde::Serialize;

use crate::emulator::DecodedPacket;

#[derive(Debug, Clone, Serialize)]
pub struct Path {
    pub unit: u32,
    pub speed: f32,
    /// `[x, z]` map coordinates; the first point is where the unit is now.
    pub points: Vec<[f32; 2]>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WaypointGroup {
    pub paths: Vec<Path>,
}

const MAX_ENTRIES: u32 = 512;
const MAX_BUFFER: u32 = 1 << 16;

pub fn read(p: &DecodedPacket) -> Option<WaypointGroup> {
    let count = p.u32(0x10)?;
    let ptr = p.u64(0x18)?;
    let len = p.u32(0x20)?;
    if count == 0 || count > MAX_ENTRIES || len == 0 || len > MAX_BUFFER {
        return None;
    }
    let buf = p.memory.bytes(ptr, len as usize)?;
    let paths = parse_entries(buf, count as usize)?;
    let on_map = paths
        .iter()
        .flat_map(|path| path.points.iter())
        .all(|[x, z]| (-1000.0..16000.0).contains(x) && (-1000.0..16000.0).contains(z));
    on_map.then_some(WaypointGroup { paths })
}

/// Parse exactly `count` entries; `None` unless they fill `buf` exactly.
pub fn parse_entries(buf: &[u8], count: usize) -> Option<Vec<Path>> {
    let mut r = Reader { buf, at: 0 };
    let mut paths = Vec::with_capacity(count);
    for _ in 0..count {
        let flags = r.u16()?;
        let unit = r.u32()?;
        let speed = f32::from_bits(r.u32()?);
        if flags & 1 != 0 {
            r.u8()?;
        }
        let n = usize::from((flags & 0xFF) >> 1);
        let bitmap = if n > 1 { r.take(((n - 2) >> 2) + 1)? } else { &[] };
        let bit = |i: usize| bitmap[i >> 3] & (1 << (i & 7)) != 0;

        let mut points = Vec::with_capacity(n);
        let (mut x, mut z) = (0u16, 0u16);
        for i in 0..n {
            let (dx, dz) = if i == 0 {
                (false, false)
            } else {
                (bit(2 * (i - 1)), bit(2 * (i - 1) + 1))
            };
            x = if dx { x.wrapping_add_signed(i16::from(r.u8()? as i8)) } else { r.u16()? };
            z = if dz { z.wrapping_add_signed(i16::from(r.u8()? as i8)) } else { r.u16()? };
            points.push([x as i16 as f32 * 2.0 + 7358.0, z as i16 as f32 * 2.0 + 7412.0]);
        }
        paths.push(Path { unit, speed, points });
    }
    (r.at == buf.len()).then_some(paths)
}

struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.buf.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn parses_a_two_entry_group() {
        // 16.9 netid 916, t=200.12 in a ranked replay: two units, the first
        // with four waypoints (bitmap 0x3f: all deltas), the second with one.
        let buf = hex(
            "08004e1200400000af433fd90bfef7ff17e70000\
             190200a31500400000af43d0fdcefd",
        );
        let paths = parse_entries(&buf, 2).unwrap();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].unit, 0x4000_124E);
        assert_eq!(paths[0].speed, 350.0);
        assert_eq!(paths[0].points.len(), 4);
        assert_eq!(paths[0].points[0], [13424.0, 3312.0]);
        assert_eq!(paths[1].unit, 0x4000_15A3);
        assert_eq!(paths[1].points.len(), 1);
    }

    #[test]
    fn rejects_a_buffer_with_bytes_left_over() {
        let mut buf = hex("02007a0d00400000af43ff990bc5");
        buf.push(0);
        assert!(parse_entries(&buf, 1).is_none());
    }

    #[test]
    fn count_comes_from_the_low_byte_only() {
        // flags 0x0202: high byte 2, low byte 2 -> one waypoint.
        let buf = hex("0202564b00400000af43c8f5b609");
        let paths = parse_entries(&buf, 1).unwrap();
        assert_eq!(paths[0].points.len(), 1);
    }
}
