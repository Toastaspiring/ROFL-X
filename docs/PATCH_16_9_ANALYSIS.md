# Patch 16.9 reverse-engineering analysis

Findings from a static analysis pass on `League of Legends.exe` 16.9.771.8383
(MD5 `7bf33f4e0035937d187fa6594afe93df`, captured 2026-04-30).

This is the public companion to the RE_PATCH.md workflow. It records what
**we now know** about 16.9 and what's still open. Entries here are
**discovered**, not guessed; each claim has an evidence pointer.

---

## Dispatch table architecture (confirmed)

Riot kept the same 48-byte-stride dispatch table architecture from 15.5,
but **rotated the slot order and relocated all helpers**. The table is
fragmented across many small sub-tables in `.rdata`, each grouped by a
shared helper at slot[0].

| Slot | 5-5 role               | 16.9 role           | Notes |
|------|------------------------|---------------------|-------|
| 0    | decoder                | shared helper       | usually `0x1dc000` (`ret 0`), sometimes `0x1dbc10` (`xor al,al; ret`) |
| 1    | secondary helper       | **decoder**         | this is where the per-packet decoder lives in 16.9 |
| 2    | tertiary helper        | per-packet helper   | varies |
| 3    | shared helper (`ret 3`)| per-packet helper   | varies |
| 4    | quaternary helper      | shared helper       | usually `0x2173a0` (`mov eax,3; ret`) — **identical body to 5-5's slot[3] helper** |
| 5    | sentinel (`0xffff800885b9ffff`) | per-packet helper | the magic sentinel is gone |

**Storage form change**: 5-5 stored slot values as raw RVAs (low 32 bits).
16.9 stores them as full virtual addresses (`image_base + RVA`,
`0x140000000 + RVA`). Adapt accordingly when reading 16.9's table.

**Verification of equivalence**: the 16.9 helper at RVA `0x2173a0` has body
`b8 03 00 00 00 c3` (`mov eax, 3; ret`) — byte-identical to 5-5's helper
at RVA `0x1c74c0`. Same C++ source (`return 3;` getter), same compiler,
same code, just a different RVA.

## Key 16.9 RVAs

| Symbol                | RVA in 16.9      | Notes |
|-----------------------|------------------|-------|
| Shared helper (slot 0)| `0x1dc000`       | `ret 0` — empty stub |
| Shared helper (slot 0 alt)| `0x1dbc10`   | `xor al, al; ret` — return-false stub |
| Shared helper (slot 4)| `0x2173a0`       | `mov eax, 3; ret` — direct equivalent of 5-5's `0x1c74c0` |
| `.text`               | `0x1000` (size `0x194d1c8`) | |
| `.rdata`              | `0x194f000` (size `0x42cd38`) | dispatch table starts within at file offset `0xfc460` |
| Image base            | `0x140000000`    | from PE optional header |

## Sub-tables in .rdata (538 found)

A "sub-table" is a contiguous run of >= 5 dispatch entries (48 bytes each)
where slot[0] is constant. Each sub-table groups ~10–50 packet classes.

Largest sub-tables by prologue-matching decoder count:

| rdata offset | entries | slot[0] (shared) | prologue matches |
|--------------|---------|------------------|------------------|
| `0x104c38`   | 50      | `0x1dc000`       | 8/50             |
| `0x100730`   | 16      | `0x1dc000`       | 6/16             |
| `0x ff708`   | 35      | `0x1dc000`       | 5/35             |
| `0x fc460`   | 21      | `0x1dc000`       | 3/21             |
| `0x1057f0`   | 18      | `0x1dc000`       | 2/18             |

Across all 538 sub-tables: 5,666 dispatch entries, 2,800 unique decoder
RVAs in slot[1].

## Decoder candidates (3 confidence tiers)

Stored at:
- `~/Tools/analysis/16-9/decoders_confirmed.json` — 41 high-confidence
  (prologue match AND in dispatch table)
- `~/Tools/analysis/16-9/dispatch_table_full.json` — 2,800 medium-confidence
  (in dispatch table, any prologue shape)
- `~/Tools/analysis/16-9/decoder_prologue_candidates.json` — 327 prologue-only
  (donor-shape match, regardless of dispatch reference)

**Recommendation**: start with the 41 high-confidence list when looking
for `mov_decrypt`, `ward_spawn_decrypt`, etc.

## What still needs reverse-engineering

These three gaps remain before ROFL-X can decode a 16.9 replay:

### 1. netid → table-index mapping

The dispatch table has the decoder list, but no entry contains the
netid. The packet-deserialise dispatcher consults a separate structure.
Finding it is the single biggest unblocker.

What we know: each replay block's `packet_id` (a u16) is used by Riot's
client to pick the right dispatch entry. The mapping likely lives as
either:
- A sorted `(netid, entry_index)` array somewhere in `.rdata`
- A perfect-hash function plus an index array
- A jump table inside the deserialise dispatcher in `.text`

What to look for in Ghidra: the function that takes a u16/u32 packet_id
and either calls a dispatch entry directly or returns a struct pointer.
It will reference the dispatch sub-tables we've located.

### 2. Identifying mov_decrypt / ward_spawn_decrypt specifically

Among the 41 high-confidence decoder candidates, two are the equivalents
of 5-5's `mov_decrypt` and `ward_spawn_decrypt`. To identify which:

- Filter by size: 5-5 mov ≈ 1061 bytes, ward ≈ 9649 bytes. 16.9 versions
  are likely 0.5x–2x of those.
- Once netid mapping (gap 1) is solved, mov_decrypt is the entry whose
  netid matches the highest-frequency non-heartbeat opcode in a 16.9
  replay (per `rofl-x inspect --histogram`).
- Or: feed real payload bytes through each candidate via `trace-decoder`
  and look for output that resembles the documented `PathPacket` /
  `WardSpawnPacket` shape.

### 3. alloc1, alloc2, skip helpers

Mowokuma's emulator stubs three functions in 5-5:
- `alloc1` at `0xf60420` — heap allocator wrapper
- `alloc2` at `0x1de520` — second heap allocator wrapper  
- `skip`   at `0xfca950` — safety check function (overwritten with `mov rax, 1; ret`)

For 16.9 these need to be re-discovered. The simplest approach once a
decoder RVA is known: disassemble it and find the three functions it
calls before its main loop. Those are the equivalents.

## Tool support

This analysis was produced by the following Python helpers, all
runnable against any PE binary:

```python
# 1. Find decoder candidates by donor-prologue (28-byte signature)
#    Input: League .exe + 5-5 prologue bytes
#    Output: list of RVAs that share the donor's exact prologue

# 2. Find dispatch sub-tables in .rdata
#    Heuristic: 48-byte stride windows where slot[0] is a constant .text VA
#    for >= 5 consecutive entries

# 3. Cross-reference: find prologue candidates that also appear in slot[1]
#    of a dispatch sub-table → high-confidence decoders
```

These can be productionised as `rofl-x scan-dispatch-table` and
`rofl-x find-decoders` subcommands. For now they live as ad-hoc scripts
under `~/Tools/analysis/16-9/`.

## Conclusion

16.9 is **not** a complete rebuild from 15.5 — the dispatch architecture
survived, just rotated. With this map, identifying any single specific
decoder is now a **finite Ghidra session** (find netid mapping → find
the table entry → trace → write handler) rather than an open-ended
search across 50,000+ functions.

## Update: deeper static analysis (May 2026)

Pushed further on automated identification. Confirmed and narrowed:

### Confirmed helpers

- **`skip` at RVA `0x11b8430`** (183 bytes, **88.5%** byte-identical to
  5-5's `skip` at `0xfca950`, only relocation diffs). High confidence.
- **`alloc2` candidate at `0x230f90`** (32-byte prologue match). Medium
  confidence; full body untested.

### Decoder candidate narrowing

Of the 41 prologue+dispatch high-confidence candidates:

- **8 are mov-sized** (size 530..2122 bytes; 5-5 mov was 1061):
  closest are `0xfb4070` (1086B), `0xfc2fe0` (1046B), `0xfd6270` (960B).
- **16 are ward-sized** (size 4824..19298 bytes; 5-5 ward was 9649):
  closest is `0xea9ec0` (9919B).
- All 8 mov candidates call **`0x11b8430` (skip)** and **`0xecc180`**
  (a 127-byte bit-reader helper used by every decoder).
- All 16 ward candidates also call `0xecc180`; 14/16 call `0x11b8430`.

### Likely netids in 16.9 replays

Per-netid payload size distribution (from `extract-fixture` probes on
a 46-min 16.9 replay):

| Netid | Freq    | Payload size pattern  | Likely class |
|-------|---------|----------------------|--------------|
| 1068 (0x42c) | 46.5% | constant 2 bytes | UpdateState heartbeat |
| 707  (0x2c3) | 5.5%  | constant 2-3 bytes | LeaveFog/EnterFog (small visibility events) |
| 684  (0x2ac) | 5.2%  | constant 3 bytes | similar visibility class |
| 652  (0x28c) | 4.2%  | constant 17 bytes | small fixed packet, possible state flag |
| 916  (0x394) | **2.8%** | **variable 18-352 bytes** | **best mov_decrypt candidate** |
| 446  (0x1be) | 1.0%  | variable 58-493 bytes | replication / large state |

### Why end-to-end emulation didn't succeed

Built a best-guess `16-9.patch` archive and ran `trace-decoder` against
the 4 closest mov-sized candidates with netid 916. **All produced zero
writes**. Diagnosis: when `alloc1`/`alloc2` stubs target the wrong
function bodies, the decoder hits unmapped memory or malformed control
flow and bails before writing to the output struct. The emulator
silently swallows these failures.

The remaining gap is `alloc1`. 5-5's `alloc1` (RVA `0xf60420`, 195 bytes)
has zero byte-pattern matches in 16.9 — Riot rewrote it. Finding the
new `alloc1` requires either:

1. Identifying it from a confirmed decoder's call graph (chicken-and-egg
   without a confirmed decoder).
2. Disassembling one of the 8 mov-sized candidates in Ghidra and
   following the allocator-style call before its main loop.

### Where to take it from here

**Recommended next session (~1 hour, interactive Ghidra)**:

1. Open Ghidra (the 16.9 project at `~/Tools/analysis/16-9-project/`
   has finished analysis and is ready).
2. Navigate to `0xfb4070` (the closest-size mov candidate).
3. Look at the 3-5 functions it calls in the first 100 instructions.
4. Identify which call returns a buffer pointer (allocator) — that's
   the new `alloc1`. Stub it in the patch archive and retry trace-decoder.
5. If trace-decoder produces writes at offsets 0x18, 0x20 (or close),
   that confirms `0xfb4070` as `mov_decrypt`. Update the catalog.
6. Same flow for `ward_spawn_decrypt` against `0xea9ec0` etc.

This is the irreducible 1-hour Ghidra task that closes the
last gap.

---

## Confirmed decoder map (16.9)

Filled in incrementally from brute-force matching + decompiler review.
Each entry maps a netid to a decoder RVA, what we believe it represents
(from frequency + payload signature + Zhu's S12 schema), and the field
offsets we've seen the decoder write to.

| Netid    | Decoder RVA | Freq    | Likely Class                    | Notes |
|----------|-------------|---------|---------------------------------|-------|
| 1068     | `0xf9d4e0`  | 46.5%   | `UpdateState` or `LeaveFog`     | 2B payload, single 4B output at 0x10. High freq + tiny payload = heartbeat-class. |
| 916      | `0xfb4070`  | 1-3%    | `WaypointGroup` (mov)           | Variable 18-352B. Writes vector at 0x10/0x18, inline final pos at 0x20/0x24. |
| 707      | `0xfaffc0`  | 5.5%    | `EnterFog` or visibility event  | 2-3B fixed payload, scalar field. Other half of the LeaveFog/EnterFog pair. |
| 684      | `0xfdb200`  | 5.2%    | `DoSetCooldown` candidate       | 3B fixed payload, two fields: 1B at 0x8 (slot?) + 4B at 0xc (cooldown_ms?). |
| 652      | `0xeba9e0`  | 4.2%    | `Replication_short` candidate   | 17B fixed, multiple atomic fields (-1, 0, 1, 2 values). Per-property update. |
| 806      | `0xeba9e0`  | 4.1%    | `BuffAdd` or `Replication_long` | 136B fixed, larger struct. Status effect or many-property packet. |
| 446      | `0xeba9e0`  | 2.6%    | `Replication_variable`          | Variable 58-493B = variable-length property dict. Zhu's `Replication` was 15.8% in S12. |
| 876      | `0xf8f840`  | 0.9%    | `CastSpellAns` candidate        | 30B fixed, complex struct. Skill cast metadata (caster, spell, targets). |

**Caveat on shared decoders**: `0xeba9e0` wins as the brute-force top
match for several netids (652, 806, 446, plus several lower-frequency
ones it's filtered out of by gap heuristic). It writes to ~70 distinct
offsets per call — too many for a single packet class. Two
interpretations:

1. **It's a generic class deserializer** that handles multiple packet
   variants (sub-classes). If so, the netid → variant routing happens
   inside the function and we'd need to RE that routing to fully
   decode each variant.
2. **It's a "loud" shared helper** (e.g., a base-class initializer)
   that other decoders call into, and the brute-force is finding it
   instead of the per-class decoder.

Either way, brute-force results for these netids are LOWER CONFIDENCE
than for netids with cleaner gap-to-runner-up separation.

## Field-level decoding caveat

The output struct after a decoder returns holds RE-OBFUSCATED bytes
(Henry Zhu's "decrypt-access-release" pattern). Plaintext lives only
during decode, written by atomic 4-byte stores BEFORE a byte-by-byte
obfuscation loop encrypts them.

`replay_info::build_output_json` captures the LAST atomic write per
offset (size >= 4) before obfuscation. For most fields this recovers
the plaintext value. The recovered values are surfaced as
`decoded_fields[]` in the JSON output, with multiple type
interpretations (`u32_le`, `i32_le`, `f32_le`) since the decoder
itself doesn't carry type metadata.

What we DON'T yet know (the genuine "100% true decoding" gap):

- What each field MEANS per packet class (which f32 is HP vs. mana
  vs. position?)
- Whether the encoded value is then transformed by game logic before
  use (some fields are bitfields, some are scaled by a global factor)

Naming each field requires either reading Riot's source (we can't),
correlating decoded values with observable game state across many
replays (statistical), or per-class manual analysis.
