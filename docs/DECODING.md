# Decoding every packet through the game's own code

How a format-2 `.patch` archive lets ROFL-X decode **every** block of a
replay, not just the two packet classes Mowokuma reverse-engineered, and
how we know the result is right. Everything here was established on
patch 16.9 (`League of Legends.exe` 16.9.772.1032, MD5
`7bf33f4e0035937d187fa6594afe93df`); RVAs below are for that binary.

Format 1 (Mowokuma's archives, 15.x) is unchanged and still supported;
see [REFERENCE_MOWOKUMA.md](REFERENCE_MOWOKUMA.md).

---

## The short version

```
block (netid, param, payload)
  │
  ├─ factory(out, netid) ─────────▶ packet object with its class's real vtable
  │                                  (game code, 0xe83930 on 16.9)
  ├─ varint(param) ++ payload ────▶ the stream the live client would have read
  │
  └─ vtable[1](obj, &cursor, end)  the class's own Deserialize, run in Unicorn
        │                          with plaintext capture on
        ▼
   returns true and cursor == end?  ── yes ─▶ clean decode
        │
        └─ plaintext object image  ──▶ packet/handlers/*.rs read fields by offset
```

Per patch, the archive names seven things (`scripts/build_patch.py`
finds them all in the binary, no manual reverse engineering): the netid
factory, operator new, operator delete/free, the base-class
deserializer, the team registry, the TLS template and the CFG dispatch
thunk, plus the list of hidden import slots and the netid → class-name
table.

Results on the five 16.9 replays we hold: **7,927,123 blocks, every one
of them a clean decode**, and the per-player totals rebuilt from the
decoded packets match Riot's own end-of-game stats in the same file on
**300 / 300** checks (see § How we know it is right).

---

## 1. The netid factory

Every packet class has a constructor; one function maps a netid to the
right one. On 16.9 it is `0xe83930`:

```
cmp  esi, 0x4ad                        ; netid < 1198
ja   default
lea  r14, [rip - 0xe8396c]             ; image base
mov  eax, [r14 + rsi*4 + 0xe93818]     ; u32 jump table, one entry per netid
add  rax, r14
jmp  rax
...
case: mov ecx, 0x30 ; call operator_new ; mov rcx, rax ; call Ctor ; ...
```

Calling it in the emulator with `(out_ptr, netid)` returns a freshly
constructed object of the right class, vtable included. Slot 1 of that
vtable is the class's `Deserialize(this, &cursor, end)`.

This replaces the netid → decoder mapping the
`decrypt/tooling-and-16-9-recon` branch built by brute force (feed each
netid's payloads to candidate functions, keep the one writing the most
struct offsets). That mapping was wrong for some of the most frequent
classes: the factory gives `0xfd6270` for netid 916 (the brute-force
match was `0xfb4070`) and `0xf07e20` for netid 1068 (match `0xf9d4e0`).
A wrong decoder still writes "fields", which is why those results looked
plausible and why the old field values were mostly defaults.

`build_patch.py` finds the factory as the image-base-relative jump table
whose cases call operator new (`mov ecx, SIZE; call X`); operator new is
the most common `X`.

## 2. What a deserializer does

The deserializers are generated code with one shape. A class reads one to
three tag bytes, then for each field pulls a small "kind" code out of
them (`ecc180` is the bit-field reader) and either stores a constant
(0, 1, 2, -1, 0.0, 1.0, -1.0, 2.0, ...) or calls a type-specific reader
that consumes bytes from the stream through its own byte permutation
(varints, floats, strings, net ids, nested objects, vectors of nested
objects with their own vtables). Every stored field is then obfuscated
in place, a byte at a time: this is the decrypt-access-release pattern
Henry Zhu describes ([REFERENCE_HENRY_ZHU.md](REFERENCE_HENRY_ZHU.md)).

Running the real deserializer means none of this has to be understood
per class. It also gives a strong correctness signal: the function
returns false the moment a kind code, length prefix or enum value is
inconsistent, and the cursor only lands exactly on `end` when every byte
was accounted for.

## 3. The net id lives in the block header

Every deserializer starts by calling the base-class deserializer
(`0x11b8430` on 16.9). It reads an obfuscated 7-bit varint and stores it
at `this + 0xc`, XORed with `0x40000000` unless the low 24 bits are zero:
the sender net id. Replay payloads **do not** contain it: decoding it
from the first payload bytes never matches anything, while the block
header's `param` holds exactly that net id (and, read with a signed
delta, lands on the ten champions' ids for champion-only classes; see
[ROFL_FORMAT.md](ROFL_FORMAT.md) § Block framing). The replay writer
moved it to the header.

Format 1 stubs this function out (`skip_rva`, `mov rax, 1; ret`), which
is right for the top-level object but also skips it for nested objects
that carry their own net id. Format 2 instead re-frames the stream:
`param` is re-encoded with the game's own byte mapping and prepended to
the payload, and the real function runs. The mapping is learned at
startup by emulating the base deserializer on all 256 one-byte inputs
(final bytes) and 256 two-byte inputs (continuation bytes).

## 4. The environment the deserializers expect

A static image is not a running process. What the deserializers touch,
and what the emulator (`src/emulator/machine.rs`) puts there:

| need | what happens without it | stand-in |
|------|-------------------------|----------|
| **Heap.** operator new (`0x1123c60`) calls Riot's allocator through a global initialised at startup. | null dereference | `ret` plus a hook that bump-allocates; delete (`0x1123c90`, a `jmp` to free `0x11283c0`) and free become `ret` |
| **TLS.** Thread-safe statics read `_Init_thread_epoch` at `gs:[0x58] → [tls_index] → +0x140`. | read of unmapped memory | a TEB whose TLS array points at a copy of the image's TLS template (`0x1c1edd0`, 325 bytes; the epoch starts at INT_MIN) |
| **Imports.** The real import table is hidden behind Riot's loader stub (`stub.dll!packman` is the only declared import); IAT slots hold obfuscated values. | jump to garbage | every slot read by a `call/jmp [rip+X]` whose value is not an in-image pointer is pointed at `xor eax, eax; ret` (500 slots on 16.9) |
| **Control Flow Guard.** Indirect calls go through `__guard_dispatch_icall` (`jmp rax`, `0x1922480`). The CRT's encoded function-pointer cache is never initialised in a static image. | jump to garbage | a hook on the dispatch thunk redirects any target outside mapped memory to the same stub |
| **Team registry.** Enum readers range-check the decoded value against a global built at session start (`0x1e1a9b0`: count at `+0x08`, ready flag at `+0x70`). | every team-indexed field fails (the deserializer returns false) | an object with the same count and the ready flag set |

The team registry deserves a note. Its initializer (`0x625c40`) writes
count 4 and the team ids 100 / 200 / 300 (order, chaos, neutral): it is
the team table, and the enum readers decode team indices. One reader
(`0xeee030`) uses the count as a **size**, reading a `(count + 7) / 8`-byte
team bitset, so the count must be the real one: a permissive stand-in
(count 255) made netid 446 read 32 bytes where the stream holds one.
`build_patch.py` recovers the count from the initializer's
`mov dword [reg+8], imm` and falls back to 4.

## 5. Plaintext capture

After `Deserialize` returns, the object holds obfuscated bytes. The
plaintext existed only when each field was stored, so the emulator hooks
every heap read and write and keeps, per byte, the last plaintext value:

- a **one-byte write to a byte that was read since its last write** is
  the in-place obfuscation pass and is ignored;
- every other write is a plaintext value. That includes wider
  read-modify-writes: varint readers OR 7-bit groups straight into the
  destination (`*dst |= group << shift`), and those must count.

The view handed to the semantic layer (`PlainMemory`) is the heap after
the call with those plaintext bytes substituted. The heap is zeroed
between decodes so nothing leaks from one block into the next.

The rule is checked end to end by the statsJson comparison below: damage
amounts (floats read through a varint-style reader), item ids, net ids
and counters all come out exact.

## 6. Riot's class names

Release builds strip RTTI for the packet classes. Measured on 16.9:
`.data` holds 174 distinct MSVC type descriptors, every one of them a
`std::` type or a `std::function` binder, and not one a `PKT_*`. So the
shortcut of constructing a packet through the factory and reading the
class name out of the vtable's `CompleteObjectLocator` does not work;
it was tried and returns nothing for every netid.

What survives is indirect. `std::function` keeps a `type_info` for the
lambdas `Riot::MakeFunction<Client, ..., const PKT_X_s&>` wraps, and the
class name is spelled inside the lambda's own descriptor. Those are
still in `.data`, e.g.

```
.?AV<lambda_1>@?1???$MakeFunction@VJunglePathComponentClient@@V1@_NAEBVPKT_S2C_UpdateJunglePathDisplayed_s@@@Riot@@...
```

They are referenced by code, through RIP-relative `lea`s, never by stored
pointers (which is why a scan for 8-byte references finds nothing). The
chain to a netid:

```
type descriptor  ◀── lea rax,[rip+TD]; ret        std::_Func_impl::_Target_type
                 ◀── _Func_impl vtable, slot 3     (in .rdata)
                 ◀── lea rax,[rip+vtable]           where the std::function is built
                      ...                           (inline, or in a MakeFunction helper)
                      mov r8d, <netid>
                      call RegisterHandler          (0x6c5c20 on 16.9)
```

On 16.9 this names **315 netids** (no netid gets two different names),
covering 75.9 % of the blocks in our five 16.9 replays. The unnamed rest are
classes dispatched without a `std::function` handler: object creation,
movement, replication, visibility. Two of them are recognised by shape
(`packet/handlers/spawn.rs`, `packet/handlers/waypoints.rs`), and only
when ≥ 95 % of a netid's blocks match.

## How we know it is right

Three independent levels:

1. **The deserializer's own verdict.** Clean = returned true and
   consumed exactly the payload. 16.9, five replays: 7,927,123 / 7,927,123
   blocks clean. (Inside length-delimited nested objects a misparse can
   stay local, so this checks framing, not every field.)
2. **Physics and geometry.** Waypoint groups parse to exactly their
   buffer length with every point on the map; spawn positions, death
   positions and respawn points land where the game puts them.
3. **Riot's own record.** statsJson, in the same file, holds each
   player's end-of-game numbers. `rofl-x file` rebuilds them from decoded
   packets and compares (`checks` in the output):

   | check | built from | 16.9, 5 replays |
   |-------|------------|-----------------|
   | kills / deaths / assists | last `PKT_Update_HeroStats_KDA_s` | 50 / 50 |
   | kills / assists from kill events | `PKT_NPC_Hero_Die_s` killer + assist-counter steps | 50 / 50 |
   | champion level | highest `PKT_NPC_LevelUp_s` | 50 / 50 |
   | final items, 7 slots | replay of buy / remove / swap / use answers | 50 / 50 |
   | damage dealt to champions | sum of `PKT_UnitApplyDamage_s` amounts, champion → enemy champion | 50 / 50 (within 2 points of float rounding) |
   | wards placed | ward spawns owned by the player | 50 / 50 |
   | visibility | every `PKT_S2C_On{Enter,Leave}TeamVisibility_s` for a champion names the *other* team | 50 / 50 |

## Limits

- **The binary must be unpacked.** From some patch after 16.9 the
  shipped `League of Legends.exe` keeps `.text`/`.rdata`/`.data`
  encrypted on disk (new `.riot0`/`.riot1`/`.riot2` sections; entropy
  7.997 throughout `.text`). Getting at the code would mean defeating that
  protection, which ROFL-X will not do (README § Non-goals). See
  [COMPATIBILITY.md](COMPATIBILITY.md).
- **Unnamed classes stay unnamed** unless a shape recogniser covers them:
  they are decoded (clean) but not typed. Their fields are still
  measured, class by class, in [FIELDS.md](FIELDS.md).
- **Absent fields read as their defaults.** A field the stream did not
  carry holds the constant the deserializer stored for it, which is the
  value the game itself uses.
- **One team count.** The registry count is the session's team table
  (4 on Summoner's Rift). Modes with more teams (Arena) would need the
  replay's own count; not handled yet.
