# ROFL-X

A documented parser for League of Legends replay files (`.rofl`), written in
Rust. Extends [Mowokuma's ROFL](https://github.com/Mowokuma/ROFL) (an
already-excellent parser) and pairs it with a **written packet catalog** so
that patch-to-patch regressions become detectable instead of silent.

ROFL-X exists because Riot publishes no official documentation of the ROFL
format, and every existing parser extracts a narrow slice of the data that
is actually in the file. Our goal is a parser that covers as much as the
community's reverse-engineering knowledge allows, and a catalog that honestly
records the rest as `UNKNOWN` so it's obvious what we haven't figured out yet.

---

## Credit where it's very much due

This project is only possible because two people did extraordinary work
first. Both deserve loud, specific thanks.

### Mowokuma ([@Mowokuma](https://github.com/Mowokuma)): [Mowokuma/ROFL](https://github.com/Mowokuma/ROFL)

Mowokuma shipped the Rust parser that ROFL-X builds on. It's small (~500
lines of Rust), archived, and quietly brilliant:

- She **sidestepped reverse-engineering the decode logic entirely** by
  spinning up a [Unicorn](https://www.unicorn-engine.org/) x86-64 VM,
  mapping the League client's `.text` / `.data` / `.rdata` sections into it,
  and **calling the game's own decode functions** on each packet payload.
  The obfuscation tables change every patch; her answer is "don't port the
  decoder, port a call-site."
- She reverse-engineered specific per-patch RVAs and struct-field offsets,
  stubbed the client's allocators with **her own hand-rolled x86-64
  shellcode** (a bump allocator, 91 bytes), and patched the one safety
  check that would otherwise abort emulation.
- She worked out the block-framing delta-encoding (marker byte +
  timestamp/packet_id/param bitfields) and the chunk + zstd pipeline
  cleanly enough that our hex walkthrough of a real 2026 patch-16.8 replay
  validates against her code with zero block-parse errors across 1.9 million
  blocks.
- She pinned the project when it was working, archived it transparently,
  and made it available to build on. Our catalog, tests, and new packet
  handlers all stand on top of what she figured out.

Her approach, her code, and her willingness to publish it are the reason
ROFL-X has a starting line. Thank you, @Mowokuma.

### Henry Zhu ([@maknee](https://github.com/maknee)): ["League of Legends data scraping the hard and tedious way for fun"](https://maknee.github.io/blog/2025/League-Data-Scraping/)

Henry Zhu's 2025 write-up is the most detailed public explanation of how
ROFL-level reverse engineering actually works. He took Mowokuma's core idea
(let the game decode itself) and pushed it further:

- He named and explained the **decrypt-access-release pattern**: the game
  decrypts a field, uses it, re-encrypts it, and zeroes the plaintext.
  Anti-RE on purpose. Knowing this is named and intentional reframes the
  whole decoding problem.
- He mapped the client's **three-stage packet lifecycle** (`Packet::Packet`
  → `DeserializePacket` → `UsePacket`) and tied each stage to what a parser
  like ours can and can't observe.
- He implemented a **second, surgical emulator strategy**: INT3 breakpoints
  plus exception handlers that read values directly out of CPU registers at
  the moment the game computes them, and showed the trade-offs against
  whole-function emulation.
- He traced and documented **nine distinct packet classes** (damage, spell
  cast, basic attack, summoner creation, entity creation, state update,
  death, fog-of-war on/off) end-to-end. ROFL-X's initial catalog starts
  from this list.
- He then did the extraordinary thing of **running his parser over 1.4
  million+ replays and releasing the decoded output as two public
  [Hugging Face datasets](https://huggingface.co/datasets/maknee/league-of-legends-decoded-replay-packets).**
  That is a generational-level community contribution. Nobody else has
  made semantic packet data at that scale publicly available.

If you want to understand *why* this is hard, read his post. Thank you,
@maknee. ROFL-X would not have a packet catalog worth writing without your
work.

### Further prior art also worth your time

- [@fraxiinus](https://github.com/fraxiinus), [fraxiinus/roflxd](https://github.com/fraxiinus/roflxd):
  an umbrella of ROFL parsers across languages; a useful cross-check that
  Riot's obfuscation really does change per patch.
- [@robertabcd](https://github.com/robertabcd), `lol-ob`: older Ruby work
  on Blowfish decryption of chunk/keyframe data from an earlier era of the
  format.

---

## Status

**Phase 1 (reconnaissance): complete.** Mowokuma's source, Henry Zhu's
write-up, and a byte-level walkthrough of a real replay have been read,
summarised, and cross-checked.

**Phase 2 (domain model + format spec): in progress.**

See the docs below for specifics.

---

## Documentation

Read in this order:

| Doc | What it's for |
|-----|---------------|
| [docs/REFERENCE_MOWOKUMA.md](docs/REFERENCE_MOWOKUMA.md) | Module-by-module summary of Mowokuma's Rust parser, with corrections from our byte-level audit |
| [docs/REFERENCE_HENRY_ZHU.md](docs/REFERENCE_HENRY_ZHU.md) | Summary of Henry Zhu's write-up, techniques, named packet classes, dataset links |
| [docs/SAMPLE_A_WALKTHROUGH.md](docs/SAMPLE_A_WALKTHROUGH.md) | Annotated hex of a real patch-16.8 replay, with an opcode histogram across 1.9M blocks |
| [docs/DOMAIN.md](docs/DOMAIN.md) | Entities and lifecycles, the shape of the thing we're parsing |
| [docs/ROFL_FORMAT.md](docs/ROFL_FORMAT.md) | Byte-level format spec, every claim cited to evidence |
| [docs/MODULE_LAYOUT.md](docs/MODULE_LAYOUT.md) | Proposed Rust crate layout for Phase 3 onwards |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Phase-by-phase project plan with stop points |
| [docs/PACKETS.md](docs/PACKETS.md) | Packet catalog, one entry per opcode, including `UNKNOWN` |
| [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) | Per-patch coverage table |
| [docs/RE_PATCH.md](docs/RE_PATCH.md) | How to build a `.patch` archive for a new League patch |
| [docs/DATASETS.md](docs/DATASETS.md) | Fetch + profile Henry Zhu's public Hugging Face corpus |

---

## Honesty > completeness

A parser that silently misses packets after a patch is worse than no parser.
ROFL-X's catalog records three statuses (`DOCUMENTED`, `PARTIAL`,
`OBSERVED-ONLY`, `UNKNOWN`) and the test harness treats the appearance of
an undocumented opcode as a hard failure, so "we don't yet know what
opcode 0x0278 does" is a first-class thing the project is designed to
surface, not hide.

---

## Running the reproducer

Phase 1's sample walkthrough is reproducible:

```bash
python scripts/sample_walkthrough.py
```

Requires Python 3.10+ and the `zstandard` package. The script reads one
local `.rofl` file (path hard-coded at the top; never commits the replay)
and prints the annotated structure used in `docs/SAMPLE_A_WALKTHROUGH.md`.

---

## License

MIT, with a formal attribution section in the [LICENSE](LICENSE) file pointing
to [CREDITS.md](CREDITS.md).

Mowokuma's upstream ships with no LICENSE file, which would ordinarily make
its status "all rights reserved". She has confirmed (April 2026) that she is
happy for ROFL-X to port and extend her work on the condition that she is
credited, which CREDITS.md does in detail.

---

## TODO

Concrete work that's been scoped and triaged but not done. Ordered by
how much it unblocks downstream.

### Reverse-engineering (needs a disassembler)

- [ ] **Find the netid-to-class mapping table** in Mowokuma's 5-5
  binary. The class-descriptor table is known (see
  [docs/RE_PATCH.md](docs/RE_PATCH.md) for its layout at image RVA
  `0x169b000`, ~500 entries at 48-byte stride), but there is no netid
  field inside an entry. The packet-deserialise dispatcher must
  consult a separate structure. Finding this is the single biggest
  unblocker for the catalog: once we can map "netid N" to "class
  descriptor entry K", every decoder RVA becomes accessible.
- [ ] **Port `CreateHero`, `HeroDie`, `CreateTurret`**, three of the
  simplest Zhu classes (2-4 fields each). Workflow:
  Ghidra session → identify decoder RVA → feed into
  `rofl-x trace-decoder` → add handler + fixture + test →
  promote from `OBSERVED-ONLY` to `DOCUMENTED` in
  [docs/PACKETS.md](docs/PACKETS.md).
- [ ] **Port `Replication`** (Zhu's per-entity stat-update packet,
  15.8 % of all packets in S12). Harder than the `Hero*` ones because
  the payload is a variable-length dict of field updates, but this is
  the packet that carries HP, movement speed, and every other stat
  the viewer needs to render health bars and grey-on-death.
- [ ] **Port `UnitApplyDamage`** and **`NPCDieMapView`**. Together
  these give us a kill feed.
- [ ] **Build a 16.8 `.patch` archive.** Skeleton exists at
  `reference/patch-16-8-skeleton.patch`; the RVAs marked `NEEDS_RE`
  in its `result.json` need filling in against the current League
  binary. Full recipe in [docs/RE_PATCH.md](docs/RE_PATCH.md).

### Parser and tooling

- [ ] **Opcode-coverage regression test.** `tests/opcode_coverage.rs`
  walking `ROFL_X_SAMPLES_DIR` and asserting every observed opcode is
  in the catalog. Trips loudly when a new patch introduces a new
  opcode.
- [ ] **`tests/zhu_schema_compat.rs`**, a strict-comparison test
  asserting our output can represent every field in Zhu's 22-class
  schema. Will trivially fail while the catalog has 2 `DOCUMENTED`
  classes; useful as a coverage baseline that unlocks once more
  classes are in.
- [x] **Cross-patch decoder byte-pattern matching.** Done:
  `scripts/ghidra/export_decoders.py` exports anchor patterns from a
  labeled donor binary; `rofl-x scan-decoder` finds candidate RVA
  ranges in a target patch by anchor-clustering. Workflow documented
  in [docs/RE_PATCH.md](docs/RE_PATCH.md) § "Cross-patch byte-pattern
  porting". Not a replacement for RE on rewrites; accelerator for
  consecutive patches.
- [x] **Per-decoder scaffolding tools.** Done: `rofl-x extract-fixture`
  pulls raw payloads from a `.rofl` by netid; `rofl-x new-handler`
  scaffolds the test stub and `docs/PACKETS.md` row. The handler
  source file under `src/` stays manual (StubEmulator integration
  shape is per-decoder). Workflow in
  [docs/RE_PATCH.md](docs/RE_PATCH.md) § "Per-decoder workflow
  tooling".

### Viewer

- [ ] **Hardcoded SR turret positions on the map** (22 positions,
  static, no decoder work needed). Visual improvement independent of
  any RE.
- [ ] **Ward icons from Data Dragon items** (3340 Yellow, 2055
  Control, 3363 Blue Trinket). No decoder work needed.
- [ ] **Team sidebars** showing each player's champion icon and role.
  Data is already in the output JSON.
- [ ] **Simulated fog of war** with a Blue/Red/Both POV toggle,
  computed from ally player positions + active wards. No decoder
  work needed.
- [ ] **HP bars and grey-on-death**. Blocked on `Replication` and
  `HeroDie` decoders.
- [ ] **Kill feed sidebar**. Blocked on `UnitApplyDamage` +
  `NPCDieMapView` / `HeroDie` decoders.

### Parity

- [ ] **Parity runs against Mowokuma's binary on 15.1-15.4**.
  Currently only 15.5 has been measured (99.07 % content match).
  Cheap once a small shell script runs both tools and diffs the
  order-normalised JSON.

---

## Resuming the `decrypt/` branch (cold-start guide)

This section is the survival kit for picking up the `decrypt/`-prefixed
branches with **zero session memory**. Read end-to-end before resuming.

### What this branch line is

`decrypt/tooling-and-16-9-recon` is the active line of work pushing
ROFL-X past the two-decoder Mowokuma baseline toward "every packet
decoded on every patch we care about". As of the last push:

- 8 commits ahead of `main`. Latest: `6b8b065`.
- Remote: `https://github.com/Toastaspiring/ROFL-X.git`
- The first commit (`748ee3c`) added the per-decoder workflow tooling
  (`scan-decoder`, `extract-fixture`, `new-handler`, Ghidra exporter)
  documented in [docs/RE_PATCH.md](docs/RE_PATCH.md). Everything
  after that is 16.9 reverse-engineering.

### Coverage right now (16.9, single replay benchmark)

Three honest layers — don't conflate them:

| Layer | Coverage | What it means |
|-------|----------|---------------|
| (1) Class identified (RVA known) | **164 / 195 (84.1%)** | We know which function in the binary decodes this netid |
| (2) Block-level decoder runs | **98.4%** | The emulator runs the decoder for this many of the replay's blocks |
| (3) Field plaintext extracted | **98.4% (100% per-class success)** | `decoded_fields[]` produced — pre-obfuscation u32/i32/f32 values surfaced in the JSON |
| (4) Class semantically named | 8 / 195 hypothesised | Best-effort mapping to Zhu's class names from frequency + payload shape |
| (5) Field-level semantics | 0 / 195 | "this u32 is HP not gold" — no shortcut, manual per-class |

Avg samples per class: **25.4**. Avg fields per sample: **5.1**
(median 4, max 15).

The top-by-frequency netids dominate. 31 netids remain unmatched
(~1.6 % of total blocks, ~27 K out of 1.7 M); 22 of them have zero
hits across the prologue-AND-dispatch-table 41-candidate sweep,
meaning their decoders are NOT in the prologue+dispatch intersection
and require the wider 327-prologue-only sweep (or external
identification) to capture.

### Major architectural breakthrough: netid dispatcher located

`scripts/find_dispatcher.py` + Ghidra decompile of `0xe83930` revealed
the **netid jump-table dispatcher**: a u32 jump table at .text
RVA `0xe93818` (1198 entries, one per netid in `[0..0x4ae)`). The
prologue of `0xe83930` reads `*(u32*)(table + netid*4)` and
indirect-jumps. Each jump target is a per-class **constructor** that
allocates an instance and writes a vtable pointer — the vtable is one
of the 538 dispatch sub-tables in `.rdata` and slot[1] of that vtable
is the per-class decoder.

Caveat: `scripts/map_netid_to_decoder.py` follows the constructor
chain and resolves 1182/1198 netids, but the picked vtable is often
the *base-class* one (LEA scan limitation across multi-vtable
hierarchies). Brute-force matching remains the source of truth for
crisp per-netid → decoder mappings.

### What's installed on the dev machine

ROFL-X targets reverse-engineering a Windows binary, so the toolchain
lives outside the repo. From a fresh shell:

| Path | Contents |
|------|----------|
| `~/Tools/ghidra/ghidra_12.0.4_PUBLIC/` | Ghidra. Desktop shortcut "Ghidra" launches it. Needs JDK 21 (Eclipse Temurin, also installed). |
| `~/Tools/donors/mowokuma-5-5/` | Mowokuma's release: her `ROFL.exe` and `5-1.patch` ... `5-5.patch` |
| `~/Tools/donors/5-5-extracted/` | 5-5.patch unzipped (text.bin, data.bin, rdata.bin, result.json) |
| `~/Tools/donors/donors-15-5.json` | Output of the Ghidra exporter run on Mowokuma's binary — `scan-decoder`'s donor input |
| `~/Tools/analysis/16-9/league_16-9.exe` | Copy of the live League binary (16.9.771.8383, captured 2026-04-30) |
| `~/Tools/analysis/16-9-project/` | Ghidra project — analysis already complete (~24 min run). Reuse with `analyzeHeadless -process league_16-9.exe -noanalysis`. |
| `~/Tools/analysis/16-9/decomp/` | Decompiled C of every confirmed decoder, plus the helpers (skip, alloc1, etc.) |
| `~/Tools/analysis/16-9/brute_match_results.json` | Latest brute-force results (60 netids x 41 candidates) |
| `~/Tools/analysis/16-9/decoders_confirmed.json` | The 41 high-confidence decoder candidates (prologue match + dispatch table) |
| `patch/5-5.patch` | Mowokuma's 15.5 archive, staged for `rofl-x file --patch-dir ./patch` |
| `patch/16-9.patch` | Our 16.9 archive with 39 wired `extra_decoders` + the mov_decrypt + ward_spawn slots |

### Critical 16.9 architecture facts (don't relearn from scratch)

Documented in detail in [docs/PATCH_16_9_ANALYSIS.md](docs/PATCH_16_9_ANALYSIS.md). TL;DR:

- **Dispatch table is in `.rdata`**, fragmented into 538 sub-tables.
  Each entry is **48 bytes (6 u64s)**, stored as full virtual
  addresses (image_base + RVA), NOT raw RVAs like 5-5 used.
- **Slot layout rotated** since 5-5: now slot[0] is a shared
  destructor stub (`ret 0` at RVA `0x1dc000`), **slot[1] is the
  decoder**, slot[4] is the `return 3` helper at `0x2173a0`
  (byte-identical to 5-5's helper at `0x1c74c0`).
- **Confirmed helpers in 16.9**: `skip = 0x11b8430` (88.5% byte
  match to 5-5's skip), `alloc1 = 0x10053f0` (vector resize, signature
  matches Mowokuma's stub), `alloc2 = 0x10053f0` (same — stubbed
  identically).
- **Image base** in the emulator: `0x140000000` (PE OptionalHeader),
  not the runtime VA `0x7ff76afd0000` Mowokuma's port hardcodes —
  her shellcode encodes the latter so we kept that.
- **Heap bumped to 1 MiB** (was 8 KiB). Some 16.9 decoders allocate
  more than the original heap could hold.
- **Decoded values are RE-OBFUSCATED in the post-call struct**.
  Plaintext lives only briefly during decode. Capture it via the
  LAST atomic write per offset (size ≥ 4) — `replay_info.rs` does
  this and emits `decoded_fields[]` per sample.

### Active TODO (priority ordered)

#### Layer 1/2 push (mechanizable)

- [ ] **Run the long-tail brute-force** to push class identification
  toward 100%. `scripts/brute_match_decoders.py` already has all 196
  netids. ~2 hr wall clock; +2.2% block coverage max. Easy overnight
  job.
  ```bash
  python scripts/brute_match_decoders.py
  python scripts/wire_confirmed_decoders.py   # post-process + write 16-9.patch
  cargo build --release
  ./target/release/rofl-x.exe file --replay <r.rofl> --patch-dir ./patch --output out.json
  ```
- [ ] **Tighten the noise filter** in
  `scripts/wire_confirmed_decoders.py`. Current: drops RVAs that win
  for ≥ 3 different netids. Misses cases where a "shared base
  deserializer" wins for several legitimate variants. Better
  heuristic: gap-to-runner-up > 3 AND distinct_offsets in [3, 30]
  range (real decoders, not heartbeat-style 1-field nor 70-write
  loud functions).

#### Layer 4/5 push (semantic, manual)

- [ ] **Find entity_id source for 16.9 mov_decrypt (`0xfb4070`)**.
  fb4070 doesn't write entity_id to the output struct — the caller
  does. Check `~/Tools/analysis/16-9/decomp/fb4070.c`'s callers via
  `scripts/ghidra/xrefs_to.py 0xfb4070` (already produced
  `xrefs_fb4070.json`: 2 data refs, both to dispatch-table slots).
  The CODE callers of those slots are the dispatcher we want.
  Closes layer-5 for the mov class.
- [ ] **Per-class field naming** for the top 8 classes already
  hypothesised. Decompiled C is in `~/Tools/analysis/16-9/decomp/`.
  Per class (~30 min): read the C, identify what each `param_1+0xN =
  value` means, add a typed Rust struct in `src/emulator/packet.rs`,
  wire it into `replay_info::parse_and_decode`. Documented pattern
  in [docs/ADDING_DECODERS.md](docs/ADDING_DECODERS.md).
- [ ] **Decompile `FUN_140f41410`** and the other variable-length
  helpers. fb4070 calls `f41410` for tags 2/4/5 — those are the
  variable-length read paths that decode actual entity values
  (id, time, etc.). Without understanding `f41410` we can't see what
  values those tags carry.

#### Architectural

- [ ] **`extra_decoders[]` schema is a stopgap**. The clean design
  per [docs/MODULE_LAYOUT.md](docs/MODULE_LAYOUT.md) is the
  one-file-per-handler `src/packet/handlers/<name>.rs` registry.
  When a class graduates from "raw struct dump" to "typed semantics",
  it should leave the `extra_decoders` array and become a real
  handler. The framework is ready; just hasn't been done for any
  16.9 class yet.
- [ ] **Find the netid → table-index mapping**. Still unsolved.
  Would replace brute-force matching entirely. Look for a function
  that takes a u16/u32 netid as arg, indexes into one of the 538
  sub-tables. `xrefs_to.py` against the table base addresses is the
  starting point.

### Common commands

```bash
# Inspect any replay's full opcode histogram
./target/release/rofl-x.exe inspect --replay <r.rofl> --histogram

# Decode a 16.9 replay end-to-end (uses patch/16-9.patch)
./target/release/rofl-x.exe file --replay <r.rofl> --patch-dir ./patch --output out.json

# Run trace-decoder on a candidate decoder (probe its struct writes)
./target/release/rofl-x.exe trace-decoder --replay <r.rofl> --netid <N> \
    --patch-dir ./patch --rva-start 0x... --rva-end 0x... --samples 5

# Pull payload fixtures for a netid
./target/release/rofl-x.exe extract-fixture --replay <r.rofl> --netid <N> \
    --name foo --count 5 --out-dir tests/fixtures

# Decompile arbitrary RVAs in 16.9 via Ghidra (fast — reuses analyzed project)
$GHIDRA/support/analyzeHeadless.bat ~/Tools/analysis/16-9-project league_169 \
    -process league_16-9.exe -noanalysis \
    -scriptPath ./scripts/ghidra \
    -postScript decompile_funcs.py ~/Tools/analysis/16-9/decomp 0xRVA1 0xRVA2 ...

# Brute-force match netids to candidate decoders
python scripts/brute_match_decoders.py            # ~2 hr for 196 netids
python scripts/wire_confirmed_decoders.py         # filter + auto-write to 16-9.patch
```

### Honest gotchas

- **Some "winning" decoders are shared deserializers**. `0xeba9e0`
  (filtered out as noise) wins for many netids because it writes to
  ~70 offsets unconditionally. `0xf6ab10`, `0xf8f840` are similar.
  After filtering them, the runner-ups (e.g. `0xf9bae0` winning for
  9 netids) are themselves likely real shared decoders for a class
  hierarchy — they DO carry signal, just for multiple variants
  routed by netid internally.
- **decoded_fields[]'s f32 column is sometimes garbage**. We surface
  every interpretation (u32/i32/f32) of every captured 4-byte value;
  if the field is actually an int, the float interpretation will
  often be NaN or absurdly large. Pick the right column per offset
  per class.
- **5-5 retro compat is preserved**. Mowokuma's `5-5.patch` archive
  has no `output_format` field; the Rust side defaults to
  `buffer-stream` and her schema parses unchanged. Don't break this
  when extending the schema.
- **The Ghidra script directory must be added once per Ghidra
  project**. In the Script Manager, right-click → Script Directories
  → add the worktree's `scripts/ghidra` path. Then headless runs
  pick it up via `-scriptPath`.

### When in doubt

Read in this order:

1. [docs/PATCH_16_9_ANALYSIS.md](docs/PATCH_16_9_ANALYSIS.md) — what we know about 16.9 specifically
2. [docs/ADDING_DECODERS.md](docs/ADDING_DECODERS.md) — how to add a new packet class
3. [docs/RE_PATCH.md](docs/RE_PATCH.md) — the cross-patch porting workflow
4. [docs/PACKETS.md](docs/PACKETS.md) — the catalog (still mostly OBSERVED-ONLY for 16.9)
5. The decompiled C in `~/Tools/analysis/16-9/decomp/` — concrete ground truth per decoder

---

## Non-goals

- **We will not publish a decoded-replay dataset.** Henry Zhu's Hugging Face
  drops already exist at a scale no fork could realistically match. ROFL-X's
  contribution is the *documented parser*, not the data.
- **We will not ship anti-cheat bypasses.** The parser operates strictly on
  files the user already has on their own disk. If a direction of work
  starts to look like live-game decryption or client-side tamper, we stop
  and flag it.
- **We will not attempt to statically reimplement the packet decode
  functions.** The obfuscation changes per patch and Mowokuma/Zhu both
  independently concluded emulation is the right answer; we agree.
