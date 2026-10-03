# ROFL-X

A documented parser for League of Legends replay files (`.rofl`), written in
Rust. Extends [Mowokuma's ROFL](https://github.com/Mowokuma/ROFL) (an
already-excellent parser) and pairs it with a **written packet catalog** so
that patch-to-patch regressions become detectable instead of silent.

ROFL-X exists because Riot publishes no official documentation of the ROFL
format, and every existing parser extracts a narrow slice of the data that
is actually in the file. Our goal is a parser that covers as much as the
community's reverse-engineering knowledge allows, and a catalog that honestly
records the rest as unnamed so it's obvious what we haven't figured out yet.

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

On patch **16.9**, ROFL-X decodes every packet of a replay through the
game's own code: the game's netid factory builds each packet object and
its own deserializer reads the payload, inside a Unicorn VM
([docs/DECODING.md](docs/DECODING.md)). On the five 16.9 replays we hold:

- **7,927,123 / 7,927,123 blocks decode clean** (the deserializer
  returned true and consumed exactly the payload).
- **315 netids carry Riot's own class name**, recovered from the
  binary's handler registrations (75.9 % of all blocks).
- **14 classes are typed**: champion deaths with killer and assists,
  respawns, K/D/A, levels, damage, spell casts, item buys / removals /
  swaps / uses, NPC and building deaths, movement paths, unit spawns
  (wards).
- **Every per-player total rebuilt from those packets matches Riot's
  own end-of-game stats in the same file**: K/D/A, level, all seven
  final item slots, damage to champions, wards placed. 300 / 300.

On **15.1 – 15.5**, Mowokuma's format-1 archives give positions and
wards, at parity with her binary on 15.5.

On **current patches (16.19)** semantic decoding is blocked: the
shipped binary keeps its code encrypted on disk and ROFL-X will not
circumvent that (§ Non-goals). Transport parsing and the end-of-game
scoreboard work on every patch.

Per-patch detail: [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md). Plan:
[docs/ROADMAP.md](docs/ROADMAP.md).

---

## Usage

```bash
cargo build --release          # needs cmake + a C toolchain for unicorn
```

Summary and scoreboard of any replay, no archive needed:

```bash
rofl-x inspect -r EUW1-1234567890.rofl --histogram
```

Decode a replay (patch archive in `./patch`, named `<major>-<minor>.patch`):

```bash
rofl-x file -r EUW1-1234567890.rofl -o game.json
```

Output: `players` (with net ids), `events` (kills with assists,
respawns, level-ups, spell casts, purchases, removals, item uses,
wards placed, buildings destroyed), `players_state` (per-second positions) and
`wards` (lifecycles), both in Mowokuma's shape, `checks` (the statsJson
comparison) and `decode` (block counts). No Riot ID or PUUID is written.

Research view, every block of one netid as plaintext object words:

```bash
rofl-x decode -r EUW1-1234567890.rofl --netid 472 --jsonl out.jsonl
rofl-x decode -r EUW1-1234567890.rofl --find YellowTrinket
```

Build a format-2 archive from an unpacked `League of Legends.exe` of the
same patch ([docs/RE_PATCH.md](docs/RE_PATCH.md)):

```bash
python scripts/build_patch.py --binary <League of Legends.exe> --output patch/16-9.patch --replay <a 16.9 .rofl>
```

Archives and replays are never committed (`.gitignore`): archives carry
the game's code, replays carry player data.

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
| [docs/DECODING.md](docs/DECODING.md) | How every packet is decoded through the game's own code, and how we know it is right |
| [docs/PACKETS.md](docs/PACKETS.md) | Packet catalog by Riot class name, typed layouts, netid table for 16.9 |
| [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) | Per-patch coverage, and why current patches are blocked |
| [docs/RE_PATCH.md](docs/RE_PATCH.md) | Building a `.patch` archive for a League patch |
| [docs/MODULE_LAYOUT.md](docs/MODULE_LAYOUT.md) | Rust crate layout, as built and as first proposed |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Phase-by-phase project plan with stop points |
| [docs/DATASETS.md](docs/DATASETS.md) | Fetch + profile Henry Zhu's public Hugging Face corpus |

---

## Honesty > completeness

A parser that silently misses packets after a patch is worse than no
parser. Every block ROFL-X decodes carries the game deserializer's own
verdict (clean, partial, rejected, fault), every class in the catalog
carries an honest status (TYPED, NAMED, UNNAMED), and every typed field
is checked against something independent, most of them against Riot's
own end-of-game stats. `tests/decode_replays.rs` fails if a block stops
decoding clean or a single self-check stops matching.

---

## Running the reproducer

Phase 1's sample walkthrough is reproducible:

```bash
python scripts/sample_walkthrough.py --replay <a .rofl>
```

Requires Python 3.10+ and the `zstandard` package. Never commits the
replay; prints the annotated structure used in
`docs/SAMPLE_A_WALKTHROUGH.md`.

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

Scoped and triaged, ordered by how much it adds to the decoded output.

### More typed classes (16.9, no reverse engineering needed)

Every class below already decodes clean; typing it means reading its
dumped object and checking the fields against something independent
([docs/PACKETS.md](docs/PACKETS.md) § Adding a class).

- [ ] **Spell names for spell casts.** `PKT_NPC_CastSpellAns_s` is typed
  but carries a 32-bit spell id, not a plain FNV-1a hash of the spell
  name; probably a hash of a game-data path.
- [ ] **CS and gold over time.** Not in a named class; candidates are the
  unnamed per-champion sync (netid 806) and the replication class
  (netid 446). Check against statsJson `MINIONS_KILLED`,
  `NEUTRAL_MINIONS_KILLED`, `GOLD_EARNED`.
- [ ] **`PKT_CHAR_SetCooldown_Broadcast_s`**, **`PKT_Basic_Attack_Pos_s`**,
  **`PKT_S2C_OnEnterTeamVisibility_s`** (39 % of all blocks).
- [ ] **Champion roster from the keyframe hero records** instead of
  "net ids are contiguous in statsJson order" (verified on 16.9, but an
  assumption).

### Parser and tooling

- [ ] **Team count from the replay** for modes with more than two teams
  (Arena); the format-2 team registry stand-in assumes Summoner's Rift.
- [ ] **`tests/zhu_schema_compat.rs`**: 9 of Henry Zhu's 22 classes are
  typed on 16.9, 5 more named.
- [ ] **Format-2 archive for 15.x** from Mowokuma's section dumps (the
  15.5 factory is confirmed, [docs/RE_PATCH.md](docs/RE_PATCH.md)); the
  dumps lack the PE headers `build_patch.py` reads.

### Current patches

- [ ] Blocked: from some patch after 16.9 the binary is packed
  ([docs/COMPATIBILITY.md](docs/COMPATIBILITY.md)). Unblocked only by an
  unpacked binary obtained legitimately; a format-2 archive then builds
  in minutes.

### Parity

- [ ] **Parity runs against Mowokuma's binary on 15.1-15.4**.
  Currently only 15.5 has been measured (99.07 % content match).

---

## Non-goals

- **We will not publish a decoded-replay dataset.** Henry Zhu's Hugging Face
  drops already exist at a scale no fork could realistically match. ROFL-X's
  contribution is the *documented parser*, not the data.
- **We will not ship anti-cheat bypasses.** The parser operates strictly on
  files the user already has on their own disk. If a direction of work
  starts to look like live-game decryption or client-side tamper, we stop
  and flag it. That includes unpacking the encrypted code of current
  League binaries or reading it out of the running game.
- **We will not attempt to statically reimplement the packet decode
  functions.** The obfuscation changes per patch and Mowokuma/Zhu both
  independently concluded emulation is the right answer; we agree, and
  format 2 takes it to its conclusion: the game also picks the decoder.
