# ROFL-X

A documented parser for League of Legends replay files (`.rofl`), written in
Rust. Extends [Mowokuma's ROFL](https://github.com/Mowokuma/ROFL) (an
already-excellent parser) and pairs it with a **written packet catalog** so
that patch-to-patch regressions become detectable instead of silent.

ROFL-X exists because Riot publishes no official documentation of the ROFL
format, and every existing parser extracts a narrow slice of the data that
is actually in the file.

**The goal is the whole file: a `.rofl` in, and JSON out describing
everything that happened in the match** — every player's movement and
move orders, spells cast, attacks, damage dealt, deaths and respawns,
items bought and used, wards placed and killed, structures destroyed,
every entity created and destroyed, and who could see what and when.
Not a summary of the match: the actions it was made of.

That is not finished, and the catalog says so rather than hiding it —
every packet class carries an honest status, and the ones we cannot
name yet are recorded as unnamed so the gaps are visible.

It works two ways, and the second matters more as time goes on. Where an
unpacked client binary of the patch exists, the game's own deserializers
decode the payloads. Where it does not — every patch since 16.9 ships its
code encrypted — the replay is still a file on your disk, and a surprising
amount of it reads without the game's help at all
([docs/EXPLORE.md](docs/EXPLORE.md)).

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

### With the game's own code (patch 16.9)

Where an unpacked `League of Legends.exe` of the patch exists, ROFL-X
decodes every packet through the game itself: its netid factory builds
each packet object and its own deserializer reads the payload, inside a
Unicorn VM ([docs/DECODING.md](docs/DECODING.md)). On the five 16.9
replays we hold:

- **7,927,123 / 7,927,123 blocks decode clean** — the deserializer
  returned true and consumed exactly the payload.
- **Riot's own class name for 151 of the 273 netids a replay uses**,
  75.9 % of all blocks, recovered from the binary's handler
  registrations. The binary registers 315 in total.
- **Every class's field layout measured**, all 273 of them, in
  [docs/FIELDS.md](docs/FIELDS.md): per 4-byte word, whether it is a
  net id, a coordinate, game time, an enum, a counter, or a pointer to
  an array, and what the array's entries look like.
- **16 classes typed** — fields named *and* independently checked:
  champion deaths with killer and assists, respawns, K/D/A, levels,
  damage, spell casts, item buys / removals / swaps / uses, NPC and
  building deaths, team visibility, movement paths, unit spawns. 47.0 %
  of all blocks.
- **Every per-player total rebuilt from those packets matches Riot's
  own end-of-game record in the same file**: K/D/A, kill events, level,
  all seven final item slots, damage to champions, wards placed, and
  visibility. **350 / 350**.

On **15.1 – 15.5**, Mowokuma's format-1 archives give positions and
wards, at parity with her binary on 15.5.

### Without it (patch 16.19, and every patch since)

From some patch after 16.9 the shipped binary keeps its code encrypted
on disk, so there is no deserializer to run, and ROFL-X reads replays
rather than touching the game (§ Non-goals). The replay is a different
matter: it is a file, and most of it was never obscured. On the twenty
16.19 replays we hold, with **no binary at all**
([docs/EXPLORE.md](docs/EXPLORE.md)):

- **The full end-of-game scoreboard**, a few hundred fields per player,
  and a transport activity profile — `rofl-x summary`.
- **Which entity is which champion**, from the shape of the traffic.
  The same code on 16.9, where the decoder knows the answer
  independently, gets it right in 5 replays out of 5.
- **Three payload fields read outright**, because the obfuscation is a
  per-class byte substitution and those classes' plaintext is pinned
  from outside: the two visibility classes' team byte (37.9 % of all
  blocks, 31,154 labelled champion blocks, 0 exceptions) and the death
  class's killer field (199 of 200 player-games predicted right). Out
  of them come a **per-champion vision timeline** and a **kill feed**.
- **Netids identified by proof rather than guess**: 1080 is the
  champion-death class and 590 the running score, each holding in 19 of
  19 replays against Riot's own counters; 601 is the death timer,
  settled three separate ways.
- **63 of 273 netids matched to a 16.9 class by behaviour**, 62.3 % of
  blocks — hypotheses, with a measured error rate.
- **One class's obfuscation turns out to have structure.** The cast
  class's substitution is `perm(plaintext) XOR 0xe6`, a permutation of
  the eight bit positions — affine over GF(2) on all 256 entries, and
  checked by decoding 11,898 held-out fields exactly. That makes its
  table 16 bits rather than 256 free choices, so nine pairs determine
  it (measured: 247 of 247 predicted). Five other classes, including
  the movement class, fit nothing of the kind — so it is a per-class
  property worth testing, not a way in to all of them.
- **Spell casts located:** netid 474 carries spell casts. It
  tracks the cast counters (r = +0.827) and not activity
  (r = −0.019 against minions killed); its surplus over the counter
  falls on the champions whose abilities are held or recast (Pantheon
  +206 %) and not on the point-and-click ones (Annie −9 %); and the
  same analysis picks the real cast class first on 16.9. Which Riot
  class it *is* stays unsettled: its payload shares the head signature
  of 16.9's `PKT_NPC_CastSpellAns_s` but not that class's trailing
  coordinate triple ([docs/EXPLORE.md](docs/EXPLORE.md) § 3). Located,
  not decoded — reading which ability at whom needs its table.
- **Items and wards located**, on the seven Summoner's Rift replays of
  the set: netid 890 for purchases, 524 for item removal — which the
  fingerprint matcher independently and confidently calls
  `PKT_RemoveItemAns_s` — and 467 for ward placement. Thinner evidence
  than the casts, four to six games each.
- **Positions, to a measured accuracy.** 27 of the movement class's
  256 table entries are exact, recovered from the coordinate high
  byte's slow drift, and they cover the whole playable map at that
  byte's resolution. A constant-speed search then fills the low byte
  in approximately: scored against 16.9, where the true table is
  known, a **median of 4 map units**, nine times in ten within 10,
  and never worse than 32 — every one of the 256 entries lands within
  16 steps of right, and the high byte being exact bounds the rest. A
  champion hitbox is about 65 units, so that is trajectory accuracy.
  Accurate is still not exact — 53 of 256 entries are actually right —
  so no positions are emitted from it yet.

Per-patch detail: [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md). Plan:
[docs/ROADMAP.md](docs/ROADMAP.md).

---

## Usage

```bash
cargo build --release          # needs cmake + a C toolchain for unicorn
```

Scoreboard and transport summary of any replay, from the file alone,
no archive needed (works on every patch, packed-client ones included):

```bash
rofl-x inspect -r EUW1-1234567890.rofl --histogram   # printed overview
rofl-x summary -r EUW1-1234567890.rofl -o summary.json
```

`summary` writes the full statsJson scoreboard per player (Riot IDs and
PUUIDs reduced to their length unless `--include-identities`) and a
transport activity profile. No game binary is read and no payloads are
decoded.

Go further on the same evidence — who the champions are, how busy each
was minute by minute, and what each packet class is — still with no
binary and no payload ([docs/EXPLORE.md](docs/EXPLORE.md)):

```bash
rofl-x explore -r EUW1-1234567890.rofl -o explore.json
rofl-x explore -r <packed patch>.rofl --reference <16.9 replay> --self-test
```

With `--reference`, netids are matched against a patch ROFL-X can
decode, so each one gets a proposed class and a score. `--self-test`
first measures how often that matching is right on a patch where the
answer is known. `--dump-netid N` writes every block of one netid with
its raw payload bytes, which is where payload work starts.

On a packed patch this still reads some payloads outright, because the
obfuscation is a per-class byte substitution and the tables can be
recovered from plaintext the file itself pins down. The visibility
classes (37.9 % of all blocks) come from the champions' known teams;
the death class's killer field comes from statsJson's
`CHAMPIONS_KILLED`, giving a kill feed. The same code on 16.9 agrees
with the emulator to within 0.1 s, which `tests/decode_replays.rs`
asserts.

Decode a replay (patch archive in `./patch`, named `<major>-<minor>.patch`):

```bash
rofl-x file -r EUW1-1234567890.rofl -o game.json
```

Output: `players` (with net ids), `events` (kills with assists,
respawns, level-ups, spell casts, purchases, removals, item uses,
wards placed, buildings destroyed), `players_state` (per-second positions) and
`wards` (lifecycles), both in Mowokuma's shape, `vision` (when each
champion was inside the enemy team's vision, and for how long),
`checks` (the self-check) and `decode` (block counts). No Riot ID or
PUUID is written.

What every class carries, measured rather than guessed: decode a
sample of each netid's blocks and report, per word of the packet
object, what the value was across the sample (sender net id, net id,
game time, float, enum, counter, pointer to a vector or a string).
[docs/FIELDS.md](docs/FIELDS.md) is this run over the five 16.9
replays, one table for each of the 273 classes they use:

```bash
rofl-x fields -r EUW1-1234567890.rofl --samples 2000 --markdown FIELDS.md
```

Research view, every block of one netid as plaintext object words,
and one class's payload worked out byte by byte:

```bash
rofl-x decode -r EUW1-1234567890.rofl --netid 472 --jsonl out.jsonl
rofl-x decode -r EUW1-1234567890.rofl --find YellowTrinket
rofl-x decode -r EUW1-1234567890.rofl --netid 201 --layout
```

Add `--heap` to a `--jsonl` run to get the plaintext bytes of every
buffer the deserializer allocated, alongside the raw payload. That is
what a class carrying a list puts its contents in, and none of it
appears in the packet object.

`--layout` reports which object field each payload byte carries, the
order and byte order the class writes its fields in, and the
substitution table that turns one into the other — measured against
the deserializer's own output, not guessed. For a class that carries a
list in an allocated buffer rather than in object fields, it also
works out how that buffer sits inside the payload; the waypoint class
turns out to write it **backwards**, and with that the table comes out
complete, 256 of 256 entries with no conflict. It is what says where
to look on a patch whose payloads cannot be decoded
([docs/EXPLORE.md](docs/EXPLORE.md)).

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
| [docs/FIELDS.md](docs/FIELDS.md) | Measured field layout of every class on 16.9, generated |
| [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) | Per-patch coverage, and why current patches are blocked |
| [docs/EXPLORE.md](docs/EXPLORE.md) | How far a replay goes with no binary at all, including reading a payload without one: the 16.19 run |
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
decoding clean, if a single self-check stops matching, or if the
file-only path stops agreeing with the decoded one.

The same rule governs the work on patches with no binary, where there
is no deserializer to vouch for anything. Everything claimed there is
labelled by *how* it is known, and the three grades are not
interchangeable:

- **Measured.** An identity that has to hold ten players at a time in
  game after game — a netid whose per-champion block count *is* one of
  Riot's end-of-game counters, or a payload byte whose meaning is
  forced by a rule of the game. These are reported as facts.
- **Checked against a known answer.** Where a method can be run on a
  patch we *can* decode, it is, and the error rate is published rather
  than asserted. The netid matcher reports its own calibration table;
  the file-only champion roster is compared against the decoder's.
- **Hypothesis.** A behavioural match with a score and a margin. These
  say so in the output and in the docs, with the measured precision of
  the gate that admitted them, and nothing is built on top of one.

Negative results are kept for the same reason. Per-class substitution
tables turn out to be unrelated to each other, RTTI turns out to be
stripped for packet classes, and the running-score class turns out to
pack its numbers in bits rather than bytes — all three are written
down with their measurements, because each one closes a line of attack
that would otherwise look promising twice.

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

Scoped and triaged. The frontier is the first section: patches whose
client ships packed, which from 16.10 onwards is all of them.

### Patches with no binary (16.19 and later)

Done:

- [x] File-only analysis (`rofl-x summary`): the end-of-game scoreboard
  and a transport activity profile, no game contact.
- [x] `rofl-x explore`: champion entity ids from framing alone (right
  in 5 of 5 on a patch where the decoder knows the answer), a
  per-champion per-minute timeline, and a netid matcher with a
  published calibration table.
- [x] Identify netids by **proof**: a per-champion block count that
  equals one of Riot's end-of-game counters, including short sums of
  them. Gives 1080 as the champion-death class and 590 as the running
  score, each in 19 of 19 replays.
- [x] Identify netids by **when they fire**, using the kill feed as a
  label. Four hits on the control patch, all correct, including one
  nobody would have gone looking for; two on 16.19, one of them found
  by nothing else.
- [x] **Read a payload with no binary**, by recovering a class's byte
  substitution from plaintext the file itself pins down. Done for the
  visibility pair (37.9 % of blocks, 0 exceptions over 31,154 labelled
  blocks) and the death class's killer field (199 of 200 player-games).
- [x] `decode --layout`: a class's payload byte by byte and its
  substitution table, measured against the deserializer's own output on
  a patch that has one. Recovers all 256 entries of two classes with
  no conflict, and works out that the waypoint class writes its buffer
  backwards.

- [x] **Coarse positions.** 27 of the movement class's 256 table
  entries, recovered from the coordinate high byte's slow drift and
  checked against the flags byte. Covers the whole playable map to
  within about 512 units on its own; the low byte below closes most
  of the rest ([docs/EXPLORE.md](docs/EXPLORE.md) § 3).

Next:

- [ ] **Exact positions.** The coordinate's low byte is approximate
  rather than unknown: it moves by most of its range between packets,
  so the drift trick does not reach it, but making every unit travel
  at one fitted speed does — a median of **4 map units** on 16.9 with
  every entry within 16 steps, scored by `scripts/lowbyte_probe.py`,
  and the search cost tracks that score closely enough to stand in for
  it on a patch with no table to check against. What is missing is
  exactness: 203 entries are close but wrong, and the cost stops
  falling, so that gap is the objective's limit rather than the
  search's. The obvious next idea is measured and does
  **not** work — the path deltas give a path's shape exactly but yield
  no exact constraint on the table, three separate ways
  ([docs/EXPLORE.md](docs/EXPLORE.md) § 3). What is left is stronger
  soft models: the displacement between two packets lies on the path's
  polyline, which is true but only readable for 2,770 pairs today.
- [ ] **The running score's numbers.** Netid 590 is identified but its
  three counters are bit-packed rather than byte-aligned, so the
  technique that read the killer does not reach it. Reading it would
  give assists, which statsJson only gives as a total.
- [ ] Fit the matcher's weights to a measured *cross-patch* error rate
  instead of setting them by hand. Needs two patches with both a
  decodable binary and replays; we have one.
- [ ] A scoreboard CSV across a folder of replays.

Blocked, and staying blocked:

- [ ] Emulated decoding. From some patch after 16.9 the binary ships
  packed ([docs/COMPATIBILITY.md](docs/COMPATIBILITY.md)). Unblocked
  only by an unpacked binary obtained legitimately — a format-2
  archive then builds in minutes — and not by us going after Riot's
  protection (§ Non-goals).

### More typed classes (16.9, no reverse engineering needed)

Every class below already decodes clean; typing it means reading its
dumped object and checking the fields against something independent
([docs/PACKETS.md](docs/PACKETS.md) § Adding a class). All 273 have a
measured field table in [docs/FIELDS.md](docs/FIELDS.md) to start from.

- [ ] **`PKT_CHAR_SetCooldown_Broadcast_s`** (104),
  **`PKT_Basic_Attack_Pos_s`** (831) and its minion twin (876), and the
  busiest unnamed classes: 707 (5.0 %), 806 (3.3 %), 684 (3.1 %), 406
  (2.3 %).
- [ ] **CS and gold over time.** Not in a named class; candidates are
  the unnamed per-champion sync (netid 806) and the replication class
  (netid 446). Check against statsJson `MINIONS_KILLED`,
  `NEUTRAL_MINIONS_KILLED`, `GOLD_EARNED`.
- [ ] **Spell names for spell casts.** `PKT_NPC_CastSpellAns_s` is
  typed but carries a 32-bit spell id, not a plain FNV-1a hash of the
  spell name; probably a hash of a game-data path.
- [ ] **Champion roster from the keyframe hero records** instead of
  "net ids are contiguous in statsJson order". Verified on 16.9 and
  cross-checked by the file-only detector, but still an assumption.

### Parser and tooling

- [ ] **Team count from the replay** for modes with more than two teams
  (Arena); the format-2 team registry stand-in assumes Summoner's Rift.
- [ ] **`tests/zhu_schema_compat.rs`**: 9 of Henry Zhu's 22 classes are
  typed on 16.9, 5 more named.
- [ ] **Format-2 archive for 15.x** from Mowokuma's section dumps (the
  15.5 factory is confirmed, [docs/RE_PATCH.md](docs/RE_PATCH.md)); the
  dumps lack the PE headers `build_patch.py` reads.

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
- **We will not hand-reverse-engineer the obfuscation out of the
  binary, patch by patch.** Where an unpacked client exists, emulating
  it is simply better: Mowokuma and Zhu each concluded the same
  independently, and format 2 takes it to its conclusion by letting the
  game pick the decoder too. Re-deriving per-patch decode functions by
  hand is work that the emulator does for free and more reliably.

  This is **not** a limit on recovering a class's encoding *from a
  replay*, which is a different thing and is squarely a goal. Where no
  binary exists to emulate, that is the only route to the payloads, and
  ROFL-X takes it: a class's byte substitution is recovered from the
  file, using plaintext the file itself pins down — Riot's own
  end-of-game numbers, or a rule of the game that fixes what a field
  can mean. Three fields on patch 16.19 are read that way already
  ([docs/EXPLORE.md](docs/EXPLORE.md) § 3), and the intent is to keep
  going until the replay reads as completely as a decodable patch does.

  The line that does hold is the one about the game, not about the
  file: nothing here unpacks Riot's protection, reads the running
  client, or is any use against a live match. A replay is a record of
  a game that already finished, sitting on the disk of someone who
  played it.
