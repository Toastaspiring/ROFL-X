//! Field profiler: measure what every packet class carries, class by class.
//!
//! Typing a class by hand (`packet::handlers`) means dumping its decoded
//! object, working out a layout and checking it against something
//! independent. That scales to a dozen classes, not to the 273 netids a
//! 16.9 replay uses. This module does the mechanical half for all of them
//! at once: decode a sample of every netid's blocks and, for each 4-byte
//! word of the packet object, record what the value looked like across the
//! sample.
//!
//! Every classifier is a property we can state, not a guess:
//!
//! - **sender net id**: the word equals the block's `param`. The base
//!   deserializer writes it at `+0x0c`; finding it anywhere else would
//!   mean the layout assumption is wrong.
//! - **net id**: top byte `0x40`, the runtime entity-id space.
//! - **game time**: within a second of the block's own timestamp.
//! - **heap pointer**: the 64-bit value lands inside the plaintext heap
//!   this decode allocated, so the class owns a vector, string or nested
//!   object there. A second pointer right behind it is `std::vector`'s
//!   `last`, and the span is the payload size in bytes.
//! - **float**, **enum**, **counter**: by range, and by how many distinct
//!   values the word ever took across the sample.
//!
//! What comes out is not a typed handler; it is the evidence a handler
//! gets written from, for every class at once. [`Profile::markdown`]
//! renders it as `docs/FIELDS.md`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;

use serde::Serialize;
use serde_json::{json, Value};

use crate::emulator::machine::{HEAP_BASE, HEAP_SIZE};
use crate::emulator::{DecodeStatus, DecodedPacket, PacketDecoder, PatchArchive, PlainMemory};
use crate::error::{Result, RoflError};
use crate::rofl::block::BlockIterator;
use crate::rofl::decompress::decompress_chunk;
use crate::rofl::stream::StreamTag;
use crate::rofl::Replay;

/// How far into a packet object we profile. No 16.9 class writes past
/// this; `written_words` stops here too.
const OBJECT_LIMIT: u64 = 0x200;
/// Distinct values kept per word before the field is called "many".
const DISTINCT_CAP: usize = 48;
/// Example values printed per field.
const EXAMPLES: usize = 6;
/// Strings listed per string-bearing offset.
const STRING_CAP: usize = 8;
/// Candidate strings tracked per offset before counting stops.
const STRING_POOL: usize = 256;
/// A printable run is a real string once this many blocks carry it at the
/// same offset.
const STRING_REPEATS: u64 = 2;
/// A property holding for this share of the sample is called universal.
const MOSTLY: f64 = 0.95;
/// Summoner's Rift is ~14 870 units square; allow a margin.
const MAP_MAX: f32 = 16_384.0;
/// A float this close to the block's own timestamp is game time.
const TIME_EPS: f32 = 1.0;
/// Shortest inline run of printable bytes reported as a string.
const MIN_STRING: usize = 4;
/// Where the base deserializer writes the sender net id, for every class.
const SENDER: u64 = 0x0c;
/// Above this, an integer is not a counter: it is a hash or an id.
const HASH_FLOOR: u32 = 0x0010_0000;
/// Largest value accepted as an array's element count.
const COUNT_MAX: u32 = 1 << 20;
/// Largest array entry we look for, in bytes.
const STRIDE_MAX: u64 = 512;
/// Entries profiled per block, so a 10 000-entry array stays cheap.
const ENTRIES_PER_BLOCK: u64 = 64;
/// An entry layout is reported once this share of the class's arrays
/// agreed on the entry size.
const STRIDE_VOTE: f64 = 0.5;

/// One 4-byte word of a packet object, across a sample of blocks.
#[derive(Debug, Default, Clone)]
struct Word {
    /// Blocks where the deserializer wrote at least one of the 4 bytes.
    seen: u64,
    /// Blocks where it wrote byte `i` of the word.
    bytes: [u64; 4],
    values: BTreeSet<u32>,
    overflow: bool,
    zero: u64,
    eq_param: u64,
    entity: u64,
    float: u64,
    time: u64,
    coord: u64,
    pointer: u64,
    /// Blocks where the 64-bit value pointed into the game image: a
    /// vtable, so the field is a polymorphic sub-object, or a static.
    image_ptr: u64,
    image_rvas: BTreeSet<u64>,
    /// `std::vector`-style span in bytes, when the next word pair is a
    /// pointer at or after this one.
    spans: BTreeSet<u64>,
    /// Blocks where the two words behind this pointer were an equal pair
    /// of small integers: the count/capacity form of a dynamic array.
    counted: u64,
    count_max: u32,
    strings: BTreeSet<String>,
    umax: u32,
    fmin: f32,
    fmax: f32,
}

impl Word {
    fn merge(&mut self, o: &Word) {
        self.seen += o.seen;
        for i in 0..4 {
            self.bytes[i] += o.bytes[i];
        }
        for v in &o.values {
            if self.values.len() < DISTINCT_CAP {
                self.values.insert(*v);
            } else if !self.values.contains(v) {
                self.overflow = true;
            }
        }
        self.overflow |= o.overflow;
        self.zero += o.zero;
        self.eq_param += o.eq_param;
        self.entity += o.entity;
        self.float += o.float;
        self.time += o.time;
        self.coord += o.coord;
        self.pointer += o.pointer;
        self.image_ptr += o.image_ptr;
        for rva in &o.image_rvas {
            if self.image_rvas.len() < EXAMPLES {
                self.image_rvas.insert(*rva);
            }
        }
        self.counted += o.counted;
        self.count_max = self.count_max.max(o.count_max);
        for s in &o.spans {
            if self.spans.len() < DISTINCT_CAP {
                self.spans.insert(*s);
            }
        }
        for s in &o.strings {
            if self.strings.len() < STRING_CAP {
                self.strings.insert(s.clone());
            }
        }
        self.umax = self.umax.max(o.umax);
        self.fmin = if self.float == 0 { o.fmin } else { self.fmin.min(o.fmin) };
        self.fmax = self.fmax.max(o.fmax);
    }

    fn record(&mut self, o: &Obs) {
        let Obs { value: v, param, now, .. } = *o;
        if let Some(count) = o.count {
            self.counted += 1;
            self.count_max = self.count_max.max(count);
        }
        self.seen += 1;
        for (have, byte) in o.written.iter().zip(&mut self.bytes) {
            *byte += u64::from(*have);
        }
        if self.values.len() < DISTINCT_CAP {
            self.values.insert(v);
        } else if !self.values.contains(&v) {
            self.overflow = true;
        }
        self.umax = self.umax.max(v);
        if v == 0 {
            self.zero += 1;
        }
        if v == param && v != 0 {
            self.eq_param += 1;
        }
        if is_entity(v) {
            self.entity += 1;
        }
        let f = f32::from_bits(v);
        if is_float(f) {

            if self.float == 0 {
                self.fmin = f;
                self.fmax = f;
            } else {
                self.fmin = self.fmin.min(f);
                self.fmax = self.fmax.max(f);
            }
            self.float += 1;
            if (f - now).abs() <= TIME_EPS {
                self.time += 1;
            }
            if (0.0..=MAP_MAX).contains(&f) {
                self.coord += 1;
            }
        }
        if o.pointer.is_some() {
            self.pointer += 1;
        }
        if let Some(rva) = o.image_rva {
            self.image_ptr += 1;
            if self.image_rvas.len() < EXAMPLES {
                self.image_rvas.insert(rva);
            }
        }
        if let Some(span) = o.span {
            if self.spans.len() < DISTINCT_CAP {
                self.spans.insert(span);
            }
        }
    }
}

/// One word of one block, as the profiler saw it.
struct Obs {
    value: u32,
    written: [bool; 4],
    param: u32,
    /// The block's own timestamp, for the game-time test.
    now: f32,
    pointer: Option<u64>,
    /// Image RVA, when the 64-bit value at this word points into the
    /// game's own image.
    image_rva: Option<u64>,
    span: Option<u64>,
    count: Option<u32>,
}

/// One packet class, across a sample of its blocks.
#[derive(Debug, Default, Clone)]
pub struct ClassProfile {
    pub netid: u16,
    pub class: Option<String>,
    pub clients: Vec<String>,
    /// Blocks of this netid in the replay. Counted from framing, so this
    /// is the true total even where only a sample was decoded.
    pub blocks: u64,
    pub sampled: u64,
    status: HashMap<DecodeStatus, u64>,
    words: BTreeMap<u64, Word>,
    /// Printable runs found at each offset, with how many sampled blocks
    /// carried them. A coordinate float spelling four printable bytes is
    /// debris and never repeats; an asset name repeats every block.
    inline_strings: BTreeMap<u64, BTreeMap<String, u64>>,
    /// The entries behind each array pointer, keyed by the pointer's
    /// offset and then by the entry size measured for that block.
    arrays: BTreeMap<u64, BTreeMap<u64, ArrayProfile>>,
    payload_min: usize,
    payload_max: usize,
}

impl ClassProfile {
    fn merge(&mut self, o: &ClassProfile) {
        self.netid = o.netid;
        if self.class.is_none() {
            self.class.clone_from(&o.class);
            self.clients.clone_from(&o.clients);
        }
        self.blocks += o.blocks;
        if self.sampled == 0 {
            self.payload_min = o.payload_min;
        } else if o.sampled > 0 {
            self.payload_min = self.payload_min.min(o.payload_min);
        }
        self.sampled += o.sampled;
        self.payload_max = self.payload_max.max(o.payload_max);
        for (s, n) in &o.status {
            *self.status.entry(*s).or_default() += n;
        }
        for (off, w) in &o.words {
            self.words.entry(*off).or_default().merge(w);
        }
        for (off, strides) in &o.arrays {
            let into = self.arrays.entry(*off).or_default();
            for (stride, a) in strides {
                into.entry(*stride).or_default().merge(a);
            }
        }
        for (off, seen) in &o.inline_strings {
            let into = self.inline_strings.entry(*off).or_default();
            for (s, n) in seen {
                if let Some(have) = into.get_mut(s) {
                    *have += n;
                } else if into.len() < STRING_POOL {
                    into.insert(s.clone(), *n);
                }
            }
        }
    }

    fn clean(&self) -> u64 {
        self.status.get(&DecodeStatus::Clean).copied().unwrap_or(0)
    }

    /// Highest offset the class ever wrote, plus the word's width.
    pub fn object_size(&self) -> u64 {
        self.words.keys().next_back().map_or(0, |off| off + 4)
    }

    /// The measured layout: one entry per written word, in offset order.
    /// Words covered by a preceding 8-byte pointer are folded into it.
    pub fn fields(&self) -> Vec<Field> {
        let clean = self.clean().max(1) as f64;
        let mut out = Vec::new();
        let mut skip = 0u64;
        for (off, w) in &self.words {
            if *off == skip {
                continue;
            }
            let share = w.seen as f64 / clean;
            let pointer = w.pointer as f64 / w.seen.max(1) as f64 >= MOSTLY;
            if pointer {
                skip = off + 4;
            }
            let strings = strings_at(self.inline_strings.get(off), self.clean());
            let (mut kind, fmt) = classify(*off, w, &strings);
            let entries = match pointer.then(|| self.entry_layout(*off)).flatten() {
                Some((a, fields)) => {
                    kind = format!(
                        "pointer to array, {}-byte entries, up to {}",
                        fields.len() * 4,
                        a.max_entries
                    );
                    fields
                }
                None => Vec::new(),
            };
            out.push(Field {
                offset: format!("+0x{off:03x}"),
                width: if pointer { 8 } else { width(w) },
                kind,
                present: pct(share),
                distinct: if w.overflow {
                    json!(format!("{}+", w.values.len()))
                } else {
                    json!(w.values.len())
                },
                examples: examples(w, fmt, &strings),
                entries,
            });
        }
        out
    }

    /// The entry layout behind the array pointer at `off`, when enough
    /// of its blocks agreed on one entry size.
    fn entry_layout(&self, off: u64) -> Option<(&ArrayProfile, Vec<Field>)> {
        let strides = self.arrays.get(&off)?;
        let total: u64 = strides.values().map(|a| a.blocks).sum();
        let (_, best) = strides.iter().max_by_key(|(_, a)| a.blocks)?;
        if (best.blocks as f64) < STRIDE_VOTE * total as f64 || best.entries == 0 {
            return None;
        }
        Some((best, best.fields()))
    }

    pub fn to_json(&self) -> Value {
        let mut status: Vec<(&str, u64)> =
            self.status.iter().map(|(s, n)| (s.label(), *n)).collect();
        status.sort_unstable();
        json!({
            "netid": self.netid,
            "class": self.class,
            "clients": self.clients,
            "blocks": self.blocks,
            "sampled": self.sampled,
            "status": status.into_iter().collect::<BTreeMap<_, _>>(),
            "payload_bytes": {"min": self.payload_min, "max": self.payload_max},
            "object_size": self.object_size(),
            "fields": self.fields(),
        })
    }
}

/// The entries behind one array pointer, at one measured entry size.
#[derive(Debug, Default, Clone)]
struct ArrayProfile {
    /// Blocks whose array had this entry size.
    blocks: u64,
    /// Entries profiled (capped per block).
    entries: u64,
    /// Largest entry count seen, including the ones not profiled.
    max_entries: u32,
    words: BTreeMap<u64, Word>,
}

impl ArrayProfile {
    fn merge(&mut self, o: &ArrayProfile) {
        self.blocks += o.blocks;
        self.entries += o.entries;
        self.max_entries = self.max_entries.max(o.max_entries);
        for (off, w) in &o.words {
            self.words.entry(*off).or_default().merge(w);
        }
    }

    fn fields(&self) -> Vec<Field> {
        let n = self.entries.max(1) as f64;
        let mut out = Vec::new();
        let mut skip = u64::MAX;
        for (off, w) in &self.words {
            if *off == skip {
                continue;
            }
            let wide = w.pointer.max(w.image_ptr) as f64 / w.seen.max(1) as f64 >= MOSTLY;
            if wide {
                skip = off + 4;
            }
            let (kind, fmt) = classify(u64::MAX, w, &[]);
            out.push(Field {
                offset: format!("+0x{off:03x}"),
                width: if wide { 8 } else { width(w) },
                kind,
                present: pct(w.seen as f64 / n),
                distinct: if w.overflow {
                    json!(format!("{}+", w.values.len()))
                } else {
                    json!(w.values.len())
                },
                examples: examples(w, fmt, &[]),
                entries: Vec::new(),
            });
        }
        out
    }
}

/// One word of a packet object, as measured.
#[derive(Debug, Clone, Serialize)]
pub struct Field {
    pub offset: String,
    pub width: u8,
    pub kind: String,
    /// Share of the class's clean blocks that wrote this word.
    pub present: String,
    pub distinct: Value,
    pub examples: Vec<String>,
    /// For an array pointer: the layout of one entry.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<Field>,
}

/// Every class seen in one or more replays.
#[derive(Debug, Default, Clone)]
pub struct Profile {
    pub classes: BTreeMap<u16, ClassProfile>,
    pub replays: Vec<String>,
    pub patch: String,
}

impl Profile {
    pub fn merge(&mut self, other: &Profile) {
        for (netid, c) in &other.classes {
            self.classes.entry(*netid).or_default().merge(c);
        }
        self.replays.extend(other.replays.iter().cloned());
        if self.patch.is_empty() {
            self.patch.clone_from(&other.patch);
        }
    }

    pub fn blocks(&self) -> u64 {
        self.classes.values().map(|c| c.blocks).sum()
    }

    pub fn sampled(&self) -> u64 {
        self.classes.values().map(|c| c.sampled).sum()
    }

    /// Classes by block count, busiest first.
    pub fn by_volume(&self) -> Vec<&ClassProfile> {
        let mut v: Vec<&ClassProfile> = self.classes.values().filter(|c| c.blocks > 0).collect();
        v.sort_by_key(|c| (std::cmp::Reverse(c.blocks), c.netid));
        v
    }

    pub fn to_json(&self) -> Value {
        json!({
            "patch": self.patch,
            "replays": self.replays.len(),
            "blocks": self.blocks(),
            "sampled": self.sampled(),
            "classes": self.by_volume().iter().map(|c| c.to_json()).collect::<Vec<_>>(),
        })
    }

    /// The catalog as `docs/FIELDS.md`.
    pub fn markdown(&self) -> String {
        use std::fmt::Write;

        let total = self.blocks().max(1);
        let mut s = String::new();
        let named = self.classes.values().filter(|c| c.class.is_some()).count();
        let _ = write!(
            s,
            "# Measured field layouts, patch {}\n\n\
             What each packet class carries, one table per class, measured \
             rather than guessed.\n\
             Generated by `rofl-x fields`; the method and every classifier \
             are in `src/fields.rs`.\n\
             Regenerate with the command at the bottom. Do not hand-edit.\n\n\
             Source: {} replay(s), {} blocks, {} decoded and profiled, \
             {} classes ({} with a Riot class name).\n\n\
             Reading a table:\n\n\
             - **offset** is into the decoded packet object. `+0x00` is the \
             vtable and never appears; `+0x0c` is the sender net id, written \
             by the base deserializer for every class.\n\
             - **present** is the share of the class's clean blocks that \
             wrote the word. Below 100 % the field is optional, and what \
             gates it is not yet known.\n\
             - **kind** is the narrowest statement that held for at least \
             95 % of the sample. `net id` means the top byte was `0x40`; \
             `game time` means the float was within a second of the block's \
             own timestamp; `pointer` means the 64-bit value landed in the \
             heap the decode allocated.\n\
             - **distinct** counts the values the word ever took. A `+` \
             means the counter stopped at the cap.\n\
             - An **array** pointer gets a second table for one entry. \
             The entry size is measured, not assumed: the deserializer \
             writes the same fields of every entry, so the map of which \
             bytes it wrote repeats with the entry size.\n\n\
             A field here is a measurement, not a name. Classes with a \
             handler in `src/packet/handlers/` have named, independently \
             checked fields; see [PACKETS.md](PACKETS.md).\n\n---\n\n",
            self.patch,
            self.replays.len(),
            thousands(self.blocks()),
            thousands(self.sampled()),
            self.classes.values().filter(|c| c.blocks > 0).count(),
            named,
        );

        for c in self.by_volume() {
            let title = c.class.clone().unwrap_or_else(|| "*(no class name)*".into());
            let _ = write!(
                s,
                "## {} — netid {}\n\n{} blocks ({:.2} % of all), {} profiled",
                title,
                c.netid,
                thousands(c.blocks),
                c.blocks as f64 * 100.0 / total as f64,
                thousands(c.sampled),
            );
            let clean = c.clean();
            if clean != c.sampled {
                let _ = write!(s, ", {} clean", thousands(clean));
            }
            if !c.clients.is_empty() {
                let _ = write!(s, ". Registered by {}", c.clients.join(", "));
            }
            let _ = writeln!(
                s,
                ". Payload {}–{} bytes, object {} bytes.\n",
                c.payload_min,
                c.payload_max,
                c.object_size()
            );
            let fields = c.fields();
            if fields.is_empty() {
                let _ = writeln!(s, "No plaintext written: the class carries no payload.\n");
                continue;
            }
            let mut nested: Vec<&Field> = Vec::new();
            let _ = writeln!(s, "| offset | w | kind | present | distinct | values seen |");
            let _ = writeln!(s, "|--------|--:|------|--------:|---------:|-------------|");
            for f in &fields {
                row(&mut s, f);
                if !f.entries.is_empty() {
                    nested.push(f);
                }
            }
            s.push('\n');
            for f in nested {
                let _ = writeln!(s, "One entry of the array at `{}`:\n", f.offset);
                let _ = writeln!(s, "| offset | w | kind | present | distinct | values seen |");
                let _ = writeln!(s, "|--------|--:|------|--------:|---------:|-------------|");
                for e in &f.entries {
                    row(&mut s, e);
                }
                s.push('\n');
            }
        }
        let _ = writeln!(
            s,
            "---\n\nRegenerated with:\n\n```\nrofl-x fields -r <replay> [-r <replay> ...] \\\n  \
             --patch-dir patch --samples 2000 --markdown docs/FIELDS.md\n```"
        );
        s
    }
}

fn row(s: &mut String, f: &Field) {
    use std::fmt::Write;
    let _ = writeln!(
        s,
        "| `{}` | {} | {} | {} | {} | {} |",
        f.offset,
        f.width,
        f.kind,
        f.present,
        f.distinct,
        f.examples.join(", ")
    );
}

fn thousands(n: u64) -> String {
    let d = n.to_string();
    let mut out = String::with_capacity(d.len() + d.len() / 3);
    for (i, ch) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Decode a sample of every netid's blocks and profile their objects.
///
/// `samples` is the target number of blocks decoded per netid across the
/// whole replay; it is spread over the game chunks so the sample is not
/// all from the first minute. 0 means every block.
pub fn profile(
    replay: &Replay<'_>,
    archive: &PatchArchive,
    samples: usize,
    threads: usize,
) -> Result<Profile> {
    use rayon::prelude::*;

    let mut records = Vec::new();
    for chunk in replay.chunks() {
        let chunk = chunk?;
        if chunk.stream_tag == StreamTag::GameChunk {
            records.push(chunk);
        }
    }
    let per_chunk = if samples == 0 {
        u64::MAX
    } else {
        (samples as u64).div_ceil(records.len().max(1) as u64)
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(|e| RoflError::Emulator(format!("thread pool: {e}")))?;
    let outs: Vec<Result<BTreeMap<u16, ClassProfile>>> = pool.install(|| {
        records
            .par_iter()
            .map_init(
                || PacketDecoder::new(archive).map_err(|e| e.to_string()),
                |decoder, record| {
                    let decoder = decoder.as_mut().map_err(|e| RoflError::Emulator(e.clone()))?;
                    profile_chunk(decoder, archive, &decompress_chunk(record)?, per_chunk)
                },
            )
            .collect()
    });

    let mut profile = Profile {
        patch: replay.header.patch().to_string(),
        ..Default::default()
    };
    for out in outs {
        for (netid, c) in out? {
            profile.classes.entry(netid).or_default().merge(&c);
        }
    }
    Ok(profile)
}

fn profile_chunk(
    decoder: &mut PacketDecoder,
    archive: &PatchArchive,
    body: &[u8],
    per_chunk: u64,
) -> Result<BTreeMap<u16, ClassProfile>> {
    let image = archive.image_base..archive.image_base + archive.image_size;
    let mut out: BTreeMap<u16, ClassProfile> = BTreeMap::new();
    for block in BlockIterator::new(body) {
        let block = block?;
        let c = out.entry(block.packet_id).or_default();
        if c.blocks == 0 {
            c.netid = block.packet_id;
            c.class = archive.class_of(block.packet_id).map(str::to_owned);
            c.clients = archive
                .packets
                .get(&block.packet_id)
                .map(|v| v.iter().map(|p| p.client.clone()).collect())
                .unwrap_or_default();
        }
        c.blocks += 1;
        if c.sampled >= per_chunk {
            continue;
        }
        let pkt = decoder.decode(block.packet_id, block.param, block.payload)?;
        if c.sampled == 0 {
            c.payload_min = block.payload.len();
        } else {
            c.payload_min = c.payload_min.min(block.payload.len());
        }
        c.payload_max = c.payload_max.max(block.payload.len());
        c.sampled += 1;
        *c.status.entry(pkt.status).or_default() += 1;
        if pkt.status == DecodeStatus::Clean {
            observe(c, &pkt, block.param, block.timestamp, &image);
        }
    }
    Ok(out)
}

fn observe(c: &mut ClassProfile, pkt: &DecodedPacket, param: u32, now: f32, image: &Range<u64>) {
    let words = pkt.written_words(OBJECT_LIMIT);
    for (off, v) in &words {
        let (ptr, span, count) = if off % 8 == 0 {
            let ptr = pkt.u64(*off).filter(|p| is_heap(*p));
            let span = ptr
                .zip(pkt.u64(off + 8).filter(|p| is_heap(*p)))
                .and_then(|(a, b)| b.checked_sub(a));
            let count = ptr
                .and(pkt.u32(off + 8))
                .filter(|n| (1..=COUNT_MAX).contains(n))
                .filter(|n| pkt.u32(off + 12) == Some(*n));
            (ptr, span, count)
        } else {
            (None, None, None)
        };
        c.words.entry(*off).or_default().record(&Obs {
            value: *v,
            written: [0u64, 1, 2, 3].map(|k| pkt.memory.written(pkt.object + off + k)),
            param,
            now,
            pointer: ptr,
            image_rva: image_rva(pkt, pkt.object + off, image),
            span,
            count,
        });
        if let (Some(ptr), Some(count)) = (ptr, count) {
            observe_array(c, pkt, *off, ptr, count, param, now, image);
        }
        let at = ptr.and_then(|p| ascii_at(pkt, p));
        let inline = ascii_at(pkt, pkt.object + off);
        for s in at.into_iter().chain(inline) {
            let into = c.inline_strings.entry(*off).or_default();
            if let Some(n) = into.get_mut(&s) {
                *n += 1;
            } else if into.len() < STRING_POOL {
                into.insert(s, 1);
            }
        }
    }
}

/// Profile the entries behind one array pointer.
#[allow(clippy::too_many_arguments)]
fn observe_array(
    c: &mut ClassProfile,
    pkt: &DecodedPacket,
    off: u64,
    ptr: u64,
    count: u32,
    param: u32,
    now: f32,
    image: &Range<u64>,
) {
    let Some(stride) = stride_of(&pkt.memory, ptr, count) else {
        return;
    };
    let a = c.arrays.entry(off).or_default().entry(stride).or_default();
    a.blocks += 1;
    a.max_entries = a.max_entries.max(count);
    for e in 0..u64::from(count).min(ENTRIES_PER_BLOCK) {
        let base = ptr + e * stride;
        a.entries += 1;
        for k in (0..stride).step_by(4) {
            let written = [0u64, 1, 2, 3].map(|b| pkt.memory.written(base + k + b));
            if !written.iter().any(|b| *b) {
                continue;
            }
            let Some(value) = pkt.memory.u32(base + k) else {
                continue;
            };
            let inner = pkt.memory.u64(base + k).filter(|p| in_heap(&pkt.memory, *p));
            a.words.entry(k).or_default().record(&Obs {
                value,
                written,
                param,
                now,
                pointer: inner,
                image_rva: image_rva(pkt, base + k, image),
                span: None,
                count: inner
                    .and(pkt.memory.u32(base + k + 8))
                    .filter(|n| (1..=COUNT_MAX).contains(n))
                    .filter(|n| pkt.memory.u32(base + k + 12) == Some(*n)),
            });
        }
    }
}

/// The size of one entry in the array of `count` entries at `ptr`.
///
/// The deserializer asked the allocator for the whole array in one
/// block, and the emulator's allocator records every size it hands out,
/// so this is a division, not a guess. It is then checked against the
/// map of which bytes the deserializer wrote: it writes the same fields
/// of every entry, so that map has to repeat with the entry size.
fn stride_of(mem: &PlainMemory, ptr: u64, count: u32) -> Option<u64> {
    let count = u64::from(count);
    if count < 2 {
        return None;
    }
    let size = mem.block_size(ptr)?;
    let stride = size / count;
    if !(4..=STRIDE_MAX).contains(&stride) || size % count != 0 || stride % 4 != 0 {
        return None;
    }
    repeats(mem, ptr, count, stride).then_some(stride)
}

/// Whether the written-byte map of the first entry holds for all of them.
fn repeats(mem: &PlainMemory, ptr: u64, count: u64, stride: u64) -> bool {
    let mut any = false;
    for i in 0..stride {
        let first = mem.written(ptr + i);
        any |= first;
        for e in 1..count {
            if mem.written(ptr + e * stride + i) != first {
                return false;
            }
        }
    }
    any
}

/// The strings at one offset, if that offset really holds a string.
///
/// Two gates, both measured. A run has to repeat across blocks, and the
/// runs together have to cover most of the sample: a real name field
/// spells a name in every block, while float debris spells one in a
/// handful.
fn strings_at(seen: Option<&BTreeMap<String, u64>>, clean: u64) -> Vec<&String> {
    let Some(seen) = seen else { return Vec::new() };
    if seen.values().sum::<u64>() * 2 < clean {
        return Vec::new();
    }
    let mut v: Vec<(&String, u64)> = seen
        .iter()
        .filter(|(_, n)| **n >= STRING_REPEATS)
        .map(|(s, n)| (s, *n))
        .collect();
    v.sort_by_key(|(s, n)| (std::cmp::Reverse(*n), s.len()));
    v.into_iter().take(STRING_CAP).map(|(s, _)| s).collect()
}

/// A NUL-terminated identifier-looking run at `addr`, if there is one.
///
/// Four printable bytes in a row happen by accident all the time: a map
/// coordinate float is `7V<F`, and the next field's low byte is often 0,
/// so it even looks NUL-terminated. Requiring a leading letter and mostly
/// identifier characters keeps the Riot asset names (`YellowTrinket`,
/// `SRU_Plant_Vision`) and drops the float debris.
fn ascii_at(pkt: &DecodedPacket, addr: u64) -> Option<String> {
    let s = pkt.memory.cstr(addr, 64)?;
    if s.len() < MIN_STRING || !s.bytes().all(|b| (0x20..0x7F).contains(&b)) {
        return None;
    }
    if !s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    let wordy = s
        .bytes()
        .filter(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'.' || *b == b'/')
        .count();
    (wordy * 4 >= s.len() * 3).then_some(s)
}

fn is_heap(v: u64) -> bool {
    (HEAP_BASE..HEAP_BASE + HEAP_SIZE).contains(&v)
}

fn in_heap(mem: &PlainMemory, v: u64) -> bool {
    is_heap(v) && v < mem.end()
}

/// The image RVA the 64-bit value at `addr` points at, if it points
/// into the game image at all.
fn image_rva(pkt: &DecodedPacket, addr: u64, image: &Range<u64>) -> Option<u64> {
    let v = pkt.memory.u64(addr)?;
    image.contains(&v).then(|| v - image.start)
}

fn is_entity(v: u32) -> bool {
    v & 0xFF00_0000 == 0x4000_0000
}

/// A `u32` that plausibly is an `f32`: finite, normal, and inside the
/// magnitudes the game works in. Small integers land in the denormal
/// range and are excluded by the lower bound.
fn is_float(f: f32) -> bool {
    f.is_finite() && f.is_normal() && f.abs() >= 1e-4 && f.abs() <= 1e7
}

/// Narrowest width the deserializer ever wrote at this word.
fn width(w: &Word) -> u8 {
    match (w.bytes[3], w.bytes[2], w.bytes[1]) {
        (0, 0, 0) => 1,
        (0, 0, _) => 2,
        _ => 4,
    }
}

fn pct(share: f64) -> String {
    if share >= 0.9995 {
        "100%".into()
    } else {
        format!("{:.1}%", share * 100.0)
    }
}

/// How a word's example values should be printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fmt {
    Hex,
    Float,
    Dec,
    Str,
}

/// The narrowest statement that held for [`MOSTLY`] of the sample, and
/// how to print the word's values.
///
/// Zero is excluded from the denominator of the value tests: a position
/// field is still a float on the blocks where it happens to be 0.0, and
/// an optional net id is still a net id when it is unset.
fn classify(off: u64, w: &Word, strings: &[&String]) -> (String, Fmt) {
    let n = w.seen.max(1) as f64;
    let nz = (w.seen - w.zero).max(1) as f64;
    let most = |k: u64| k as f64 / n >= MOSTLY;
    let most_nz = |k: u64| k as f64 / nz >= MOSTLY;

    if w.eq_param > 0 && most_nz(w.eq_param) {
        let kind = if off == SENDER {
            "sender net id (block `param`)"
        } else {
            "net id, equals the block's `param`"
        };
        return (kind.into(), Fmt::Hex);
    }
    if most(w.image_ptr) {
        let kind = if w.image_rvas.len() == 1 {
            let rva = w.image_rvas.iter().next().copied().unwrap_or(0);
            format!("vtable, one class for every entry (image +0x{rva:x})")
        } else {
            "pointer into the game image (vtable or static)".into()
        };
        return (kind, Fmt::Hex);
    }
    if most(w.pointer) {
        let what = if strings.is_empty() { "heap" } else { "string" };
        let kind = match span_hint(w) {
            Some(hint) => format!("pointer to {what} ({hint})"),
            None => format!("pointer to {what}"),
        };
        return (kind, Fmt::Str);
    }

    if w.zero == w.seen {
        return ("always 0".into(), Fmt::Dec);
    }
    if w.values.len() == 1 && !w.overflow {
        let v = *w.values.iter().next().unwrap();
        return (format!("constant 0x{v:08x}"), Fmt::Hex);
    }
    if most_nz(w.entity) {
        return (optional("net id", w), Fmt::Hex);
    }
    if most_nz(w.time) {
        return (optional("float, game time (s)", w), Fmt::Float);
    }
    if most_nz(w.float) {
        let kind = if most_nz(w.coord) && w.fmax > 100.0 {
            "float, map coordinate range"
        } else {
            "float"
        };
        let kind = format!("{kind} [{:.1}, {:.1}]", w.fmin, w.fmax);
        return (optional(&kind, w), Fmt::Float);
    }
    let unit = match width(w) {
        1 => "u8",
        2 => "u16",
        _ => "u32",
    };
    if !w.overflow && w.values.len() <= 8 {
        return (format!("{unit} enum, {} values", w.values.len()), Fmt::Dec);
    }
    // `u32::MAX` alongside small values is the game's "none", not a range.
    let none = w.values.contains(&u32::MAX);
    let high = w.values.iter().rev().find(|v| **v != u32::MAX).copied().unwrap_or(0);
    if high > HASH_FLOOR {
        return (format!("{unit}, wide range (hash or id)"), Fmt::Hex);
    }
    let kind = if none {
        format!("{unit} counter, max {high}, 0xffffffff as none")
    } else {
        format!("{unit} counter, max {high}")
    };
    (kind, Fmt::Dec)
}

/// Fields only some blocks fill are optional; say so, since a layout
/// table is otherwise read as "always there".
fn optional(kind: &str, w: &Word) -> String {
    if w.zero * 20 > w.seen {
        format!("{kind}, 0 when unset")
    } else {
        kind.to_string()
    }
}

/// What the words behind a pointer say about the array it points at.
///
/// Two shapes appear. `std::vector` is three pointers, so the gap between
/// the first two is the payload in bytes and the gcd over the sample is
/// the entry size. The other is pointer plus an equal count/capacity
/// pair, which gives the number of entries but not their size.
fn span_hint(w: &Word) -> Option<String> {
    let spans: Vec<u64> = w.spans.iter().copied().filter(|s| *s > 0).collect();
    if spans.is_empty() {
        if w.counted * 2 >= w.pointer {
            return Some(format!(
                "array, count at +8, up to {} entries",
                w.count_max
            ));
        }
        return None;
    }
    let gcd = spans.iter().copied().fold(0u64, gcd);
    let max = spans.iter().copied().max().unwrap_or(0);
    if gcd >= 4 && spans.len() > 1 {
        Some(format!("vector, {gcd}-byte entries, up to {} seen", max / gcd))
    } else {
        Some(format!("vector, up to {max} bytes"))
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn examples(w: &Word, fmt: Fmt, strings: &[&String]) -> Vec<String> {
    if fmt == Fmt::Str || !strings.is_empty() {
        return strings.iter().take(EXAMPLES).map(|s| format!("{s:?}")).collect();
    }
    if w.values.len() > EXAMPLES && w.overflow {
        return Vec::new();
    }
    w.values
        .iter()
        .take(EXAMPLES)
        .map(|v| match fmt {
            Fmt::Float => format!("{:.2}", f32::from_bits(*v)),
            Fmt::Hex => format!("0x{v:08x}"),
            _ if *v > 0xFFFF => format!("0x{v:08x}"),
            _ => format!("{v}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_exclude_small_integers() {
        assert!(!is_float(f32::from_bits(100)));
        assert!(!is_float(f32::from_bits(0)));
        assert!(is_float(1.0));
        assert!(is_float(7_500.25));
        assert!(!is_float(f32::from_bits(0x7F80_0000)));
    }

    #[test]
    fn entity_ids_are_the_0x40_space() {
        assert!(is_entity(0x4000_00AE));
        assert!(!is_entity(0x0000_00AE));
        assert!(!is_entity(0x8000_0000));
    }

    #[test]
    fn width_follows_the_bytes_written() {
        let mut w = Word {
            bytes: [3, 0, 0, 0],
            ..Default::default()
        };
        assert_eq!(width(&w), 1);
        w.bytes[1] = 3;
        assert_eq!(width(&w), 2);
        w.bytes[3] = 1;
        assert_eq!(width(&w), 4);
    }

    #[test]
    fn param_match_names_the_sender_field() {
        let mut w = Word::default();
        for _ in 0..100 {
            w.record(&Obs {
                value: 0x4000_00AE,
                written: [true; 4],
                param: 0x4000_00AE,
                now: 10.0,
                pointer: None,
                image_rva: None,
                span: None,
                count: None,
            });
        }
        assert_eq!(classify(0x0c, &w, &[]).0, "sender net id (block `param`)");
    }

    #[test]
    fn vector_span_reports_the_entry_size() {
        let w = Word {
            spans: [88u64, 176, 264].into_iter().collect(),
            ..Default::default()
        };
        assert_eq!(
            span_hint(&w).as_deref(),
            Some("vector, 88-byte entries, up to 3 seen")
        );
    }
}
