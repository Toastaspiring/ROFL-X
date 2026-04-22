# Proposed Rust module layout

Maps the domain model in [DOMAIN.md](DOMAIN.md) onto a Rust crate
structure. One-liner per module plus the rationale for the shape.

Nothing here is written yet. This is a proposal for Phase 3 to build
against; changes before code lands are free.

---

## Tree

```
ROFL-X/
├── Cargo.toml
├── src/
│   ├── lib.rs                       public crate surface; re-exports
│   ├── main.rs                      thin entrypoint; calls cli::run()
│   ├── cli.rs                       clap CLI: file / audit / dump subcommands
│   ├── error.rs                     thiserror RoflError with variants
│   │
│   ├── rofl/                      TRANSPORT LAYER (no game knowledge)
│   │   ├── mod.rs                   parse_replay(&[u8]) -> Replay
│   │   ├── header.rs                FileHeader: magic, version string, unknowns
│   │   ├── metadata.rs              tail-first metadata parse; Metadata struct
│   │   ├── signature.rs             256-byte opaque wrapper; round-trip preserved
│   │   ├── chunk.rs                 ChunkRecord + walk iterator over the region
│   │   ├── stream.rs                StreamTag enum (GameChunk/Keyframe/Start*/Sentinel)
│   │   ├── decompress.rs            zstd frame decoder (one frame per chunk)
│   │   └── block.rs                 marker-byte block framing over decompressed bytes
│   │
│   ├── packet/                    SEMANTIC LAYER (game knowledge)
│   │   ├── mod.rs                   decode_blocks(&[Block], &Registry) -> Vec<Packet>
│   │   ├── opcode.rs                Opcode(u16) newtype with Debug/Display
│   │   ├── registry.rs              Registry<Handler>: opcode -> handler fn ptr
│   │   ├── unknown.rs               UnknownPacket: passes through raw bytes
│   │   └── handlers/                one file per documented opcode (additive)
│   │       ├── mod.rs               register_all(reg): calls every handler's register()
│   │       ├── ward_spawn.rs        ports Mowokuma's ward-spawn handler
│   │       ├── movement.rs          ports Mowokuma's path-packet handler + parse()
│   │       └── (future)             one file per new opcode; never edit others
│   │
│   ├── emulator/                  per-patch game-binary emulator (decode driver)
│   │   ├── mod.rs                   Emulator trait; select(patch) -> Box<dyn Emulator>
│   │   ├── unicorn.rs               Unicorn-engine-backed impl (Mowokuma's approach)
│   │   └── config.rs                per-patch .patch archive reader (zip of section bytes)
│   │
│   ├── game/                      state reconstruction + high-level events
│   │   ├── mod.rs                   GameState accumulator over packet stream
│   │   ├── entities.rs              EntityKind, Entity structs, lifecycle hooks
│   │   ├── players.rs               Player, role inference, id-range mapping
│   │   └── events.rs                GameEvent enum + emit rules
│   │
│   └── output/                    JSON serialisation (stable schema)
│       ├── mod.rs                   serialize(Replay, Options) -> serde_json::Value
│       ├── schema_v1.rs             Mowokuma-compatible top-level keys
│       └── schema_extended.rs       additive sibling keys for --full
│
├── tests/                         integration + property tests
│   ├── fixtures/                    anonymised byte slices per opcode (committed)
│   ├── golden/                      full JSON outputs for 2-3 short replays
│   ├── parity_mowokuma.rs           byte-for-byte diff vs Mowokuma's binary
│   ├── opcode_coverage.rs           walks ROFL_X_SAMPLES_DIR, fails on undocumented
│   └── proptest_block_framing.rs    proptest: never panics on any byte sequence
│
├── scripts/                       dev-time reproducers (not part of build)
│   └── sample_walkthrough.py
│
├── docs/                          human documentation
├── reference/                     gitignored: Mowokuma clone for study
└── README.md
```

---

## Rationale for each choice

### Why a `rofl/` vs `packet/` split?

This is the same split as [DOMAIN.md](DOMAIN.md) § Transport vs Semantic:
`rofl/` can compile with zero knowledge of the game, only format bytes.
`packet/` is where "what does opcode 0x036B mean" lives. If we ever need
`no_std` support for parsing bytes in embedded contexts (unlikely but
possible), the line between `rofl/` and `packet/` is the cleanest place
to cut.

### Why is `emulator/` a sibling of `packet/`, not a submodule?

The emulator is a driver, not a packet handler. One `emulator` instance
can serve many packet handlers, and the unicorn implementation
(~500 LOC) is heavy enough to deserve its own module. Handler code
should not need to know whether its backend is Unicorn, Henry Zhu's
INT3 style, or a pure-Rust decoder that happens to exist.

### Why is the packet handler registry "additive" (one file per opcode)?

Two reasons. First, Phase 4 work will add many opcodes over time; if
adding one means editing a central match statement, merge conflicts
will be constant. Second, every new opcode has an associated fixture
and test; colocating them with the handler keeps the three artefacts
together.

Shape of an additive handler file:

```rust
// src/packet/handlers/ward_spawn.rs
use crate::packet::{Registry, Handler, Opcode, Packet};

pub fn register(reg: &mut Registry) {
    reg.insert(Opcode(0x0110), Handler::Emulator("ward_spawn_decrypt"));
    // Future: reg.insert(Opcode(0x????), Handler::Emulator("ward_spawn_decrypt"));
    // when patch-specific opcode changes land.
}

// Decoded-shape tests + fixtures go alongside in
// tests/fixtures/ward_spawn.bin and tests/handlers/ward_spawn.rs.
```

`handlers/mod.rs` has exactly one function:

```rust
pub fn register_all(reg: &mut Registry) {
    ward_spawn::register(reg);
    movement::register(reg);
    // Adding an opcode = new file + new line here.
}
```

No other file in the crate is touched when a new opcode lands.

### Why is `output/` a separate module?

Three reasons. (a) The output schema is a public contract that must
remain backwards-compatible with Mowokuma's JSON shape. Centralising
serialisation makes schema stability auditable. (b) `--full` output
should be an additive difference, which reads naturally as a separate
schema module. (c) If someone later wants a non-JSON output (parquet
for the `audit` subcommand, for instance), it slots in here.

### Why is `cli.rs` one file and not a directory?

Clap's derive macros make the CLI compact enough (< 200 LOC expected)
that splitting is over-engineering. If the CLI grows past three
non-trivial subcommands, revisit.

### Why keep `error.rs` as one file, one enum?

`thiserror` lets us fold all failure modes into one `RoflError` with
context-carrying variants. The parser has enough layers that plumbing
per-layer error types back to the CLI would be busywork. One enum
with well-named variants is easier to use and easier to present in
error messages.

### Why the `tests/` layout shown?

- `tests/fixtures/`: byte slices per documented opcode, committed to
  the repo. Small enough to not bloat the repo; anonymised enough to
  not leak PII; tied to catalog entries one-to-one.
- `tests/golden/`: small, anonymised replays plus their expected JSON
  outputs. `cargo test` re-parses and diffs. 2–3 replays is enough to
  catch schema regressions.
- `tests/parity_mowokuma.rs`: optional test, enabled when Mowokuma's
  binary + `.patch` archive are present locally. Output diff is
  byte-for-byte for the opcodes she covers.
- `tests/opcode_coverage.rs`: reads `ROFL_X_SAMPLES_DIR`, parses every
  `.rofl` found, asserts the set of opcodes observed is a subset of
  the catalog. This is the patch-regression canary. For us it points
  at Riot's default `%USERPROFILE%\Documents\League of Legends\replays\`.
- `tests/proptest_block_framing.rs`: property-based test that block
  framing never panics on any byte sequence. Catches integer overflows
  and OOB reads in the transport layer.

### Cargo dependencies (concrete)

- `clap` (derive), CLI.
- `thiserror`, error enum.
- `serde`, `serde_json`, metadata JSON + output JSON.
- `zstd`, decompression. Same crate and version family Mowokuma uses.
- `rayon`, parallel block iteration and parallel replay-batch parse.
- `tracing` + `tracing-subscriber`, structured logging. Prefer over
  `log` + `fern` because structured fields pair well with the audit
  subcommand.
- `unicorn-engine`, emulator backend. Gated behind a Cargo feature
  (`emulator`) so the pure-Rust-only build path stays thin.
- `proptest` (dev), property tests.

### Cargo features

- `emulator` (default on): pulls in `unicorn-engine`, enables all
  handlers marked `Handler::Emulator(...)`. Turning it off disables
  emulator-backed opcodes; pure-Rust handlers still work.
- `parity` (default off): enables the Mowokuma-parity integration test,
  which expects a local install of her binary + `.patch` archive.

---

## What this layout does not do

- **No plugin system.** Handlers are compiled in; there is no
  dynamic-library loading path. Simpler, smaller surface area.
- **No `no_std`** in the initial design. All modules assume `std`. If
  that becomes a reason later, the cut line is the `rofl/` / `packet/`
  boundary.
- **No async.** Parsing a file is CPU-bound; Rayon covers the
  parallelism need. Async would add dependency mass without a clear
  benefit.
- **No build.rs codegen.** Opcode descriptors are hand-written in each
  `handlers/*.rs`. If that gets tedious at 200+ opcodes we'll revisit.

---

## Phase 3 starting point

With this layout, Phase 3 (port Mowokuma + reach parity) is:

1. Scaffold `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/cli.rs`,
   `src/error.rs`.
2. Implement `rofl/` end-to-end against the format spec. Transport
   layer is fully tested against sample_a and any other replays we add.
3. Implement `emulator/unicorn.rs` and `emulator/config.rs` as near
   line-by-line ports of Mowokuma's `stub_emulator.rs` and `config.rs`.
4. Implement `packet/handlers/ward_spawn.rs` and
   `packet/handlers/movement.rs` as ports of her decoders.
5. Implement `output/schema_v1.rs` producing JSON byte-identical to
   Mowokuma's on a shared replay (parity test passes).
6. `game/` fleshes out enough to drive `players_state[]` and `wards[]`
   the way Mowokuma does it.

After that, Phase 4 expansion is purely additive: new `handlers/*.rs`,
new catalog entries, new fixtures.
