# Roadmap

Phase-by-phase plan for ROFL-X. Each phase has a clear stop point; we
pause at every boundary so direction can be corrected before code or docs
calcify.

---

## Phase 1, reconnaissance, DONE

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

## Phase 2, domain and format spec, DONE

Write down what we're parsing and how it lays out on disk before
committing to a code structure.

Deliverables landed:

- [docs/DOMAIN.md](DOMAIN.md): entities, lifecycles, transport-vs-semantic split.
- [docs/ROFL_FORMAT.md](ROFL_FORMAT.md): byte-level format with per-field evidence tags.
- [docs/MODULE_LAYOUT.md](MODULE_LAYOUT.md): proposed Rust crate tree plus the additive-handler-registry rationale.

Commits: `8d9da90`, `fa39b5e`, `24b093f`.

---

## Phase 3, port Mowokuma and reach parity, DONE

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
5. Implement the two handlers her decoders feed. (As built:
   `packet/handlers/spawn.rs` and `packet/handlers/waypoints.rs`; her
   `PathPacket::parse` lives verbatim in `emulator/packet.rs`.)
6. Produce the exact JSON shape her README documents (`metadata`,
   `players_state`, `wards`). (As built: `replay_info.rs`; there is no
   separate `output/` module, see
   [MODULE_LAYOUT.md](MODULE_LAYOUT.md).)
7. Acquire her release binary plus one patch-matched `.patch` archive,
   parse the same replay with both tools, and diff the outputs.

Stop point: a green byte-for-byte diff against her binary on at least
one shared replay, plus a green `cargo test`. (As built, the test
suite is `tests/transport_smoke.rs` and `tests/decode_replays.rs` over
a whole replay folder, plus the library's unit tests; the golden-JSON,
opcode-coverage and proptest targets proposed here were never needed
once whole replays were checked against statsJson.)

Outcome: content parity reached on patch 15.5 (metadata + wards
byte-identical, 99.07 % of individual player entries match). Full
byte-identity is not achievable because her output uses `HashMap`
iteration order inside `players_state` and is non-deterministic
run-to-run on her side (0.1 % self-similarity across her own runs).
Ours is deterministic via `BTreeMap`. Commits: `fa2e059`, `deb7bf0`,
`40108b0`, `8581732`.

Phase 3c deliverable: a documented way to produce a `.patch` archive
on a new League version (`docs/RE_PATCH.md`). Shipped as
`scripts/build_patch.py` rather than the `rofl-x extract-patch`
subcommand proposed here. Superseded by Phase 4b's
`scripts/build_patch.py`, which needs no manual RVA work.

---

## Phase 4, packet catalog expansion, SEEDED

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
catalog entry; no catalog entry without an honest status word. (As built the vocabulary
settled on three: **TYPED**, **NAMED**, **UNNAMED**; see
[PACKETS.md](PACKETS.md).)

Priority order for expansion: the high-frequency opcodes from
sample_a's global histogram (top of [SAMPLE_A_WALKTHROUGH.md](SAMPLE_A_WALKTHROUGH.md) § Global
opcode coverage), then Henry Zhu's named packet classes from
[REFERENCE_HENRY_ZHU.md](REFERENCE_HENRY_ZHU.md) § 5.

Stop point: one full pass of the top-40 opcodes, each either
documented or unknown-with-evidence, and `docs/COMPATIBILITY.md`
filled in for patch 16.8.

Status as of commit `64ff98d`: catalog scaffolded in `docs/PACKETS.md`
with two `DOCUMENTED` classes (MOVEMENT_PATH, WARD_SPAWN_OR_DESTROY),
20 `OBSERVED-ONLY` classes (from Zhu's public schema), and 15
`UNKNOWN` entries (top opcodes from sample_a's histogram, with
frequency counts). Promoting an entry needed a new decoder function
reverse-engineered per class, which is what Phase 4b removed.

### Phase 4b, every class through the game's own code, DONE (16.9)

The per-class bottleneck turned out to be avoidable: the game's netid
factory builds a packet object with its real vtable, and vtable slot 1
is that class's deserializer. Running the factory and the deserializer
in the emulator decodes every class with no per-class work, and the
deserializer's own verdict (returns true, consumes exactly the payload)
checks every block. Riot's class names come back from the binary's
`std::function` handler registrations. Method:
[DECODING.md](DECODING.md).

Delivered on 16.9 (five replays): every one of 7,927,123 blocks decoded
clean; 315 netids named; 16 classes typed (deaths, respawns, K/D/A,
levels, damage, spell casts, the four item answers, NPC and building
deaths, team visibility both ways, movement paths, unit spawns
including wards); `scripts/build_patch.py`
builds the archive from the binary in minutes. The discipline above
still holds in spirit, with one change: fixtures are whole replays
checked against statsJson (`tests/decode_replays.rs`) rather than
byte slices, since a handler is only as good as the decode it reads.

The detour that preceded it (brute-force netid → decoder matching on
the `decrypt/tooling-and-16-9-recon` branch) produced a mapping that
was wrong for several high-volume classes; it is gone from the tree
and documented in [RE_PATCH.md](RE_PATCH.md) § History.

---

## Phase 5, validation against Henry Zhu's public dataset, INITIAL PASS DONE

Henry Zhu released decoded replays on Hugging Face in two drops of
roughly 700,000 each, 1.4M in total
([dataset](https://huggingface.co/datasets/maknee/league-of-legends-decoded-replay-packets);
sizes in [DATASETS.md](DATASETS.md)).
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
   don't, add a `docs/PACKETS.md` entry as **UNNAMED** with a
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

Caveat: patch 12.x is years older than our reference patch (16.9). Opcode numbers will not match; packet class names and
semantics almost certainly will. Use his data for schema and semantics,
not for opcode-number mapping.

Deliverables:

- `docs/DATASETS.md` describing the fetch and the gitignored cache layout.
- `scripts/zhu_dataset_stats.py` (and any helper scripts for fetching).
- `tests/zhu_schema_compat.rs`.
- New catalog entries for every Zhu packet class not already present,
  annotated **UNNAMED** with a Zhu-reference.
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

Initial pass delivered (commit `f791fd9`):

- `scripts/zhu_dataset_fetch.py` + `scripts/zhu_dataset_stats.py` for
  reproducible slices and profiling.
- `docs/DATASETS.md` with dataset layout, Apache-2.0 license note,
  fetch recipes, per-class frequency table across 20 S12 games.
- `docs/PACKETS.md` cross-references: `WaypointGroup` ~ our
  `MOVEMENT_PATH`; ward events ride on Zhu's `SpawnMinion` (confirmed
  by probing names for `SightWard`, `VisionWard`, `JammerDevice`,
  `PlantVision`, `WardCorpse`); our `WARD_SPAWN_OR_DESTROY` output is
  strictly richer than his (`owner_id` present in ours, absent in his
  schema).

Still outstanding:

- `tests/zhu_schema_compat.rs` to assert our output can round-trip
  every Zhu-class field. With Phase 4b, 11 of his 22 classes are typed
  on 16.9 and 3 more are named ([PACKETS.md](PACKETS.md) § Henry Zhu's
  classes on 16.9).
- Raw-replay byte-diff: Zhu's dataset ships decoded JSON only, not
  source `.rofl` files. Still an open question whether the raw files
  are available anywhere.

### Phase 5b, validation against the replay's own stats, DONE (16.9)

A stronger reference than any external dataset sits in every replay:
statsJson, Riot's end-of-game numbers per player. `rofl-x file`
rebuilds them from decoded packets and compares: K/D/A, kills and
assists from kill events, level, all seven final item slots, damage to
champions, wards placed, and team visibility. 350 / 350 checks pass on
the five 16.9 replays; `tests/decode_replays.rs` keeps it that way, and
also asserts that the file-only path reaches the same answer as the
decoder where both can be run.

---

## Phase 6, emulated decoding on current patches, BLOCKED AND STAYING SO

From some patch after 16.9 the shipped binary keeps its code encrypted
on disk (16.19: `.riot0/.riot1/.riot2` sections, `.text` entropy 7.997).
Format-2 archives need that code; getting it would mean defeating
Riot's protection, which this project does not do. `build_patch.py`
detects a packed binary and refuses it.

Unblocked only by an unpacked binary of the patch arriving through
legitimate means, at which point an archive builds in minutes and
Phase 4b applies unchanged. Details:
[COMPATIBILITY.md](COMPATIBILITY.md).

What such a replay *does* give is Phase 7 below, through `rofl-x
summary`, `rofl-x explore` and the measured field catalog
([FIELDS.md](FIELDS.md), [EXPLORE.md](EXPLORE.md)) — considerably more
than the transport layer and the scoreboard this phase was first
written to expect.

---

## Phase 7, reading a packed patch from the file alone, IN PROGRESS

Phase 6 being shut does not make the replay unreadable; it makes the
*game* unavailable. The replay is still a file, its framing was never
obscured, and it ships with Riot's own end-of-game record inside it.
Phase 7 is how far that goes. Everything here is measured on the twenty
16.19 replays, with the five 16.9 ones used as a control wherever a
method can be checked against a known answer.
[EXPLORE.md](EXPLORE.md) is the full account.

**Done.**

- Champion entity ids from the shape of the traffic alone; right in 5
  of 5 on the control patch, where the decoder knows independently.
- A per-champion, per-minute activity timeline for the whole game.
- Netids identified by **proof**: a per-champion block count equal to
  one of Riot's counters, or to a short sum of them. Four on the
  control patch, all correct; the champion-death class and the running
  score on 16.19, each in 19 of 19 replays.
- Netids identified by **when they fire**, with the kill feed as the
  label. Four on the control patch, all correct and one of them a
  class nobody would have gone looking for; two on 16.19.
- Netids **proposed** by behavioural match against a decodable patch,
  with the gate calibrated by splitting the control patch's replays
  and measuring how often the answer comes back: 63 of 273 netids on
  16.19, at a measured precision around 96 % under conditions kinder
  than a real patch gap.
- **Three payload fields read with no binary at all**, by recovering a
  class's byte substitution from plaintext pinned from outside: the
  visibility pair's team byte and the death class's killer field. Out
  of them come a vision timeline and a kill feed.
- `decode --layout`, which recovers a class's payload layout and
  substitution table against the deserializer's own output on a patch
  that has one — including the discovery that the waypoint class
  writes its buffer backwards.
- Netids identified by **correlation with a counter the exact test
  cannot use**, with the confound ruled out and the method validated
  against the control patch first: spell casts, item purchase, item
  removal and ward placement. A cast is not one packet, so no equality
  test can ever find it; see [EXPLORE.md](EXPLORE.md) § 2.
- **Positions to a measured accuracy**, median 4 map units with every
  table entry within 16 steps of right, scored against the control
  patch by `scripts/lowbyte_probe.py`.
- **Positions emitted**, as JSON lines carrying the accuracy each was
  read at: 1,761,827 of them over 28,508 units on 16.19. The parser is
  checked against the control patch, where given the decoder's own
  table it reproduces all 172,857 of the game's positions exactly.

**Stop point.** *Exact* positions. Approximate ones are emitted with
their error bar, and that part is done. Exactness is a different
matter, and the search cannot reach it: scoring the true table under
the same objective shows the search's own answer costs *less* than the
truth, so its optimum is not the answer and no better optimiser helps.
Nor is the constraint that would close it the path deltas, which give
no exact arithmetic three separate ways, nor the affine structure,
which one class of 23 has. The next boundary is reached when some
other source of known plaintext appears, or when the remaining error
is shown to be irreducible from the file alone.

## Phase 8, what is left on 16.19, in the order it is worth doing

Measured on the sample replay: **49.1 %** of its 1,140,285 blocks are
read in some form, **6.3 %** are known to carry nothing an action could
come from, and **44.5 %** are still unread. What that leaves, with why
each is where it is:

**1. The cast class's missing table entries.** 89 were missing; **62**
are now, after elimination on the bijection, on what a cast can reach,
and on the fact that a cast aimed at a champion lands on one
(EXPLORE.md). Wards went 49 to 59 of 84, exact ones 23 to 36. The
most direct lever on the list: it holds back both the 35 wards that
cannot be placed and the 26 that sit on a ±16 or ±32 grid. More replays
no longer help — going from 7 to 20 added four entries — so it needs a
different constraint rather than more data.

**2. Damage, netid 95, 4.9 %.** The class is settled by a signature
method that picks the right one on the control patch. Reading the amount
is blocked for a stated reason: a float's exponent byte is narrow, and
there is no narrow byte anywhere in that payload, at either alignment or
any length. Either the amount is an integer, or the fixed tail is
bit-packed. Nothing measured separates those, and value ranges alone
will not: that lane needs a block whose amount is independently known —
a killing blow against a health already read, say.

**3. Netids 77 and 587, 6.4 %.** Payloads up to 7,516 bytes addressed to
the game. A payload that large is a buffer, which is what the movement
class turned out to be, so the same reading may apply: a varint header
and a reversed, right-aligned entry stream.

**4. Missiles.** *Done, as far as it goes.* Netid 138 creates a missile
and 325 ends it, both in plaintext, sharing 5,538 ids of 6,102 with a
flight time between them — median 0.50 s. Netid 135 turned out to be
something else, sharing no id with either. What is *not* done is joining
a missile to the cast that threw it: matching on time beats shuffled
times by only 2.2×, which is not enough to name a thrower.

**5. Netid 751, 2.9 %.** Ten equal streams and a float, almost certainly
the ten champions on a round-robin. Which stream is which champion is
unsolved — the obvious test fails, every stream holding its rate through
that champion's death windows at 0.92× to 1.00×.

**6. The facing class, netid 910.** Read to an axis but not along it:
geometry fixes the magnitudes, and nothing yet fixes the signs beyond a
54–68 % lean per cipher.

**7. The 212 positions that are off the map for no reason**, 0.25 % of
them, which are residual table error rather than the stream declining
to report a unit.

Not on this list, because it is a different kind of problem: damage,
items, levels and which ability a cast was all need known plaintext that
nothing outside the file pins down. Positions yielded because the map
constrains them, wards because Riot counts them, and a facing because a
unit vector has length one. These have no such handle, and finding one
is an idea rather than an amount of work.
