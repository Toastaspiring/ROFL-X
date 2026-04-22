# Reference: Mowokuma's ROFL parser

A source-level walkthrough of [Mowokuma/ROFL](https://github.com/Mowokuma/ROFL)
(archived Nov 2025), the prior art we're extending. This document is pure
observation of her code; it is the input to our own domain model and format
spec, not a substitute for them.

**Attribution:** all credit for the approach, the decoded packets, and the
emulator technique below belongs to Mowokuma. ROFL-X is an extension of her
project, not a rewrite.

**Clone location:** `reference/Mowokuma-ROFL/` in this working tree.
**Commit pinned:** `7181c9a` (tip of `master` at time of clone).

---

## TL;DR

Mowokuma's parser is two subsystems glued together:

1. **A pure-Rust file-format parser** (`src/parser/`) that walks the ROFL file
   byte-by-byte, decompresses zstd-compressed chunks, and splits the resulting
   byte streams into variable-length "blocks" (her name for individual
   serialised game packets).
2. **A Unicorn-based x86-64 emulator** (`src/emulator/`) that loads extracted
   `.text` / `.data` / `.rdata` sections from the League game binary, maps
   them into a tiny virtual machine, and **calls the game's own decrypt
   functions on each packet payload**. She doesn't reimplement the crypto,
   she reuses the client's implementation by emulating it.

The output is the two pieces of data the README advertises: per-second player
positions and ward lifecycles.

---

## Repo layout

```
Mowokuma-ROFL/
├── Cargo.toml         , deps: unicorn-engine, rayon, serde, zstd, clap, zip, fern, ...
├── Cargo.lock
├── README.md          , features + usage + sample JSON
├── .gitignore         , /target /patch  (no LICENSE file!)
└── src/
    ├── main.rs                     , CLI, orchestration, JSON assembly
    ├── parser/
    │   ├── mod.rs                  , re-exports
    │   ├── util.rs                 , little-endian byte readers, geometry helpers
    │   ├── metadata.rs             , trailing-JSON metadata extraction + Player/Metadata structs
    │   ├── chunk.rs                , ROFL chunk header + zstd decompression
    │   ├── block.rs                , per-chunk block (packet) framing with bitfield marker
    │   └── parser.rs               , glue: chunks → blocks → filter by packet_id
    └── emulator/
        ├── mod.rs                  , re-exports
        ├── config.rs               , loads per-patch .zip config (RVAs, offsets, section bytes)
        ├── packet.rs               , WardSpawnPacket / PathPacket structs + path waypoint decode
        └── stub_emulator.rs        , Unicorn setup + call-decrypt-function machinery
```

Two files are trivial `pub mod …` glue (`parser/mod.rs`, `emulator/mod.rs`)
and aren't discussed below.

---

## Top-level data flow

For `./ROFL.exe file -r replay.rofl -o out.json`:

1. `main::parse_file` reads the entire `.rofl` into `Vec<u8>`.
2. `Metadata::parse` walks the *tail* of the file to extract the
   version string and the trailing JSON blob containing player roster.
3. `Config::parse` loads the matching per-patch `.patch` zip (e.g.
   `patch/15-4.patch`) containing `result.json` (RVAs and offsets) and
   three `.bin` blobs (raw bytes of the game's `.text`, `.data`, `.rdata`
   sections).
4. `main::get_replay_info` runs two Rayon-parallelised passes over the file:
    - **Pass 1**, filter all blocks with `packet_id == ward_spawn_decrypt.netid`,
      spin up a `StubEmulator` per batch of 100, call the ward-decrypt function
      in the mapped game code for each payload, collect `WardSpawnPacket`s.
    - **Pass 2**, same pattern but with `mov_decrypt.netid` and the
      position-decrypt function, collecting `PathPacket`s.
5. Ward lifecycle is reconstructed by matching spawn packets (name
   = `YellowTrinket` / `SightWard` / `JammerDevice`) to destruction packets
   (name contains `Corpse`) at the same `(x, y)`, duration = corpse
   timestamp − spawn timestamp.
6. Player positions are reconstructed by keeping the *latest* `PathPacket`
   per player entity id, stepping forward in 1-second ticks, and asking each
   held `PathPacket` for its interpolated position at that tick.
7. Output JSON: `{ metadata, wards[], players_state[] }`.

No raw chunk bytes ever leave the parser, the emulator consumes them and
returns decoded struct fields.

---

## Per-module summary

### `parser/util.rs`

Little-endian byte readers (`parse_u8 / u16 / u32 / f32`) over any
`Iterator<Item = u8>`, plus `read_file`, `bit_test`, `sign_extend`, and a
Euclidean `point_dist`. Iterator-based so chunk/block parsers can consume a
buffer lazily. All error handling collapses to `Result<_, ()>`, no error
context.

### `parser/metadata.rs`

Defines `Player { name, skin, team, position }` and
`Metadata { version, game_len, winning_team, players }`.

`Metadata::parse` reads:

- **Version string**: ASCII bytes `buffer[16..20]` (e.g. `"15.4"`).
- **Trailing metadata size**: `u32` LE at `buffer[len-4..]`.
- **Metadata JSON blob**: UTF-8 bytes at
  `buffer[len - 4 - json_size .. len - 4]`.

Inside that JSON:

- `gameLength` → `game_len` (milliseconds).
- `statsJson` → nested JSON string (stringified JSON inside JSON), containing
  an array of 10 player objects, each with `NAME`, `SKIN`, `TEAM`
  (`"100"` = Blue, `"200"` = Red).

**Role inference is positional, not data-driven**:
`position = ["Top","Jungle","Mid","Adc","Support"][i % 5]`, this assumes
the `statsJson` array is always in canonical role order. ROFL-X will need
to revisit this if Riot ever changes `statsJson` ordering (or if roles are
flexed).

**Winning team** is derived from `(TEAM, WIN)` on `statsJson[0]`.

`get_player_from_id(id, player_id_start)` maps a packet entity id to a
roster index via `id - player_id_start`. `player_id_start` is patch-dependent
and lives in the config.

### `parser/chunk.rs`

`Chunk { id, type_, id_2, uncompressed_len, compressed_len, payload }`.

`ChunkParser::new` prepares the buffer by trimming trailers/headers:

- Strip metadata: `buffer.len() - metadata_len - 4` bytes kept.
- Strip signature: drop the last `0x100` (256) bytes.
- `skip_rofl_header_size` drains `0x10` bytes, then checks `buffer[0xC]` of
  what remains and drains either `0xC` or `0xD` more. The author flags this
  with `// FIXME: very bad`, there's a two-variant header she hasn't fully
  characterised.

`next_chunk()` then reads a 0x11-byte (17-byte) chunk header:

| offset | size | meaning                       |
|--------|------|-------------------------------|
| 0x00   | u32  | `chunk_id`                    |
| 0x04   | u8   | `chunk_type` (skip if `0x2`)  |
| 0x05   | u32  | `chunk_id_2`                  |
| 0x09   | u32  | `uncompressed_len`            |
| 0x0D   | u32  | `compressed_len`              |

followed by `compressed_len` bytes. If `compressed_len != 0`, the payload is
**zstd-decompressed** to `uncompressed_len` bytes; otherwise the chunk has
no usable payload (advanced past).

> **Correction to the ROFL-X brief**: the brief says "gzip per chunk" and
> that decryption is Blowfish-with-key-derivation. Neither is what this code
> does. Compression is **zstd**; "decryption" is **emulator-driven calls
> into the game's own code** (see `stub_emulator.rs`). This difference is
> load-bearing for the format spec we write in Phase 2.

### `parser/block.rs`

`Block { length, timestamp, packet_id, param, payload }`.

A "block" is one game packet recovered from a decompressed chunk. Blocks are
delta-encoded against their predecessor. The first byte of each block is a
**marker** with four bit-flags:

| bit   | meaning when set                                     |
|-------|------------------------------------------------------|
| 0x80  | timestamp is `u8 delta * 0.001` (accumulate on prev) |
|       | else timestamp is absolute `f32`                     |
| 0x40  | `packet_id` is reused from previous block            |
|       | else read fresh `u16`                                |
| 0x20  | `param` is `u8 delta + previous param`               |
|       | else read fresh `u32`                                |
| 0x10  | `length` is `u8`                                     |
|       | else `length` is `u32`                               |

After the marker + variable-width fields, `length` bytes of payload are
consumed verbatim. Previous-packet-id/param are kept on the parser state.

This marker-byte delta encoding is the primary space saving, the same
`packet_id` repeats constantly in a position-update stream.

### `parser/parser.rs`

Just glue:

- `get_blocks(buffer)`, walks chunks, skips those with `type_ == 0x2`
  (purpose unknown, probably keyframes, see Phase 1 step 3), expands each
  chunk to its blocks.
- `get_blocks_with_id(buffer, id)`, filters to one `packet_id` and
  returns `(timestamp, payload)` pairs in parallel via `rayon`.

This is the only entry point `main.rs` uses from `parser/`.

### `emulator/config.rs`

Holds per-patch reverse-engineering state. A "patch file" is a `.zip`
archive the user must provide, containing:

- `result.json`, a config with RVAs and struct offsets (example inlined as
  a doc-comment at the top of the file).
- `text.bin`, `data.bin`, `rdata.bin`, raw bytes of those PE sections
  extracted from the League client binary for that patch.

The struct tree:

```
Config
├── alloc1, alloc2, skip           , RVAs of functions to stub out
├── base_addr = 0x7ff76afd0000     , HARDCODED image base
├── player_id_start
├── ward_spawn_decrypt: { netid, rva_start, rva_end,
│                         id_offset, owner_id_offset,
│                         name_offset, name_len_offset,
│                         x_offset, x_write_count,
│                         y_offset, y_write_count }
├── mov_decrypt:        { netid, rva_start, rva_end,
│                         payload_offset, payload_size_offset }
└── text / data / rdata: Section { name, rva, size, raw }
```

The `*_write_count` fields tell the emulator *"after the N-th write to
this offset in the packet struct, the final value has landed"*, the
decrypt function writes intermediate values as it computes, so you have
to know which write is the one that matters. This is a reverse-engineering
artefact of the specific decompiled function.

Loading is eager: `Config::parse` unzips everything into memory.

### `emulator/packet.rs`

Two packet structs and one heavy decoder:

- `PosKey { x: i32, y: i32 }`, hash key used in `main.rs` to match ward
  spawn/destroy events by position.
- `WardSpawnPacket { timestamp, name, id, owner_id, x, y }`, built by the
  emulator *after* the game's decrypt code has populated a struct in emu
  memory; fields are read back via offsets.
- `PathPacket { timestamp, id, speed, waypoints: Vec<(f32, f32)> }`,
  movement packet. Its `parse()` method is a **near-verbatim port of
  decompiled game code** (variable names `v10 / v13 / v14 / …` give it
  away). It decodes waypoints with a bitmap prefix that says, per axis,
  whether each subsequent waypoint uses a `u8` delta or a fresh `u16`.
  The final conversion to map coordinates is:

  ```
  x = sign_extend(encoded, 16) * 2.0 + 7358.0
  y = sign_extend(encoded, 16) * 2.0 + 7412.0
  ```

  The magic constants `7358.0`, `7412.0` are map-center offsets (Summoner's
  Rift is 14820×14881 Unity units, these are `≈ map_size / 2`).

`PathPacket::get_pos(now)` interpolates linearly along the waypoint chain
using the packet's `speed` to compute per-segment duration. If the packet
was emitted < 1 s ago we return its origin.

### `emulator/stub_emulator.rs`

The core trick of the project. On every packet, she:

1. Creates a fresh x86-64 Unicorn VM.
2. Maps stack (`0x7FFFFFFF0000`, 0x2000 bytes) and heap (`0x7FFFFFFF8000`,
   0x2000 bytes) with R/W.
3. Maps `.text` / `.data` / `.rdata` at their RVAs from the config (each
   aligned up to page size) with R/W/X, copying in the bytes from the zip.
4. **Patches three functions in the mapped `.text`**:
   - The `skip` RVA is overwritten with `48 C7 C0 01 00 00 00 C3`, i.e.
     `mov rax, 1; ret`, forcing whatever safety check it was to succeed.
   - The two allocator RVAs (`alloc1`, `alloc2`) are overwritten with a
     handwritten 91-byte shellcode (`patch2`) that implements a bump
     allocator against a heap-cursor pointer at RVA 0, saving/restoring
     registers around it.
5. Sets up arguments per the Windows x64 calling convention:
   - `RCX` = pointer to a 0x90-byte struct to be filled in by the decrypt
     function (`packet_addr`).
   - `RDX` = pointer-to-pointer to the payload bytes.
   - `R8`  = payload end pointer.
6. Calls `uc.emu_start(rva_start, rva_end, 0, 0)`, run native code from
   `rva_start` until PC reaches `rva_end`.
7. For ward spawns, also installs a `MEM_WRITE` hook over the packet struct
   range: every write is counted per-offset, and the `x_offset`,
   `y_offset`, `id_offset`, `owner_id_offset` fields are captured at the
   specific write counts the config tells it to (the decrypt function
   writes intermediate values first, then the final value).
8. Reads back the populated struct fields via `read_ptr_on` / `read_u32_on`
   / `read_str_on`.
9. Resets the heap cursor and RSP for the next packet. The `.text` / `.data`
   / `.rdata` mappings persist; only the heap and registers are logically
   reset between calls.

Notes:

- This is reuse of the client's own decode logic, not cryptographic
  decryption in any meaningful sense. The packets on disk aren't
  AES/Blowfish, they're in an internal binary encoding that the game
  client decodes via a specific function.
- Per-patch fragility is total: any time Riot's compiler reorders basic
  blocks, renames a struct field, or changes allocator layout, the RVAs
  and offsets in the `.patch` zip go stale. This is the reason her project
  ships per-patch `.patch` zips.
- There is **no mechanism here for packets Riot encodes via a different
  function**. ROFL-X will need to either ship more decode-function RVAs or
  find packets whose encoding is simple enough to decode directly in Rust.

### `main.rs`

The orchestrator.

- CLI via clap with two subcommands: `file` and `folder` (plus an
  unreachable `_ => unimplemented!("Batch parsing not implemented yet")`
 , dead code, since both variants are matched above it).
- Per-patch config auto-selection: `get_appropiate_patch(version)` takes
  the metadata version string (e.g. `"15.4."`, trailing dot, note), maps
  `.` → `-`, pops the last char, and looks up `./patch/<name>.patch`.
- `get_replay_info` implements the two decode passes described in the
  data flow section, reconstructing ward lifecycles and player positions.
- Logging via `fern` with colourised level tags.
- Batch parsing uses `rayon::par_iter` over directory entries, one
  `emulator::Config::parse` shared across all files of the same patch.
- Sets the CWD to the exe's directory at startup so `./patch/...` resolves
  relative to the binary, not the user's PWD.

---

## What this tells us about the ROFL file format

Everything here is *inferred from her code*, evidence-level is "she's
shipped this for 15.x replays and the README shows it working". We will
verify against a real sample in step 3.

- **Outer layout (from tail):** `[ header ][ chunks... ][ signature (256 B) ][ metadata JSON ][ u32 metadata_len_LE ]`.
- **Header:** ≥ 0x1D bytes. A 4-byte version string lives at offset 16.
  There's a variant discriminator around offset 0x1C (`0x10 + 0xC`): byte
  0xC after the first 0x10-byte drain selects between a 0xC and 0xD
  follow-on drain. This strongly suggests two header sub-layouts, worth
  characterising precisely in Phase 1 step 3.
- **Metadata JSON:** UTF-8, no terminator; contains `gameLength`,
  `statsJson` (stringified JSON with per-player `NAME/SKIN/TEAM/WIN`),
  and implicitly everything else Riot stashes there (unread by this parser).
- **Chunks:** 17-byte header (id u32, type u8, id_2 u32, uncompressed_len
  u32, compressed_len u32), followed by `compressed_len` bytes of **zstd**
  payload. Chunks with `compressed_len == 0` have no payload to decode.
- **Chunk type `0x2`:** skipped by `get_blocks`. Almost certainly keyframes
  (the other references describe a chunk/keyframe split); this is worth
  confirming.
- **Block framing inside a chunk:** marker byte + variable-width fields
  (see table in the `block.rs` section), with timestamp/packet_id/param
  delta-encoded against the previous block. Payload is opaque until a
  packet-specific decoder is run.
- **Packet identity:** a `packet_id` (u16) + `param` (u32). Only `packet_id`
  is used by this parser to route; `param` is captured but unused.

We cannot say anything from this code alone about how the game *produced*
the file, that is, we don't see the encryption/compression pipeline going
in, only coming out. Whether there's a per-file Blowfish key derived from
`gameId` (as robertabcd/lol-ob describes) is invisible here because her
design bypasses that entirely via emulation.

---

## Packet semantics observed

### Ward spawn (`config.ward_spawn_decrypt.netid`, e.g. 272)

Decoded via emulator. Same opcode covers placement *and* destruction:

- Placement if `name ∈ { "YellowTrinket", "SightWard", "JammerDevice" }`.
- Destruction if `name.contains("Corpse")`, matched to the placement by
  identical `(x, y)` integer coords.

Fields extracted: `name`, `owner_id`, `id` (ward entity id), `x`, `y`.

The ROFL-X packet catalog will eventually hold an entry for *every* ward
name Mowokuma's placement-filter skips (e.g. `BlueTrinket`, `FarsightAlt`,
`VisionWard`, all conspicuously absent from her allowlist). This is
already a concrete completeness gap.

### Movement / position (`config.mov_decrypt.netid`)

Decoded via emulator, then parsed further in pure Rust (`PathPacket::parse`).
Emulator output is a `(payload_ptr, payload_size)` pair; the payload itself
is the waypoint stream described in the `packet.rs` section.

Fields: `id` (entity), `speed` (units/sec), `waypoints` ((f32, f32) list
in map coords).

Only packets with `id ∈ [player_id_start, player_id_start + 9]` are kept
for player-position reconstruction; everything else (minions, monsters,
turrets, etc.) is discarded. This is a second concrete completeness gap,
minion/monster pathing is useful and free from this same packet.

### Everything else

Not decoded. Her parser walks every block, filters to the two netids she
cares about, and discards the rest. We have no evidence of any other
opcode's semantics from her code alone.

---

## The `.patch` config archive (per-game-patch reverse engineering)

She ships one `.patch` zip per League patch she's reversed. Each contains:

```
result.json   , RVAs + struct offsets + netids (see Config struct above)
text.bin      , raw bytes of .text from that patch's League binary
data.bin      , raw bytes of .data
rdata.bin     , raw bytes of .rdata
```

These are not in the repo (the `.gitignore` excludes `/patch`). They were
presumably distributed with release binaries and/or reverse-engineered
off-line.

**Implication for ROFL-X:** either we (a) ship our own per-patch archives
with Mowokuma's scope + ours, (b) find a way to avoid the emulator path
for packets whose encoding is simple enough to decode in pure Rust, or
(c) both. This is the single biggest cost driver for the project, every
new League patch is a reverse-engineering task, not a code task.

---

## Corrections / concerns to flag before Phase 2

1. **Compression = zstd, not gzip.** The project brief states "gzip per
   chunk", that's incorrect; `Cargo.toml` pulls `zstd = "0.13"` and
   `chunk.rs` uses `zstd::stream::read::Decoder`. Our Phase 2 format spec
   must document zstd.

2. **"Decryption" is not crypto.** The brief says "Blowfish with the key
   derived from the gameId-XOR-encryption-key". There is no Blowfish
   anywhere in Mowokuma's tree. The packet payloads are an internal binary
   encoding that she handles by emulating the game's own decode function.
   Whether the chunk *itself* (before zstd) is Blowfish-encrypted in newer
   replays is still an open question, Phase 1 step 3 (hex walkthrough)
   should answer this by looking at the raw bytes pre-decompression.

3. **No LICENSE file in her repo.** The brief says "License-compatible with
   hers". There is no explicit licence, not MIT, not GPL, not anything.
   GitHub's default is "all rights reserved" for unlicensed public repos.
   Before we port any line of code verbatim, we should (a) check her
   Releases page / GitHub profile for a licence statement, (b) consider
   opening an issue or contacting her (account may be dormant since the
   archive), or (c) treat the repo as read-reference only and reimplement
   everything from observed behaviour. Worth raising with you.

4. **Role inference is positional.** `metadata.rs` assigns roles by array
   index `i % 5` against a hardcoded `["Top","Jungle","Mid","Adc","Support"]`.
   Any change to `statsJson` ordering or any flex-role lobby will mis-label
   players. Our metadata layer should derive role from in-game data where
   possible.

5. **Header parsing is self-described as brittle.** `skip_rofl_header_size`
   has `// FIXME: very bad` and a magic two-variant branch. Phase 1 step 3
   resolved this (see [SAMPLE_A_WALKTHROUGH.md](SAMPLE_A_WALKTHROUGH.md) §
   Section A): the real rule is a length-prefix byte at `[0x0E]` giving the
   length of the ASCII version string that follows. Her heuristic lucks
   into the right skip for the patches she tested but is not correct in
   general.

5b. **Version-reading is off by one byte.** Related to the above:
    `Metadata::parse` reads `buffer[16..20]` as the version string. The
    real version starts at `[0x0F]` (one byte earlier), she drops the
    leading character. For `sample_a` (patch 16.8.766.8562) her parser
    reports version `"6.8."` instead of `"16.8"`. Her `get_appropiate_patch`
    maps that to `patch/6-8.patch`, so her patch-file naming convention
    accidentally matches her bug; consumers of ROFL-X will see the correct
    version string.

6. **`PathPacket::parse` is a decompiled-code port.** Variables named
   `v10, v13, v14, …` are IDA/Ghidra decompiler artefacts. It works but
   it's unmaintainable and patch-fragile. ROFL-X will likely inherit it
   verbatim at first and refactor only once we have test coverage.

7. **Chunk type `0x2` is dropped silently.** No comment says what it is.
   Probably keyframes. Worth documenting.

8. **No `audit` / `dump` CLI commands.** The ROFL-X brief describes
   subcommands (`audit`, `dump --chunk N`) that don't exist here. They're
   new surface area, not a port.

9. **Windows-specific assumptions.** `base_addr = 0x7ff76afd0000` and the
   `.patch` file layout (PE sections) assume Windows x86-64 League builds.
   Mac support would require a separate patch pipeline. (The League Mac
   client is also being discontinued by Riot in 2024-25, so this may not
   matter.)

10. **Unused `batch` / folder CLI has dead `_ => unimplemented!()`.** Minor
    but worth noting.

---

## Open questions for you before Phase 2

1. **Licence:** do you want me to (a) open an issue on her repo asking for
    a licence, (b) assume read-reference only and reimplement from
    observation, or (c) adopt a default (MIT/Apache-2.0) for our code and
    cite hers as unlicensed prior art?

2. **Patch-config strategy:** are we signing up to maintain per-patch
    `.patch` zips ourselves (reverse-engineering each new League patch), or
    is the goal to catalogue the format well enough that the community can
    contribute patches? If the latter, we'll want a tool in Phase 3+ that
    automates section/RVA extraction from a League binary.

3. **Scope of emulator-dependent packets:** our catalog will split into
    "decodable in pure Rust" vs "requires per-patch emulator config". Are
    you OK with both classes existing? The alternative is to restrict the
    project to pure-Rust-decodable packets, which would drop positions and
    ward spawns from day-one coverage (they're exactly the ones Mowokuma
    needs emulation for).

4. **Header characterisation priority:** Phase 1 step 3 (hex walkthrough of
    a real sample) will resolve the `// FIXME: very bad` header logic. Do
    you want me to pick a specific replay from your `replays/` folder or
    pick opportunistically (e.g. the shortest, or the oldest patch for
    which we have cleanest structure)?

5. **Mowokuma's binary for parity testing:** the brief calls for verifying
    byte-for-byte parity with her output on a shared replay. Her Releases
    page hosts an `.exe` plus, presumably, the `.patch` zips for the
    patches she's covered. If we want parity testing, we need one of those
    `.patch` zips. Do you want me to download and cache it, or is that
    something you'd rather do manually?

---

## Next

Phase 1 step 1 is complete. The next steps are still:

- **Step 2:** read Henry Zhu's [2025 write-up](https://maknee.github.io/blog/2025/League-Data-Scraping/) end-to-end and write
  `docs/REFERENCE_HENRY_ZHU.md`.
- **Step 3:** pick one `.rofl` from `%USERPROFILE%\Documents\League of Legends\replays\`,
  produce an annotated hex walkthrough of its header, metadata trailer,
  payload header, and the first chunk or two (post-decompression) in
  `docs/SAMPLE_A_WALKTHROUGH.md`.

Per the brief, I'm stopping here for your review before continuing.
