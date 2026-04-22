# ROFL format specification

Byte-level specification of the `.rofl` file format, as best we understand
it as of patch **16.8.766.8562** (April 2026). Every field below carries
an evidence tag so that readers can judge how much to trust each claim.

## Evidence tags

| tag          | meaning                                                                |
|--------------|------------------------------------------------------------------------|
| `VERIFIED`   | Observed at a specific byte offset in a real replay, reproducible via `scripts/sample_walkthrough.py`. |
| `UPSTREAM`   | Present in Mowokuma's source code and matches observed bytes in our sample. |
| `ZHU`        | Cited from Henry Zhu's 2025 write-up; we have not independently reproduced it. |
| `INFERRED`   | Consistent with one sample but unconfirmed across patches/replays. |
| `UNKNOWN`    | Field is present but meaning is not yet known. |

All multibyte integers are **little-endian** unless stated otherwise.
This matches observed bytes and the language of Mowokuma's source.

Sample of record: `sample_a`, sha256 prefix `0224a2d7f1ff0cc3`,
15.35 MiB, patch 16.8.766.8562, described in detail in
[SAMPLE_A_WALKTHROUGH.md](SAMPLE_A_WALKTHROUGH.md).

---

## File layout

```
┌───────────────────────────────────────────────────────────┐
│  0                   FileHeader                           │
│    variable length = 0x0F + version_len                   │
├───────────────────────────────────────────────────────────┤
│  header_end         Chunks region                         │
│    N concatenated ChunkRecords                            │
│    N = lastGameChunkId + lastKeyFrameId + 2               │
│        (+2 for singleton streams 0x03 and 0x04)           │
├───────────────────────────────────────────────────────────┤
│  len - 4 - md_len - 256    Signature (256 bytes, opaque)  │
├───────────────────────────────────────────────────────────┤
│  len - 4 - md_len          MetadataBlob (UTF-8 JSON)      │
├───────────────────────────────────────────────────────────┤
│  len - 4                   u32 LE: md_len                 │
└───────────────────────────────────────────────────────────┘
```

Parse order should be: (a) read the trailing u32 to find the metadata
length, (b) lift the metadata JSON off the tail, (c) work backwards 256
bytes to lift off the signature, (d) read the file header from the front,
(e) walk chunks in the region between the header and the signature.

Reasoning: the file header doesn't carry any offset pointing to the
metadata, and the metadata doesn't carry any offset pointing to the
signature. The only deterministic way to find either is to start from the
end.

---

## Section A — FileHeader

Variable length. Layout, field by field:

### `magic` at `[0x00 .. 0x04]`

- 4 bytes: `52 49 4F 54` = ASCII `"RIOT"`.
- Tag: **VERIFIED**. Source: sample_a byte 0..4.
- Parsers must reject a file that doesn't start with this magic.

### `format_version` at `[0x04 .. 0x06]`

- u16 LE. Value `2` in sample_a.
- Tag: **INFERRED**. Source: sample_a, one data point.
- Hypothesis: format version / container version. No other values
  observed yet; no branching on this field is warranted until we have a
  second data point.

### `field_u16_0x06` at `[0x06 .. 0x08]`

- u16 LE. Value `217` (= `0x00D9`) in sample_a.
- Tag: **UNKNOWN**.
- Not file size. Not signature length. Not metadata length. Not a
  plausible chunk count. Name TBD; we store it as `header_field_0x06`
  in our struct and pass it through on round-trip.

### `field_bytes_0x08` at `[0x08 .. 0x0E]`

- 6 bytes. Value `7E E2 68 69 86 A9` in sample_a.
- Tag: **UNKNOWN**.
- Hypothesis: game id, session tag, per-file nonce, or timestamp.
  Bytes look high-entropy. Worth cross-referencing with match ID in
  the file name to see whether the filename's numeric id (e.g.
  `7828362936`) appears in these bytes. (Quick check for sample_a:
  `7828362936 = 0x1D2A4C9F8` as a u64 little-endian = `F8 C9 A4 D2 01
  00 00 00`; doesn't match the header bytes, so it's not the match id.)

### `version_len` at `[0x0E]`

- u8. Value `13` (= `0x0D`) in sample_a.
- Tag: **VERIFIED**. Source: matches the observed version-string length.

### `version_string` at `[0x0F .. 0x0F + version_len]`

- ASCII, no terminator. For sample_a: `"16.8.766.8562"` (13 chars).
- Tag: **VERIFIED**.
- Format is `<major>.<minor>.<build>.<revision>`. Major/minor are the
  public patch number (`"16.8"`). Build/revision are internal.
- The short patch tag `<major>.<minor>` is what we use as the key to
  select per-patch emulator config.

### `header_end` at `[0x0F + version_len]`

- Not a field; a computed boundary. `header_end` is where the chunk
  region begins.
- Tag: **VERIFIED**.
- For sample_a: `0x0F + 13 = 0x1C = 28`.

Mowokuma's `skip_rofl_header_size` uses a dual-branch heuristic (drain 16
bytes, then check byte `[0x1C]` relative to the already-drained buffer,
drain 12 or 13 more). That works for her test samples because she
effectively checks whether the byte after the 12-char version prefix is
still ASCII or not. Our reader uses the `version_len` field directly
and therefore has no branching here.

---

## Section B — Chunks region

Begins at `header_end`, ends at `len - 4 - md_len - 256`. The region is
`N` concatenated `ChunkRecord`s. `N` is not explicitly given in the file;
the parser iterates until the cursor reaches the signature boundary.

For sample_a: N = 106 = 1 (`0x03`-stream) + 1 (`0x04`-stream) + 69
(`0x01`-stream game chunks) + 35 (`0x02`-stream keyframes), matching the
metadata's `lastGameChunkId = 71` and `lastKeyFrameId = 35` (with the two
singletons accounting for the gap to 71).

### ChunkRecord layout (17-byte header)

| offset | size | field              | evidence |
|--------|------|--------------------|----------|
| 0x00   | 4    | `chunk_id` u32 LE  | UPSTREAM + VERIFIED |
| 0x04   | 1    | `chunk_type` u8    | UPSTREAM + VERIFIED |
| 0x05   | 4    | `chunk_id_2` u32 LE| UPSTREAM + VERIFIED (interpretation refined, see below) |
| 0x09   | 4    | `uncompressed_len` u32 LE | UPSTREAM + VERIFIED |
| 0x0D   | 4    | `compressed_len` u32 LE   | UPSTREAM + VERIFIED |

Header size is **exactly 17 (`0x11`) bytes**.

### ChunkRecord body

Comes immediately after the header.

If `compressed_len > 0`:
- `compressed_len` bytes of **zstd frame** data follow.
- The frame decompresses to exactly `uncompressed_len` bytes.
- Verified on sample_a chunk 0: 5852 compressed bytes starting with zstd
  magic `28 B5 2F FD` decompress to exactly 18,173 bytes. Tag: VERIFIED.

If `compressed_len == 0`:
- `uncompressed_len` bytes of raw data follow.
- Observed once in sample_a (chunk 1, 17 raw bytes). Tag: VERIFIED on this
  sample; INFERRED that this is a general rule.

### `chunk_id`

- Per-stream counter, starting at `1` within each `chunk_id_2` stream.
- Tag: **INFERRED** (one-sample observation, needs other replays).
- For game-chunk stream (`id2 = 0x01000000`): counts `1..=69` in sample_a.
- For keyframe stream (`id2 = 0x02000000`): counts `1..=35`.
- For singletons (`0x03` and `0x04`): each has exactly one record with
  its own `chunk_id` (observed `1` and `2` respectively; suspect the
  absolute value is meaningless for singletons, but unverified).

### `chunk_type`

- Dense monotonic counter across the full concatenation (all streams).
- Tag: **INFERRED**.
- In sample_a: `0x02` at chunk 0, incrementing by 1 per "time slot"
  (sometimes per chunk, sometimes shared when a game chunk and keyframe
  sit in the same slot), reaching `0x48` (= 72) at the last chunk.
- Best read as a time-slot index, not a type discriminator. Mowokuma
  uses `chunk_type == 0x2` to skip the initial keyframe; that works on
  sample_a only because no other chunk happens to have type 2.
- Real keyframe identification should key off `chunk_id_2 == 0x02000000`,
  not off `chunk_type`.

### `chunk_id_2`

- Stream tag, packed as `(tag << 24)`. The high byte is the tag; the low
  three bytes are zero in every chunk observed in sample_a.
- Tag: **INFERRED** (one-sample observation of the packing; cross-check
  needed).
- Observed tag values:

  | tag  | role                        | count in sample_a |
  |------|-----------------------------|-------------------|
  | `0x01` | `GameChunkStream` (deltas)| 69                |
  | `0x02` | `KeyframeStream`          | 35                |
  | `0x03` | Initial keyframe singleton| 1 (chunk index 0) |
  | `0x04` | Start sentinel singleton  | 1 (chunk index 1) |

- Rust-level representation: a `ChunkStream` enum with these four
  variants, derived from `(chunk_id_2 >> 24) & 0xFF`. Low bytes are
  currently a pass-through; if they ever become non-zero we'll revisit.

### `uncompressed_len`, `compressed_len`

- u32 LE each. Semantics per the body section above.
- Tag: **VERIFIED**.

---

## Section C — Signature

- 256 bytes at `[len - 4 - md_len - 256 .. len - 4 - md_len]`.
- Tag: **UPSTREAM + VERIFIED** (offset and size; contents opaque).
- Content: appears random. Hypothesis: RSA-2048 signature of the bytes
  preceding it. Not verified.
- Action: **preserve exactly on round-trip**. Do not modify, do not
  attempt to verify.

---

## Section D — MetadataBlob

- UTF-8 JSON, no BOM, no terminator.
- Located at `[len - 4 - md_len .. len - 4]` where `md_len` is the
  trailing u32.
- Tag: **UPSTREAM + VERIFIED**.

### Observed top-level keys

| key                | type  | evidence | notes                                    |
|--------------------|-------|----------|------------------------------------------|
| `gameLength`       | int   | VERIFIED | milliseconds, e.g. `2050250` = ~34 min   |
| `lastGameChunkId`  | int   | VERIFIED | = count of game-chunk stream records     |
| `lastKeyFrameId`   | int   | VERIFIED | = count of keyframe stream records       |
| `statsJson`        | str   | VERIFIED | stringified JSON, see below              |

No `gameVersion` at this level in sample_a. The version of record lives
in the file header.

### `statsJson` structure

- A JSON array of **10 player objects**, one per participant.
- Each object has ~347 keys in sample_a: most are per-match stat
  counters and event-mission trackers (e.g.
  `2026_S1A1_SR_GrowthSmashed`, `Event_ARAM_Docks`); these are
  event/mission telemetry Riot uses for battle-pass progression and are
  not structurally important.

The keys ROFL-X cares about:

| key                   | evidence | notes                                    |
|-----------------------|----------|------------------------------------------|
| `SKIN`                | UPSTREAM + VERIFIED | champion name (e.g. `"Sett"`)  |
| `TEAM`                | UPSTREAM + VERIFIED | `"100"` = Blue, `"200"` = Red  |
| `WIN`                 | UPSTREAM + VERIFIED | `"Win"` or `"Fail"`            |
| `NAME`                | UPSTREAM + VERIFIED, now empty | deprecated ~2023; always `""` in new replays |
| `RIOT_ID_GAME_NAME`   | VERIFIED | Riot ID "game name" portion              |
| `RIOT_ID_TAG_LINE`    | INFERRED (presence) | Riot ID "tag line" portion    |
| `PUUID`               | VERIFIED | 36-char stable player identifier         |
| `INDIVIDUAL_POSITION` | INFERRED, unverified | likely the authoritative role field; needs check |
| `TEAM_POSITION`       | INFERRED, unverified | likely team's assigned role; needs check |

Role inference: Mowokuma uses `["Top","Jungle","Mid","Adc","Support"][i % 5]`.
ROFL-X should prefer `INDIVIDUAL_POSITION` / `TEAM_POSITION` if present,
falling back to array-index only as last resort.

---

## Section E — Trailing u32

- 4 bytes at `[len - 4 .. len]`, u32 LE.
- Value: byte length of `MetadataBlob`.
- Tag: **UPSTREAM + VERIFIED**.

---

## Compression

- Algorithm: **zstd** (RFC 8878), one independent frame per chunk.
- Tag: **VERIFIED**. Sample_a chunk 0 frame starts with magic
  `28 B5 2F FD` and decompresses cleanly with the standard zstd
  decoder; decompressed size matches `uncompressed_len` exactly.
- Per-chunk frames allow parallel decompression.
- Frame parameters (window size, compression level) are whatever the
  game client's compressor chose; our decoder just consumes the frame.

---

## Block framing (inside a decompressed chunk)

Each decompressed chunk body is a tight stream of variable-length
`Block` records. This is Mowokuma's decoder, validated by our walk of
sample_a (1.9M blocks parsed with zero errors).

### Marker byte

The first byte of each block is a marker with four bit flags:

```
bit 0x80  set → timestamp is a u8 delta (scaled by 0.001 s)
          clear → timestamp is an absolute f32
bit 0x40  set → packet_id is reused from the previous block
          clear → packet_id is a fresh u16
bit 0x20  set → param is a u8 delta (added to previous param)
          clear → param is a fresh u32
bit 0x10  set → length is a u8
          clear → length is a u32
```

### Field order after the marker

In all cases, fields appear in this order:

1. `timestamp`: u8 delta (1 byte) or absolute f32 (4 bytes) per bit 0x80.
2. `length`: u8 (1 byte) or u32 (4 bytes) per bit 0x10.
3. `packet_id`: reused (0 bytes) or fresh u16 (2 bytes) per bit 0x40.
4. `param`: u8 delta (1 byte) or fresh u32 (4 bytes) per bit 0x20.
5. `payload`: exactly `length` bytes.

Parser state carried between blocks: `accumulated_time` (running f32),
`previous_packet_id`, `previous_param`. Initialised to 0 at the start of
each chunk.

### Accumulated time

When bit `0x80` is set, the u8 delta is scaled by `0.001` and added to
the running accumulator. When clear, the accumulator is reset to the
new absolute f32.

### Size bounds

- Minimum block: 1 marker + 1 ts-delta + 1 len-u8 + 0 (reuse pid) + 1
  param-delta + 0 payload = **4 bytes**.
- Maximum block header: 1 + 4 + 4 + 2 + 4 = **15 bytes**.
- Payload can be anywhere from 0 to `u32::MAX` bytes in theory; in
  practice we see ~5–20 KB for dense keyframe payloads and 4–200 bytes
  for typical game-chunk blocks.

---

## Packet payloads ("decryption")

This is the layer where the format is designed to be hostile to static
reimplementation. Evidence tag for everything in this section: **ZHU**,
reinforced by UPSTREAM.

- The payload is **not** encrypted with a named cipher (no AES, no
  Blowfish, no ChaCha). It's a custom per-packet-class obfuscation
  built from byte-wise XOR constants, rotations, and lookups into a
  small (~255-byte) table.
- Each packet class has its own decoder function inside the game
  binary, with its own lookup table.
- The decoder uses a **decrypt-access-release** pattern: fields stay
  obfuscated in memory and are decrypted on demand, used immediately,
  re-encrypted and zeroed before the function returns.
- The obfuscation changes per patch; lookup tables, constants, and
  register allocation all drift. Static porting is not a viable
  strategy.
- Our chosen decode strategy is **emulation** of the game's own decoder
  functions, same approach Mowokuma takes and Henry Zhu documents.

Our parser does **not** implement any packet-payload decoding in Rust
directly. All per-opcode decoding is either (a) routed through the
emulator with per-patch RVAs and struct offsets, or (b) marked
`UNKNOWN` / `OBSERVED-ONLY` in the catalog.

---

## Coordinate system

- Map: Summoner's Rift, roughly 14820 × 14881 map units.
- Origin: bottom-left corner.
- Mowokuma stores positions as `(x, y)` float pairs; these are actually
  the horizontal-plane coordinates (game's `x` and `z` in 3D; `y` is
  height and not stored).
- Ward positions come out as integers in the same space.
- Henry Zhu's output uses `(x, z)` naming, which is more accurate to
  the game engine's axes. ROFL-X output adopts `(x, y)` in the 2D
  top-down sense for consistency with Mowokuma's existing JSON schema.

---

## What a conformant ROFL-X parser must do

1. **Magic check**: reject files not starting with `"RIOT"`.
2. **Variable-length header**: read `version_len` at `[0x0E]` and
   consume exactly `0x0F + version_len` bytes as the header.
3. **Tail-first metadata**: read the trailing u32, lift the JSON,
   lift the 256-byte signature, remember the chunks-region bounds.
4. **Chunk walk**: iterate ChunkRecords in order; for each, either
   decompress with zstd (if `compressed_len > 0`) or consume
   `uncompressed_len` raw bytes.
5. **Stream grouping**: route chunks to logical streams by
   `chunk_id_2 >> 24`; expose all four streams to callers.
6. **Block framing**: decompose each `GameChunkStream` chunk body into
   Blocks using the marker-byte rules.
7. **Opcode routing**: look up each `Block.packet_id` in the registry.
   Known opcodes get decoded (pure Rust or emulator-backed). Unknown
   opcodes log + count and pass through as raw.
8. **Round-trip safety**: if we ever build a writer, the signature
   and any `UNKNOWN` header fields must be preserved exactly.
9. **Honest reporting**: the audit subcommand must report opcodes seen
   vs opcodes in catalog, making coverage gaps visible.

---

## Open questions, to be resolved as more samples arrive

- **Low bytes of `chunk_id_2`**: always zero in sample_a. Are they ever
  non-zero? If yes, what do they mean?
- **`field_u16_0x06`**: value 217 in our one sample. What does it vary
  with? Patch? Game length? Always the same?
- **`field_bytes_0x08`**: 6 bytes of unknown. Match id hash? Per-game
  nonce? Constant-within-patch?
- **`INDIVIDUAL_POSITION`**: is it actually present in `statsJson[i]`?
  If yes, what are its possible values?
- **Start sentinel chunk (`0x04` stream) body**: 17 opaque bytes. Same
  across replays on same patch? Different per replay? Same across
  patches?
- **Chunk type `0x02` special semantics**: is the initial-keyframe
  skip rule Mowokuma uses actually correct, or does her `chunk_type == 0x2`
  test lucky-punch agree with the more robust `chunk_id_2 >> 24 == 0x03`
  test only because of slot-ordering coincidence?

Each of these is a candidate probe when we add a second sample.

---

## Revision history

- 2026-04-22: initial version. Based entirely on sample_a (patch
  16.8.766.8562) + Mowokuma's source + Henry Zhu's write-up. Single-sample
  claims carry an `INFERRED` tag and should be re-checked as new replays
  are added.
