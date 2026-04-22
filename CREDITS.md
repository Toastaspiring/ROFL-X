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

Direct ports of her work live under `src/emulator/` and
`src/packet/handlers/`, with per-file headers that point back to the
corresponding file in her repo.

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

- The observation that there is no named cipher, only custom per-packet
  table-lookup obfuscation.
- The **decrypt-access-release** pattern: packet fields are decrypted
  on demand, used, re-encrypted, and the plaintext is zeroed before the
  destructor returns. This is anti-reverse-engineering design.
- The **three-stage packet lifecycle** (`Packet::Packet` allocation,
  `DeserializePacket`, `UsePacket`) and what each stage tells us about
  what a parser can and cannot observe.
- The alternative emulator strategy of installing INT3 breakpoints and
  reading values directly out of CPU registers during execution.
- Nine named packet classes with observed field layouts: `TakeDamagePacket`,
  `BasicAttackAtTarget`, `CastSpell`, `CreateSummoner`, `CreateEntity`,
  `UpdateState`, `Death`, `BecomeVisibleInFogOfWar`, `LeaveFromFog`.
  Our catalog (Phase 4/5 work) starts from this list.

Most importantly, Henry Zhu **released the decoded output of 1.4M+
replays publicly**. That is a generational-scale community gift; our
Phase 5 validation work (see `docs/ROADMAP.md`) is entirely premised on
being able to use his data as a reference corpus.

None of his code is ported into ROFL-X directly. His influence is in
the documentation (see `docs/REFERENCE_HENRY_ZHU.md`), the domain model,
and the catalog-expansion plan.

**Thank you, Henry Zhu.**

---

## Further prior art

These projects informed the reconnaissance without being ported from:

- [@fraxiinus](https://github.com/fraxiinus) — [fraxiinus/roflxd](https://github.com/fraxiinus/roflxd).
  An umbrella of ROFL parsers across languages. Useful cross-check that
  Riot's obfuscation really does drift per patch.
- [@robertabcd](https://github.com/robertabcd) — `lol-ob`. Older Ruby
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
- (Forthcoming in Phase 3b) `unicorn-engine` for the x86-64 emulator,
  same crate Mowokuma uses upstream.

---

## The author of this fork

The author of this repository chose not to single themselves out in
source headers, README, or this file. The git log is the authoritative
record.
