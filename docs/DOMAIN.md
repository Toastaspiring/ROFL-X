# Domain model

This document describes what a ROFL file is, and what ROFL-X is trying to
turn it into, in plain language. It exists before any Rust code so that the
module layout and types follow the shape of the problem rather than the
shape of whoever wrote the first function.

Everything below is grounded in Phase 1's findings:
[REFERENCE_MOWOKUMA.md](REFERENCE_MOWOKUMA.md),
[REFERENCE_HENRY_ZHU.md](REFERENCE_HENRY_ZHU.md),
[SAMPLE_A_WALKTHROUGH.md](SAMPLE_A_WALKTHROUGH.md).

---

## 1. What a ROFL file is, from 30,000 feet

A `.rofl` is a **binary recording of one League of Legends match**, written
by the game client when the match ends and playable back inside the client.
It is Riot's internal format: no published spec, no stable ABI across
patches, deliberate obfuscation of the per-packet payloads.

Structurally it is a small handful of things nested inside each other:

```
   Replay (the file)
       │
       ├── FileHeader         (magic, version string, some unknowns)
       │
       ├── Chunks (N records) ◀── two interleaved streams:
       │     │                    ∙ game chunks (deltas, stream tag 0x01)
       │     │                    ∙ keyframes  (snapshots, stream tag 0x02)
       │     └── each chunk's body is a zstd blob that expands to
       │         a stream of variable-length Blocks
       │
       ├── Signature          (256 opaque bytes; integrity artefact)
       │
       └── MetadataBlob       (UTF-8 JSON: game length, players, stats)
               └── statsJson  (JSON-in-JSON: per-player data)
```

The parser's job is to take a sequence of bytes and extract two things:

1. **Transport-level data**: the headers, lengths, chunk stream, and
   metadata JSON. These are fully specified by layout; there are no
   semantics to reverse-engineer.
2. **Semantic data**: what each packet inside each block *means*. This is
   where all the hard work lives, and where every prior-art parser has
   ultimately reached for an emulator.

Keeping these two levels separate in the code is the single most important
architectural decision of the project.

---

## 2. Transport-level entities

These are pure data. No League-game knowledge required to understand them.

### `Replay`

The whole file, from byte 0 to `EOF`. Always self-contained: no external
references, no side tables. Round-trippable in principle (we can read all
the bytes, write them back unchanged, and get an identical file).

### `FileHeader`

The first `0x0F + version_len` bytes. Has a fixed prefix (`"RIOT"` magic,
two u16s, six mystery bytes, a u8 length prefix) followed by a
variable-length ASCII version string (e.g. `"16.8.766.8562"` for the
sample we audited).

Responsibilities: tell the parser the format version and the game patch
version, and give the parser enough to know where the chunk region starts.

Unknowns: the u16 at offset 6 (value `217` in our sample), the six bytes
at offset 8 (possibly game id / timestamp / session tag). Documented as
`UNKNOWN` in the format spec.

### `ChunkRecord`

One record in the chunk region. Always has a 17-byte header:

```
  chunk_id            u32 LE   per-stream counter
  chunk_type          u8       dense monotonic time-slot index
  chunk_id_2          u32 LE   stream tag, packed as (tag << 24)
  uncompressed_len    u32 LE
  compressed_len      u32 LE
```

followed by either `compressed_len` bytes of **zstd** data (which expand to
exactly `uncompressed_len` bytes), or `uncompressed_len` bytes of raw data
when `compressed_len == 0`.

`ChunkRecord` is **physical**: a specific 17+N-byte slice of the file.

### `Stream`

A logical grouping of `ChunkRecord`s by `chunk_id_2` stream tag. Four
streams observed so far:

| tag    | name              | contents                                         |
|--------|-------------------|--------------------------------------------------|
| `0x01` | `GameChunkStream` | delta updates to game state, one record per "tick bundle" |
| `0x02` | `KeyframeStream`  | periodic full-state snapshots                    |
| `0x03` | `StartKeyframe`   | singleton initial snapshot (chunk index 0)       |
| `0x04` | `StartSentinel`   | singleton 17-byte opaque transition marker       |

Streams are *not* materially separated in the file; they are interleaved
by `chunk_type` (time-slot) order. Our parser materialises them as
separate logical streams after walking the chunk region.

### `DecompressedChunk`

After zstd decompression, the raw byte stream inside one chunk. No longer
a file-level concept: it's a buffer we parse block-by-block.

### `Block` (transport view)

One delta-encoded record inside a `DecompressedChunk`. Has a marker byte +
variable-width fields:

```
  marker      u8   bitfield: 0x80=timestamp-relative, 0x40=reuse-packet-id,
                             0x20=param-relative, 0x10=length-u8
  timestamp   f32 or u8-delta  (per marker)
  length      u32 or u8        (per marker)
  packet_id   u16 or reused    (per marker)
  param       u32 or u8-delta  (per marker)
  payload     length bytes
```

At this level a Block is an opaque (`packet_id`, `param`, `timestamp`,
`payload_bytes`) tuple. Transport parsing is complete once we have a stream
of these.

### `Signature`

256 bytes at fixed offset `len - 4 - metadata_len - 256`. Opaque. Almost
certainly an RSA-2048 signature over the preceding bytes (Riot's client
probably validates it on load), but we do not verify and we do not modify.
Round-trip preservation is the only contract.

### `MetadataBlob`

UTF-8 JSON bytes at `[len - 4 - metadata_len .. len - 4]`, with the u32
length suffix at `[len - 4 .. len]`. Top-level keys observed: `gameLength`
(ms), `lastGameChunkId`, `lastKeyFrameId`, `statsJson` (a stringified JSON
array of 10 player objects).

---

## 3. Semantic-level entities

These require understanding the game. This section is the one that goes
stale if Riot ever reshapes packet semantics (but per Henry Zhu, the
semantics are stable; it's the *encoding* that shifts per patch).

### `Packet`

A `Block` with a known decoded shape. A `Packet` is what a `Block` becomes
after we run the appropriate per-opcode decoder on its `payload_bytes`.

Conceptual lifecycle, mirroring the game client's own:

1. **Allocation** (the game calls `Packet::Packet`): a zeroed struct is
   reserved to hold the decoded fields.
2. **Deserialisation** (the game calls `DeserializePacket`): raw payload
   bytes are moved into the struct, still obfuscated.
3. **Use** (the game calls `UsePacket`): fields are decrypted on demand,
   used to update game state, and the plaintext is erased before returning.

Our parser observes the *result* of stage 2 (the decoded struct, produced
by emulating the game's own decoder on the payload bytes), and reconstructs
what stage 3 would have done (updating a shadow `GameState`). We do not
replicate the decrypt-access-release memory hygiene: once we read a field
out of the emulator, it stays in our Rust-level structs.

### `Opcode`

A u16 identifier (what Mowokuma calls `packet_id`, what Henry Zhu calls
`netid` / packet-class). In our sample we observe 249 distinct opcodes
across 1.9M blocks. An `Opcode` by itself is meaningless; it's a label
into the packet catalog.

The numeric value of an opcode is **not stable across patches**. The
*semantic name* (`WARD_SPAWN`, `TAKE_DAMAGE`, `CAST_SPELL`, ...) is the
stable referent.

### `Entity`

Anything with an id that exists in the game world. Entities come and go
over the course of the match. Known entity kinds:

- **Champion**: one of 10 per match, controlled by a `Player`. IDs live in
  a dense range starting at `player_id_start` (patch-dependent).
- **Minion**: lane creeps.
- **Monster**: jungle camp occupants, including epic monsters (Drake,
  Baron, Herald, Atakhan).
- **Turret**: fixed structures with ids.
- **Inhibitor**: fixed structures.
- **Ward**: placed visibility tokens. Short-lived. (`YellowTrinket`,
  `SightWard`, `JammerDevice` + "Corpse" variants for destruction events.)
- **Projectile**: spell missiles, basic attacks. Very short-lived.
- **Pet**: champion-summoned units (Annie's Tibbers, Tahm Kench's Devourer,
  Lillia's bees...). Medium-lived.
- **Plant**: jungle plants (Blast Cone, Honeyfruit, Scryer's Bloom).

ROFL-X should not hardcode this taxonomy; entity kinds are inferred from
the opcodes that spawn them and the `name` field on the spawn packet.
The list above is for orientation, not schema.

### `Player`

One of the ten human participants in the match. Distinct from
`Champion`: a `Player` controls a `Champion` entity. Fields: `riot_id`
(game name + tag line), `puuid`, `team` (`Blue` / `Red`), `role` (`Top`,
`Jungle`, `Mid`, `Adc`, `Support`), `champion_name` (e.g. `"Sett"`).

Inferred from the metadata JSON's `statsJson` array. Role inference needs
revisiting: Mowokuma's array-index fallback (`["Top","Jungle",...][i % 5]`)
is fragile; there's likely an `INDIVIDUAL_POSITION` or `TEAM_POSITION`
key in `statsJson[i]` we should use instead.

### `GameState`

The running shadow-state we accumulate as we process packets in timestamp
order. Not persisted to disk as-is; it's an internal accumulator that
lets us answer questions like "where was Sett at t=17.5s" by interpolating
against the latest `PathPacket` for Sett's entity id.

`GameState` is effectively a dict of entity id to current entity snapshot,
plus some match-global state (score, objectives, current time).

### `GameEvent`

A higher-level, exported event emitted from one or more packets. Examples:

- `WardPlaced { position, owner, duration }` (from ward-spawn packet +
  matching ward-destroy packet, as Mowokuma does it).
- `ChampionKilled { victim, killer, assists, position, timestamp }` (from
  death packet + recent damage packets).
- `ObjectiveTaken { kind: Dragon | Baron | Herald | ..., team, time }`.
- `ItemPurchased { player, item_id, gold_spent, time }`.
- `LevelUp { champion, new_level, time }`.

`GameEvent` is **our** abstraction. It's what downstream consumers of the
output JSON actually care about. The mapping from packets to events is
often many-to-one (a kill event is informed by multiple packets) and is
the ROFL-X contribution on top of the catalog.

---

## 4. Lifecycles

### The parse lifecycle

```
file bytes
   │
   ▼
[FileHeader parse]   ── extracts version, validates magic
   │
   ▼
[ChunkRecord walk]   ── iterates records, groups by stream tag
   │
   ▼
[zstd decompress]    ── per chunk: compressed_len → uncompressed_len
   │
   ▼
[Block framing]      ── per chunk: marker-byte decoder, yields Blocks
   │
   ▼
[Opcode routing]     ── per Block: look up handler by opcode
   │                           │
   │                           ├── pure-Rust decoder (if any)
   │                           │        └── typed Packet
   │                           │
   │                           └── emulator-backed decoder
   │                                    └── typed Packet
   ▼
[GameState update]   ── per Packet: mutate shadow state
   │
   ▼
[GameEvent emit]     ── per state-change of interest: append event
   │
   ▼
[JSON serialise]     ── metadata + players_state + wards + events
```

Every stage is a pure function of its input, in Rust terms. The
opcode-routing stage is where unknown opcodes are counted and logged, not
where they crash the parser.

### Entity lifecycle (generic)

Entities in a match have three observable phases:

1. **Create**: a spawn packet (ward-spawn, entity-create, create-summoner,
   minion-spawn, etc.) introduces a new id. The packet usually carries
   enough data for our state to track name, kind, owner, initial position.
2. **Update**: various per-tick packets adjust position, health, level,
   stats, visibility.
3. **Destroy**: a destruction packet (death, despawn, corpse) marks the
   end. For wards specifically, Mowokuma treats a same-opcode packet with
   `name.contains("Corpse")` at matching coordinates as destruction.

Not all kinds observe all phases cleanly. Minions die constantly; modelling
every minion death as a first-class event would blow up the output JSON.
ROFL-X's default should be to emit `GameEvent`s only for player-agentic
things (champion kills, ward lifecycles, objective takedowns, item
purchases) and keep minion/monster death as internal state unless the
caller opts in with `--full`.

### Ward lifecycle (concrete example)

One place we already have a working model, courtesy of Mowokuma:

```
t = 34.2s   ward-spawn packet, name="YellowTrinket",
            id=2138, owner_id=42, pos=(7506, 9834)
                │
                ├── state: new Ward entity 2138 tracked
                │
                ▼
t = 124.4s  ward-spawn packet, name="YellowTrinketCorpse",
            pos=(7506, 9834)
                │
                ├── state: Ward 2138 destroyed (coords match)
                │
                ▼
GameEvent: WardPlaced {
    id: 2138,
    kind: YellowTrinket,
    owner: <Player mapped from owner_id>,
    position: (7506, 9834),
    spawn_t: 34.2,
    destroy_t: 124.4,
    duration: 90.2s,
}
```

Two notes:

- The destruction packet uses the same opcode as the spawn packet, with a
  different `name`. This is a general pattern in the game's packet design:
  one opcode can carry many distinct *semantic* events distinguished by a
  string field. Our catalog should express this, not collapse it.
- Coordinate-matching is the only linking mechanism Mowokuma has. It would
  be more robust to match by entity id, if the destruction packet carries
  one. Worth probing in Phase 4 when we have emulator access to this
  opcode on patch 16.8.

### Player position lifecycle

Only slightly different:

- Each `PathPacket` (`mov_decrypt` opcode) carries a waypoint list and a
  speed for one entity.
- We keep the *latest* `PathPacket` per entity id in state.
- At query time (or at fixed 1-second tick intervals for output), we ask
  each stored `PathPacket` to interpolate: given the time since the packet
  was emitted, how far along the waypoint chain is the entity now?

No explicit "position packet" per tick; it's derivable from the most
recent path packet.

---

## 5. The emulator's role

For packets whose payload is obfuscated (almost all of them, as far as
Phase 1 has shown), we do not decode the bytes in Rust. We call the game's
own decoder on them inside a Unicorn VM.

The domain model treats the emulator as a **black-box decoder**:

```
emulator: (opcode_handler_id, payload_bytes) → typed Packet
```

where `opcode_handler_id` is a per-patch descriptor that names which
function in the game binary to call, which struct offsets to read back,
and (for ward-spawn-class packets) at which write-count each field is
"finalised". The descriptor comes from a per-patch config archive; the
bytes of the relevant `.text`/`.data`/`.rdata` sections ship alongside.

The domain model deliberately does **not** include the emulator state.
From the rest of the parser's perspective, the emulator is a pure
function. Its internal memory / heap / register state is an implementation
detail.

Packets that do not need emulator decoding (if any) go through a
pure-Rust handler that takes the same shape: `(opcode_id, payload_bytes)
→ typed Packet`. The rest of the parser cannot tell which path a given
opcode went through.

---

## 6. Boundary contract

This is the commitment between layers, restated compactly:

| Layer            | In                               | Out                                |
|------------------|----------------------------------|------------------------------------|
| FileHeader parser| `&[u8]` (whole file)             | `FileHeader`, `chunks_start_offset`|
| Metadata parser  | `&[u8]` (whole file)             | `Metadata` (with `Vec<Player>`)    |
| Chunk walker     | `&[u8]`, chunks region bounds    | `Iterator<Item = ChunkRecord>`     |
| Decompressor     | `ChunkRecord`                    | `Vec<u8>` (decompressed body)      |
| Block framer     | decompressed body                | `Iterator<Item = Block>`           |
| Opcode router    | `Block`                          | `Packet` (via pure-Rust or emu)    |
| GameState        | stream of `Packet`               | `GameState` snapshot at any `t`    |
| Event emitter    | `GameState` + `Packet`           | `Vec<GameEvent>`                   |
| Output serialiser| `Metadata`, `GameState`, events  | JSON                               |

Everywhere above, **unknown opcodes never crash the parser**. They log,
they count (for the audit subcommand), they get passed through as
`Block { opcode, payload_bytes, timestamp }` records so the output can
include a "raw blocks we couldn't decode" bucket if the caller asks for it.

---

## 7. What's deliberately not modelled here

- The game client's memory management, the decrypt-access-release cycle,
  the vtable layout, function RVAs. Those are emulator-internal.
- Signature cryptography. We preserve the 256 bytes; we don't verify.
- Replay-to-replay cross-correlation (e.g. matching summoners across
  games). Out of scope; that's analytics, not parsing.
- Live game traffic. This is a file parser; live decryption is a different
  project entirely and explicitly out of scope (see README § Non-goals).

---

## Next

Phase 2 step 2: `docs/ROFL_FORMAT.md`, a byte-level specification of
everything in section 2 above, with every field citing either a sample
offset, a prior-art source, or an explicit `UNVERIFIED` marker.
