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

Named by Henry Zhu in his public dataset (Apache 2.0, `packets.py`
schema on Hugging Face). 22 classes total, fields cross-referenced in
[DATASETS.md](DATASETS.md) with measured frequencies from one batch of
patch 12.22 replays. We have not cross-mapped the class names to our
numeric opcodes on any current patch.

| class                       | share in S12 batch | summary                                                          |
|-----------------------------|-------------------:|------------------------------------------------------------------|
| `LeaveFog`                  | 65.4 %             | entity leaves vision (`net_id`, `time`)                          |
| `Replication`               | 15.8 %             | per-entity property replication (`net_id_to_replication_datas`)  |
| `UnitApplyDamage`           |  5.0 %             | damage (`source_net_id`, `target_net_id`, `damage`)              |
| `WaypointGroup`             |  4.6 %             | movement waypoints keyed by entity id, positions as `{x, z}`      |
| `EnterFog`                  |  3.8 %             | entity enters vision                                             |
| `DoSetCooldown`             |  2.8 %             | ability cooldown update (`slot`, `cooldown`, `display_cooldown`) |
| `CastSpellAns`              |  0.57 %            | spell cast answer: caster/targets/positions/spell/mana/cd        |
| `BasicAttackPos`            |  0.52 %            | basic attack with source+target+spell metadata                   |
| `BarrackSpawnUnit`          |  0.39 %            | minion wave spawn event                                          |
| `NPCDieMapView`             |  0.38 %            | NPC death: killed+killer net ids                                 |
| `SpawnMinion`               |  0.33 %            | individual minion spawn with position and targetability          |
| `WaypointGroupWithSpeed`    |  0.11 %            | movement variant that carries speed per waypoint                 |
| `CreateTurret`              |  0.08 %            | turret / likely ward / stationary owned entity                   |
| `CreateNeutral`             |  0.06 %            | jungle-camp monster creation with position + camp id             |
| `UseItem`                   |  0.04 %            | item activation                                                  |
| `CreateHero`                |  0.04 %            | champion init (`champion`, `name`, `net_id`)                     |
| `BuyItem`                   |  0.03 %            | item purchase with gold deltas                                   |
| `RemoveItem`                |  0.01 %            | item sell or drop                                                |
| `SwapItem`                  | 0.005 %            | inventory slot swap                                              |
| `NPCDieMapViewBroadcast`    | <0.001 %           | broadcast variant of NPC death                                   |

Cross-references to our decoded opcodes:

- **`WaypointGroup`** (+ `WaypointGroupWithSpeed`) corresponds
  semantically to our `MOVEMENT_PATH` entry. Zhu's Position uses
  `{x, z}`; ours uses `{x, y}`. Our output is per-entity per-new-order;
  Zhu bundles multiple entities per `time` into one dict.
- **`WARD_SPAWN_OR_DESTROY`** rides on Zhu's `SpawnMinion`, confirmed
  by probing names in one S12 batch: 1,654 `SightWard`, 739
  `VisionWard`, 698 `JammerDevice`, 1,173 `PlantVision`, plus 5,683
  `WardCorpse` destruction events (destruction name suffix matches
  Mowokuma's corpse heuristic exactly). **Our decoded struct is
  strictly richer than Zhu's `SpawnMinion`**: Mowokuma's
  `ward_spawn_decrypt` captures `owner_id` (the placing player), a
  field Zhu's `SpawnMinion` does not have. Any Zhu-compat adapter that
  maps our output to his schema will drop `owner_id`; the reverse
  direction cannot recover it.

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

## 16.9 catalog: every netid wired

Per the work documented in this branch (and reproduced by
[scripts/build_semantic_catalog.py](../scripts/build_semantic_catalog.py)),
every one of the 217 distinct netids observed in our 16.9 benchmark
replay is wired to a decoder. The semantic catalog at
[scripts/semantic_field_names.json](../scripts/semantic_field_names.json)
groups netids by the decoder they share, with one of three confidence
tiers per offset:

- **hand-curated** (18 unique decoders, 187 of 216 netids): decoder
  was read in decompiled C, fields named based on shape + Zhu's S12
  schema. Includes mov_decrypt (`fb4070`), Replication generic
  (`f6ab10`, 51 netids), DoSetCooldown (`fdb200`), UpdateState
  (`f9d4e0`, 12 netids incl. 1068), the Replication-compact
  (`fcfd30`, 21 netids) and Replication-long (`eba9e0`) variants,
  AttackOrSpellCast (`1074580`), several event packets, and the
  ConstructorFallback / TagOnlyStub honest-reporting buckets.
- **auto-generated** (35 unique decoders, 29 netids): wired with
  generic `field_at_0xNN` names + observed type. Decoders share the
  prologue + bit-reader pattern of the hand-curated set; field
  meanings are inferable from Zhu's class shapes when needed but the
  per-offset semantics aren't yet hand-checked.
- **constructor-fallback / tag-only** (2 RVAs, 19 netids): brute-
  force matched these to `db2910` (a class constructor) or `e9dd20`
  (a 19-byte stub). The decoded_fields[] for them are init constants
  or empty — no real packet decoding. Honestly labeled as such so
  consumers don't mistake init values for decoded data.

Apply the catalog to a rofl-x output JSON with:
```bash
python scripts/apply_semantic_names.py <out.json> [--inplace]
```
This adds `name` + `type_hint` to every `decoded_fields[]` entry that
has a catalog mapping. End-to-end on the benchmark replay: 21,179
fields named across 4,989 samples covering all 216 wired generic
decoders.

## Adding a new entry

Workflow (per [ROADMAP.md](ROADMAP.md) § Phase 4):

1. Add a catalog entry here.
2. Add a handler in `src/packet/handlers/<name>.rs`.
3. Add a fixture byte slice in `tests/fixtures/<name>.bin`.
4. Add a test in `tests/handlers/<name>.rs` asserting the handler
   decodes the fixture to the expected struct.

Never the reverse. The catalog entry is the anchor; handler, fixture,
and test exist to validate what the entry says.
