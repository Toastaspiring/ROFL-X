# Building a `.patch` archive for a new League patch

A `.patch` archive is the per-game-patch config the emulator backend
needs: three raw PE section blobs (`text.bin`, `data.bin`, `rdata.bin`)
plus `result.json` with RVAs and struct offsets. The mechanical half
(unpacking the PE) is already automated; the reverse-engineering half
is manual and documented here.

If you just want to produce a skeleton, skip to the "Skeleton generation"
section below. Filling it in needs a disassembler (IDA Pro, Ghidra,
Binary Ninja) and a few hours.

## Prerequisites

- The exact `League of Legends.exe` for the target patch. Live-patched
  Riot clients overwrite the binary in place, so if the target patch
  has already moved on you will need a preserved copy.
- A PE disassembler. Ghidra is free and adequate; IDA Pro is faster for
  large binaries.
- A matching `.rofl` replay on the target patch for end-to-end
  validation.
- The corresponding section blobs from a nearby working patch archive,
  so byte-diff techniques are available. (Mowokuma's `5-5.patch` in her
  GitHub release covers patch 15.5 and is the closest thing to a golden
  reference right now.)

## Skeleton generation

```
rofl-x extract-patch \
  --binary "C:\Riot Games\League of Legends\Game\League of Legends.exe" \
  --output ./patch/<major>-<minor>.patch-skeleton
```

Writes a zip archive containing:

- `text.bin`, `data.bin`, `rdata.bin`, raw bytes of each PE section
- `result.json` with RVA placeholders (`"NEEDS_RE"`) for every value
  that must be reverse-engineered

Rename to `<major>-<minor>.patch` once the placeholders are filled in.

## Fields that must be reverse-engineered

The full list of placeholders, with hints.

### `player_id_start` (u32, hex string)

The lowest entity id assigned to a human player. The ten players are
in the range `[player_id_start, player_id_start + 9]`.

- **Reference (15.5):** `0x40000099`.
- **Signature:** search the binary for the constant `0x40000099` or
  similar (`0x4000????`) and look at the call sites. It is typically
  used as a base for a range-check loop.
- **Cross-patch stability:** historically stable for long stretches.
  Starting guess for any 15.x/16.x patch is `0x40000099`; verify by
  running a replay and checking that player positions land on the
  expected entity ids.

### `alloc1_rva`, `alloc2_rva` (u64, hex strings)

RVAs of two allocator wrapper functions Mowokuma's emulator replaces
with a 91-byte bump-allocator shellcode. Neither needs to be analysed
semantically; what matters is that calls into them from inside the
decoder functions (ward_spawn_decrypt / mov_decrypt) get intercepted.

- **Reference (15.5):** `alloc1_rva = 0xf60420`, `alloc2_rva = 0x1de520`.
- **Signature:** calls to these functions in the decoder functions'
  disassembly return a pointer and bump the allocator. In Mowokuma's
  5-5 patch they both receive a size in RDX and return a pointer in
  RAX. Check the two allocator-like helpers called from the decoder
  prologues and use their RVAs.
- **Cross-patch stability:** code relocates every patch; you must
  re-find these each time.

### `skip_rva` (u64, hex string)

RVA of a safety-check function that the decoder function calls early
and whose return value gates further execution. The shellcode we write
at this RVA is just `mov rax, 1; ret`, so any function that blocks the
decoder unless a condition is met can go here.

- **Reference (15.5):** `0xfca950`.
- **Signature:** a small, non-inlined function called near the top of
  a decoder function, tested against zero, where a non-zero return
  means "continue". In 15.5 it is the same for both decoders.
- **Cross-patch stability:** volatile across patches.

### `mov_decrypt.netid` (u32)

The `packet_id` carried on movement packets in the target patch's
block stream.

- **Reference (15.5):** `980`.
- **Finding it:** open a replay of the target patch with
  `rofl-x inspect --histogram` to see all observed opcodes. The
  movement packet is usually among the top 20 by count (player path
  updates are frequent). Cross-reference with neighbouring patches: if
  15.5 uses 980 and 16.8 has a high-count opcode near it numerically,
  that is a strong candidate.

### `mov_decrypt.rva_start`, `rva_end` (u64, hex strings)

RVAs of the start and end of the movement-decrypt function.

- **Reference (15.5):** `rva_start = 0xe45710`, `rva_end = 0xe45b35`.
- **Signature:** one of a small number of functions that takes three
  arguments (RCX = output packet struct, RDX = payload pointer
  indirection, R8 = payload end pointer) and writes to struct offsets
  `0x18` and `0x20` with a decoded pointer and size. In the decompiler
  it reads as "allocate, loop-copy bytes, return".
- **`rva_end`** is the address immediately after the final `ret`
  instruction; the emulator stops execution when the PC reaches here.

### `mov_decrypt.payload_offset`, `payload_size_offset` (u64, hex strings)

Struct offsets into the output packet struct where the function
deposits the decoded `(pointer, size)` pair.

- **Reference (15.5):** `0x18`, `0x20`.
- **Cross-patch stability:** frequently stable across several patches
  at a time; if the decoder body hasn't been rewritten between 15.5
  and 16.8, these likely still apply. Verify by extracting the decoded
  payload from a real replay and checking that the bytes parse as a
  `PathPacket` (see `src/emulator/packet.rs::PathPacket::parse`).

### `ward_spawn_decrypt.netid` (u32)

`packet_id` for the ward spawn-and-destroy packet class.

- **Reference (15.5):** `571`.
- **Finding it:** place a ward in a test game on the target patch,
  record the replay, then look for the opcode in the histogram that
  fires exactly once for that ward and then again some time later
  (the corpse event). Confirm by cross-referencing with a known-good
  15.5 replay: the ward netid in 15.5 is 571.

### `ward_spawn_decrypt.rva_start`, `rva_end` (u64)

RVAs of the ward-spawn decrypt function.

- **Reference (15.5):** `0xe3d7b0`, `0xe3fd61`.
- **Signature:** similar to mov_decrypt, three-argument function but
  bigger (ward-spawn decodes more fields: id, owner_id, name, x, y).
- **Finding it:** cross-reference from the netid (571): somewhere in
  the binary there is a packet-dispatch table mapping netid to
  decoder; following 571 to its handler lands on this function.

### `ward_spawn_decrypt` struct offsets

Seven values the mem-write hook needs to know about:

- `id_offset` (0x48 in 15.5): where the ward entity id lands.
- `owner_id_offset` (0x18): where the owner's entity id lands.
- `name_offset` (0x60): where a pointer to the ward name string lands.
- `name_len_offset` (0x68): where the length of the name string lands.
- `x_offset` (0x20), `x_write_count` (4): x-coordinate float, captured
  after the 4th write to that offset.
- `y_offset` (0x28), `y_write_count` (4): same for y-coordinate.

- **Finding them:** step through the ward-spawn decrypt function in
  the disassembler. Each time the function writes to `[rcx + N]`, note
  the offset N. Count the writes per offset; the "final value" at
  each offset is typically the last write. `x_write_count = 4` means
  the final x value lands at the 4th write to `x_offset`.
- **Cross-patch stability:** typically stable across short stretches
  (no layout change) until Riot adds a new field to the struct.

## Suggested order of operations

1. Run `rofl-x extract-patch` to produce the skeleton.
2. Copy the 15.5 result.json from Mowokuma's `5-5.patch` as a working
   reference inside your disassembler.
3. Resolve the two netids first: they are the cheapest to find and
   will anchor everything else.
4. Find the two decoder functions by following the netid dispatch.
5. Find their RVAs (`rva_start`, `rva_end`) and the struct offsets by
   disassembling them.
6. Find the allocator and skip stubs by looking at the decoder
   prologues.
7. Fill in `player_id_start` (usually unchanged from prior patch).
8. Save as `<major>-<minor>.patch` and test with
   `rofl-x file -r <replay> -o <out.json> --patch-dir <dir>`.
9. If ward lifecycle looks wrong, re-check `name_offset`,
   `name_len_offset`, and the four write counts.
10. If positions drift, re-check `mov_decrypt.rva_end` and the payload
    offsets.

## Things this document does not yet describe

- An automated byte-pattern-matching workflow that takes a known-good
  15.5 archive and tries to find the same functions in a 16.8 binary.
  This is a candidate future tool; manual inspection is the
  conservative path for now.
- Handling of patches where Riot restructures the decoder layout
  significantly. If the struct offsets cease to be stable across
  patches, we will need to track those separately per-patch.
- Automated validation that a filled-in archive produces plausible
  output. The end-to-end `file` subcommand plus cross-referencing
  against a known replay is the current approximation.

See `docs/COMPATIBILITY.md` (forthcoming in Phase 4) for the
per-patch status table.
