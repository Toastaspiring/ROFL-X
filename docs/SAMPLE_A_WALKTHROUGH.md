# Sample A, byte-level walkthrough of a real ROFL file

Phase 1 step 3: empirical verification of the ROFL outer container against a
real replay from the user's local folder. Every claim here is backed by
hex bytes at a specific offset; anything still guessed is marked.

Reproducer: `scripts/sample_walkthrough.py`. It reads one hard-coded
replay path, prints annotated structure, and never commits replay bytes.

---

## Sample metadata

| field        | value                                            |
|--------------|--------------------------------------------------|
| nickname     | `sample_a`                                       |
| region / id  | EUW1 / (elided)                                  |
| size         | 16,099,007 bytes (15.35 MiB)                     |
| sha256 head  | `0224a2d7f1ff0cc3`                               |
| game length  | 2,050,250 ms (~34 min)                           |
| patch        | **16.8.766.8562** (Apr 2026)                     |
| chunks       | 106 total: 71 game chunks + 35 keyframes         |
| statsJson[0] | SKIN=`Sett`, TEAM=`100`, WIN=`Win`, NAME=`""`    |

Anonymisation: only the sha256 prefix and high-level stats are persisted. The
file path, summoner puuid, and Riot ID are never written to committed docs
or scripts output.

---

## Top-level layout (empirical)

```
┌─────────────────────────────────────────────────────────────┐
│  offset 0x00000000 : "RIOT" magic + 28-byte file header     │ ← section A
├─────────────────────────────────────────────────────────────┤
│  offset 0x0000001C : 106 concatenated chunks                │ ← section B
│    each chunk =  17-byte header + (compressed | raw) payload│
├─────────────────────────────────────────────────────────────┤
│  offset 0x00F3EBD3 : 256-byte signature block               │ ← section C
├─────────────────────────────────────────────────────────────┤
│  offset 0x00F3ECD3 : 113,128 bytes of metadata JSON (UTF-8) │ ← section D
├─────────────────────────────────────────────────────────────┤
│  offset 0x00F5A3FB : u32 LE, metadata JSON length          │ ← section E
└─────────────────────────────────────────────────────────────┘
total = 16,099,007 bytes
```

Mowokuma's parser handles this end-to-end correctly for this sample, with two
notable caveats documented below (§§ Header-parsing fragility, Version bug).

---

## § Section A, file header (28 bytes)

First 32 bytes of `sample_a`:

```
00000000  52 49 4f 54 02 00 d9 00 7e e2 68 69 86 a9 0d 31  |RIOT....~.hi...1|
00000010  36 2e 38 2e 37 36 36 2e 38 35 36 32 01 00 00 00  |6.8.766.8562....|
```

Field-by-field, **as evidenced by this sample**:

| offset | size | bytes                       | interpretation                    |
|--------|------|-----------------------------|-----------------------------------|
| 0x00   | 4    | `52 49 4f 54`               | ASCII magic `"RIOT"`              |
| 0x04   | 2    | `02 00` → u16 LE 2          | likely a format version (`2`)     |
| 0x06   | 2    | `d9 00` → u16 LE 217        | **unknown**; not file size, not signature length, not metadata length |
| 0x08   | 6    | `7e e2 68 69 86 a9`         | **unknown**; may be game id / timestamp / session tag |
| 0x0E   | 1    | `0d` → u8 13                | **length of the version string**  |
| 0x0F   | 13   | `31 36 2e 38 2e 37 36 36 2e 38 35 36 32` | ASCII `"16.8.766.8562"` |
| 0x1C   |      |                             | **end of header; chunks start here**  |

The header is **variable-length**: it depends on the length-prefix byte at
`[0x0E]`. For a 13-char version (e.g. `16.8.xxx.yyyy`) the header is 0x1C
(28) bytes; for a 14-char version (e.g. `16.10.xxx.yyyy`) it would be 0x1D
(29) bytes.

### Correction to `REFERENCE_MOWOKUMA.md`, two related bugs

Mowokuma's `chunk.rs::skip_rofl_header_size` contains a `// FIXME: very bad`
heuristic: drain 16 bytes, then branch on `buffer[0xC]` (= original offset
`0x1C`), draining either 12 or 13 more. **That heuristic accidentally works**
for the samples she's tested against, but the *real* rule is: the byte at
`[0x0E]` is the version-string length. Pure Rust readers should use that
directly.

A consequence: Mowokuma's `metadata.rs::parse` reads the version as
`buffer[16..20]`, bytes `[0x10..0x14]`, which gives `"6.8."` for this
sample. **The correct version is `"16.8"`** (she drops the leading `"1"`
stored at `[0x0F]`, and her trailing `.` is actually the second `.` in
`"16.8."`, not a terminator). Her patch-file lookup then maps version
`"6.8."` → `"6-8"` → `patch/6-8.patch`, which is fine for her because her
patch files are named to match her bug; but our ROFL-X version reader must
produce the real value `"16.8"` (or `"16.8.766.8562"` if we want the full
build tag).

---

## § Section B, 106 chunks (concatenated)

Each chunk starts with a 17-byte header. Layout per Mowokuma:

| rel offset | size | field              |
|------------|------|--------------------|
| 0x00       | 4    | `chunk_id` u32 LE  |
| 0x04       | 1    | `chunk_type` u8    |
| 0x05       | 4    | `chunk_id_2` u32 LE|
| 0x09       | 4    | `uncompressed_len` u32 LE |
| 0x0D       | 4    | `compressed_len` u32 LE   |

Body immediately follows the header:

- If `compressed_len > 0`: `compressed_len` bytes of **zstd** data that
  decompress to exactly `uncompressed_len` bytes. Verified for this sample:
  first chunk's payload starts with `28 b5 2f fd` (zstd magic) and expands
  from 5,852 → 18,173 bytes.
- If `compressed_len == 0`: **`uncompressed_len` bytes of raw bytes** follow.
  Mowokuma handles this path too. We observed exactly one such chunk in
  `sample_a` (idx 1, `uncompressed_len=17`).

### Chunk walk, first 15 chunks of sample_a

```
 idx    offset  id type        id2      ulen     clen
   0  0x00001c   1 0x02  0x03000000    18173     5852
   1  0x001709   2 0x03  0x04000000       17        0
   2  0x00172b   1 0x03  0x02000000   123890    29342
   3  0x0089da   3 0x04  0x01000000   217501    28395
   4  0x00f8d6   4 0x05  0x01000000   359143    74360
   5  0x021b5f   2 0x05  0x02000000   168259    51431
   6  0x02e457   5 0x06  0x01000000   958289   145875
   7  0x051e3b   6 0x07  0x01000000  1413427   222803
   8  0x08849f   3 0x07  0x02000000   175300    55021
   9  0x095b9d   7 0x08  0x01000000  1641277   239842
  10  0x0d0490   8 0x09  0x01000000   959091   172245
  11  0x0fa576   4 0x09  0x02000000   175368    56727
  12  0x10831e   9 0x0a  0x01000000  1389090   217769
  13  0x13d5d8  10 0x0b  0x01000000  1295064   207128
  14  0x16ff01   5 0x0b  0x02000000   183487    61217
```

### What the fields actually mean (new observations)

**`chunk_id_2`** is not a second id, it's a **stream discriminator** packed
as `(stream_tag << 24)`. Across 106 chunks we see exactly four values:

| chunk_id_2   | count | meaning (inferred)                              |
|--------------|-------|-------------------------------------------------|
| `0x03000000` | 1     | "start keyframe", chunk 0 only                 |
| `0x04000000` | 1     | "start sentinel"/"transition", chunk 1 only, `compressed_len == 0`, `uncompressed_len = 17` |
| `0x01000000` | 69    | game-chunk stream (deltas)                      |
| `0x02000000` | 35    | keyframe stream (periodic full-state snapshots) |

Cross-check: `lastGameChunkId = 71` and `lastKeyFrameId = 35` in the
metadata JSON. We find 69 `0x01`-stream chunks here, two short of 71,
**but** the two special early chunks (stream `0x03` and `0x04`) together
account for the missing count: the file has 71 game-chunk-equivalent records
and 35 keyframe records, as advertised.

**`chunk_id`** is a per-stream counter. For stream `0x01` (game chunks) it
counts 1, 2, 3, 4, …; for stream `0x02` (keyframes) it counts 1, 2, 3, ….
The final chunk is `id=71, stream=0x01`; the penultimate relevant one is
`id=35, stream=0x02`. This matches the metadata exactly.

**`chunk_type`** (the u8) is a dense monotonic counter across the
concatenation, from `0x02` at chunk 0 to `0x48` at chunk 105 (0x48 = 72; 72
values). The pattern of the last five chunks is diagnostic:

```
chunk  idx=101  type=0x45 stream=0x02 (keyframe 34)
chunk  idx=102  type=0x46 stream=0x01 (game 69)
chunk  idx=103  type=0x47 stream=0x01 (game 70)
chunk  idx=104  type=0x47 stream=0x02 (keyframe 35)
chunk  idx=105  type=0x48 stream=0x01 (game 71)
```

Note that `type=0x47` appears twice, once as a game chunk, once as a
keyframe, at the same "time slot". So `chunk_type` looks like a
**time-slot index** (~a 30-second bucket?), and each slot can hold one game
chunk and optionally one keyframe. Mowokuma never reads this field for
anything but the keyframe-skip decision (`chunk_type != 0x2`), but it
strongly encodes temporal ordering.

**Mowokuma's "skip `chunk_type == 0x2`" heuristic** actually doesn't skip
*all* keyframes, it only skips the very first one (chunk 0). All other
keyframes (chunks 5, 8, 11, 14, …, 104) have `chunk_type` values `0x05`,
`0x07`, `0x09`, `0x0B`, etc. She's skipping the "initial state keyframe"
specifically, not the keyframe stream. This might be deliberate (the
initial keyframe carries setup data that her two decoders don't care about)
or it might be a bug of her heuristic not matching her intent. Worth
confirming with her, but it works in practice because game chunks carry the
positions/wards she wants.

We should add a catalog entry for "stream `0x02` keyframe chunks" as a
distinct container type in ROFL-X.

### What is chunk 1 (the `clen==0` sentinel)?

Raw bytes at `[0x1709..0x172B]` (17-byte header + 17-byte body):

```
header: 02 00 00 00 03 00 00 00 04 11 00 00 00 00 00 00 00
body:   <17 bytes, opaque>
```

Header fields: `id=2, type=0x03, id2=0x04000000, ulen=17, clen=0`.

This chunk is present in exactly one position (idx 1) with one unique id2
value. Our hypothesis: it's a **start-of-stream transition marker** between
the initial state keyframe (chunk 0) and the game-chunk stream (chunks 2+).
The 17-byte body is small enough to hold a per-stream encryption nonce or a
block-stream initialiser, we can't tell without more samples.

**For Phase 2 format spec:** document this as `UNKNOWN-START-MARKER`, one
per file.

---

## § Section C, signature block (256 bytes)

Located at `[sig_start..sig_start + 256]` where
`sig_start = len - 4 - metadata_len - 256`. For `sample_a`:
`sig_start = 0x00F3EBD3`.

First 64 bytes:

```
00f3ebd3  10 d3 d9 86 c0 ba fb dc 0c 6b 69 9e 7c 69 a1 d1  |.........ki.|i..|
00f3ebe3  a3 1c be 4e 57 bc 1a 1c bb 7c 00 b2 fd fc c4 69  |...NW....|.....i|
00f3ebf3  96 d9 e3 00 5d a7 07 af e4 43 86 14 12 fe 06 e9  |....]....C......|
00f3ec03  52 2f 3a d0 49 d6 6c 96 0c 97 ba 57 36 3d d7 1d  |R/:.I.l....W6=..|
```

Looks random. Mowokuma throws it away. We have no way from one sample to
tell whether this is an RSA-2048 signature of the preceding bytes, a
per-file HMAC, a symmetric MAC, or raw bytes of something else entirely.
A signature is most likely given the length (256 B = 2048-bit RSA), and
Riot's replay client probably validates it on load. For Phase 2 we mark it
`UNKNOWN-SIGNATURE` and note that our parser **must preserve it untouched
on round-trip** if we ever build a writer.

---

## § Section D, metadata JSON (113,128 bytes)

UTF-8 JSON, no BOM, no terminator. Top-level keys in `sample_a`:

```
gameLength          : int  (ms)        = 2050250
lastGameChunkId     : int               = 71
lastKeyFrameId      : int               = 35
statsJson           : str  (JSON-in-JSON)
```

**No `gameVersion` key at the top level.** The version of record lives in
the file header (`[0x0F..0x0F+len]`), not in the JSON.

### `statsJson` structure

A JSON array of 10 player objects. Each object has 347 keys, most are
per-match statistic counters (e.g. `TOTAL_DAMAGE_DEALT_TO_CHAMPIONS`,
`MINIONS_KILLED`), event-mission trackers
(`2026_S1A1_SR_GrowthSmashed`, `Event_S1_A2_AprilFools_Dragon`), etc.

The fields ROFL-X cares about at minimum:

| key                   | value in sample (player 0)        | notes |
|-----------------------|-----------------------------------|-------|
| `SKIN`                | `"Sett"`                          | champion name (same as "skin")  |
| `TEAM`                | `"100"`                           | `100`=blue, `200`=red          |
| `WIN`                 | `"Win"`                           | `"Win"`/`"Fail"`               |
| `NAME`                | `""`                              | **empty since ≈2023**, Riot deprecated `NAME` / `SUMMONER_NAME` in favour of Riot ID |
| `RIOT_ID_GAME_NAME`   | 10 chars (redacted)               | Riot ID "game name" portion     |
| `RIOT_ID_TAG_LINE`    | (not probed yet)                  | Riot ID "tag line" portion     |
| `PUUID`               | 36 chars (redacted)               | stable player identifier        |
| `SUMMONER_NAME`       | *missing*                         | retired key                     |

**Implication for ROFL-X metadata layer:** Mowokuma reads `NAME`, which has
been empty string for ~3 years. We should read `RIOT_ID_GAME_NAME`
(+ `RIOT_ID_TAG_LINE` if present) and expose a `riot_id` field in our
output, in addition to preserving `name: ""` for schema parity with
Mowokuma's consumers.

### Role inference, worth revisiting

Mowokuma: `position = ["Top","Jungle","Mid","Adc","Support"][i % 5]`. The
ordering relies on `statsJson` being in canonical role order per team. We
have not yet verified whether `statsJson` is ordered by role or by join
order, nor how autofill / flex roles are represented. There may be an
`INDIVIDUAL_POSITION` or `TEAM_POSITION` key in the object that's more
authoritative, worth probing in Phase 2.

---

## § Section E, trailing u32 (metadata length)

Last 4 bytes of the file, LE:

```
00f5a3ff  a8 b9 01 00                        |....|
```

`0x0001B9A8 = 113128` = byte length of the metadata JSON. Used by the parser
to locate the JSON by working backwards from the end of the file.

---

## Decompressed block stream, chunk 0 (initial keyframe)

Chunk 0 decompresses to 18,173 bytes. First 128 bytes of the decompressed
block stream:

```
00000000  01 00 00 00 00 ee 46 00 00 78 04 00 00 00 00 59  |......F..x.....Y|
00000010  71 1a 0b c4 34 0a 63 4f 8e 8b 4f 2d b8 1e ef ac  |q...4.cO..O-....|
00000020  36 45 b4 7e 5c 6d 4f 8f 67 9e 82 2f be 6b 8f b2  |6E.~\mO.g../.k..|
...
```

Parsing by Mowokuma's marker-byte rules (`01` → all bits clear → absolute
timestamp, u32 length, u16 packet_id, u32 param):

```
marker     = 0x01
timestamp  = 0.0  (absolute f32)
length     = 18158  (u32)
packet_id  = 1144  (0x0478)
param      = 0     (u32)
payload    = 18158 bytes of opaque data
```

The entire chunk is **one block** with `packet_id = 0x0478`. This is the
initial-state keyframe: all entity setup in a single packet. Blocks header
overhead = 15 bytes; 15 + 18158 = 18173 = `uncompressed_len`. Exactly
accounted for.

**Catalog seed:** opcode `0x0478` → `INITIAL_STATE_KEYFRAME`, status
`OBSERVED-ONLY`, semantics unknown without emulator access.

---

## Decompressed block stream, first game chunk (chunk 2)

Chunk 2 decompresses to 123,890 bytes. First 128 bytes:

```
00000000  01 00 00 00 00 02 00 00 00 fe 02 00 00 00 00 5a  |...............Z|
00000010  c3 31 00 00 00 00 02 2b 04 00 5f cb 31 00 00 00  |.1.....+.._.1...|
00000020  00 11 58 00 00 bc fe fe fe fe fe fe fc 49 2a 30  |..X..........I*0|
00000030  37 30 3e 15 26 04 31 00 00 00 00 0f 59 04 00 93  |70>.&.1.....Y...|
...
```

Parses cleanly all the way through: **5,738 blocks, 84 distinct opcodes**,
zero parse errors.

Top 10 opcodes in this single chunk:

| opcode      | count | notes (speculative)                               |
|-------------|-------|---------------------------------------------------|
| `0x036B` (875) | 2920  | dominant; probably per-tick per-entity update    |
| `0x03F7` (1015)| 224   |                                                   |
| `0x023F` (575) | 169   |                                                   |
| `0x038A` (906) | 134   |                                                   |
| `0x047D` (1149)| 120   |                                                   |
| `0x018B` (395) | 105   |                                                   |
| `0x015A` (346) | 100   |                                                   |
| `0x0331` (817) | 96    |                                                   |
| `0x0392` (914) | 75    |                                                   |
| `0x00BE` (190) | 75    |                                                   |

We cannot claim semantics for any of these without emulator-backed decoding.
All enter the catalog as `OBSERVED-ONLY`.

---

## Global opcode coverage, all 104 non-keyframe chunks of sample_a

Walking every game chunk + keyframe (excluding chunk 0's initial keyframe
which has only one block) with Mowokuma's block framing: **249 distinct
opcodes, 1,924,363 total blocks, zero parse errors**.

Top 15 opcodes across the whole replay:

| opcode      | count   | % of all blocks |
|-------------|---------|-----------------|
| `0x0389` (905) | 721,319 | 37.5 % |
| `0x036B` (875) | 102,200 |  5.3 % |
| `0x03D6` (982) |  92,028 |  4.8 % |
| `0x02DA` (730) |  70,277 |  3.7 % |
| `0x0118` (280) |  48,429 |  2.5 % |
| `0x0357` (855) |  46,779 |  2.4 % |
| `0x0473` (1139)|  44,000 |  2.3 % |
| `0x001E` (30)  |  42,819 |  2.2 % |
| `0x015A` (346) |  38,230 |  2.0 % |
| `0x0090` (144) |  26,950 |  1.4 % |
| `0x0076` (118) |  26,041 |  1.4 % |
| `0x027C` (636) |  24,826 |  1.3 % |
| `0x0126` (294) |  23,962 |  1.2 % |
| `0x018B` (395) |  23,451 |  1.2 % |
| `0x03F7` (1015)|  22,815 |  1.2 % |

`0x0389` alone is 37.5 % of all blocks, this is almost certainly a
per-entity-per-tick state update (the "heartbeat" packet). On a 34-minute
game that's ~350 ticks per second of game time for this one packet type,
which is plausible for 30 Hz game ticks × ~12 tracked entities × 34 min ≈
730k.

The **long tail** is long: 249 distinct opcodes, of which ~100 appear
<100 times in a 34-minute game. Rare opcodes probably include objective
kills (dragon, baron, turret), per-level-up events, summoner-spell uses,
game-result packets. Phase-4 work.

---

## Mapping to Mowokuma's config

In her example `result.json` she hardcodes `ward_spawn_decrypt.netid = 272`
for a specific patch. `272 = 0x0110`. Our opcode walk here finds
**no opcode `0x0110`** in sample_a, so either:

1. The netid changed between that patch and `16.8` (most likely, she
   explicitly tracks netids per-patch), or
2. There were simply no ward events in this sample.

Sample_a is a 34-minute game so option 2 is essentially impossible. Hence
**option 1: the ward-spawn netid in patch 16.8 is different**. We do not
know which of the 249 opcodes here is ward-spawn without either her
patch `.zip` for patch 16.8 or independent semantic identification.

This is concrete motivation for the ROFL-X catalog: **one numeric netid per
packet class is not stable across patches**, so our catalog needs to index
by *semantic name* and record per-patch numeric mappings as a separate
table (`docs/COMPATIBILITY.md` per the brief).

---

## Findings summary (Phase 1 complete)

### Verified vs Mowokuma's source

| claim                                          | verified on sample_a |
|------------------------------------------------|----------------------|
| zstd decompression of chunk payloads           | ✅ (5852 → 18173 B)  |
| 17-byte chunk header layout                    | ✅                   |
| Chunks with `compressed_len==0` store raw bytes| ✅ (chunk 1)         |
| 256-byte signature immediately before metadata | ✅                   |
| Trailing u32 = metadata JSON length            | ✅                   |
| Marker-byte block framing bitfields            | ✅ (5738 blocks, 0 errors) |
| `statsJson` is a stringified-JSON inside JSON  | ✅                   |

### Contradicted / refined

| Mowokuma claim                                   | refinement                                |
|--------------------------------------------------|-------------------------------------------|
| Version = `buffer[16..20]` = `"6.8."`            | Real version = `[0x0F..0x0F+n]` per length prefix at `[0x0E]`. For sample_a: `"16.8.766.8562"` (13 chars). Her reader drops leading "1". |
| Header is 28 or 29 bytes (discriminator byte)    | Header is `0x0F + n` bytes where `n = byte[0x0E]`. Her heuristic works only because she lucks into the right skip. |
| Skip `chunk_type == 0x2` to ignore keyframes     | Only chunk 0 has `type == 0x2` in sample_a. Other keyframes have `type` values `0x05, 0x07, 0x09, 0x0B, …`. Her heuristic only skips the *initial* keyframe, not all keyframes. She must be OK with this because ward/position data lives in game chunks anyway. |
| Player `NAME` field is meaningful                | `NAME` has been empty string since ≈2023. Use `RIOT_ID_GAME_NAME` + `RIOT_ID_TAG_LINE` for Riot ID. |

### New structural knowledge

- **Magic bytes**: `52 49 4f 54` = `"RIOT"`, at offset 0.
- **Variable-length header**: bytes `[0x0E] = version_len`, followed by the
  version string. Header ends at `0x0F + version_len`.
- **`chunk_id_2` is a stream discriminator**, packed as `stream_tag << 24`:
  `0x01` = game-chunk stream, `0x02` = keyframe stream, `0x03` = initial
  keyframe (1×), `0x04` = start sentinel (1×, clen=0, ulen=17, body opaque).
- **`chunk_type` is a dense monotonic counter / time-slot index**, not a
  type discriminator as its name in Mowokuma's source implies.
- **Total chunk count**: matches `lastGameChunkId + lastKeyFrameId` from the
  metadata (modulo the two singleton chunks).
- **~250 distinct opcodes per replay** is typical. The catalog is a
  250-row table, not a 5-row table.

### Concerns and open questions for Phase 2

1. **Version-field ambiguity**: do we want our version string to be the
   short form (`"16.8"`) to match Mowokuma's patch-file naming convention
   `patch/<major>-<minor>.patch`, or the full build tag
   (`"16.8.766.8562"`)? I'd vote for preserving the full string internally
   and exposing both `"version": "16.8.766.8562"` and `"patch": "16.8"` in
   the output.

2. **The `chunk_id_2 = 0x04000000` sentinel chunk** with 17 raw bytes, we
   need more samples to know whether the body is constant, per-file, or
   per-stream. Cheap to verify: parse a handful of other replays.

3. **Signature verification**: do we want to do any? Probably not,
   Riot's live signing key is inside the client and we don't need
   write-side integrity. We just preserve it on round-trip.

4. **Role inference from `statsJson`**: there's probably a
   `TEAM_POSITION` or `INDIVIDUAL_POSITION` key we can use instead of
   array-index fallback. Worth verifying before Phase 2.

5. **Patch-specific opcode table**: we'll need one per League patch. For
   patches we can't reverse, we ship `UNKNOWN` catalog entries only and
   the audit subcommand shows coverage as "N known / M observed / K
   unknown". That's fine and honest.

6. **Do we trust Mowokuma's zstd path?** Yes, validated on sample_a
   (5852 compressed → exactly 18173 uncompressed, matches header). No
   Blowfish layer on top in this sample. If older patches (say 12.x or
   earlier) had a different pipeline, we'll find out when we add older
   samples, but that's explicitly out of scope for now.

---

## What's next

Phase 1 is complete. All three reconnaissance deliverables exist:

- `docs/REFERENCE_MOWOKUMA.md`, Mowokuma's source, per-module
- `docs/REFERENCE_HENRY_ZHU.md`, Henry Zhu's blog post + techniques
- `docs/SAMPLE_A_WALKTHROUGH.md`, this file

**Per the phase-gate, I am stopping here and asking for review before
starting Phase 2** (the domain model and format spec).

A couple of small pending decisions flagged in the open questions above
(version string format, role inference) should land before Phase 2 so I
can encode them in the domain model directly.
