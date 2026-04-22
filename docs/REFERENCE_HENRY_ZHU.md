# Reference: Henry Zhu's "League of Legends data scraping" write-up

Notes extracted from Henry Zhu's 2025 blog post:
https://maknee.github.io/blog/2025/League-Data-Scraping/

The post is the second major data point we have on the ROFL packet layer (after
Mowokuma's source). This document records what the post teaches us, what it
confirms from Mowokuma, what it adds, and what it does *not* cover.

**Attribution:** techniques and findings here are Henry Zhu's; any framing is
ours.

---

## TL;DR

- The "encryption" in replays is **custom lookup-table obfuscation**, not a
  standard cipher. There is nothing to decrypt in the cryptographic sense.
- The safest way to recover plaintext is to **let the game's own code decrypt
  values and read them out of CPU registers at the right moment**. Two variants
  of this exist: Mowokuma runs the decrypt function end-to-end in a Unicorn VM
  and reads the output struct; Henry Zhu installs INT3 breakpoint hooks and
  reads registers as the function is executing. Both depend on per-patch RVAs
  from the game binary.
- The client implements a **decrypt-access-release** pattern: packet fields
  stay encrypted in memory and are decrypted on demand, used, then re-encrypted
  (and the plaintext is explicitly zeroed before the destructor runs). This is
  anti-reverse-engineering, not anti-tamper.
- Packet processing in the game is three-stage: `Packet::Packet` (allocation)
  → `DeserializePacket` (in-memory shape) → `UsePacket` (apply to game state).
  Our domain model should reflect this.
- Henry Zhu's scope is much broader than Mowokuma's: damage, spell casts,
  basic attacks, summoner creation, entity creation, state updates, deaths,
  fog-of-war visibility.
- He has published **1.4M+ decoded replays on Hugging Face** as public
  datasets. This is candidate parity data.

---

## What the post covers

Scope is the **decrypt + access layer**, how an in-memory packet object is
populated from a chunk's bytes, and how fields are read back out.

It deliberately does *not* document:

- Magic bytes or byte offsets of the ROFL outer container.
- The compression algorithm (Mowokuma's source tells us zstd).
- Blowfish / AES / any named cipher (there isn't one).
- The chunk-header layout.
- The block-framing / marker-byte encoding inside a decompressed chunk
  (Mowokuma's `parser/block.rs` has this).

So the two references are complementary: Mowokuma → outer format + block
framing, Henry Zhu → packet-field decrypt layer + semantic catalog.

---

## Confirms from Mowokuma

- Parsing requires the game binary, not just the `.rofl` file. Static
  reimplementation of the decrypt logic breaks every patch.
- The decrypt path works at the level of a *per-packet-type* function (you
  have to know which function to call / hook for which packet type).
- Rayon-parallelisable at the packet-batch level.
- Output is JSON.

---

## New information beyond Mowokuma

### 1. The "encryption" is custom obfuscation

The post shows decompiled snippets with shapes like:

```
v = (16843008 * ((2050 * v10) & 0x22110 | (32800 * v10) & 0x88440))
XOR constants: 0xF2, 0xE6, 0xED, 0xE7
Byte rotations: __ROL1__, __ROR1__
A 255-byte lookup table (RVA 0x1019891E0 in the build he looked at)
```

Characteristics:

- Bit-slice manipulations combined with table lookups.
- Different lookup tables *per function*.
- Changes per patch, the constants, the tables, and the operations all shift.

This is not a cipher we can hope to reverse-engineer cleanly and implement in
Rust. It is purpose-built to be tedious to statically port, which is precisely
why both prior-art authors reach for emulation.

### 2. The decrypt-access-release pattern

Quoted from the post:
> "The decrypt-access-release cycle … when checking if Ezreal is the target,
> the `id` field is decrypted, compared, and immediately re-encrypted and
> deleted."

What it implies for us:

- Plaintext values never live in the packet object for long.
- The emulator approach must intercept values *during* the access window,
  either by reading the output struct right after the decrypt returns
  (Mowokuma's strategy) or by hooking the specific instruction where the
  plaintext is live in a register (Henry Zhu's strategy).
- A third strategy, scanning process memory for decoded values, doesn't
  work here, because the decoded values are gone by the time you look.

### 3. The three-stage packet lifecycle

Henry Zhu describes the game's packet handling as:

1. **`Packet::Packet`**, allocation. The packet metadata and struct space
   are set up.
2. **`DeserializePacket`**, the chunk's raw bytes are moved into the packet
   object's fields, still in obfuscated form.
3. **`UsePacket`**, game state is updated from the packet. Individual field
   accesses inside this stage are where decrypt-access-release cycles fire.

Our domain model (`docs/DOMAIN.md` in Phase 2) should mirror this: a
`Packet` is not just bytes-with-opcode, it's an object with a lifecycle, and
our "semantics" column in the catalog is about what `UsePacket` does.

### 4. Exception-based hooking (the second emulator strategy)

Mowokuma runs the decrypt function in a Unicorn VM and reads the output
struct by offset. Henry Zhu's approach:

- Map the game binary into an emulator.
- Identify the instruction(s) where the decrypted value is live in a
  register (e.g. `RSI` = object id, `RAX` = pointer, `XMM0` = damage float).
- Overwrite those instructions with `INT3`.
- Install an exception handler that reads the register(s), then emulates
  the replaced instruction(s) and resumes.
- Use trampolines to avoid having to stitch state manually.

Example hook from the post:
```
emulator.hook_address(TakeDamageAddress, move |emulator, context| {
    let damage_float = convert_xmm_to_float(context.xmm0);
    // …
});
```

**Trade-offs vs. Mowokuma's approach:**

|                            | Mowokuma (whole-fn)        | Zhu (register hooks)         |
|----------------------------|----------------------------|------------------------------|
| Per-patch config cost      | RVAs + struct offsets      | RVAs + register-at-inst map  |
| Robustness to code changes | Struct layout can shift    | Register allocation can shift|
| Output richness            | Only final struct fields   | Any live register anywhere   |
| Speed                      | Heavy setup per packet     | Lighter per hook             |
| Implementation complexity  | Unicorn + patching allocs  | Emulator with INT3 + hooks   |

Both are fragile in different ways. Neither is obviously superior; for ROFL-X
we should probably **adopt Mowokuma's whole-function approach for packets she
already handles (positions, wards)** to maintain parity, and **consider
register-hook style only if we need to extract intermediates that the output
struct doesn't expose**.

### 5. Packet catalog, Henry Zhu's named types

These are opcodes / packet classes the post describes. We should seed our
`docs/PACKETS.md` with entries for each, even as `OBSERVED-ONLY` for the
ones we can't yet replicate.

| Packet class             | Status in post     | Notes                                                |
|--------------------------|--------------------|------------------------------------------------------|
| `TakeDamagePacket`       | DOCUMENTED (type `0x2385`) | target id @+16, damage f32 @+24, source id @+36 |
| `BasicAttackAtTarget`    | DOCUMENTED         | source/target ids + positions, windup, spell meta    |
| `CastSpell`              | DOCUMENTED         | caster, spell name, level, src/tgt pos, windup, cd, mana, slot |
| `CreateSummoner`         | DOCUMENTED         | time, champion id, name, summoner id                 |
| `CreateEntity`           | DOCUMENTED         | entity spawns (wards among them)                     |
| `UpdateState`            | DOCUMENTED         | per-entity stats (hp, ms, …)                         |
| `Death`                  | DOCUMENTED         | victim id + timestamp                                |
| `BecomeVisibleInFogOfWar`| DOCUMENTED         | visibility on                                        |
| `LeaveFromFog`           | DOCUMENTED         | visibility off (noted as ~2.8% redundant repeats)    |

The only opcode number the post hands us directly is `0x2385` for damage.
Mowokuma's config provides numeric netids for ward spawn (e.g. `272`) and
movement, Henry Zhu's names vs. Mowokuma's netids **have not been
cross-referenced yet**. A clear mapping would be:

- Henry Zhu's `CreateEntity` ≟ Mowokuma's `ward_spawn_decrypt.netid`?
  Plausible: ward spawn is one kind of entity creation.
- Henry Zhu doesn't seem to name a "movement/path" packet distinctly, his
  list doesn't include per-tick positions; he derives motion from the
  waypoint-carrying packets embedded in `BasicAttackAtTarget` / `CastSpell`
  and so on.
- Mowokuma's `mov_decrypt.netid` may correspond to a packet Henry Zhu
  lumps under `UpdateState` or handles via a different channel. Unknown.

Resolving this mapping is a Phase 4 task (packet-catalog expansion).

### 6. Specific RVAs and constants observed

Noted in the post (build-specific; will not match ours):

- `0x101BECE88`, `0x101BED530`, `0x101BE9618`, packet vtable addresses
- `0x1019891E0`, 255-byte lookup table
- `0x101C60850`, global ID → object-pointer map
- `0x1920`, offset in object vtable for damage-application function
- XOR constants `0xF2, 0xE6, 0xED, 0xE7`
- Bit masks `0x22110, 0x88440`; multipliers `2050, 32800`

The `0x101xxxxxxx` RVAs look Mach-O-ish (typical PIE-base addresses on
macOS). Mowokuma's addresses are `0x7ff76afd0000`-based, which is Windows
x64 PE with a high user-mode base. **These might be from different platform
builds of the same game.** Worth confirming if it matters to our patch
pipeline.

### 7. Performance numbers

- 3 seconds per 15-minute, 11.5 MB replay.
- ~135 MB of prettified JSON output per replay.
- Peak memory ~400 MB during parse.
- Alternative approach (running the full game client and sampling state) is
  ~96 seconds for a 26-minute replay, i.e. ~30× slower.

For ROFL-X this sets a sanity target: we should be within 2–5× of Mowokuma's
speed on the same replay, and our output size should be comparable (probably
smaller by default; larger with `--full`).

### 8. Coordinate convention

Henry Zhu stores positions as `{x, z}` with Y implicit. This is Unity/LoL
3D world convention, Y is vertical (elevation). Mowokuma stores `(x, y)`
as u16-derived floats; those are actually the (x, z) horizontal plane.
Our naming should make this explicit so downstream consumers don't trip on it.

---

## Methodology (tools + process)

- IDA Pro (implied, not named outright) for decompilation.
- Manual tracing of the packet-handling pipeline from network/file input
  down to game-state mutations.
- Identification of decrypt-access sites by finding places where an
  obviously-useful quantity (an ID, a float damage value) is live in a
  register.
- Custom emulator written in Rust; exception-based hooks via INT3.
- Dataset of 1.4M+ replays parsed using this toolchain, released publicly.

---

## Public datasets

Two Hugging Face datasets, both authored by `maknee`:

- `maknee/league-of-legends-decoded-replay-packets` (S12, ~700K)
- `maknee/leaague-of-legends-decoded-replay-packets-s12-unorganized` (also S12;
  note the typo in the URL, `leaague`)

Season 12 means patch 12.x, ~2022. Old by now (we're on 16.x as of April 2026),
but the **schema** of the output is potentially the most authoritative public
reference for packet semantics we have access to. Worth sampling one of these
datasets as a schema example before we finalise our output JSON shape.

---

## Optimisation observations (for later, not now)

Henry Zhu notes several things that would help Riot if they ever redesigned
the format, not actionable for us, but they double as insights into the
format's design intent:

- ~2.8% of packets are near-duplicates (e.g. many `LeaveFromFog` at the same
  timestamp for the same id). Our `audit` subcommand should probably
  surface this as a statistic.
- IDs and hashes could use LEB128 instead of fixed-width. They currently
  don't.
- Similar packet types could be merged; they currently aren't, which is
  actually helpful for us, one opcode = one semantic type.

---

## What the post does not tell us

- Anything about the `.rofl` outer container (magic bytes, header layout,
  metadata location). Mowokuma's source remains our only source here until
  we do the hex walkthrough in step 3.
- The compression algorithm (Mowokuma confirms zstd).
- The chunk-header layout (Mowokuma provides this).
- Block-level framing inside a chunk (Mowokuma provides this).
- The block-marker-byte bitfield semantics (Mowokuma).
- The relationship between Mowokuma's numeric netids and Henry Zhu's packet
  class names.
- Whether `chunk_type == 0x2` (which Mowokuma skips) is a keyframe.
- Whether there is a per-file "encryption key" in the payload header at all,
  or whether the per-patch lookup tables are all that's needed.

---

## Implications for ROFL-X

1. **We should not try to reimplement any packet decryption in pure Rust.**
   The obfuscation changes per patch and the community has twice reached for
   emulation. Our architecture must accommodate an emulator-driven decode
   path as a first-class thing, not a workaround.

2. **The packet catalog's Semantics column is about `UsePacket` behaviour**,
   not about how to decode the bytes. Decoding is orthogonal (per-patch,
   emulator-driven); semantics are stable across patches in principle (the
   game keeps doing the same things with the same packet types even if the
   struct layouts shift).

3. **Catalog seed list:** every packet class Henry Zhu names, plus
   Mowokuma's two numeric netids, gets an initial catalog entry. Status
   defaults to `OBSERVED-ONLY` until we can independently reproduce the
   decode in our harness.

4. **Parity story is now two-sided:**
   - Against Mowokuma: byte-for-byte JSON diff on the two packet types she
     handles, on the same replay.
   - Against Henry Zhu's dataset (optional): structural comparison of our
     output shape against a sample from his Hugging Face drop, to ensure
     naming and semantics are compatible with community prior art.

5. **Platform question:** Mowokuma's patch files target Windows PE; Henry
   Zhu's RVAs look Mach-O. Our patch-config format needs to be explicit
   about which binary it was extracted from, and our extraction tooling
   should work for at least Windows (the primary LoL platform).

6. **We should not publish a dataset.** Henry Zhu's datasets already exist
   and are far larger than anything we'd produce. Our contribution is the
   *documented parser*, not decoded data.

---

## Next

Phase 1 step 2 is complete. Remaining in Phase 1:

- **Step 3:** pick one `.rofl` from `%USERPROFILE%\Documents\League of Legends\replays\`,
  produce annotated hex walkthrough → `docs/SAMPLE_A_WALKTHROUGH.md`.
  - This will resolve the `// FIXME: very bad` header logic, confirm zstd
    magic bytes in chunk payloads, and verify the 4-byte-trailer metadata
    pattern.
- **Step 4:** stop and present all of Phase 1 for review before Phase 2.

Per the phase-gate, I'll proceed to step 3 now (same phase), then stop.
