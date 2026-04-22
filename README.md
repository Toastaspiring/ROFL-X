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
| `docs/PACKETS.md` | *(coming in Phase 4)* the packet catalog, one entry per opcode, including `UNKNOWN` |
| `docs/COMPATIBILITY.md` | *(coming later)* per-patch coverage tracking |

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

Undecided, Mowokuma's upstream has no LICENSE file, which makes its status
"all rights reserved" by GitHub's default. Treating it as read-reference
only and adopting a permissive licence for ROFL-X's original code is the
likely direction, but this is pending.

See [docs/REFERENCE_MOWOKUMA.md](docs/REFERENCE_MOWOKUMA.md) for the open
question in detail.

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
