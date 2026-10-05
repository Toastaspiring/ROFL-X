//! Decode any block through the game's own deserializer.
//!
//! For one block `(netid, param, payload)`:
//!
//! 1. Construct the packet object by calling the game's netid factory, so
//!    the object carries the real vtable of its class (no RVA guessing).
//! 2. Re-frame the stream as the live network path sees it. Every class's
//!    Deserialize first calls the base-class Deserialize, which reads the
//!    sender net id as an obfuscated varint. Replay blocks carry that net
//!    id in the block header (`param`) instead, so it is re-encoded with
//!    the game's own byte mapping and prepended to the payload.
//! 3. Call vtable slot 1, `Deserialize(this, &cursor, end)`, with plaintext
//!    capture on (see `machine.rs`).
//!
//! The deserializer's own verdict is the correctness check: it returns
//! true and leaves the cursor exactly at `end` only when every field it
//! expected was present and consistent. See `docs/DECODING.md`.

use std::collections::HashMap;

use crate::emulator::archive::PatchArchive;
use crate::emulator::machine::{Machine, HEAP_BASE};
use crate::error::{Result, RoflError};

/// How the deserializer finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecodeStatus {
    /// Returned true and consumed exactly the whole payload.
    Clean,
    /// Returned true but stopped short of (or past) the payload end.
    Partial,
    /// Returned false.
    Rejected,
    /// Never returned (unmapped access or instruction budget exhausted).
    Fault,
    /// The factory has no class for this netid.
    NoClass,
}

impl DecodeStatus {
    pub fn label(self) -> &'static str {
        match self {
            DecodeStatus::Clean => "clean",
            DecodeStatus::Partial => "partial",
            DecodeStatus::Rejected => "rejected",
            DecodeStatus::Fault => "fault",
            DecodeStatus::NoClass => "no-class",
        }
    }
}

/// Plaintext view of the emulated heap after one decode: the packet object
/// plus everything it allocated (vectors, strings, nested objects).
#[derive(Debug, Clone)]
pub struct PlainMemory {
    base: u64,
    bytes: Vec<u8>,
    written: Vec<bool>,
}

impl PlainMemory {
    pub fn bytes(&self, addr: u64, len: usize) -> Option<&[u8]> {
        let start = addr.checked_sub(self.base)? as usize;
        self.bytes.get(start..start.checked_add(len)?)
    }

    pub fn u8(&self, addr: u64) -> Option<u8> {
        self.bytes(addr, 1).map(|b| b[0])
    }

    pub fn u16(&self, addr: u64) -> Option<u16> {
        self.bytes(addr, 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&self, addr: u64) -> Option<u32> {
        self.bytes(addr, 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }

    pub fn u64(&self, addr: u64) -> Option<u64> {
        self.bytes(addr, 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }

    pub fn f32(&self, addr: u64) -> Option<f32> {
        self.u32(addr).map(f32::from_bits)
    }

    /// Whether the deserializer stored a plaintext value at `addr`.
    pub fn written(&self, addr: u64) -> bool {
        addr.checked_sub(self.base)
            .and_then(|i| self.written.get(i as usize))
            .copied()
            .unwrap_or(false)
    }

    /// Address of the first occurrence of `needle`, if any.
    pub fn find(&self, needle: &[u8]) -> Option<u64> {
        if needle.is_empty() {
            return None;
        }
        self.bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|i| self.base + i as u64)
    }

    /// NUL-terminated string at `addr` (lossy UTF-8), at most `max` bytes.
    pub fn cstr(&self, addr: u64, max: usize) -> Option<String> {
        let start = addr.checked_sub(self.base)? as usize;
        let tail = self.bytes.get(start..)?;
        let end = tail.iter().take(max).position(|b| *b == 0)?;
        Some(String::from_utf8_lossy(&tail[..end]).into_owned())
    }
}

/// One decoded block.
#[derive(Debug, Clone)]
pub struct DecodedPacket {
    pub netid: u16,
    pub status: DecodeStatus,
    /// Address of the packet object inside `memory`.
    pub object: u64,
    /// First heap address past the object: the decode allocates the
    /// payload buffer right after it, so anything at or above this is the
    /// stream, not a field.
    pub object_end: u64,
    pub memory: PlainMemory,
}

impl DecodedPacket {
    pub fn u8(&self, off: u64) -> Option<u8> {
        self.memory.u8(self.object + off)
    }
    pub fn u16(&self, off: u64) -> Option<u16> {
        self.memory.u16(self.object + off)
    }
    pub fn u32(&self, off: u64) -> Option<u32> {
        self.memory.u32(self.object + off)
    }
    pub fn u64(&self, off: u64) -> Option<u64> {
        self.memory.u64(self.object + off)
    }
    pub fn f32(&self, off: u64) -> Option<f32> {
        self.memory.f32(self.object + off)
    }

    /// Offsets (within the object) of 4-byte-aligned words holding at
    /// least one plaintext byte, up to `limit` and never past the object.
    /// Used for raw dumps and by `fields`.
    pub fn written_words(&self, limit: u64) -> Vec<(u64, u32)> {
        let limit = limit.min(self.object_end.saturating_sub(self.object));
        (0..limit)
            .step_by(4)
            .filter(|off| (0..4).any(|k| self.memory.written(self.object + off + k)))
            .filter_map(|off| Some((off, self.u32(off)?)))
            .collect()
    }
}

/// The base deserializer's net-id varint, learned by emulation: for each
/// 7-bit group, the raw byte that decodes to it as a final or as a
/// continuation byte.
struct NetIdCodec {
    terminal: [Option<u8>; 128],
    continuation: [Option<u8>; 128],
}

impl NetIdCodec {
    fn learn(machine: &mut Machine, base_deserialize_rva: u64) -> Result<Self> {
        let mut codec = NetIdCodec {
            terminal: [None; 128],
            continuation: [None; 128],
        };
        let run = |machine: &mut Machine, bytes: &[u8]| -> Result<Option<(u64, u32)>> {
            machine.reset_heap()?;
            let obj = machine.alloc(0x40)?;
            let buf = machine.alloc(bytes.len() as u64)?;
            let cur = machine.alloc(8)?;
            machine.write(buf, bytes)?;
            machine.write(cur, &buf.to_le_bytes())?;
            let out = machine.call_rva(base_deserialize_rva, &[obj, cur, buf + bytes.len() as u64])?;
            if !out.returned || out.rax & 0xFF != 1 {
                return Ok(None);
            }
            let used = machine.read_u64(cur)? - buf;
            let stored = machine.read_u32(obj + 0xC)?;
            Ok(Some((used, unmask(stored))))
        };
        for b in 0..=255u8 {
            if let Some((1, value)) = run(machine, &[b])? {
                if value < 128 && codec.terminal[value as usize].is_none() {
                    codec.terminal[value as usize] = Some(b);
                }
            }
        }
        let zero = codec.terminal[0]
            .ok_or_else(|| RoflError::Emulator("net-id codec: no byte decodes to 0".into()))?;
        for b in 0..=255u8 {
            if codec.terminal.contains(&Some(b)) {
                continue;
            }
            if let Some((2, value)) = run(machine, &[b, zero])? {
                let g = (value & 0x7F) as usize;
                if codec.continuation[g].is_none() {
                    codec.continuation[g] = Some(b);
                }
            }
        }
        if codec.terminal.iter().chain(&codec.continuation).any(Option::is_none) {
            return Err(RoflError::Emulator(
                "net-id codec: base deserializer does not look like a 7-bit varint reader".into(),
            ));
        }
        Ok(codec)
    }

    fn encode(&self, net_id: u32) -> Vec<u8> {
        let mut v = mask(net_id);
        let mut out = Vec::with_capacity(5);
        loop {
            let g = (v & 0x7F) as usize;
            v >>= 7;
            if v == 0 {
                out.push(self.terminal[g].unwrap());
                return out;
            }
            out.push(self.continuation[g].unwrap());
        }
    }
}

/// The game stores net ids XORed with 0x40000000 unless their low 24 bits
/// are zero (base deserializer, `docs/DECODING.md`).
fn mask(net_id: u32) -> u32 {
    if net_id & 0x00FF_FFFF != 0 {
        net_id ^ 0x4000_0000
    } else {
        net_id
    }
}

fn unmask(stored: u32) -> u32 {
    mask(stored)
}

pub struct PacketDecoder {
    machine: Machine,
    codec: NetIdCodec,
    factory_rva: u64,
    max_netid: u32,
    /// Deserializer VA per netid, learned from the first construction.
    deserializers: HashMap<u16, Option<u64>>,
}

impl PacketDecoder {
    pub fn new(archive: &PatchArchive) -> Result<Self> {
        let mut machine = Machine::new(archive)?;
        let codec = NetIdCodec::learn(&mut machine, archive.base_deserialize_rva)?;
        Ok(PacketDecoder {
            machine,
            codec,
            factory_rva: archive.factory_rva,
            max_netid: archive.max_netid,
            deserializers: HashMap::new(),
        })
    }

    /// Construct a fresh packet object for `netid`; 0 if the factory has
    /// no class for it.
    fn construct(&mut self, netid: u16) -> Result<u64> {
        if u32::from(netid) >= self.max_netid {
            return Ok(0);
        }
        let out = self.machine.alloc(16)?;
        let r = self.machine.call_rva(self.factory_rva, &[out, u64::from(netid)])?;
        if !r.returned {
            return Ok(0);
        }
        self.machine.read_u64(out)
    }

    /// Image RVA of the class deserializer for `netid` (vtable slot 1).
    pub fn deserializer_rva(&mut self, netid: u16) -> Result<Option<u64>> {
        if let Some(d) = self.deserializers.get(&netid) {
            return Ok(d.map(|va| va - self.machine.image_base()));
        }
        self.machine.reset_heap()?;
        let obj = self.construct(netid)?;
        let va = if obj == 0 {
            None
        } else {
            Some(self.machine.read_u64(self.machine.read_u64(obj)? + 8)?)
        };
        self.deserializers.insert(netid, va);
        Ok(va.map(|va| va - self.machine.image_base()))
    }

    pub fn decode(&mut self, netid: u16, param: u32, payload: &[u8]) -> Result<DecodedPacket> {
        let m = &mut self.machine;
        m.reset_heap()?;
        let out = m.alloc(16)?;
        let constructed = if u32::from(netid) < self.max_netid {
            let r = m.call_rva(self.factory_rva, &[out, u64::from(netid)])?;
            if r.returned {
                m.read_u64(out)?
            } else {
                0
            }
        } else {
            0
        };
        if constructed == 0 {
            return Ok(DecodedPacket {
                netid,
                status: DecodeStatus::NoClass,
                object: 0,
                object_end: 0,
                memory: PlainMemory {
                    base: HEAP_BASE,
                    bytes: Vec::new(),
                    written: Vec::new(),
                },
            });
        }
        let obj = constructed;
        let deser = m.read_u64(m.read_u64(obj)? + 8)?;
        self.deserializers.entry(netid).or_insert(Some(deser));

        let mut stream = self.codec.encode(param);
        stream.extend_from_slice(payload);
        let buf = m.alloc(stream.len() as u64)?;
        let cur = m.alloc(8)?;
        m.write(buf, &stream)?;
        m.write(cur, &buf.to_le_bytes())?;

        m.set_capture(true);
        let r = m.call(deser, &[obj, cur, buf + stream.len() as u64]);
        m.set_capture(false);
        let r = r?;

        let status = if !r.returned {
            DecodeStatus::Fault
        } else if r.rax & 0xFF == 0 {
            DecodeStatus::Rejected
        } else if m.read_u64(cur)? != buf + stream.len() as u64 {
            DecodeStatus::Partial
        } else {
            DecodeStatus::Clean
        };
        let (bytes, written) = m.plaintext_heap(m.heap_cursor())?;
        Ok(DecodedPacket {
            netid,
            status,
            object: obj,
            object_end: buf,
            memory: PlainMemory {
                base: HEAP_BASE,
                bytes,
                written,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_id_mask_round_trips() {
        for id in [0u32, 0x4000_00AE, 0x4000_2142, 0x0100_0000, 0xFF00_0000] {
            assert_eq!(unmask(mask(id)), id);
        }
        assert_eq!(mask(0x4000_00AE), 0xAE);
        assert_eq!(mask(0), 0);
    }

    #[test]
    fn plain_memory_reads_little_endian() {
        let m = PlainMemory {
            base: 0x1000,
            bytes: vec![0xAE, 0, 0, 0x40, 0, 0, 0x80, 0x3F, b'h', b'i', 0],
            written: vec![true; 11],
        };
        assert_eq!(m.u32(0x1000), Some(0x4000_00AE));
        assert_eq!(m.f32(0x1004), Some(1.0));
        assert_eq!(m.cstr(0x1008, 8).as_deref(), Some("hi"));
        assert_eq!(m.u32(0x1009), None);
    }
}
