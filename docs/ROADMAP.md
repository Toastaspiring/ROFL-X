# Roadmap

Phase-by-phase plan for ROFL-X. Each phase has a clear stop point; we
pause at every boundary so direction can be corrected before code or docs
calcify.

---

## Phase 1, reconnaissance — DONE

Read the prior art and characterise one real replay end-to-end before
writing any code of our own.

Deliverables landed:

- [docs/REFERENCE_MOWOKUMA.md](REFERENCE_MOWOKUMA.md): module-by-module walkthrough of her archived parser.
- [docs/REFERENCE_HENRY_ZHU.md](REFERENCE_HENRY_ZHU.md): summary of his 2025 write-up, named packet classes, dataset links.
- [docs/SAMPLE_A_WALKTHROUGH.md](SAMPLE_A_WALKTHROUGH.md): annotated hex of a real patch-16.8 replay, 1.9M blocks parsed with zero errors.
- `scripts/sample_walkthrough.py`: reproducer for the walkthrough.
- `README.md`, `.gitignore`.

Commit: `b9873a7`.

---

## Phase 2, domain and format spec — DONE

Write down what we're parsing and how it lays out on disk before
committing to a code structure.

Deliverables landed:

- [docs/DOMAIN.md](DOMAIN.md): entities, lifecycles, transport-vs-semantic split.
- [docs/ROFL_FORMAT.md](ROFL_FORMAT.md): byte-level format with per-field evidence tags.
- [docs/MODULE_LAYOUT.md](MODULE_LAYOUT.md): proposed Rust crate tree plus the additive-handler-registry rationale.

Commits: `8d9da90`, `fa39b5e`, `24b093f`.

---

## Phase 3, port Mowokuma and reach parity — PENDING

First code. Build the transport layer end-to-end, stand up the emulator
backend, port Mowokuma's two handlers (ward-spawn and movement), and
prove byte-for-byte JSON identity with her binary on a shared replay.

Sub-steps:

1. Scaffold the crate: `Cargo.toml`, `src/lib.rs`, `src/main.rs`,
   `src/cli.rs`, `src/error.rs`.
2. Implement `rofl/` (FileHeader, ChunkRecord walker, zstd decompression,
   block framing, metadata JSON, signature wrapper) against
   [ROFL_FORMAT.md](ROFL_FORMAT.md). Full coverage on sample_a.
3. Implement `emulator/config.rs` as a near line-by-line port of her
   `emulator/config.rs`, reading the `.patch` archive format she already
   defined.
4. Implement `emulator/unicorn.rs` as a port of her `stub_emulator.rs`,
   with the same heap/stack layout, the same allocator shellcode patch,
   and the same function-stub logic.
5. Implement `packet/handlers/ward_spawn.rs` and `packet/handlers/movement.rs`
   (the latter carrying her `PathPacket::parse` verbatim for now).
6. Implement `output/schema_v1.rs` producing the exact JSON shape her
   README documents (`metadata`, `players_state`, `wards`).
7. Acquire her release binary plus one patch-matched `.patch` archive,
   parse the same replay with both tools, and diff the outputs.

Stop point: a green byte-for-byte diff against her binary on at least
one shared replay, plus a `cargo test` run with the golden-JSON, opcode-
coverage, and proptest targets all passing.

Open questions blocking Phase 3:

- License direction for the Rust code we write here (Mowokuma's upstream
  has no LICENSE).
- Whether to maintain per-patch `.patch` archives ourselves, lean on the
  community, or both.
- Whether to fetch her release binary + `.patch` archive now (required
  for the parity test) or defer until someone in the project has done
  so manually.

---

## Phase 4, packet catalog expansion — PENDING

With parity achieved, grow the catalog. This is where the project's
value compounds: every new opcode documented is a permanent gain, and
every `UNKNOWN` recorded is an honest statement about the state of
reverse-engineering work.

Discipline for each new packet type, in this order:

1. Catalog entry in `docs/PACKETS.md` (even if the only known field is
   the opcode number and the observed size distribution).
2. Handler in `src/packet/handlers/<name>.rs`, registered in
   `handlers/mod.rs`. Pure-Rust decode if feasible; emulator-backed
   otherwise.
3. Fixture in `tests/fixtures/<name>.bin`: a short anonymised byte
   slice carrying one example payload.
4. Test in `tests/handlers/<name>.rs` asserting the handler decodes the
   fixture to the expected typed struct.

Never the reverse. No handler without a fixture; no fixture without a
catalog entry; no catalog entry without an honest status word
(`DOCUMENTED`, `PARTIAL`, `OBSERVED-ONLY`, `UNKNOWN`).

Priority order for expansion: the high-frequency opcodes from
sample_a's global histogram (top of [SAMPLE_A_WALKTHROUGH.md](SAMPLE_A_WALKTHROUGH.md) § Global
opcode coverage), then Henry Zhu's named packet classes from
[REFERENCE_HENRY_ZHU.md](REFERENCE_HENRY_ZHU.md) § 5.

Stop point: one full pass of the top-40 opcodes, each either
`DOCUMENTED` or `UNKNOWN`-with-evidence, and `docs/COMPATIBILITY.md`
filled in for patch 16.8.

---

## Phase 5, validation against Henry Zhu's public dataset — PENDING

Henry Zhu released 1.4M+ decoded replays on Hugging Face
([dataset](https://huggingface.co/datasets/maknee/league-of-legends-decoded-replay-packets)).
The decoded data is not our output format and is from patch 12.x, but
it is the largest public body of ROFL-semantic data and is worth
treating as a reference corpus. This phase uses it to stress-test our
catalog, schema, and (if feasible) parser output.

Sub-steps:

1. **Acquire a small slice of the dataset**. Pull a few hundred to a
   few thousand decoded-replay records. Never commit the data; document
   the fetch recipe in `docs/DATASETS.md` and gitignore the local
   cache. Keep the slice sized so that a laptop can process it.
2. **Profile the data**. Write `scripts/zhu_dataset_stats.py` to enumerate:
   - All packet-class names present and their per-replay frequencies.
   - The union and intersection of fields per class.
   - Field-type and value-range distributions, especially for coordinate
     and id fields.
3. **Diff against our catalog**. For every packet class Zhu has that we
   don't, add a `docs/PACKETS.md` entry as `OBSERVED-ONLY` with a
   reference to the Zhu-dataset frequency. For every opcode we have
   that seems semantically equivalent to one of his names, annotate
   the mapping in `docs/COMPATIBILITY.md` so downstream consumers can
   cross-walk.
4. **Schema compatibility test**. Add `tests/zhu_schema_compat.rs` that
   loads a sample of his JSON and asserts our output schema can
   represent every field in every packet class. Fails loudly if we
   would silently drop a field.
5. **If his raw `.rofl` files turn out to be available alongside the
   decoded data**: run ROFL-X over a matched subset, diff our output
   against his per-replay. This is the strongest possible regression
   signal but depends on whether the dataset actually ships source
   files, which Phase 1 left as an open question. If only decoded JSON
   is available, the diff is schema-compatible-vs-his, not
   byte-identical.
6. **Refresh compatibility docs**. `docs/COMPATIBILITY.md` gets a
   "Zhu dataset cross-reference" table: for each packet class he names,
   record whether we decode it, which patches we decode it on, and
   how his field names map to ours.

Caveat: patch 12.x is 4+ years older than our current reference patch
(16.8). Opcode numbers will not match; packet class names and
semantics almost certainly will. Use his data for schema and semantics,
not for opcode-number mapping.

Deliverables:

- `docs/DATASETS.md` describing the fetch and the gitignored cache layout.
- `scripts/zhu_dataset_stats.py` (and any helper scripts for fetching).
- `tests/zhu_schema_compat.rs`.
- New catalog entries for every Zhu packet class not already present,
  annotated as `OBSERVED-ONLY` with a Zhu-reference.
- Updated `docs/COMPATIBILITY.md` with a Zhu-cross-reference section.

Stop point: schema-compatibility test green on a representative sample;
no Zhu packet class is missing from the catalog.

Open questions specific to Phase 5:

- Which dataset variant to use: `maknee/league-of-legends-decoded-replay-packets`
  (the organised S12 drop) or the unorganised one. The organised drop
  is the likely starting point.
- How big a sample is "enough"? First pass: a few hundred replays for
  schema discovery; second pass: a few thousand for frequency
  statistics.
- Whether to attempt patch-12.x `.rofl` parsing at all (may be a Phase 6
  candidate; likely requires its own emulator config archive).
