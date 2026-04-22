# Per-patch compatibility

What ROFL-X can do on each League patch it has been tested against.

Columns:
- **Transport**: can we parse the file structure (header, chunks,
  block framing) without decoding packet semantics.
- **Emulator config**: is there a usable `.patch` archive for this
  patch.
- **`MOVEMENT_PATH`**: can we decode the movement opcode end-to-end.
- **`WARD_SPAWN_OR_DESTROY`**: can we decode ward events end-to-end.
- **Mowokuma parity**: if run against her binary on a shared replay,
  what does the diff look like.

| Patch  | Transport | Emulator config         | MOVEMENT_PATH | WARD_SPAWN_OR_DESTROY | Mowokuma parity                     |
|--------|-----------|-------------------------|---------------|-----------------------|-------------------------------------|
| 15.1   | green     | Mowokuma `5-1.patch`    | netid 980     | netid 571             | untested                            |
| 15.2   | green     | Mowokuma `5-2.patch`    | netid 980     | netid 571             | untested                            |
| 15.3   | green     | Mowokuma `5-3.patch`    | netid 980     | netid 571             | untested                            |
| 15.4   | green     | Mowokuma `5-4.patch`    | netid 980     | netid 571             | untested                            |
| 15.5   | green     | Mowokuma `5-5.patch`    | netid 980     | netid 571             | metadata + wards byte-identical; 90.8 % of `players_state` records fully equal, 99.07 % of individual player entries match. (Mowokuma's own output diverges run-to-run on ~99.9 % of state records, so full byte-identity is not achievable.) |
| 16.8   | green     | skeleton only (`reference/patch-16-8-skeleton.patch`); RVAs unresolved | **blocked** | **blocked** | blocked on emulator config |

Assumed but untested: 14.x replays. Mowokuma's release doesn't cover
14.x, and the version-string length encoding probably changed at some
point earlier in history. The transport smoke test accepts any version
that starts with a digit, so parsing should work; semantic decode has
no chance without an archive.

## Known gaps

1. **16.8 emulator config.** The hard-work item. See
   [RE_PATCH.md](RE_PATCH.md) for methodology.
2. **Parity run against Mowokuma on 15.1-15.4.** Cheap to do once the
   parity harness is scripted.
3. **Opcode numbers on 16.8 for the two decoded opcode classes.** Can
   be inferred by diffing the sample_a histogram against the top
   opcodes from a 15.5 replay and looking for numerically-nearby
   candidates. `0x03D6 = 982` (close to 980) is a prime suspect for
   `MOVEMENT_PATH` on 16.8.

## How to update this file when adding a new patch

1. Run `rofl-x inspect --histogram -r <replay>` to confirm the
   transport layer is clean on the new patch.
2. If filling in an emulator config, run `rofl-x file` on a short
   replay and spot-check the output.
3. If doing a parity run against Mowokuma's binary, normalize both
   outputs by sorting `players_state[].players` by `champ` and
   `wards` by `(timestamp, pos)` before diffing. Her HashMap ordering
   drifts every run; ours is deterministic.
4. Add a row to the table above.
