# Building a `.patch` archive for a League patch

A `.patch` archive is the per-patch input the emulator needs: the
binary's sections plus a `result.json` naming the game functions to run.
Two formats exist:

- **Format 2** (current): every packet class through the game's own
  factory and deserializers. Built automatically by
  `scripts/build_patch.py`, no manual reverse engineering. Method and
  evidence: [DECODING.md](DECODING.md).
- **Format 1** (Mowokuma's, 15.x): two hand-reverse-engineered decoder
  functions and the struct offsets to read back. Still read by
  `rofl-x file`; reference below for anyone holding an old binary.

## Prerequisites

- The exact `League of Legends.exe` of the target patch. The Riot
  client patches it in place, so keep a copy before the patch moves on
  (the `Game/` folder's `code-metadata.json` names the build).
- The binary must be **unpacked**. 16.9 ships plain; 16.19 ships with
  its code encrypted on disk and cannot be used (see
  [COMPATIBILITY.md](COMPATIBILITY.md)). `build_patch.py` checks and
  refuses packed binaries.
- Python 3.10+ with `pefile`, `capstone`, `numpy`, `unicorn`
  (`zstandard` too for `--replay` validation).
- A replay of the same patch, to validate.

## Format 2

```
python scripts/build_patch.py \
  --binary "C:\Riot Games\League of Legends\Game\League of Legends.exe" \
  --output patch/<major>-<minor>.patch \
  --replay "%USERPROFILE%\Documents\League of Legends\replays\<same patch>.rofl"
```

Takes about three minutes on a 16.9 binary. What it prints, with the
16.9 values:

```
factory           0xe83930 (table 0xe93818, 1198 netids)
alloc / free      0x1123c60 / ['0x1123c90', '0x11283c0']
base_deserialize  0x11b8430
team_registry     0x1e1a9b0 count=4 (21 readers)
import slots      500
tls               0x1c1edd0 +325
cfg dispatch      0x1922480
packet names      315 netids (RegisterHandler 0x6c5c20)
wrote patch\16-9.patch (14.0 MB)
validation: 213 netids, 100.000% of 1223581 blocks decode cleanly
```

How each line is found, in short (details in [DECODING.md](DECODING.md)):

| entry | found as |
|-------|----------|
| factory | the image-base-relative jump table (`lea r, [rip-rva]; mov eax, [r + idx*4 + table]; add rax, r; jmp rax`) whose cases allocate (`mov ecx, SIZE; call new`); bound from the `cmp` before it |
| alloc | the most common call target in those cases |
| free | operator delete sits right after operator new and is a `jmp` to free; both are stubbed |
| base_deserialize | the first call in the deserializers of objects the factory constructs |
| team_registry | the global behind the enum readers' `cmp byte [rax+0x70], 0; cmp dl, [rax+8]` check; count from the initializer's `mov dword [reg+8], imm` |
| import slots | `call/jmp [rip+X]` slots whose on-disk value is not an in-image pointer |
| tls, cfg dispatch | PE TLS directory; load config `GuardCFDispatchFunctionPointer` |
| packet names | `std::function` handler registrations, see [DECODING.md](DECODING.md) § 6 |

The validation line decodes a sample of every netid in the replay and
must read 100 %. Then run `rofl-x file` on the replay: the self-check
line should read `n/n` on every field.

If a future patch changes one of these shapes, the script stops with the
step that failed. Every step is a few lines in the script; the
signatures it relies on (allocator wrappers, CRT thread-safe statics,
`std::function`, generated deserializers) are compiler and library
shapes, not per-patch tables, which is why they have held from 15.5 to
16.9.

## Format 1 reference (Mowokuma, 15.x)

Format 1 needs these, all found by hand in a disassembler. Values are
for 15.5 (Mowokuma's `5-5.patch`).

| field | 15.5 | what it is |
|-------|------|------------|
| `player_id_start` | `0x40000099` | net id of the first champion; the ten are contiguous in statsJson order (16.9: `0x400000AE`) |
| `alloc1_rva`, `alloc2_rva` | `0xf60420`, `0x1de520` | allocator wrappers, replaced by her 91-byte bump-allocator shellcode |
| `skip_rva` | `0xfca950` | stubbed to `mov rax, 1; ret`. It is the base-class deserializer that reads the sender net id (format 2's `base_deserialize`); replay payloads do not carry that varint, so skipping it is right for the top-level packet |
| `mov_decrypt` | netid 980, `0xe45710..0xe45b35`, payload ptr/size at `+0x18`/`+0x20` | WaypointGroup's deserializer; the buffer is the entry list documented in [PACKETS.md](PACKETS.md) |
| `ward_spawn_decrypt` | netid 571, `0xe3d7b0..0xe3fd61`, id `+0x48`, owner `+0x18`, name `+0x60`/`+0x68`, x/y `+0x20`/`+0x28` at the 4th write | SpawnMinion's deserializer |

The 15.5 binary already has the netid factory format 2 uses: in
Mowokuma's section dumps, the `r14`-relative jump table at `0xd256a8`
(bound `0x3f9`, 1018 netids) sends netid 980 to a constructor whose
vtable's slot 1 is `0xe45710`, her `mov_decrypt`, and netid 571 to one
whose slot 1 is `0xe3d7b0`, her `ward_spawn_decrypt`. (The "48-byte
descriptor table with a sentinel at +0x28" this file used to describe
is those vtables, read from slot 1.) A format-2 archive for 15.x has
not been built: her dumps lack the PE headers, TLS directory and load
config the script reads.

## History

The branch that led here tried, in order: matching 15.5 decoder bytes
in 16.9 (`scan-decoder`), brute-forcing which candidate function writes
the most struct offsets for each netid's payloads, and walking the
constructor chain by `lea` scanning. The factory call in
[DECODING.md](DECODING.md) § 1 replaced all three; the brute-force
mapping turned out wrong for several high-volume netids. Those tools
are in the git history (`git log -- scripts/ src/scan_decoder.rs`).
