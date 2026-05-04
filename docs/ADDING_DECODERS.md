# Adding decoders for new packet classes

How to extend ROFL-X to decode a new packet class on a new patch. The
goal is "every packet decoded"; this doc is the playbook for each
incremental step toward that.

## Architecture summary

ROFL-X is a multi-patch parser. Each `.patch` archive describes ONE patch
of the League client (e.g. `5-5.patch`, `16-9.patch`). At decode time,
ROFL-X picks the archive matching the replay's patch tag and uses its
decoder definitions.

Within an archive, `result.json` declares decoders. Today it supports
**two** classes: `mov_decrypt` and `ward_spawn_decrypt`. To get to "every
packet decoded" we add more classes one at a time, each with:

1. A decoder function RVA in the patched binary
2. An output-struct layout (which offsets hold which fields)
3. Optionally: helpers to stub (allocators, safety checks)
4. A Rust struct + parser to interpret the decoder's writes
5. JSON output schema for the new packet kind

## Per-decoder workflow

For each new packet class on each new patch, run this loop. The tools
collapse it from "weeks of RE per packet" to "hours per packet".

### Step 1 — find the decoder RVA

Use either:

- **Cross-patch byte-pattern porting**: if you already have a donor
  archive on a nearby patch, run `rofl-x scan-decoder --donors
  <donor.json> --target <new>.patch-skeleton`. Top candidates are likely
  the same decoder relocated.
- **Dispatch table walk**: run the dispatch-table mapper script from
  `docs/PATCH_16_9_ANALYSIS.md`. This gives you a list of all decoder
  candidates structurally.
- **Ghidra inspection**: open the binary in Ghidra, navigate to a
  candidate, decompile (`scripts/ghidra/decompile_funcs.py` does this
  headlessly). Read the C and identify the function shape.

### Step 2 — find the netid

Run `rofl-x inspect --histogram --replay <recent.rofl>`. Cross-reference
with:

- Frequency of the packet class in Henry Zhu's published dataset
  (`docs/DATASETS.md`).
- Numeric proximity to known netids on adjacent patches.
- Payload size signature: pull a few samples with
  `rofl-x extract-fixture --netid <N> --name probe_<N> --count 5` and
  compare to the expected shape of the class.

### Step 3 — find the output-struct layout

Run `rofl-x trace-decoder --replay <r.rofl> --netid <N>
--patch-dir ./patch --rva-start 0x... --rva-end 0x...`. The output table
shows which struct offsets the decoder writes to and how often. Match
those against the class's known fields (e.g. ward-spawn writes to id,
owner_id, name, x, y).

### Step 4 — wire into the patch archive

Edit `result.json`. Today's schema supports two top-level decoders.
Future patches will include an `extra_decoders: [...]` array (TODO,
tracked in this doc). For now: the new decoder either replaces an
existing slot for that patch, or waits for the array refactor.

Schema v2 example (16.9 with inline-floats movement):

```json
{
  "mov_decrypt": {
    "netid": 916,
    "rva_start": "0xfb4070",
    "rva_end":   "0xfb44b0",
    "output_format": "inline-floats",
    "inline_x_offset": "0x20",
    "inline_y_offset": "0x24"
  },
  "ward_spawn_decrypt": { ... },
  "alloc1_rva": "0x10053f0",
  ...
}
```

The `output_format` field is the key extension point. Adding a new
format means adding a Rust enum variant in `src/emulator/config.rs`
(`MovOutputFormat`) and a corresponding match arm in
`StubEmulator::call_decrypt_pos_packet`.

### Step 5 — run end-to-end

```
rofl-x file --replay <r.rofl> --output out.json --patch-dir ./patch
```

Inspect `out.json`. For inline-floats partial decoders, the (x, y) data
lands in `raw_positions[]` (no entity attribution) until the decoder's
entity-id source is RE'd separately.

## What's required to add a totally new packet class (e.g. `CreateHero`)

The `mov_decrypt` / `ward_spawn_decrypt` slots are reserved for those
two specific classes. Adding a third class (e.g. `CreateHero`) requires
the following code changes — currently not yet implemented, but
straightforward extensions:

1. **Add a struct in `src/emulator/packet.rs`**:
   ```rust
   pub struct CreateHeroPacket {
       pub timestamp: f32,
       pub net_id: u32,
       pub champion: String,
       pub name: String,
   }
   ```

2. **Add a config struct in `src/emulator/config.rs`**:
   ```rust
   pub struct CreateHeroDecrypt {
       pub netid: u32,
       pub rva: u64,
       pub end_rva: u64,
       pub net_id_offset: u64,
       pub champion_offset: u64,
       pub name_offset: u64,
       // ...write counts as needed
   }
   ```

3. **Add an emulator entry point in `src/emulator/unicorn.rs`**:
   ```rust
   pub fn call_decrypt_create_hero_packet(
       &mut self, call_rva: u64, end_rva: u64, timestamp: f32,
   ) -> Result<CreateHeroPacket> {
       // Same shape as call_decrypt_ward_spawn_packet:
       // - emu_start the decoder
       // - read fields out of the struct at known offsets
       // - construct the typed packet
   }
   ```

4. **Wire into `replay_info::parse_and_decode`** to iterate blocks with
   the new netid, batch them through the emulator, and collect the
   results.

5. **Extend `build_output_json`** to add a top-level `create_hero[]`
   array with one entry per decoded packet.

The pattern is uniform across all 22 named packet classes from Henry
Zhu's schema and the long-tail UNKNOWN opcodes. The key claim: each new
class is **incremental** code, not architectural.

## Roadmap toward "every packet decoded"

In order of expected leverage:

| Class           | Frequency in S12 | Status | Priority |
|-----------------|------------------|--------|----------|
| Replication     | 15.8%            | unknown | high (carries HP, MP, every stat) |
| UnitApplyDamage |  5.0%            | unknown | high (kill feed) |
| WaypointGroup   |  4.6%            | partial (mov_decrypt) | done-ish |
| LeaveFog/EnterFog | 65.4% / 3.8%   | unknown | medium (visibility tracking) |
| DoSetCooldown   |  2.8%            | unknown | low |
| CastSpellAns    |  0.57%           | unknown | medium (skill usage) |
| BasicAttackPos  |  0.52%           | unknown | low |
| BarrackSpawnUnit|  0.39%           | unknown | low |
| NPCDieMapView   |  0.38%           | unknown | medium (NPC kills) |
| SpawnMinion     |  0.33%           | unknown | low (also carries wards) |
| CreateTurret    |  0.08%           | unknown | low |
| CreateNeutral   |  0.06%           | unknown | low |
| UseItem         |  0.04%           | unknown | medium (item tracking) |
| CreateHero      |  0.04%           | unknown | medium (champion init) |
| BuyItem         |  0.03%           | unknown | medium (gold/item tracking) |
| ...             |                  |        |   |

A reasonable two-week target: `Replication` + `UnitApplyDamage` +
`NPCDieMapView` decoded on 16.9 — that's HP bars, kill feed, and NPC
deaths all working in the viewer.

## Cross-patch maintenance

When Riot ships a new patch (e.g. 16.10):

1. `rofl-x extract-patch --binary <new>.exe --output ./patch/16-10.patch-skeleton`
2. `rofl-x scan-decoder --donors ./patch/16-9-donors.json --target ./patch/16-10.patch-skeleton`
3. For each high-score match: paste the new RVA into the new
   `result.json`. Use `trace-decoder` to verify struct offsets are
   stable; if not, update them.
4. Save as `16-10.patch`. ROFL-X auto-routes 16.10 replays to it via
   `Config::resolve_patch_file`.

The expectation is that consecutive patches share most decoder bodies —
`scan-decoder` will hit 0.8+ scores most of the time. Only when Riot
rewrites a decoder body (rare) does manual RE recur.

## Output format compatibility table

| Patch range | mov_decrypt format     | ward_spawn format | other classes |
|-------------|------------------------|-------------------|---------------|
| 15.1 – 15.5 | `buffer-stream` (5-5)  | known (5-5)       | none          |
| 16.7 – 16.9 | `inline-floats` (16.9) | unknown           | none          |
| future      | TBD                    | TBD               | grow this column |

Old archives (without `output_format`) default to `buffer-stream` so
Mowokuma's `5-5.patch` archive continues to work unmodified.
