# Henry Zhu's decoded-replay dataset

A reference corpus of ~700,000 already-decoded League of Legends
replays, released publicly on Hugging Face:
https://huggingface.co/datasets/maknee/league-of-legends-decoded-replay-packets

License: **Apache 2.0**. Full dataset size: 108 GB, 1,348 batch files.

ROFL-X does not commit any bytes from the dataset to the repo. Everything
lives in `reference/zhu-dataset/` (gitignored). The two scripts below
describe how to fetch a manageable slice.

## Layout of the Hugging Face repo

```
league-of-legends-decoded-replay-packets/
├── packets.py                # schema definitions (22 dataclasses)
├── 12_22/
│   ├── batch_001.jsonl.gz    # ~80 MB gzipped; ~1-10 matches per file
│   ├── batch_002.jsonl.gz
│   └── ...
├── 12_23/
└── ...                       # one directory per LoL patch in S12
```

Every `.jsonl.gz` is one match per line. Each line is:

```json
{ "events": [
  { "WaypointGroup": { "time": 1.2, "waypoints": {"1001": [{"x": 100.5, "z": 200.3}]}}},
  { "CastSpellAns": { "time": 10.2, "champion_caster_id": 1073741859, ...}},
  ...
]}
```

## Fetching and profiling

```bash
# One batch file (~80 MB) from patch 12.22, plus the schema:
python scripts/zhu_dataset_fetch.py

# More batches from a different patch:
python scripts/zhu_dataset_fetch.py --patch 12_23 --n 5

# Just the schema (tiny):
python scripts/zhu_dataset_fetch.py --schema-only

# Profile classes and field unions across 20 games:
python scripts/zhu_dataset_stats.py
python scripts/zhu_dataset_stats.py --games 50
```

`scripts/zhu_dataset_stats.py` refuses to print values for fields whose
names look like PUUIDs, Riot IDs, or summoner names. It does surface the
presence of such keys so their drift across patches is auditable.

## Schema summary

Zhu's dataset defines 22 packet classes via Python dataclasses in
`packets.py`. Profiling 20 games from `12_22/batch_001.jsonl.gz`
(9.9M events) gave the following distribution:

| class                     | share   | fields                                                                                                                |
|---------------------------|---------|-----------------------------------------------------------------------------------------------------------------------|
| `LeaveFog`                | 65.4 %  | `net_id`, `time`                                                                                                      |
| `Replication`             | 15.8 %  | `net_id_to_replication_datas`, `time`                                                                                 |
| `UnitApplyDamage`         |  5.0 %  | `damage`, `source_net_id`, `target_net_id`, `time`                                                                    |
| `WaypointGroup`           |  4.6 %  | `time`, `waypoints` (Dict[net_id, List[Position]])                                                                    |
| `EnterFog`                |  3.8 %  | `net_id`, `time`                                                                                                      |
| `DoSetCooldown`           |  2.8 %  | `cooldown`, `display_cooldown`, `net_id`, `slot`, `time`                                                              |
| `CastSpellAns`            |  0.57 % | `caster_net_id`, `cooldown`, `level`, `mana_cost`, `slot`, `source_position`, `spell_*`, `target_*`, `windup_time`    |
| `BasicAttackPos`          |  0.52 % | (superset of CastSpellAns + source/target positions)                                                                  |
| `BarrackSpawnUnit`        |  0.39 % | `barrack_net_id`, `minion_level`, `minion_net_id`, `minion_type`, `time`, `wave_count`                                |
| `NPCDieMapView`           |  0.38 % | `killed_net_id`, `killer_net_id`, `time`                                                                              |
| `SpawnMinion`             |  0.33 % | `bot`, `level`, `name`, `net_id`, `position1`, `position2`, `skin_name`, `targetable_*`, `time`                       |
| `WaypointGroupWithSpeed`  |  0.11 % | `time`, `waypoints`                                                                                                   |
| `CreateTurret`            |  0.08 % | `name`, `net_id`, `owner_net_id`, `time`                                                                              |
| `CreateNeutral`           |  0.06 % | `camp_id`, `direction`, `level`, `name`, `net_id`, `neutral_type`, `position1`, `position2`, `skin_name`, `time`      |
| `UseItem`                 |  0.04 % | `items_in_slot`, `net_id`, `slot`, `spell_charges`, `time`                                                            |
| `CreateHero`              |  0.04 % | `champion`, `name`, `net_id`, `time`                                                                                  |
| `BuyItem`                 |  0.03 % | `entity_gold_after_change`, `item_gold`, `item_id`, `item_name`, `items_in_slot`, `net_id`, `slot`, `spell_charges`, `time` |
| `RemoveItem`              |  0.01 % | `entity_gold_after_change`, `items_in_slot`, `net_id`, `slot`, `time`                                                 |
| `SwapItem`                |  0.005 %| `net_id`, `source_slot`, `target_slot`, `time`                                                                        |
| `NPCDieMapViewBroadcast`  | <0.001 %| `killed_net_id`, `killer_net_id`, `time`                                                                              |

## What Zhu's dataset tells us about wards

None of the 22 classes is a dedicated "ward" class. Wards almost
certainly ride on `CreateTurret` (Zhu's category for stationary
owner-tagged entities) or `CreateNeutral`. Our `WARD_SPAWN_OR_DESTROY`
catalog entry is likely broader than any single Zhu class, since it
covers both placement and destruction in one packet. This is a
schema-shape divergence we'll cross-reference in
[PACKETS.md](PACKETS.md) and [COMPATIBILITY.md](COMPATIBILITY.md).

## Coordinate naming

Zhu uses `{x, z}` consistently in `Position` (3D-origin convention).
Mowokuma uses `{x, y}` (top-down 2D convention). For output parity with
her we keep `{x, y}`; a Zhu-compat adapter can rename at
serialisation time.

## How the Zhu corpus feeds our roadmap

- **Phase 4 catalog expansion**: every Zhu class not already in our
  catalog becomes an `OBSERVED-ONLY` entry with his field list
  documented. Once we can decode the corresponding opcode on a current
  patch, it is promoted to `DOCUMENTED`.
- **Phase 5 schema compatibility**: our output schema must be able to
  represent every Zhu field. For the classes we decode, this means
  matching field names on top of Mowokuma's existing keys, preferably
  as additive sibling keys (`zhu_compat.*`) rather than reshaping
  `metadata` / `players_state` / `wards`.
- **Phase 5 future work**: if Zhu ever publishes the raw `.rofl` files
  alongside his decoded output (currently only decoded JSON is
  published), we could run ROFL-X end-to-end over his input and diff
  against his decoded output for byte-level validation.
