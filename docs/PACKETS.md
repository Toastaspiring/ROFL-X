# Packet catalog

One entry per known opcode, plus placeholders for opcodes we have only
observed frequency-of. Statuses:

- **DOCUMENTED**: we decode it end-to-end on at least one patch, and the
  field layout in the decoded struct is written down.
- **PARTIAL**: we decode some fields but the full struct layout is still
  unknown or unstable.
- **OBSERVED-ONLY**: we see the opcode in real replays and have a
  semantic label for it (from prior art), but we cannot decode its
  payload on any patch yet.
- **UNKNOWN**: we see the opcode but have no semantic label at all.

All opcode numbers below are patch-specific. See
[COMPATIBILITY.md](COMPATIBILITY.md) for per-patch coverage.

---

## DOCUMENTED

### `MOVEMENT_PATH`

**Status:** DOCUMENTED on patches 15.1 through 15.5.
**Patch-specific opcode numbers:**
| patch | netid |
|-------|-------|
| 15.5  | 980   |
| 15.4  | (same, per Mowokuma's 5-4.patch) |
| 15.3  | (same) |
| 15.2  | (same) |
| 15.1  | (same) |
| 16.8  | **unknown** (blocked on patch archive) |

**Decoded via:** Unicorn emulator calling the game's `mov_decrypt`
function. Pure-Rust decoding is not feasible (obfuscated body).

**Decoded struct:**
```rust
PathPacket {
    timestamp: f32,           // from the enclosing block
    id: u32,                  // entity id
    speed: f32,               // current move speed, map units/sec
    waypoints: Vec<(f32, f32)>, // path points in (x, y) map coords
}
```

**Semantics:** emitted whenever an entity's movement order changes
(new command, spell-locked, stopped). Position at any later time is
derivable by walking the waypoint chain at `speed`, which
`PathPacket::get_pos` does.

**Coordinate constants** (see `PathPacket::parse`): final `(x, y)`
derived as `x = sign_extend(encoded, 16) * 2.0 + 7358.0`,
`y = sign_extend(encoded, 16) * 2.0 + 7412.0`. The offsets are the
map-centre coordinates for Summoner's Rift; they have been stable
across patches in our observation.

**References:** Mowokuma/ROFL `src/emulator/packet.rs::PathPacket::parse`.

### `WARD_SPAWN_OR_DESTROY`

**Status:** DOCUMENTED on patches 15.1 through 15.5.
**Patch-specific opcode numbers:**
| patch | netid |
|-------|-------|
| 15.5  | 571   |
| 15.1 - 15.4 | (per Mowokuma's archives) |
| 16.8  | **unknown** (blocked on patch archive) |

**Decoded via:** Unicorn emulator calling the game's
`ward_spawn_decrypt` function with a memory-write hook that captures
the output struct fields at the configured write counts.

**Decoded struct:**
```rust
WardSpawnPacket {
    timestamp: f32,
    name: String,             // e.g. "YellowTrinket", "SightWard",
                              // "JammerDevice", or "*Corpse" for death
    id: u32,                  // ward entity id
    owner_id: u32,            // player entity id that placed it
    x: i32,
    y: i32,
}
```

**Semantics:** one opcode carries both the placement and the
destruction event. A placement has `name` in
`{YellowTrinket, SightWard, JammerDevice}` (the three common ward
kinds Mowokuma enumerated); a destruction has `name` containing
`"Corpse"`. Coordinate-match destructions to placements with the same
`(x, y)` integer pair.

**Known unknowns:** other ward variants (`BlueTrinket`, `FarsightAlt`,
`VisionWard`) almost certainly use this same opcode; they do not
appear in Mowokuma's placement allowlist but probably should. This
is a concrete completeness gap to resolve when we get fixtures for
the other variants.

**References:** Mowokuma/ROFL `src/emulator/stub_emulator.rs::call_decrypt_ward_spawn_packet`,
`src/main.rs` ward-lifecycle reconstruction logic.

---

## OBSERVED-ONLY

Named by Henry Zhu in his 2025 write-up; we have not cross-mapped the
class names to our numeric opcodes on any patch yet. Full details in
[REFERENCE_HENRY_ZHU.md](REFERENCE_HENRY_ZHU.md) § 5.

| class name                  | summary                                                        |
|-----------------------------|----------------------------------------------------------------|
| `TakeDamagePacket`          | damage application (target id, damage f32, source id)          |
| `BasicAttackAtTarget`       | melee/ranged basic attack with positions                       |
| `CastSpell`                 | ability cast: caster, spell, level, src/tgt pos, windup, cd    |
| `CreateSummoner`            | player init: time, champion id, name, summoner id              |
| `CreateEntity`              | entity spawn (wards fall under this; likely same netid as our  |
|                             | `WARD_SPAWN_OR_DESTROY` above, to be cross-checked)            |
| `UpdateState`               | per-entity stat update (hp, movement speed, ...)               |
| `Death`                     | entity death: victim id + timestamp                            |
| `BecomeVisibleInFogOfWar`   | visibility on                                                  |
| `LeaveFromFog`              | visibility off (noted by Zhu as ~2.8% redundant repeats)       |

---

## UNKNOWN: top opcodes observed in sample_a (patch 16.8)

From `scripts/sample_walkthrough.py` run on one 34-minute 16.8 replay:
1,924,363 blocks total, 249 distinct opcodes. Top 15 by frequency:

| opcode (hex) | opcode (dec) | count     | % of blocks | guess                                |
|--------------|--------------|-----------|-------------|--------------------------------------|
| 0x0389       | 905          | 721,319   | 37.5 %      | per-entity per-tick heartbeat (`UpdateState`?) |
| 0x036B       | 875          | 102,200   | 5.3 %       | high-frequency; candidate for movement or visibility |
| 0x03D6       | 982          |  92,028   | 4.8 %       | near `mov_decrypt.netid` value on 15.5 (980); strong candidate for movement on 16.8 |
| 0x02DA       | 730          |  70,277   | 3.7 %       | ?                                    |
| 0x0118       | 280          |  48,429   | 2.5 %       | ?                                    |
| 0x0357       | 855          |  46,779   | 2.4 %       | ?                                    |
| 0x0473       | 1139         |  44,000   | 2.3 %       | ?                                    |
| 0x001E       |  30          |  42,819   | 2.2 %       | ?                                    |
| 0x015A       | 346          |  38,230   | 2.0 %       | ?                                    |
| 0x0090       | 144          |  26,950   | 1.4 %       | ?                                    |
| 0x0076       | 118          |  26,041   | 1.4 %       | ?                                    |
| 0x027C       | 636          |  24,826   | 1.3 %       | ?                                    |
| 0x0126       | 294          |  23,962   | 1.2 %       | ?                                    |
| 0x018B       | 395          |  23,451   | 1.2 %       | ?                                    |
| 0x03F7       | 1015         |  22,815   | 1.2 %       | ?                                    |

Observation: `0x03D6 = 982` is numerically very close to patch 15.5's
`mov_decrypt.netid = 980`. When someone reverse-engineers 16.8, 982 is
the first candidate to check for movement.

## Opcodes seen in sample_a but absent from the Henry Zhu / Mowokuma pools

~230 opcodes remaining, each appearing between 1 and ~8,000 times in
sample_a. Full list reproducible via `scripts/sample_walkthrough.py`.
Each eventually gets either a named label (via reverse engineering or
cross-reference) or an explicit "no idea, N occurrences" entry in the
catalog.

## Adding a new entry

Workflow (per [ROADMAP.md](ROADMAP.md) § Phase 4):

1. Add a catalog entry here.
2. Add a handler in `src/packet/handlers/<name>.rs`.
3. Add a fixture byte slice in `tests/fixtures/<name>.bin`.
4. Add a test in `tests/handlers/<name>.rs` asserting the handler
   decodes the fixture to the expected struct.

Never the reverse. The catalog entry is the anchor; handler, fixture,
and test exist to validate what the entry says.
