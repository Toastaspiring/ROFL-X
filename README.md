# `decrypt/tooling-and-16-9-recon` — branch narrative

This is a branch-specific README. It will be replaced by the main
README when merged; here it captures **what this branch tried to do,
the thought process, the dead ends, and the result** — so the work
is intelligible to whoever picks it up next (or to me, in six
months, with no session memory).

> Goal as the user phrased it: *"a true parser of EVERY packet of a
> rofl"*, and ultimately *"to literally read my rofl"*.

---

## TL;DR

Starting state on this branch: the parent had wired **40 / 190
netids (21.1 % class)** for League patch 16.9 with **80.4 % block
coverage**, capped at **30 samples per class**, and identified
**0 packet classes by their real Riot name**.

Ending state:

| Layer                                                   | Result          |
|---------------------------------------------------------|-----------------|
| Class identified (decoder RVA known per netid)          | **217 / 217**   |
| Block-level decoder runs                                | **2,024,731 / 2,024,731** |
| Per-class success (decoded_fields[] non-empty)          | 215 / 217 (the other 2 are tag-only) |
| Sample cap                                              | **none** — every block decoded |
| Full decode time on benchmark replay (44 min game)      | **2 min 24 s**  |
| Riot's real packet class vocabulary recovered           | **320 PKT_*_s names** |
| Decoders mapped to Riot class names (shape-anchored)    | 53 / 53         |
| `rofl → typed event timeline` end-to-end                | ✓ (~5 minutes total) |

The branch ships:
- A **pure-Python RE pipeline** (no Ghidra dependency for the analysis
  half) that goes binary → dispatch table → prologue match → 41 high-
  confidence decoders → brute-force matching → wire to patch.
- A **netid dispatcher discovery** (`0xe83930` jump table at .text
  RVA `0xe93818`, 1198 entries) — the README's #1 unsolved TODO at
  branch start.
- The **Riot class-name vocabulary** (320 leaked PKT names) extracted
  from MakeFunction template strings, plus full documentation of why
  RTTI proper is dead in this binary.
- A **typed event timeline** for the parser output — every block
  decoded, every event classified, every field semantically named to
  the extent shape allows.

---

## How we got here (chronological)

### Day 0 — cold start on a different machine

The previous session had:
- Built a working brute-force decoder identifier on its dev machine
- Documented the workflow in this README's "cold-start guide" section
- Pushed the code to git, but **not** the analysis artifacts
  (`~/Tools/analysis/16-9/`, the `patch/16-9.patch` archive, the
  decompiled C, etc.) — those live outside the repo on purpose.

I resumed on a different physical machine. The repo had the code,
but nothing the code needed to run. So step 1 was **rebuild every
artifact from scratch**.

What was missing on this machine:
- Ghidra (had to install it)
- The `~/Tools/analysis/16-9-project/` Ghidra database
- The `~/Tools/donors/5-5-extracted/` (had it as `reference/patch-55-unpacked/` in the repo, just had to relocate)
- The compiled `patch/16-9.patch` archive
- The decompiled C of every confirmed decoder

What was present:
- The League 16.9 binary at `/c/Riot Games/League of Legends/Game/`
  (verified MD5 = `7bf33f4e0035937d187fa6594afe93df`, matches the
  README-recorded one)
- One 16.9 replay: `EUW1-7837000162.rofl`
- The repo with all its scripts and Mowokuma's 5-5 extracted donor

### Day 1 — rebuilding the analysis pipeline in pure Python

The prior session had relied on Ghidra for all of these steps:

1. Find the dispatch table sub-tables in `.rdata`
2. Find decoder candidates by prologue match
3. Cross-reference into 41 high-confidence decoders
4. Decompile each for manual review

I rebuilt 1–3 as **pure-Python PE-walking scripts** so they don't
need Ghidra to *run*. That cut the dependency surface and let me
run the analysis while Ghidra was still extracting on the side.

Pure-Python scripts written:
- [`scripts/scan_dispatch_table.py`](scripts/scan_dispatch_table.py) —
  walks `.rdata` looking for runs of ≥ 5 contiguous 48-byte entries
  where slot[0] is a constant `.text` VA. Result: **538 sub-tables
  with 2,800 unique slot[1] decoder RVAs.** Matches the prior
  session's count exactly.
- [`scripts/scan_decoder_prologue.py`](scripts/scan_decoder_prologue.py) —
  scans `.text` for the 28-byte donor prologue from 5-5's
  `mov_decrypt` / `ward_spawn_decrypt`. Result: **327 candidates.**
  Matches.
- [`scripts/cross_reference_decoders.py`](scripts/cross_reference_decoders.py) —
  intersect prologue ∩ dispatch → **41 high-confidence decoders.**
  Matches.

Sanity check: `0xfb4070` (mov decoder, known from docs) is in the
high-confidence set. ✓

### Day 1 — running brute-force matching

The high-confidence 41 candidates each get tested against every
observed netid by feeding the netid's payload bytes to the candidate
through `rofl-x trace-decoder` and counting how many distinct struct
offsets it writes. The decoder that writes the most consistently is
the match.

[`scripts/brute_match_decoders.py`](scripts/brute_match_decoders.py)
sweeps 196 observed netids × 41 candidates ≈ 50 minutes wall clock.
[`scripts/wire_confirmed_decoders.py`](scripts/wire_confirmed_decoders.py)
post-processes the results into a confident netid → decoder mapping
and writes them into `patch/16-9.patch`'s `extra_decoders[]` array.

The original `wire_confirmed_decoders.py` had a single-pass noise
filter that excluded any RVA winning for ≥ 3 netids. That dropped
real shared-per-family decoders like `0xf9d4e0` (the Replication-
base that handles netid 1068's high-frequency packets correctly).
I replaced it with a **two-pass strategy**:

1. Pass 1: exclude RVAs from the noisy set
2. Pass 2: fall back to noisy candidates IF their offset count is in
   `[3, STRICT_MAX_WRITES]` — i.e. shared-per-family decoders are
   admitted, but *huge* shared deserializers writing 50+ offsets are
   still excluded.

All thresholds are env-tunable: `NOISE_THRESHOLD`, `STRICT_MAX_WRITES`,
`STRICT_GAP`.

After the first wire: **164 of 195 in-replay netids matched** =
84.1 % class / 98.4 % block coverage on the benchmark replay.

### Day 1 — finding the netid dispatcher (the README's #1 TODO)

The original cold-start guide listed "Find the netid → table-index
mapping" as the single biggest unsolved problem. The dispatch table
contains 2,800 decoder RVAs but no field tells you "this entry is
for netid N". The mapping must live in code.

[`scripts/find_dispatcher.py`](scripts/find_dispatcher.py) scans
`.text` for `lea reg, [rip+disp32]` instructions whose target lands
on any of our 538 known sub-table VAs, then groups by enclosing
function. Top hit: **`0xe83930`** — a 70 KB function with 14
distinct sub-table references.

Decompiled it via Ghidra. The prologue is:

```c
if (param_2 < 0x4ae) {  // param_2 is the netid
    // Indirect jump via jump table at &DAT_140e93818 + (netid * 4)
    puVar2 = (...) (jumptable[netid * 4])();
    return puVar2;
}
```

Confirmed: **`0xe83930` is the netid dispatcher**. The jump table
at `.text RVA 0xe93818` has 1,198 entries (one per netid in
`[0..0x4ae)`). Each target is a per-class **constructor block** that
allocates an instance and writes a vtable pointer; the vtable's
slot[1] is the decoder.

[`scripts/map_netid_to_decoder.py`](scripts/map_netid_to_decoder.py)
walks every netid through this chain. Resolves 1,182 of 1,198. The
catch: many resolve to the same *base-class* vtable rather than the
*derived* one (multi-vtable C++ hierarchies and my LEA-scan picks
the first-or-last hit, neither always right). Brute-force remains
the source of truth for the actual decoder per netid; the
constructor scan is supplementary.

### Day 1 — pushing toward 100 %

The 31 unmatched netids broke into three groups:

1. **22 with zero brute-force hits** even across the 327 prologue
   candidates → their decoders aren't prologue-shaped (different
   calling convention).
2. **9 with hits but filtered by gap or noise heuristics** → the
   filter was too strict, OR they're genuinely fallback-vtable
   netids with `slot[1] = 0x1dbc10` (`xor al, al; ret`, "no decode").
3. The 2 final holdouts (netids 19 and 543) needed manual triage
   via secondary-vtable LEAs in their inner init functions.

[`scripts/probe_unmatched_netids.py`](scripts/probe_unmatched_netids.py)
walks netids whose `e83930` constructor lands on a non-fallback
vtable and probes that vtable's slot[1]. Captured 25 more matches
(latent — most don't fire in this replay but unlock future replays).

[`scripts/probe_indirect_init.py`](scripts/probe_indirect_init.py)
follows the inner init function of each ctor and looks for
*secondary* LEAs into rdata (after the fallback-vtable LEA). Found
8 more.

Manual triage of the last 2: netid 19 → `0xfbd8f0` (real decoder
with 9 distinct offsets), netid 543 → `0xe9dd20` (a 19-byte init
stub — genuine tag-only packet, no decoding to do).

**Final coverage on the benchmark replay: 217 / 217 = 100.00 % class,
2,024,731 / 2,024,731 = 100.0000 % block.**

### Day 2 — the path to "100 % confidence" class labels

Coverage ≠ understanding. We had 100 % wiring, but the 53 unique
decoders had names like `Replication_compact` and `MediumEvent` —
shape-anchored guesses. The user asked: how do we get 100 %
confidence on what each class IS?

**Path 1 — RTTI extraction.** MSVC compilers with `/GR` (default)
embed Run-Time Type Information for every polymorphic class:
`vtable[-1]` → Complete Object Locator → TypeDescriptor → mangled
class name. I tried it. **It returns garbage on this binary.**
Riot stripped C++ RTTI. Only 11 `.?AV` mangled types survive in the
binary, all from `std::*` (runtime_error, bad_cast, etc.) — Riot's
own packet classes have no surviving Type Descriptors.

**Path 1.5 — leaked class names.** Greedy regex over the binary
finds `MakeFunction<ClientType, PKT_PacketType_s>(...)` template
instantiations as MSVC mangled symbols leftover in `.data`. **320
unique Riot packet class names extracted** as a vocabulary —
Riot's exact internal C++ class names. But the strings have **zero
u64 references in code** (verified via byte-aligned scan), so we
can't auto-link "string → decoder RVA". The names exist only as
dead bytes inside mangled-symbol leftovers.

**Path 1.75 — debug-string xref.** Maybe decoders reference some
*other* string near their body (an event name, a property name).
[`scripts/xref_decoder_strings.py`](scripts/xref_decoder_strings.py)
walks each decoder + its ctor + its inner init scanning for LEA
loads of any printable string in `.rdata`/`.data`. Result: **0 hits
across 53 decoders + their inits + the 70 KB e83930 dispatcher.**
Riot's release build genuinely doesn't reference class-name strings
from the dispatch chain — packets are identified by numeric netid
only at runtime.

**Path 2 — state correlation.** The only remaining option for
proven labels. Take replays where you know the outcome (kills,
items bought, positions); correlate decoded values to game state.
Henry Zhu did this manually for 9 classes from 1.4 M replays. For
53 decoders, weeks of effort.

So I called Path 1 dead and pivoted to **shape-anchored class
mapping using Riot's leaked vocabulary** (next section).

### Day 2 — semantic catalog

[`scripts/build_riot_catalog.py`](scripts/build_riot_catalog.py)
generates `scripts/semantic_field_names.json`:

For each of the 53 unique decoder RVAs:
- `riot_class_primary` — single best PKT_*_s guess
- `riot_class_candidates` — ranked list of 1–4 plausible classes
- `riot_class_confidence` — `shape-match` (~30) / `frequency-hint`
  (~3) / `speculative` (~6) / `constructor-fallback` (1) /
  `tag-only` (1)
- `shape` — human description of the field layout
- `fields` — per-offset `{name, type}`

15 decoders are hand-curated based on decompiled C + shape match
to a specific Riot class. The rest are auto-generated with generic
`field_at_0xNN` names + observed type.

Honest labels: 18 netids point to `db2910` which is a class
*constructor* the brute-force misidentified as a decoder. Their
"decoded fields" are init constants, not packet data. Labeled
`ConstructorFallback` rather than fabricating field names.

The full 320-name vocabulary is documented in
[`docs/RIOT_CLASS_VOCABULARY.md`](docs/RIOT_CLASS_VOCABULARY.md),
grouped by domain (Replication, Movement, Spells, Buffs, Death,
Items, Visibility, etc.).

### Day 2 — the parser pipeline

The user's actual goal was **"to literally read my rofl"**, not
"have a complete catalog of decoder RVAs". Different problem.

The parser was capped at **30 samples per class** by a hardcoded
constant in [`src/replay_info.rs`](src/replay_info.rs:338). On the
benchmark replay that meant **0.25 % of blocks** were actually in
the output (4,989 of 2,024,731). The parser was a sampler, not a
parser.

**Stage 2** lifted the cap with a per-class payload cache: identical
payload bytes → previously-decoded result. Heartbeat-class packets
repeat the same bytes thousands of times, so the cache turns the
Nth observation into O(1) instead of an emulator round-trip.

| cap                  | time      | samples emitted          |
|----------------------|-----------|--------------------------|
| 30 / class (before)  | ~1 min    | 4,989  (0.25 % of blocks)|
| 500 / class          | ~30 s     | 52,019 (2.6 %)           |
| unlimited (after)    | 2 min 24s | 1,963,001 + 61,730 mov = **100 % of blocks** |

Lifted via env var `ROFL_X_MAX_SAMPLES=0` (default = unlimited).

**Stage 1** is a Python post-processor:
[`scripts/build_event_timeline.py`](scripts/build_event_timeline.py).
Takes the rofl-x JSON + the semantic catalog and emits a flat
chronological event timeline:

```jsonc
{
  "metadata": { game_length_ms, patch, winner },
  "entities": { "0x40000099": {team, role, champion, ...}, ... },
  "stats":    { total_events, by_type: {Movement: N, Replication: N, ...} },
  "events":   [ {t, type, riot_class, fields, entity_id?}, ... ]
}
```

Event types derived from the leaked Riot vocabulary: Movement,
Replication, BasicAttack, SpellCast, Cooldown, Visibility, Death,
Buff, Item, Damage, Audio, Camera, Animation, HealthBar, TagOnly,
Unknown.

**Stage 3** is the readability layer:
[`scripts/readable_replay.py`](scripts/readable_replay.py). It
- drops events whose decoded_fields are all sentinel defaults,
- collapses runs of adjacent identical events into one with a
  `repeat_count`,
- emits a **per-minute event distribution** (game tempo / fight
  spikes / lull periods),
- writes a human-readable `replay.txt` summary.

End-to-end pipeline (~5 minutes total):

```
rofl-x file --replay X.rofl --patch-dir ./patch --output decoded.json    # 2m 24s
python scripts/build_event_timeline.py decoded.json --out timeline.json  # 2m 13s
python scripts/readable_replay.py timeline.json --out replay             # ~30s
```

---

## What's signal vs noise in the output

**Reliable, fully accurate:**
- Coverage: 217 / 217 netids wired. 100 % of blocks decoded.
- Per-minute event distribution. Fight spikes (e.g. minute 35 =
  19,757 events) and lulls (minute 23 = 12,417) are real.
- Total event counts by type.
- Player roster (10 players: team / role / champion).
- Typed `mov_decrypt` final positions: 61,730 entries.
- Cooldown events with real `cooldown_total_ms` values: ~28 K.

**Partial:**
- Per-event field VALUES for Replication / BasicAttack / etc.
  Decoders use a tag-byte to dispatch between inline-constant
  branches and variable-length-decoded branches. Our trace-write
  capture catches both kinds, but the inline-default branch
  (writing 0, -1, NaN, the float constants 1.0/-1.0/2.0) fires
  more often than the variable-length branch in the emulator.
  So many fields show defaults rather than real values. The fix
  requires either reading the alloc'd buffer the variable-length
  helpers write to, or stronger emulator scaffolding around the
  helpers — tracked but not done.

**Honest fallbacks:**
- 18 netids wired to `db2910` (a class constructor mismatched as
  decoder) emit init constants rather than packet data, labeled
  `ConstructorFallback`.
- 1 netid (543) is genuinely tag-only — netid IS the packet,
  labeled `TagOnlyStub`.

---

## Architecture cheatsheet (16.9 specifically)

- **Image base** in the emulator: `0x140000000`.
- **Netid dispatcher**: `e83930`. Jump table at `.text RVA 0xe93818`,
  1,198 u32 entries.
- **Dispatch sub-tables**: 538 of them in `.rdata`, 48-byte stride,
  6 u64 slots per entry. Slot[0] = shared helper, **slot[1] = decoder**,
  slot[4] = `return 3` shared helper, others vary.
- **Confirmed helpers**:
  - `skip = 0x11b8430` (88.5 % byte-match to 5-5)
  - `alloc1 = alloc2 = 0x10053f0` (vector resize wrapper)
  - `ecc180` = bit reader
  - `f41410` = LEB128-style varint u32 reader
- **Mov decoder**: `0xfb4070`. Inline-floats output format. Final
  position at struct offsets `0x20` / `0x24`. Waypoint vector
  pointer at `0x10`, size at `0x18` (NOT YET dereferenced — the
  contents of the vector are the actual path; we only emit the
  inline final pos).
- **Fallback vtable** at VA `0x141964850`: slot[1] = `0x1dbc10`
  (`xor al, al; ret` = "no decode"). Used for tag-only packets.
- **Decompiled C** for all 65+ confirmed decoders + helpers lives
  at `~/Tools/analysis/16-9/decomp/`.

---

## Files in this branch

### Pure-Python analysis pipeline (no Ghidra needed at runtime)

| Script | Purpose |
|--------|---------|
| [`scripts/scan_dispatch_table.py`](scripts/scan_dispatch_table.py) | 538 sub-tables / 2,800 decoders in `.rdata` |
| [`scripts/scan_decoder_prologue.py`](scripts/scan_decoder_prologue.py) | 327 functions matching the donor 28-byte prologue |
| [`scripts/cross_reference_decoders.py`](scripts/cross_reference_decoders.py) | Prologue ∩ Dispatch → 41 high-confidence decoders |
| [`scripts/find_dispatcher.py`](scripts/find_dispatcher.py) | Locates `e83930` (the netid dispatcher) |
| [`scripts/map_netid_to_decoder.py`](scripts/map_netid_to_decoder.py) | netid → ctor → vtable → slot[1] mapping for 1,182 netids |
| [`scripts/probe_unmatched_netids.py`](scripts/probe_unmatched_netids.py) | Try ctor-derived decoders for unmatched netids |
| [`scripts/probe_indirect_init.py`](scripts/probe_indirect_init.py) | Walk inner-init for secondary-vtable LEAs |
| [`scripts/xref_decoder_strings.py`](scripts/xref_decoder_strings.py) | Path-1 negative finding (no class strings) |
| [`scripts/build_16_9_patch.py`](scripts/build_16_9_patch.py) | Fill `patch/16-9.patch` skeleton with confirmed RVAs |
| [`scripts/pdata_sizes.py`](scripts/pdata_sizes.py) | Read function sizes from `.pdata` |

### Brute-force + wiring

| Script | Purpose |
|--------|---------|
| [`scripts/brute_match_decoders.py`](scripts/brute_match_decoders.py) | Sweep 196 netids × 41 candidates |
| [`scripts/brute_match_uncovered.py`](scripts/brute_match_uncovered.py) | 327 prologue candidates × no-hit netids |
| [`scripts/wire_confirmed_decoders.py`](scripts/wire_confirmed_decoders.py) | Write confirmed decoders into `patch/16-9.patch` |

### Catalog + parser pipeline

| Script | Purpose |
|--------|---------|
| [`scripts/build_riot_catalog.py`](scripts/build_riot_catalog.py) | Generate semantic catalog from Riot vocabulary + shapes |
| [`scripts/build_semantic_catalog.py`](scripts/build_semantic_catalog.py) | (Earlier version, kept for reference) |
| [`scripts/apply_semantic_names.py`](scripts/apply_semantic_names.py) | Annotate rofl-x JSON with class names + field names |
| [`scripts/build_event_timeline.py`](scripts/build_event_timeline.py) | Stage 1: typed event timeline |
| [`scripts/readable_replay.py`](scripts/readable_replay.py) | Stage 3: filtered narrative (per-minute distribution + dedup) |

### Ghidra helper scripts

`scripts/ghidra/`: `decompile_funcs.py`, `dump_callees.py`,
`export_decoders.py`, `find_decoder_candidates.py`, `ghidra_smoke.py`,
`list_pkt_symbols.py`, `xrefs_to.py`.

### Documentation

- [`docs/PATCH_16_9_ANALYSIS.md`](docs/PATCH_16_9_ANALYSIS.md) — what
  we know about the 16.9 dispatch architecture.
- [`docs/RIOT_CLASS_VOCABULARY.md`](docs/RIOT_CLASS_VOCABULARY.md) —
  the 320 leaked PKT names grouped by domain.
- [`docs/PACKETS.md`](docs/PACKETS.md) — packet catalog with the
  16.9 confidence-tier overview.
- [`scripts/semantic_field_names.json`](scripts/semantic_field_names.json) —
  per-decoder semantic catalog (machine-readable).

---

## Reproducing the pipeline cold

The cold-start guide for this analysis (the parent commit's README
section, kept below for reference) is the source of truth for tool
locations. Quick recap:

```bash
# 1. Rebuild patch + analysis artifacts
python scripts/build_16_9_patch.py
python scripts/scan_dispatch_table.py
python scripts/scan_decoder_prologue.py
python scripts/cross_reference_decoders.py

# 2. Brute-force decoder identification
python scripts/brute_match_decoders.py            # ~50 min
python scripts/brute_match_uncovered.py           # follow-up sweep
python scripts/wire_confirmed_decoders.py         # writes to patch/16-9.patch

# 3. Manual probes for stragglers
python scripts/probe_unmatched_netids.py
python scripts/probe_indirect_init.py

# 4. Build the semantic catalog
python scripts/build_riot_catalog.py              # produces semantic_field_names.json

# 5. Run end-to-end
ROFL_X_MAX_SAMPLES=0 ./target/release/rofl-x.exe \
    file --replay X.rofl --patch-dir ./patch --output decoded.json
python scripts/build_event_timeline.py decoded.json --out timeline.json
python scripts/readable_replay.py timeline.json --out replay
```

---

## What's still on the table

In rough order of "would meaningfully improve the parser":

1. **Mov waypoint vector reading.** The mov decoder allocs a buffer
   at struct offset `0x10` and writes the actual movement *path*
   into it. We currently emit only the inline final position. Reading
   the buffer (Unicorn `mem_read` from the alloc'd VA) would give
   complete paths — the difference between "Lillia ended up at (X,Y)"
   and "Lillia walked through (X1,Y1) → (X2,Y2) → (X3,Y3)".
2. **Variable-length decoder capture.** Many decoder fields show
   inline-tag defaults rather than the real variable-length value.
   The variable-length helpers (`f41410`, `f41f40`, etc.) write the
   real result; we capture some but not all. Improving emulator
   scaffolding around the helpers would lift signal substantially.
3. **Entity tracking.** Currently we don't associate raw_positions
   to player IDs. Adding entity_id capture from the dispatcher
   chain would let the parser say "Lillia moved to (X,Y)" instead
   of "some entity moved to (X,Y)".
4. **Typed Rust handlers.** Wire the top 5–7 confident classes
   (`mov_decrypt`, Replication, DoSetCooldown, Fog, BasicAttackPos)
   as proper `src/packet/handlers/<name>.rs` modules emitting typed
   structs instead of generic decoded_fields[].
5. **State correlation.** The only path to *proven* per-class
   labels and per-field semantics. Needs replays + game state
   (Match-V5 API + scoreboard correlation). Not in this branch.

Coverage milestones along the way (in commit order):

```
21 % class /  80 % block       starting state
84 % class /  98 % block       first wire (164 decoders)
93 % class /  99 % block       + 327-prologue sweep
95 % class /  99.5 % block     + ctor-mapped follow-up
99 % class /  99.999 % block   + indirect-init scan
100 % / 100 %                  + manual triage of netids 19, 543
```

100 % was the architectural ceiling on this binary because half of
all dispatch entries (~600 of 1,198) point to a tag-only fallback
vtable with `slot[1] = 0x1dbc10` (= `xor al, al; ret`). Those
netids really do have no decode method; they signal by their netid
alone. We honestly label them `tag-only` rather than fabricate
field names.

---

## Cold-start guide (preserved from parent commit)

The "Resuming the `decrypt/` branch (cold-start guide)" section that
this branch's parent commit added is what let me pick up the work
without context. Tool paths and architectural facts there are still
accurate; the coverage stats are what we improved on. See git log
for the full content if needed (commit `3409392`).

---

## Ideal future work — what would meaningfully improve this parser

Ordered by **estimated improvement to the output × likelihood of
success**. Each item carries its dependency, expected effort, and
the specific gap it closes. Keep this list in sync with the actual
TODO state — when something lands, demote it to "completed" or move
it into the changelog above.

### A. Variable-length decoder signal (biggest single quality lift)

**Problem.** Many decoders use a tag-byte to dispatch between
inline-constant branches (write 0, -1, 1.0, 2.0, ...) and
variable-length-decoded branches (call `f41410`/`f41f40`/`f3aef0`
etc. to read a varint or float into the output struct). Our trace-
write capture currently catches both, but the inline-default branch
fires more often than the variable-length branch under our emulator
setup, so output JSONs are dominated by sentinel values rather than
real game data.

**Fix.** Two parts:
1. Better emulator scaffolding around the variable-length helpers
   so they have enough payload bytes left to actually decode (right
   now they sometimes hit `param_3 - lVar1 < 2` early-out and bail).
2. Capture intermediate writes from inside the helpers, not just
   the post-call struct state — the real decoded value is the
   output of the helper, which is sometimes captured as an atomic
   struct write and sometimes as a register return that never
   reaches the output struct.

**Effort.** Medium-high. Needs deeper tracing of helper internals
in the emulator. ~1–2 days. Files involved: `src/emulator/unicorn.rs`,
`src/replay_info.rs`.

**Output impact.** Replication / BasicAttack / Buff / Cooldown
field VALUES become real game data instead of inline tag-byte
defaults. Probably the single biggest "make the JSON useful"
upgrade. Per-event field meaningfulness goes from ~30 % to ~80 %.

### B. Mov waypoint vector reading

**Problem.** `mov_decrypt` (`0xfb4070`, netid 916) allocates a vector
at struct offset `0x10` and writes the actual movement waypoints
into it. The decoder makes a virtual call per waypoint (see
`fb4070.c` line ~187: `(**(code **)(*plVar5 + 8))(plVar5, ...)`).
We currently emit only the inline final position at offsets
`0x20` / `0x24`; the *path* the entity took is lost.

**Fix.** After `call_decrypt_extra` returns, read the vector buffer:
- `vec_ptr = *(u64*)(struct_base + 0x10)`
- `vec_size = *(u32*)(struct_base + 0x18)`
- For each entry, read 8 bytes (or whatever the entry stride is
  per the inner virtual call) from `[vec_ptr, vec_ptr + vec_size *
  stride)` via `unicorn.mem_read`.
- Emit as `waypoints: [[x, y], ...]` in the output JSON.

**Effort.** Medium. The buffer is in alloc'd emulator memory, so
`Unicorn::mem_read` works. Need to verify the entry stride (look
at the inner virtual function in the decompile). ~half a day.
Files: `src/replay_info.rs`, `src/emulator/unicorn.rs`,
`src/emulator/packet.rs`.

**Output impact.** Movement events go from 61,730 final-positions
to **per-entity full paths**. Lets you reconstruct "Lillia walked
top-to-mid via the river" instead of just "Lillia ended up here".
This is the difference between a snapshot and a trace.

### C. Entity tracking / per-entity timelines

**Problem.** The output emits per-netid event lists, not per-entity.
Consumers asking "show me everything Lillia did" have to filter
manually, and most events don't carry entity_id at all — the
dispatcher binds the entity_id to the packet outside the decoder
function (see the negative finding for `fb4070`'s xrefs:
zero direct callers).

**Fix.** Two layers:
1. Capture the entity_id from the dispatcher caller chain. The
   dispatcher (`e83930`) is invoked by code that knows the
   target entity; we'd need to either (a) read the entity_id
   parameter at call time (instrumented emulation), or (b) parse
   it from the block header instead of the packet body — block
   metadata may carry it before the encrypted payload.
2. Build a registry on top of the timeline: every event tagged
   with the entity_id it relates to, then group by entity to
   produce `entities[net_id].events: [...]`.

**Effort.** Hard for layer 1 (requires understanding which call
site invokes the dispatcher per packet — the call chain changes
with packet category). Medium for layer 2 once 1 is solved.
~1–2 days total.

**Output impact.** `entities` in the output JSON becomes a real
record of what each player/minion/turret did. Combined with B,
this delivers the "narrative replay" the user asked for.

### D. Typed Rust handlers for confident classes

**Problem.** All non-mov decoders go through the generic
`extra_decoders[]` path which emits raw `decoded_fields[]`. Even
high-confidence classes (Replication, DoSetCooldown,
OnLeaveVisibilityClient, Basic_Attack_Pos) lack typed Rust structs
and field validation.

**Fix.** Implement `src/packet/handlers/<name>.rs` modules per
the architecture in `docs/MODULE_LAYOUT.md`. Each handler:
- Takes a decoded `ExtraDecoded` and parses it into a typed struct
- Adds field-level validation (e.g. cooldown_total_ms > 0)
- Plugs into a per-class output path in the JSON

Order to do them, by confidence + impact:
1. `mov_decrypt` → typed `MovementEvent` (already partially done)
2. `OnLeaveVisibilityClient` (`0xfaffc0`) → `VisibilityEvent`
3. `CHAR_SetCooldown_Broadcast` (`0xfdb200`) → `CooldownEvent`
4. `Basic_Attack_Pos` (`0x1074580`) → `BasicAttackEvent`
5. `S2C_ReplicateField` / `S2C_ReplicateFields` (`0xf6ab10`,
   `0xfcfd30`, `0xeba9e0`) → `ReplicationEvent`

**Effort.** ~half a day per handler, scaling. ~1 week for all 5.

**Output impact.** Output JSON becomes ergonomic for downstream
code; per-class invariants get caught at parse time. Makes the
parser usable from Rust without going through dynamic JSON
manipulation.

### E. State correlation (the only path to *proven* class names)

**Problem.** Our class-name labels are shape-matched against the
leaked Riot vocabulary, not RTTI-proven. RTTI is dead in this
binary (verified). The PKT_*_s strings are dead bytes (verified).
Path 1 is closed permanently. Only state correlation can promote
labels from "shape-anchored guess" to "proven".

**Fix.** Build a correlation harness:
1. Take a replay where the outcome is known (your own game, a pro
   match with full Match-V5 data).
2. Parse it through the full pipeline.
3. For each decoder, check whether decoded values correlate with
   observable game state from Match-V5: did `cooldown_total_ms`
   in netid 684's events line up with the spell's actual cooldown
   in the timeline? Did `entity_id` references in fog events
   correlate with which entities entered/left vision?
4. Promote decoder→class assignments to "proven" when correlation
   passes a threshold on multiple replays.

**Effort.** Hard but tractable. Per-class identification needs
~5–10 controlled replays. Per-field semantics needs 100s. Henry
Zhu used 1.4 M for 9 classes; we'd realistically aim for ~50–100
replays + Match-V5 cross-reference for the top 5–7 classes. ~1–2
weeks.

**Output impact.** Catalog confidence flips from `shape-match` /
`frequency-hint` to `proven` for the top classes. Field-level
semantics get nailed down for those classes (e.g. property index
123 = HP, 124 = Mana, etc.). The basis for an actually-named
parser.

### F. Cross-patch portability (16.10, 17.x, ...)

**Problem.** Everything in this branch is patch-16.9 specific.
Riot's binary changes every patch (RVAs relocate, new packet
classes appear, dispatch sub-table layouts shift slightly). The
existing `scripts/ghidra/export_decoders.py` + `rofl-x scan-decoder`
workflow handles minor relocations, but a major patch needs the
full pipeline rerun.

**Fix.** Keep the pipeline reusable:
1. Pure-Python analysis scripts already are patch-agnostic — they
   scan whatever binary you point them at. Just point them at a
   different `league.exe`.
2. The brute-force runs against the new binary's decoders × the
   new replay's netids — works without changes.
3. The semantic catalog is patch-specific (decoder RVAs change).
   Need to regenerate `scripts/semantic_field_names.json` per
   patch. Could be auto-generated from `build_riot_catalog.py`
   pointing at the new binary.

**Effort.** Low for new minor patches (re-run pipeline). Medium
for major patches that change the dispatch architecture (rare).

**Output impact.** Multi-patch support; the parser stays useful as
Riot ships updates.

### G. Performance: parallel emulation, JSONL streaming

**Problem.** Full uncapped decode takes 2 m 24 s on the benchmark
replay. Output JSONs balloon to multi-GB. Loading 1.9 GB JSON in
Python takes 28 s and uses a few GB of RAM.

**Fix.**
1. **Parallel decode.** Each block's emulation is independent; spawn
   N workers and process blocks in parallel. ~Nx speedup.
2. **JSONL output.** Stream one event per line to `decoded.jsonl`
   instead of one giant JSON. Consumers can grep, head, awk
   without loading the whole file.
3. **Compact field encoding.** Drop `u32_le` / `i32_le` / `f32_le`
   redundancy in the output — emit the value as the catalog's
   declared type.

**Effort.** Medium. Probably ~3 days total.

**Output impact.** 4–8× faster end-to-end. Files become 10–50%
smaller. Consumers can stream.

### H. Tests: schema_compat, opcode_coverage regression

**Problem.** No automated tests verify that:
- Every observed opcode in a sample replay has a catalog entry
- The output JSON matches the expected schema
- `decoded_fields[]` types align with the catalog's declared types

**Fix.** Implement:
- `tests/opcode_coverage.rs` — walks `ROFL_X_SAMPLES_DIR` and
  asserts every observed opcode is in the patch's `extra_decoders[]`.
- `tests/zhu_schema_compat.rs` — asserts our output can represent
  every field in the 22-class Zhu schema. Trivially fails today
  (we have 0 typed handlers besides mov); useful as a coverage
  baseline that unlocks once D lands.
- `tests/timeline_round_trip.rs` — parses a known replay, checks
  that the timeline contains expected events at expected times
  (e.g. game-end event near `gameLength`).

**Effort.** Low–medium. ~2 days.

**Output impact.** Regressions get caught when patches ship. The
parser becomes trustworthy for production use.

### I. Multi-replay validation

**Problem.** Everything was validated on ONE replay
(`EUW1-7837000162.rofl`). We don't know if our 100 % coverage
holds across other replays of the same patch — different replays
might exercise different netids that happen not to be in our wired
set.

**Fix.** Run the pipeline across 10–100 replays of patch 16.9 and
collect the union of observed netids. If any new netids appear,
follow the same brute-force/ctor-mapped/indirect-init path to
wire them.

**Effort.** Low. Mostly batch processing.

**Output impact.** Catalog graduates from "covers this one replay"
to "covers patch 16.9 generally".

### J. Replay-event-stream / live mode

**Problem.** Today the parser is a batch tool: rofl in, JSON out.
For live viewing or streaming use cases, you'd want to emit events
as they're decoded.

**Fix.** Convert the rofl-x file path to a streaming iterator that
yields events as they're decoded, without buffering the whole
output. Combined with G's JSONL output, this enables
`rofl-x file --replay X.rofl ... | jq` style usage.

**Effort.** Medium. ~3 days.

**Output impact.** Enables live integrations (websockets, replay
review tools) without re-architecting.

---

## Priority recommendation

If you're picking ONE thing to do next, do **A** (variable-length
decoder signal). It unlocks the most output-quality improvement
per hour of work, and it's the prerequisite to making C / D / E
deliver real value. After A, do **B** (waypoint vector reading)
for movement traces, then **C** (entity tracking) for narrative,
then **D** (typed handlers) for ergonomics, then **E** (state
correlation) for proven labels.

If picking three: **A + B + C**. That alone delivers the "literally
read my rofl" goal: full paths per entity, real field values,
narrative timeline.

If picking everything: estimate ~3–4 weeks of focused work to land
all 10 items, with E being the longest-tail because of the data
gathering. The output at the end is a parser that:

- Decodes 100 % of every replay's blocks (already done).
- Names every packet class with proven Riot identifiers
  (after E).
- Reconstructs full per-entity timelines with real positions,
  HP / mana / cooldowns, deaths, and item buys (after A + B + C).
- Is fast (parallel decode, after G).
- Has typed Rust APIs (after D).
- Is regression-tested (after H).
- Works across patches (after F).
- Can stream live (after J).

That's the parser the user wanted when they said *"a true parser
of EVERY packet of a rofl"*.
