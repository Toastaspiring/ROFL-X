//! Recover a class's payload layout and its substitution table, on a
//! patch we can decode.
//!
//! A packet's payload is its fields serialised in order and run through
//! one 256-entry byte substitution per class (`docs/EXPLORE.md` § 3).
//! On a patch with a format-2 archive both sides are in hand — the raw
//! bytes from the file, and the plaintext object the game's own
//! deserializer produced — so neither the order nor the table has to be
//! guessed.
//!
//! **The layout.** For every payload byte and every byte of every
//! written object word, ask whether the map between them holds in both
//! directions across every block. A real correspondence survives
//! thousands of blocks; a coincidence does not. What comes out is which
//! field each payload byte belongs to, in what order the class writes
//! its fields, and in which byte order — all of it measured.
//!
//! **The table.** Once the layout is known the substitution follows:
//! each payload byte is the cipher for the plaintext byte the layout
//! assigns it. Learning it from three float fields of
//! `PKT_S2C_FaceDirection_s` recovers all 256 entries with no conflict.
//!
//! Both halves need blocks. A class that fires sixty times a game
//! cannot settle a table: when every block carries a different value
//! for two bytes, any pairing of those bytes is bijective and proves
//! nothing, so spurious assignments survive and the table comes out
//! self-contradictory. [`Layout::settled`] says which case you are in.
//!
//! This is the engine behind reading a packed patch. There, neither
//! side is available; but a class's field order rarely changes between
//! patches, so the layout measured here says *where* to look, and the
//! table recovered here proves what kind of thing is being looked for.
//! Two tables from one patch are unrelated to each other — measured, by
//! comparing a full table against another full table — so the table
//! itself never transfers. Only the shape does.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{json, Value};

use crate::emulator::DecodedPacket;

/// Object words are profiled this far in, matching `fields`.
const OBJECT_LIMIT: u64 = 0x200;
/// Blocks needed before a correspondence is believed.
const MIN_BLOCKS: usize = 16;
/// Distinct values a field byte must take before its pairing is used
/// to build the substitution table.
const MIN_VARIETY: usize = 16;

/// One block's two sides: the payload as it sits in the file, and the
/// plaintext the deserializer produced from it — the object words, and
/// the largest buffer it allocated, if any.
pub struct Sample {
    pub payload: Vec<u8>,
    pub words: Vec<(u64, u32)>,
    /// The biggest heap block the deserializer allocated other than the
    /// object itself. A class that carries a list writes it there, and
    /// none of it shows up in the object words.
    pub buffer: Vec<u8>,
}

impl Sample {
    pub fn of(payload: &[u8], pkt: &DecodedPacket) -> Sample {
        let buffer = pkt
            .memory
            .allocations()
            .iter()
            .filter(|(addr, size)| *addr != pkt.object && *size > 0)
            .max_by_key(|(_, size)| *size)
            .and_then(|(addr, size)| pkt.memory.bytes(*addr, *size as usize))
            .unwrap_or_default()
            .to_vec();
        Sample {
            payload: payload.to_vec(),
            words: pkt.written_words(OBJECT_LIMIT),
            buffer,
        }
    }
}

/// How a class's allocated buffer sits inside its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferFit {
    /// True when the payload carries the buffer back to front.
    pub reversed: bool,
    /// Payload bytes before the buffer's data starts; for a reversed
    /// fit the buffer's first byte is the payload's last.
    pub header: usize,
    pub blocks: usize,
    pub table: BTreeMap<u8, u8>,
    pub conflicts: usize,
}

/// Work out whether a class's payload is just its buffer run through
/// the substitution, and recover the table if it is.
///
/// Worth trying because a class that carries a list puts none of it in
/// the object, so the field-by-field pass above sees nothing at all.
/// Two arrangements are tried. Forwards, the buffer's first byte is the
/// first byte after a fixed-length header. Reversed, the buffer's first
/// byte is the payload's *last* — which is what the waypoint class
/// does, and which no amount of staring at offsets from the front would
/// have shown.
///
/// A fit is only accepted if one table explains every byte of every
/// block with no contradiction, which a wrong arrangement never does.
pub fn recover_buffer(samples: &[Sample]) -> Option<BufferFit> {
    let usable: Vec<&Sample> = samples
        .iter()
        .filter(|s| !s.buffer.is_empty() && s.payload.len() > s.buffer.len())
        .collect();
    if usable.len() < MIN_BLOCKS {
        return None;
    }
    let mut best: Option<BufferFit> = None;
    for reversed in [true, false] {
        // Forwards needs a header length; reversed is anchored to the end.
        let headers: Vec<usize> = if reversed { vec![0] } else { (0..=8).collect() };
        for header in headers {
            let mut table: BTreeMap<u8, u8> = BTreeMap::new();
            let mut conflicts = 0usize;
            let mut blocks = 0usize;
            for s in &usable {
                if !reversed && s.payload.len() < header + s.buffer.len() {
                    continue;
                }
                blocks += 1;
                for (j, plain) in s.buffer.iter().enumerate() {
                    let i = if reversed {
                        s.payload.len() - 1 - j
                    } else {
                        header + j
                    };
                    let cipher = s.payload[i];
                    match table.get(plain) {
                        Some(have) if *have != cipher => conflicts += 1,
                        Some(_) => {}
                        None => {
                            table.insert(*plain, cipher);
                        }
                    }
                }
            }
            let fit = BufferFit { reversed, header, blocks, table, conflicts };
            if fit.conflicts == 0
                && fit.blocks >= MIN_BLOCKS
                && best.as_ref().map_or(true, |b| b.table.len() < fit.table.len())
            {
                best = Some(fit);
            }
        }
    }
    best
}

/// What one payload byte was found to carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Carries {
    /// Byte `.1` of the object word at offset `.0`.
    Field(u64, usize),
    /// Several candidates survived, because the field barely varies.
    Ambiguous(Vec<(u64, usize)>),
    /// Nothing in the object tracks this byte.
    Unknown,
}

/// A class's payload, byte by byte, plus the substitution table that
/// the layout implies.
#[derive(Debug, Clone)]
pub struct Layout {
    pub netid: u16,
    /// The payload length this was measured at: the commonest one, so
    /// that positions mean the same thing in every sample.
    pub payload_len: usize,
    pub blocks: usize,
    pub bytes: Vec<Carries>,
    /// Plaintext byte to payload byte.
    pub table: BTreeMap<u8, u8>,
    /// Plaintext bytes that mapped to more than one payload byte. A
    /// real class has none; any at all means the layout is wrong
    /// somewhere, or the class does not use a single table.
    pub table_conflicts: usize,
    /// Payload positions the table was built from: the ones whose
    /// field byte varies enough for the pairing to mean something.
    pub table_from_positions: usize,
    /// Positions whose assignment the table contradicts. These are
    /// fields the layout pass guessed wrong, usually because they take
    /// too few values to be pinned down.
    pub disagreeing_positions: Vec<usize>,
}

/// Measure the layout of one class from blocks of it.
pub fn recover(netid: u16, samples: &[Sample]) -> Option<Layout> {
    let mut lengths: HashMap<usize, usize> = HashMap::new();
    for s in samples {
        *lengths.entry(s.payload.len()).or_default() += 1;
    }
    let (&payload_len, &n) = lengths.iter().max_by_key(|(len, n)| (**n, **len))?;
    if n < MIN_BLOCKS {
        return None;
    }
    let rows: Vec<&Sample> = samples.iter().filter(|s| s.payload.len() == payload_len).collect();

    // Only words every one of these blocks wrote can be compared
    // position by position.
    let mut offsets: Option<BTreeSet<u64>> = None;
    for s in &rows {
        let here: BTreeSet<u64> = s.words.iter().map(|(off, _)| *off).collect();
        offsets = Some(match offsets {
            Some(have) => have.intersection(&here).copied().collect(),
            None => here,
        });
    }
    let offsets: Vec<u64> = offsets.unwrap_or_default().into_iter().collect();

    let value_at = |s: &Sample, off: u64| -> u32 {
        s.words.iter().find(|(o, _)| *o == off).map_or(0, |(_, v)| *v)
    };

    let mut bytes = Vec::with_capacity(payload_len);
    for i in 0..payload_len {
        let mut hits = Vec::new();
        for &off in &offsets {
            for j in 0..4 {
                if tracks(&rows, i, off, j, &value_at) {
                    hits.push((off, j));
                }
            }
        }
        bytes.push(match hits.len() {
            0 => Carries::Unknown,
            1 => Carries::Field(hits[0].0, hits[0].1),
            _ => Carries::Ambiguous(hits),
        });
    }

    // The table is built only from positions whose field byte really
    // moves. A field with three possible values pairs bijectively with
    // almost anything, so taking its word for the table would poison
    // it with assignments that are only coincidences.
    let variety = |i: usize| -> usize {
        let Carries::Field(off, j) = bytes[i] else { return 0 };
        rows.iter()
            .map(|s| value_at(s, off).to_le_bytes()[j])
            .collect::<BTreeSet<u8>>()
            .len()
    };
    let mut table: BTreeMap<u8, u8> = BTreeMap::new();
    let mut table_conflicts = 0usize;
    let mut from_positions = 0usize;
    for (i, carries) in bytes.iter().enumerate() {
        if variety(i) < MIN_VARIETY {
            continue;
        }
        from_positions += 1;
        let Carries::Field(off, j) = *carries else { continue };
        for s in &rows {
            let plain = value_at(s, off).to_le_bytes()[j];
            let cipher = s.payload[i];
            match table.get(&plain) {
                Some(have) if *have != cipher => table_conflicts += 1,
                Some(_) => {}
                None => {
                    table.insert(plain, cipher);
                }
            }
        }
    }

    // Every other assigned position is then checked against it. One
    // that disagrees is telling us its assignment is wrong, not that
    // the class has two tables.
    let mut disagree = Vec::new();
    for (i, carries) in bytes.iter().enumerate() {
        let Carries::Field(off, j) = *carries else { continue };
        if variety(i) >= MIN_VARIETY {
            continue;
        }
        let bad = rows.iter().any(|s| {
            let plain = value_at(s, off).to_le_bytes()[j];
            table.get(&plain).is_some_and(|c| *c != s.payload[i])
        });
        if bad {
            disagree.push(i);
        }
    }

    Some(Layout {
        netid,
        payload_len,
        blocks: rows.len(),
        bytes,
        table,
        table_conflicts,
        table_from_positions: from_positions,
        disagreeing_positions: disagree,
    })
}

/// Whether payload byte `i` and byte `j` of the word at `off` are the
/// same thing: a one-to-one map that holds across every block.
///
/// Both sides have to vary. A payload byte that never changes matches
/// every field byte that never changes, which says nothing about
/// either, so a constant is left unexplained rather than paired with
/// the first constant it meets.
fn tracks(
    rows: &[&Sample],
    i: usize,
    off: u64,
    j: usize,
    value_at: &impl Fn(&Sample, u64) -> u32,
) -> bool {
    let mut forward: HashMap<u8, u8> = HashMap::new();
    let mut back: HashMap<u8, u8> = HashMap::new();
    for s in rows {
        let plain = value_at(s, off).to_le_bytes()[j];
        let cipher = s.payload[i];
        if *forward.entry(plain).or_insert(cipher) != cipher {
            return false;
        }
        if *back.entry(cipher).or_insert(plain) != plain {
            return false;
        }
    }
    forward.len() > 1
}

impl Layout {
    /// Fields in the order the class writes them, with the byte order
    /// it writes each in.
    pub fn field_order(&self) -> Vec<(u64, &'static str, usize)> {
        let mut out: Vec<(u64, &'static str, usize)> = Vec::new();
        let mut run: Vec<(u64, usize)> = Vec::new();
        let flush = |run: &mut Vec<(u64, usize)>, out: &mut Vec<(u64, &'static str, usize)>| {
            if run.len() < 2 {
                run.clear();
                return;
            }
            let off = run[0].0;
            let order = if run.iter().all(|(o, _)| *o == off) {
                let idx: Vec<usize> = run.iter().map(|(_, j)| *j).collect();
                let ascending: Vec<usize> = (0..idx.len()).collect();
                if idx == ascending {
                    "little-endian"
                } else if idx.iter().rev().copied().collect::<Vec<_>>() == ascending {
                    "big-endian"
                } else {
                    "mixed"
                }
            } else {
                "mixed"
            };
            out.push((off, order, run.len()));
            run.clear();
        };
        for c in &self.bytes {
            match c {
                Carries::Field(off, j) => {
                    if run.first().is_some_and(|(o, _)| o != off) {
                        flush(&mut run, &mut out);
                    }
                    run.push((*off, *j));
                }
                _ => flush(&mut run, &mut out),
            }
        }
        flush(&mut run, &mut out);
        out
    }

    /// Whether the table came out self-consistent. Conflicts mean at
    /// least one of the field assignments above is wrong, which on a
    /// rare class usually just means too few blocks: when every block
    /// carries a different value for two bytes, any pairing of them is
    /// bijective and proves nothing.
    pub fn settled(&self) -> bool {
        self.table_conflicts == 0 && self.table_from_positions > 0
    }

    pub fn to_json(&self, class: Option<&str>) -> Value {
        let bytes: Vec<Value> = self
            .bytes
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let what = match c {
                    Carries::Field(off, j) => format!("+0x{off:03x} byte {j}"),
                    Carries::Ambiguous(v) => format!("{} candidates", v.len()),
                    Carries::Unknown => "not in the object".into(),
                };
                json!({"payload_byte": i, "from_end": i as i64 - self.payload_len as i64, "carries": what})
            })
            .collect();
        json!({
            "netid": self.netid,
            "class": class,
            "payload_bytes": self.payload_len,
            "blocks": self.blocks,
            "fields_in_stream_order": self.field_order().iter()
                .map(|(off, order, n)| json!({
                    "offset": format!("+0x{off:03x}"), "bytes": n, "byte_order": order,
                }))
                .collect::<Vec<_>>(),
            "layout": bytes,
            "substitution_table": {
                "entries": self.table.len(),
                "conflicts": self.table_conflicts,
                "built_from_positions": self.table_from_positions,
                "settled": self.settled(),
                "positions_it_contradicts": self.disagreeing_positions,
                "map": self.table.iter()
                    .map(|(p, c)| (format!("{p:02x}"), format!("{c:02x}")))
                    .collect::<BTreeMap<_, _>>(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_reversed_buffer_and_its_table() {
        let t = |b: u8| b ^ 0x3C;
        let samples: Vec<Sample> = (0..40u32)
            .map(|i| {
                let buffer: Vec<u8> = (0..12u8).map(|k| k.wrapping_mul(7).wrapping_add(i as u8)).collect();
                let mut payload = vec![0xAA, 0xBB, 0xCC];
                payload.extend(buffer.iter().rev().map(|b| t(*b)));
                Sample { payload, words: Vec::new(), buffer }
            })
            .collect();
        let fit = recover_buffer(&samples).unwrap();
        assert!(fit.reversed);
        assert_eq!(fit.conflicts, 0);
        for (plain, cipher) in &fit.table {
            assert_eq!(*cipher, t(*plain));
        }
    }

    #[test]
    fn finds_a_forward_buffer_behind_a_header() {
        let t = |b: u8| b.wrapping_add(17);
        let samples: Vec<Sample> = (0..40u32)
            .map(|i| {
                let buffer: Vec<u8> = (0..12u8).map(|k| k.wrapping_mul(5).wrapping_add(i as u8)).collect();
                let mut payload = vec![0u8; 4];
                payload.extend(buffer.iter().map(|b| t(*b)));
                Sample { payload, words: Vec::new(), buffer }
            })
            .collect();
        let fit = recover_buffer(&samples).unwrap();
        assert!(!fit.reversed);
        assert_eq!(fit.header, 4);
        assert_eq!(fit.conflicts, 0);
    }

    #[test]
    fn no_fit_when_the_payload_is_unrelated() {
        let samples: Vec<Sample> = (0..40u32)
            .map(|i| Sample {
                payload: vec![i as u8; 20],
                words: Vec::new(),
                buffer: (0..12u8).map(|k| k.wrapping_mul(3).wrapping_add(i as u8)).collect(),
            })
            .collect();
        assert!(recover_buffer(&samples).is_none());
    }

    /// Four bytes that vary independently. Bytes that are each an
    /// injective function of one counter would all track each other,
    /// which real fields do not.
    fn value(i: u32) -> u32 {
        let b = |k: u32| {
            let mut x = i.wrapping_mul(0x9E37_79B9) ^ k.wrapping_mul(0x85EB_CA6B);
            x ^= x >> 15;
            x = x.wrapping_mul(0xC2B2_AE35);
            (x >> 13) & 0xFF
        };
        b(0) | (b(1) << 8) | (b(2) << 16) | (b(3) << 24)
    }

    /// A class writing one u32 little-endian, through a toy table,
    /// behind a constant header byte.
    fn sample(v: u32, table: impl Fn(u8) -> u8) -> Sample {
        let mut payload = vec![table(0xAA)];
        payload.extend(v.to_le_bytes().map(&table));
        Sample { payload, words: vec![(0x10, v)], buffer: Vec::new() }
    }

    #[test]
    fn recovers_order_and_table() {
        let t = |b: u8| b ^ 0x5A;
        let samples: Vec<Sample> = (0..200u32).map(|i| sample(value(i), t)).collect();
        let l = recover(7, &samples).unwrap();
        assert_eq!(l.payload_len, 5);
        assert_eq!(l.table_conflicts, 0);
        assert_eq!(l.bytes[1], Carries::Field(0x10, 0));
        assert_eq!(l.bytes[4], Carries::Field(0x10, 3));
        assert_eq!(l.field_order(), vec![(0x10, "little-endian", 4)]);
        for (plain, cipher) in &l.table {
            assert_eq!(*cipher, t(*plain));
        }
    }

    #[test]
    fn a_constant_payload_byte_is_left_unexplained() {
        let t = |b: u8| b ^ 0x5A;
        let samples: Vec<Sample> = (0..200u32).map(|i| sample(value(i), t)).collect();
        // The header never varies, so nothing in the object can claim it.
        assert_eq!(recover(7, &samples).unwrap().bytes[0], Carries::Unknown);
    }

    #[test]
    fn a_big_endian_field_is_named_as_one() {
        let t = |b: u8| b.wrapping_add(3);
        let samples: Vec<Sample> = (0..200u32)
            .map(|i| {
                let v = value(i);
                let mut payload = vec![t(0xAA)];
                payload.extend(v.to_be_bytes().map(t));
                Sample { payload, words: vec![(0x10, v)], buffer: Vec::new() }
            })
            .collect();
        let l = recover(7, &samples).unwrap();
        assert_eq!(l.field_order(), vec![(0x10, "big-endian", 4)]);
    }

    #[test]
    fn too_few_blocks_is_no_answer() {
        let samples: Vec<Sample> = (0..4u32).map(|i| sample(value(i), |b| b)).collect();
        assert!(recover(7, &samples).is_none());
    }
}
