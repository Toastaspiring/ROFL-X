# Per-patch compatibility

What ROFL-X can do on each League patch it has been tested against.

Columns:
- **Transport**: header, metadata, chunks and block framing parse
  cleanly (`rofl-x inspect`, `tests/transport_smoke.rs`). This includes
  the end-of-game scoreboard in the metadata, so every patch gets
  K/D/A, level, CS, gold, damage and items per player without any
  archive.
- **Archive**: the per-patch `.patch` archive the emulator needs, and
  its format (1 = Mowokuma's two hand-RE'd decoders, 2 = every class
  through the game's own factory and deserializers, see
  [DECODING.md](DECODING.md)).
- **Decode**: packet-payload decoding (`rofl-x file`) on that patch.
  Every patch additionally supports file-only analysis with no archive
  and no game contact: `rofl-x summary` for the statsJson scoreboard
  and a transport activity profile, `rofl-x explore` for the champion
  roster, a per-champion timeline and netid identification
  ([EXPLORE.md](EXPLORE.md)).

| Patch  | Transport | Archive | Decode |
|--------|-----------|---------|--------|
| 15.1 – 15.4 | green | format 1: Mowokuma `5-1` .. `5-4.patch` | positions + wards (her JSON). Parity untested |
| 15.5   | green | format 1: Mowokuma `5-5.patch` | positions + wards. Parity with her binary: metadata and wards byte-identical; 99.07 % of player entries in `players_state` match (her own output drifts run to run, so full byte identity is not reachable) |
| 16.7   | green (1 replay) | none: no binary of this patch kept | file-only (`summary`, `explore`) |
| 16.8   | green (sample_a) | none: no binary of this patch kept | file-only (`summary`, `explore`) |
| 16.9   | green (5 replays) | **format 2**, built by `scripts/build_patch.py` from `League of Legends.exe` 16.9.772.1032 | **every block decoded clean** (7,927,123 / 7,927,123); 315 classes named, 16 typed (47.0 % of blocks), every class's fields measured ([FIELDS.md](FIELDS.md)); events, positions, wards, items, vision; self-check 350 / 350 |
| 16.10 – 16.18 | untested | unknown | the binary packing below appeared somewhere in this range |
| 16.19  | green (20 replays) | **none possible**: the shipped binary is packed | no emulation. File-only ([EXPLORE.md](EXPLORE.md)): scoreboard, champion entity ids, per-champion timeline; the visibility pair's payload **read without the binary** (37.9 % of blocks, 31,154 labelled champion blocks, 0 exceptions) giving a vision timeline; netid 1080 proved to be the champion-death packet and its killer field read too (199 of 200 player-games predicted right), giving a kill feed; netid 590 proved to be the running score and 601 the death timer; the movement class identified (1267) with its wire format settled; 63 of 273 netids matched to a 16.9 class by behaviour, 62.3 % of blocks |

## The 16.19 binary is packed

From some patch after 16.9, `League of Legends.exe` ships with its
code and data encrypted on disk. Measured on 16.19.823.722:

| | 16.9 | 16.19 |
|-|------|-------|
| sections | `.text .rdata .data .pdata ... .stub` | adds `.riot0 .riot1 .riot2`; entry point inside `.riot1` |
| `.text` entropy (64 KiB windows) | 5.9 – 6.4 | 7.997 everywhere |
| `48 89 5C 24 08` (`mov [rsp+8], rbx`) occurrences in `.text` | 23,276 | 0 |

The code is decrypted at runtime, inside a process Vanguard protects.
Reading it would mean unpacking Riot's protection or reading the running
game's memory; both are anti-cheat circumvention, which ROFL-X will not
do (README § Non-goals). `scripts/build_patch.py` detects a packed
binary and refuses it.

What still works on such patches: everything that does not need the
game's code, which is more than it sounds. `rofl-x summary` reads the
file alone and writes the full end-of-game scoreboard (statsJson: a few
hundred fields per player — KDA, gold, CS, damage breakdowns, vision,
objectives, pings, runes, items, mission trackers) plus a transport
activity profile (per-netid, per-entity and per-minute block counts,
all from plaintext block framing). `rofl-x inspect` prints the same
scoreboard and an opcode histogram. Only the obfuscated packet payloads
stay opaque; the netid, entity id (`param`), timestamp and length of
every block do not.

`rofl-x explore` goes further on the same evidence
([EXPLORE.md](EXPLORE.md)): it names the ten champions' entity ids from
the shape of the traffic (verified against the decoder's own roster on
16.9, 5 of 5), gives a per-champion per-minute timeline, proves the
identity of netids whose per-champion block count *is* one of Riot's
end-of-game counters, and proposes a class for the rest by matching
their behaviour against 16.9. On the twenty 16.19 replays that yields
three netids proved outright, two more settled by when they fire, and
63 confident proposals out of 273.

It also reads one class's payload outright. The obfuscation is a
per-class byte substitution, not a cipher — measurable on 16.9, where
both the raw bytes and the decoded object are in hand — so a class
whose plaintext something outside the file pins down can be recovered
from the file. The visibility pair qualifies, and on 16.19 it decodes
with 31,154 labelled champion blocks agreeing and none disagreeing:
37.9 % of all blocks, and a per-champion vision timeline that matches
the emulator's to within 0.1 s when the same code is run on 16.9. So
does the killer field of the death class, where statsJson's
`CHAMPIONS_KILLED` supplies the labels: 199 of 200 player-games come
out right and the twenty replays yield a kill feed.

What would change this: Riot shipping an unpacked binary again, or an
unpacked binary of a given patch becoming available by legitimate
means. A format-2 archive builds from such a binary in a few minutes,
with no manual reverse engineering.

Assumed but untested: 14.x replays. The transport smoke test accepts
any version that starts with a digit, so parsing should work; semantic
decode needs that patch's binary.

## Known gaps

1. **Current patches.** Emulated decoding is blocked by the packing
   above. The file alone goes a long way further than it used to —
   three payload fields are read on 16.19 with no binary at all, the
   spell-cast class is identified and validated against 16.9, and a
   position comes out to a measured median of 12 map units — but that
   last one is approximate, so it is not emitted. See
   [EXPLORE.md](EXPLORE.md).
2. **Parity runs against Mowokuma on 15.1 – 15.4.** Cheap once a script
   runs both tools and diffs order-normalised JSON.
3. **Game modes with more than two teams** (Arena). The format-2 team
   registry stand-in uses the Summoner's Rift count (4); see
   [DECODING.md](DECODING.md) § Limits.

## How to update this file when adding a new patch

1. `rofl-x inspect --histogram -r <replay>`: the transport layer is
   clean on the new patch.
2. With the patch's binary: `python scripts/build_patch.py --binary
   <exe> --output patch/<major>-<minor>.patch --replay <replay>`. The
   validation line should read 100 % clean.
3. `rofl-x file -r <replay> -o out.json`: every self-check line should
   read `n/n`. `tests/decode_replays.rs` asserts the same over a whole
   replay folder.
4. Add a row to the table above.

For a patch with **no** binary, step 2 is impossible and the honest
run is `rofl-x explore` instead. Give it the new patch's replays and
`--reference` a patch that does have an archive, and `--self-test` so
the netid matcher reports its own error rate before any of its
proposals are believed. [EXPLORE.md](EXPLORE.md) is what that output
means.
