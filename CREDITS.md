# Credits

This file exists because this project could not exist without two specific
people. The README summarises their contributions; this file is the formal
record.

---

## Mowokuma ([@Mowokuma](https://github.com/Mowokuma))

Upstream: [Mowokuma/ROFL](https://github.com/Mowokuma/ROFL) (archived November 2025).

**Original author of the parser approach this project builds on.** Mowokuma
figured out:

- The ROFL outer-container layout (file header, 17-byte chunk records,
  zstd-compressed bodies, trailing signature + JSON metadata + u32 length).
- The marker-byte block framing inside decompressed chunks: the bitfield
  semantics for timestamp-relative, packet-id reuse, param-delta, and
  length-u8 encoding. Our `src/rofl/block.rs` implements exactly this
  scheme.
- The observation that the per-packet decode logic is intentionally
  hostile to static reimplementation, and the decision to **call the
  game client's own decoder inside a Unicorn CPU emulator** rather than
  port the obfuscation.
- The per-patch emulator configuration format (a `.zip` archive bundling
  `result.json` + `text.bin` / `data.bin` / `rdata.bin`), the heap/stack
  layout inside the emulator, the hand-rolled 91-byte x86-64 shellcode
  that stubs the client's allocator, and the patch to the one safety
  check that would otherwise abort emulation.
- The specific handlers for ward-spawn (name, owner id, coordinates)
  and path/position packets (entity id, speed, waypoint chain), along
  with the ward-lifecycle reconstruction via coordinate-matched
  "Corpse" destruction packets.

Direct ports of her work live under `src/emulator/`: `config.rs`,
`packet.rs` and `unicorn.rs` each name the file of hers they came from
in their own header, and her `PathPacket::parse` is in `packet.rs`
verbatim.

The handlers in `src/packet/handlers/` are **not** ports. They read
objects the game's own deserializers produced, which is a different
route to the same place. Two of them are nonetheless hers in substance
and say so in their headers:

- `waypoints.rs` is the class her `mov_decrypt` decoded, and uses her
  `PathPacket` layout with two corrections measured on 16.9 (the
  waypoint count is the low byte of the flags, and the one-byte
  coordinate deltas are signed).
- `spawn.rs` is the class her `ward_spawn_decrypt` decoded, and the
  ward lifecycle in it — pair a placement with its `*Corpse` to get a
  duration — is her idea. Our one change is the pairing rule: she
  matches on an exact position, and on 16.9 the corpse can land ~100
  units away, so we match the owner's nearest live ward instead.

Her upstream has no LICENSE file, which makes the default "all rights
reserved". She has confirmed (as relayed by the project's current author
in April 2026) that she is content to have her work ported and extended
provided she is credited clearly. This CREDITS file is that credit, and
`LICENSE` repeats the attribution.

**Thank you, Mowokuma.**

---

## Henry Zhu ([@maknee](https://github.com/maknee))

Blog post: [League of Legends data scraping the hard and tedious way for fun](https://maknee.github.io/blog/2025/League-Data-Scraping/) (2025).

Datasets: [league-of-legends-decoded-replay-packets](https://huggingface.co/datasets/maknee/league-of-legends-decoded-replay-packets)
(700k+ S12 replays) and its unorganised sibling (another 700k+).

Henry Zhu's write-up is the most detailed public explanation of
ROFL-level reverse engineering that exists. Without it, our Phase 1
reconnaissance would have spent weeks re-discovering material he had
already documented:

- The observation that there is no named cipher, only custom
  per-packet table-lookup obfuscation. **This turned out to be exactly
  right, and measurably so.** On a patch ROFL-X can decode, comparing
  a class's raw payload against the plaintext its own deserializer
  produced gives one 256-entry byte substitution per class, injective,
  with zero contradictions over thousands of blocks — and the tables of
  two different classes are unrelated to each other. That is what made
  it possible to read three payload fields on patch 16.19, where no
  binary exists to emulate at all
  ([docs/EXPLORE.md](docs/EXPLORE.md) § 3). His analysis is the reason
  that was ever attempted.
- The **decrypt-access-release** pattern: packet fields are decrypted
  on demand, used, re-encrypted, and the plaintext is zeroed before the
  destructor returns. This is anti-reverse-engineering design.
- The **three-stage packet lifecycle** (`Packet::Packet` allocation,
  `DeserializePacket`, `UsePacket`) and what each stage tells us about
  what a parser can and cannot observe.
- The alternative emulator strategy of installing INT3 breakpoints and
  reading values directly out of CPU registers during execution.
- Nine named packet classes with observed field layouts:
  `TakeDamagePacket`, `BasicAttackAtTarget`, `CastSpell`,
  `CreateSummoner`, `CreateEntity`, `UpdateState`, `Death`,
  `BecomeVisibleInFogOfWar`, `LeaveFromFog`. His datasets name 22 in
  all; all 22 are now mapped onto patch 16.9 in
  [docs/PACKETS.md](docs/PACKETS.md) § Henry Zhu's classes on 16.9 —
  11 typed, 3 named, the rest unnamed because object creation and
  replication do not go through handler registration.

Most importantly, Henry Zhu **released the decoded output of 1.4M+
replays publicly**. That is a generational-scale community gift; our
Phase 5 validation work (see [docs/ROADMAP.md](docs/ROADMAP.md)) is
premised on being able to use his data as a reference corpus. In the
end the sharpest reference turned out to be one inside every replay —
Riot's own end-of-game numbers — but his corpus is what the schema and
the class names were checked against first, and it remains the only
way to test against patches we hold no replays for.

None of his code is ported into ROFL-X directly. His influence is in
the documentation (see `docs/REFERENCE_HENRY_ZHU.md`), the domain model,
and the catalog-expansion plan.

**Thank you, Henry Zhu.**

---

## Further prior art

These projects informed the reconnaissance without being ported from:

- [@fraxiinus](https://github.com/fraxiinus), [fraxiinus/roflxd](https://github.com/fraxiinus/roflxd).
  An umbrella of ROFL parsers across languages. Useful cross-check that
  Riot's obfuscation really does drift per patch.
- [@robertabcd](https://github.com/robertabcd), `lol-ob`. Older Ruby
  work on Blowfish decryption of chunk data from an earlier era of the
  format. Historical interest only; the format has changed since.

---

## External crates

Direct runtime dependencies, each the standard choice for its job in the
Rust ecosystem:

- `clap` for the CLI.
- `serde` / `serde_json` for metadata and output JSON.
- `zstd` for chunk decompression.
- `thiserror` for the error enum.
- `unicorn-engine` (pinned to `2.1.1`) for the x86-64 emulator, the
  same crate Mowokuma uses upstream. Optional, behind the default
  `emulator` feature; without it the transport layer and the file-only
  analysis still build.
- `rayon` for one emulator per worker thread.
- `zip` for `.patch` archives.

The archive builder (`scripts/build_patch.py`) uses `pefile`,
`capstone`, `numpy` and the `unicorn` Python binding.

---

## The author of this fork

The author of this repository chose not to single themselves out in
source headers, README, or this file. The git log is the authoritative
record.
