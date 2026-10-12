# Exploring a replay whose client is packed

From some patch after 16.9, `League of Legends.exe` ships with its code
encrypted on disk ([COMPATIBILITY.md](COMPATIBILITY.md)). Without the
game's code there is no deserializer to run, and ROFL-X will not go
after the code (README § Non-goals).

The replay is a different matter. It is a file, it belongs to whoever
played the game, and most of it was never obscured in the first place:
a block's `timestamp`, `netid`, `param` and length are plaintext, and
the file carries Riot's own end-of-game record alongside them. `rofl-x
explore` is what that is worth — and it is worth more than framing,
because the payload obfuscation turns out to be a per-class byte
substitution rather than a cipher, so a class whose plaintext something
outside the file pins down can be read from the file too (§ 3).

```
rofl-x explore -r <replay> [-r <replay> ...] \
  [--reference <16.9 replay> ...] [--self-test] [-o out.json]
rofl-x explore -r <replay> --dump-netid 539 --dump bytes.jsonl
```

This page is the run on the twenty patch-16.19 replays we hold, and
everything in it came out of those files. No binary is unpacked, no
process is read, nothing touches the game.

---

## 1. Which entity is which champion

Entity ids are handed out in order as the game creates objects, and the
ten champions are created together, so their ids are ten consecutive
values. They are also the entities the server has the most different
things to say about: a champion collects more distinct netids than any
minion, ward or turret. Scoring every consecutive run of ten by how
many distinct netids its members carry, and keeping only runs whose
members are alive at both ends of the game, picks them out.

| patch | champions | check |
|-------|-----------|-------|
| 16.9 | `0x400000ae` .. `0x400000b7` | identical to the decoder's own roster in all 5 replays |
| 16.19 | `0x40000034` .. `0x4000003d` | no decoder to check against; the run is unique |

The 16.9 column is the point: the same code, run on a patch where the
answer is known independently, gets it right 5 times out of 5. That is
what licences using it on 16.19.

With the champions named, the output carries blocks per champion per
minute for the whole game — a per-player activity timeline, from a file
whose payloads are unreadable.

## 2. Netids proved by Riot's own numbers

statsJson, in the same file, is a few hundred integers per player that
the game produced from the same events the packets describe. So:

> if a netid's **per-champion block count equals one of those counters,
> for all ten players, in every replay**, that netid is that event.

This is not a guess. It is an identity that has to hold ten players at
a time, in game after game, with a single constant offset. A field has
to vary across the players for a match to count, since a column of
zeroes matches any netid that never fires, and the identity has to
survive every replay that could test it bar one.

A class is not always tied to a single counter, so a few sums are
tried as well. The one carrying a champion's running kill / death /
assist totals fires whenever any of the three changes, so its block
count is their sum; a matcher on single fields cannot see it at all.
The list of sums is deliberately short — every extra combination is
another chance for a coincidence to clear the test.

On 16.9, where every answer is already known:

| netid | counter | class it turned out to be | replays |
|------:|---------|---------------------------|--------:|
| 472 | `NUM_DEATHS` | `PKT_NPC_Hero_Die_s` | 5 / 5 |
| 715 | `NUM_DEATHS` | `PKT_S2C_UpdateDeathTimer_s` | 5 / 5 |
| 115 | `CHAMPIONS_KILLED + NUM_DEATHS + ASSISTS` | `PKT_Update_HeroStats_KDA_s` | 5 / 5 |
| 64 | `LEVEL` − 1 | `PKT_NPC_LevelUp_s` | 3 / 4 |

Four proposals, four correct. On 16.19, with no binary at all:

| netid | counter | replays | what else is known about it |
|------:|---------|--------:|------------------------------|
| 1080 | `NUM_DEATHS` | 19 / 19 | payload 16–37 bytes, only ever names the ten champions |
| 590 | `CHAMPIONS_KILLED + NUM_DEATHS + ASSISTS` | 19 / 19 | the running score. Every one of its 7,764 blocks lands at the same instant as a kill from the feed in § 3 — two classes, identified separately, agreeing |
| 601 | `NUM_DEATHS` | 3 / 3 on a 6-replay run, not on all 20 | payload exactly 5 bytes, champions only, 1,962 blocks against 1,080's 1,955 |

Netid 590's **values** are a different matter. Its payload grows from
two bytes to five as the counters grow, and no byte of it tracks which
counter moved, so the class packs its three numbers in bits rather
than bytes. Byte-wise substitution still describes the payload, but
the fields no longer line up with bytes, so the technique that read
the killer does not apply. The identification stands; the numbers are
not out yet.

**Netid 1080 on 16.19 is the champion-death packet.** Two independent
things say so: it fires exactly as many times per champion as that
champion died, in every one of the nineteen games that could test it;
and its payload length range and champion-only addressing are those of
`PKT_NPC_Hero_Die_s` on 16.9 (17–37 bytes, champions only).

Netid 601 is its death-timer companion, `PKT_S2C_UpdateDeathTimer_s`:
5-byte payload exactly as on 16.9, champions only, and almost exactly
one per death. It is listed second because "almost" is not "exactly" —
over twenty games it drifts by a few blocks, so the strict test drops
it.

## 3. Reading a payload without the binary

The obfuscation is not a cipher, and that can be measured rather than
assumed. On 16.9 both sides are in hand — the raw payload bytes from
the file and the plaintext object the game's own deserializer produced
— so the transform between them can simply be read off.

What it is: **one 256-entry byte substitution per class**, applied to
the fields serialised in order. The same plaintext byte always becomes
the same payload byte, at any position in the payload, in every block
of every game. No key, no block state, no per-session nonce.

`PKT_S2C_UpdateDeathTimer_s` on 16.9 carries a single `f32`, and 62
blocks of it yield 107 distinct plaintext bytes with **zero
conflicts**, injectively. `PKT_S2C_FaceDirection_s` yields a table too,
a different one — so the table is per class, not per patch. Sibling
classes share: `PKT_S2C_OnEnterTeamVisibility_s` and
`PKT_S2C_OnLeaveTeamVisibility_s` both encode team 1 as `c719` and team
2 as `c7c1`.

That changes the problem. A class whose plaintext we can pin down from
outside is readable on a packed patch, because the table can be
recovered from the file.

### The visibility pair, decoded on 16.19

The clean case. The payload is two fixed bytes, the only varying field
is the team, and the game guarantees which team it is: a unit is always
visible to its own side, so a visibility change can only ever name the
other one. The champions' teams come from the metadata, so every block
whose `param` is a champion is a **labelled sample**, and a single
block landing on the wrong side would falsify the mapping.

| patch | netid | payload → team | labelled blocks | exceptions |
|-------|------:|----------------|----------------:|-----------:|
| 16.9 | 1068 (enter), 920 (leave) | `c719` → 1, `c7c1` → 2 | 1,713 | 0 |
| 16.19 | 539 (enter), 69 (leave) | `1b03` → 1, `1b50` → 2 | **31,154** | **0** |

Across 19 of the 20 replays (the twentieth is a two-minute remake with
too few labelled blocks to pass the floor), 31,154 champion blocks
agree and none disagree. `1b03` is ORDER, `1b50` is CHAOS.

Which of the pair opens a window is settled structurally: every object
that becomes visible emits an enter, but a great many are destroyed
while still visible and never emit a leave, so enter outnumbers leave
heavily — 3,091,816 to 42,014 on 16.9. Timestamps do not settle it;
both fire at t=0 and a champion's earliest event splits 6–4 between
them.

**That is 37.9 % of every block in a 16.19 replay, fully read, with no
binary and no emulator.** The output gains the same per-champion vision
timeline that a decodable patch gets: when each player was inside the
enemy team's vision, and for how long.

### How we know it is right

The same code runs on 16.9, where the emulator's answer is independent
ground truth. Per-player vision time agrees to **within 0.1 second on
all five replays** — float rounding. `tests/decode_replays.rs` asserts
it, so the file-only path cannot drift from the decoded one without the
test suite failing.

### The death packet's killer field, on 16.19

The second case, and a harder one: a field with ten possible values
instead of two, inside a payload whose length varies from 16 to 37
bytes.

Netid 1080 is already known to be the champion-death class (§ 2). One
of its bytes has to say who got the kill, and statsJson records
`CHAMPIONS_KILLED` per player — so **across a whole game that byte must
name each player exactly that many times**. That is what finds the
field and what checks it.

Finding *where* it sits needs one fact about the format, taken from
16.9: a net id is written as a 7-bit varint, **low group first**, and
each byte carries its continuation bit. So a field naming a unit ends
with that unit's higher groups, and the byte we want sits a known
number of places from the end — one for a roster at 0x34..0x3d, two
for a roster at 0xae..0xb7, which both occur in these twenty replays.
The offset is derived from the roster, not searched for.

The continuation bit is part of the plaintext, not noise to be
stripped. Net id 0x34 is written as the single byte `0x34` and comes
out as `0x94`; net id 0xb4 is written as `0xb4` then `0x01` and its
first byte comes out as `0x56`. Same low seven bits, different
plaintext, different cipher byte — the measurement says so, and keying
the table on the low bits alone makes the two rosters contradict each
other.

The method is then deliberately split in two, so that it proves rather
than fits:

- **Learn** only from a replay where the assignment is *forced* — each
  player's kill count matched by exactly one byte value, no value
  serving two players. Players who tie on kills are left unsolved and
  picked up on a later pass once their neighbours are pinned.
- **Predict** every other replay from the table so built, including
  ones that were ambiguous on their own, and score it.

| | |
|---|---|
| player-games predicted | **200** across all 20 replays |
| predicted exactly right | **199** |
| table entries recovered | 36 |
| deaths placed with a killer | 1,944 of 1,955 |
| deaths with no champion killer | 11 (turret and minion executes) |

The one miss is a single player short by one kill, in the replay whose
last kill ends the game.

What comes out is a kill feed from a replay whose payloads are
supposedly unreadable:

```
   68.8s  B Swain          killed  R Amumu
   72.5s  R KogMaw         killed  B Yone
   75.6s  B Swain          killed  R Velkoz
   80.2s  B Jinx           killed  R Mordekaiser
   82.5s  B Swain          killed  R Kayle
   88.2s  R Amumu          killed  B Jinx
```

### The waypoint class writes its payload backwards

The movement class carries a list, and a list does not go in the
packet object — it goes in a buffer the deserializer allocates. So the
field-by-field pass finds **nothing**: every payload byte of netid 916
reads "not in the object".

Comparing the payload against that buffer directly fails too, at every
header length, with tens of thousands of contradictions. The reason
turns out to be visible in one measurement. Two blocks whose entry
buffers are byte-identical have payloads that differ *only in the
first three bytes*:

```
buffer 0200b200004000f04b440bf339f2
sync 6176   payload e3140c 79d2686a3959ceaadbd6dbdb55dbd4
sync 6310   payload cb568c 79d2686a3959ceaadbd6dbdb55dbd4
```

The header carries a counter, and everything after it is a
deterministic function of the entry data — so there *is* a
substitution, and the alignment was simply wrong. Solving the
alignment the same way the field pass does, by asking which payload
byte tracks which buffer byte across every block, gives the answer:

```
  payload[ 5] <- buffer[13]
  payload[ 6] <- buffer[12]
  ...
  payload[17] <- buffer[ 1]
```

**The payload carries the buffer back to front**, anchored to the
payload's end rather than its start. Nothing about staring at offsets
from the front would have shown that. With the reversal, one table
explains **256 of 256 bytes over 6,000 blocks with zero conflicts**,
and `T(0x00)` comes out as `0xdb` — which is exactly the byte that
frequency analysis said covered 20.3 % of the class's payload.

`decode --layout` now tries this arrangement whenever a class
allocates a buffer, so none of the above has to be done by hand again.

### What it gives on 16.19

The reversal means the *first* entry's header sits at the *end* of the
payload, at fixed offsets, whatever the rest of the packet does. The
entry begins `flags(2) unit(4) speed(4)`, and a unit net id's top byte
is `0x40` for every unit in the game. So:

| payload position | the model says | measured on 118,885 blocks |
|-----------------:|----------------|----------------------------|
| −1 | flags low byte: waypoint count and a flag | 16 distinct values |
| −2 | flags high byte, 0 or 2 | `0x42` on 99 % |
| −3 | unit id byte 0 | 247 distinct |
| −4 | unit id byte 1, 0 for ids under 0x100 | `0x42` on 99 % |
| −5 | unit id byte 2, always 0 here | **`0x42` on 100 %** |
| −6 | unit id byte 3, **always `0x40`** | **`0x29` on 100 %** |
| −10 | speed's exponent byte | one value on 90 % |

So on 16.19's movement class, `T(0x40) = 0x29` and `T(0x00) = 0x42`,
the latter agreed on three ways over — the frequency profile, position
−5, and position −4.

That is two entries of the 256 needed, and the position of every
coordinate in the packet. It is not positions yet. What is left is a
constraint solve: the entry stream has to parse to exactly its length,
the waypoint counts in the flags have to agree with the bytes
consumed, and every coordinate has to land on the map — and that check
either holds for a candidate table or it does not. The parser for it
already exists, in `packet::handlers::waypoints`.

### Where the shape comes from: `decode --layout`

Everything above needed to know *where* a field sits and *how* it is
written. On a patch with an archive, neither has to be guessed, and
`rofl-x decode --netid N --layout` works it out:

```
rofl-x decode -r <16.9 replay> --netid 472 --layout
```

For every payload byte and every byte of every object word it asks
whether the map between the two holds in both directions across every
block. A real correspondence survives thousands of blocks; a
coincidence does not. On `PKT_S2C_FaceDirection_s`:

```
fields in the order the class writes them:
  +0x01c  4 bytes, little-endian
  +0x018  4 bytes, little-endian
  +0x014  4 bytes, little-endian
substitution table: 256 of 256 entries from 10 positions, 0 conflicts
```

A complete table, no conflicts, from one class in one replay. That is
the measurement behind "one 256-entry substitution per class".

The same run on `PKT_NPC_Hero_Die_s` gives the layout this section
relies on — fields written `z`, `height`, `x`, then two more, then the
death timer, then the killer, every one little-endian, the killer last
— and then says:

```
NOT SETTLED: with 34 blocks some of the layout above is coincidence.
```

Which is the honest answer. A class that fires sixty times a game
cannot settle a table: when every block carries a different value for
two bytes, any pairing of them is bijective and proves nothing. The
field order survives that (it is corroborated across replays and by
the 16.19 payload matching it byte for byte), the table does not.

### Two tables from the same patch are unrelated

Worth stating because it rules out a shortcut. With a full table for
`PKT_S2C_FaceDirection_s` (256 entries) and most of one for
`PKT_S2C_UpdateDeathTimer_s` (107), the two were compared over their
107 shared inputs: not identical, and no constant XOR, no constant
offset, no input shift explains more than 3 of the 107. They are
independent permutations.

So a table recovered on one class says nothing about any other, and a
table recovered on 16.9 says nothing about 16.19. Every class has to
be opened on its own evidence. Only the *shape* — field order, byte
order, where the varint sits — carries across.

### What this does and does not generalise to

It generalises to any class whose plaintext can be pinned from
outside. Two kinds have worked so far: a field with few possible values
and a game rule that fixes which (the visibility team byte), and a
field whose values are counted by statsJson (the killer). A third looks
reachable — a coordinate, constrained by having to lie on the map and
to move continuously in time — and is not done.

It does **not** give a general decoder. Each class has its own table,
and a class with ten varying bytes and no external handle on their
values stays shut. Nothing here reads the game's code; this is file
analysis, and the tables come out of the file.

### The movement class's table, partly recovered

The payload being the entry buffer backwards puts a single-waypoint
entry's coordinates at `[-11]`..`[-14]`, and that is where the class
gives itself away. For blocks of that shape the coordinate **low**
bytes take all 256 values, while the **high** bytes take 24 and 22 —
which is the alphabet a map coordinate is allowed, since `x` on the
map is `v * 2 + 7358` and `x` in [0, 15000] confines `v` to
[-3679, 3821]. A speed mantissa in the same packet takes 255 values,
for contrast.

A high byte that narrow also moves slowly: measured over 320,428
consecutive packets for the same unit, it is **unchanged 91.9 %** of
the time. So when it does change it changes by one, and the pairs seen
changing lay the plaintext alphabet out in order. Walking that chain
from a known entry recovers its neighbours, then theirs.

Nothing in it is assumed:

- `T(0x00) = 0x42` is the byte the net id's always-zero field carries,
  at `[-5]`, on 100 % of blocks.
- The walk's **direction** comes from the flags high byte at `[-2]`,
  which the format allows to be only 0 or 2. Whichever direction puts
  that byte's other value on plaintext 2 is the right one.
- Which entry shape to read is **not guessed**. Counting blocks picks
  the wrong one, because units are usually given a path rather than a
  point, so the commonest shape is two waypoints. Taking the shortest
  payload picks the wrong one too, because one anomalous block poisons
  a minimum. Every candidate shape is tried and the check below
  decides; only `0xd1` survives it.

The check is the flags **low** byte, which the chain never looks at
while it walks. That byte is the waypoint count doubled, so the
chain's small even plaintexts have to be values it actually takes, and
plaintext 1 — zero waypoints — must not be. **Five of them agree, none
clash.**

What comes out is **27 of 256 entries**, spanning plaintexts `0xf3` to
`0x0d`:

```
f3->5d f4->23 f5->20 f6->59 f7->52 f8->d8 f9->b7 fa->9d fb->91 fc->c5
fd->63 fe->e6 ff->e0 00->42 01->07 02->d1 03->f8 04->94 05->d4 06->d9
07->fe 08->2d 09->dc 0a->0d 0b->6d 0c->b5 0d->d7
```

That range covers `v` from minus 3328 to plus 3583, which is map
coordinates from about 700 to 14500 — **the whole playable map, at the
resolution of a high byte**. So a unit's position on patch 16.19 is
now readable to within about 512 units: enough for a lane-level
heatmap, not enough for a trajectory.

The low byte is what stands between that and a real position, and it
does not yield to the same trick: a unit moves a few hundred units
between packets, which is most of a low byte, so consecutive low bytes
are not neighbours and no chain forms. That is the open problem.

`T(0x40) = 0x29` is known too, from the net id's top byte, but it is
not in the table above and cannot be: a coordinate's high byte never
takes the value `0x40`, so the chain never reaches it.

**Where positions have got to.** Both classes that carry them are now
partly read: the movement class to within a high byte, the death class
not at all.

The **waypoint class** is netid 1267, which is where movement and
therefore move orders live. It is 3.21 % of all blocks, it matches
16.9's waypoint class with the largest margin of any pair in the set,
its payload runs 18–352 bytes against 16.9's 18–367, and its byte
concentration is 23.8 % against 16.9's 20.3 %. Its wire format is
known, two entries of its table are known, and the position of every
coordinate in the packet is known — all of which is above. What is
missing is the rest of the table.

The **death class's** coordinates are *located* but not *read*. The payload map shows them
plainly — three consecutive little-endian floats, each with an
exponent byte taking ten values, exactly matching the 16.9 layout:

```
  [ 8..12]  float   mantissa, mantissa, mantissa, exponent (10 values)
  [12..16]  float   same shape
  [16..20]  float   same shape, narrower -- the height
  [31..35]  float   5b 5b mantissa exponent   <- the death timer, and
                                                 5b is this class's T(0x00)
  [35]      killer varint
```

Reading either needs the table at the mantissa bytes, which is
roughly 200 more entries, and the constraints available are weak: an
exponent byte is pinned by the map's bounds, but a mantissa byte is
uniform and only constrained by the pair `(x, z)` having to be
somewhere a champion can stand. With 1,955 deaths that is an
under-determined constraint problem, not a lookup.

The waypoint class is the better target for exactly the reason the
death class is a poor one, and it is now much further along. Its wire
format is settled, it has 118,885 blocks rather than 1,955, and the
check on a candidate table is brutal: the entry stream has to parse to
exactly its own length, the waypoint counts in the flags have to agree
with the bytes consumed, and every coordinate has to land on the map.
A wrong table fails that on the first block. The parser is already
written — `packet::handlers::waypoints` — so what remains is the
search, not the format.

## 4. Netids identified by when they fire

Once there is a kill feed, it labels the rest. For every champion we
know when they died and when they got a kill, so any *other* class
that only ever talks about a champion at those moments is about that
event. This needs no payload from the class being identified — only
when it fires and about whom.

The comparison is against chance, not against zero: the windows around
a champion's deaths cover some share of the game, so a class ticking
steadily lands in them occasionally by accident. What is reported is
how many times more often than that it actually does.

On 16.9, as a control, it finds four and all four are right:

| netid | fires on | blocks | vs chance | what it is |
|------:|----------|-------:|----------:|------------|
| 472 | their own death | 328 / 328 | 107× | `PKT_NPC_Hero_Die_s` — the death itself |
| 715 | their own death | 328 / 328 | 107× | `PKT_S2C_UpdateDeathTimer_s` |
| 277 | their own death | 331 / 350 | 102× | `PKT_S2C_HideOwnerEmotes_s` — emotes are hidden when you die |
| 115 | any kill in the game | 1,057 / 1,057 | 15× | `PKT_Update_HeroStats_KDA_s` — the score |

No false positives, and netid 277 is one nobody would have thought to
look for. On 16.19:

| netid | fires on | blocks | vs chance | |
|------:|----------|-------:|----------:|-|
| 1080 | their own death | 1,955 / 1,955 | 53× | the death class, already known |
| 601 | their own death | 1,955 / 1,962 | 53× | **third** line of evidence that this is the death timer |
| 918 | their own death | 1,925 / 2,058 | 50× | **new.** Nothing else had found it |
| 590 | any kill in the game | 7,764 / 7,764 | 6× | the score class, already proved |

Netid **601** is now settled by three independent routes: a payload of
exactly 5 bytes addressed only to champions, which is the death
timer's shape on 16.9; its per-champion block count matching
`NUM_DEATHS`; and now firing only when its champion dies.

Netid **918** is new, and its profile matches 16.9's
`PKT_S2C_HideOwnerEmotes_s` closely — 2,058 champion blocks against
1,955 deaths, where 16.9 had 350 against 328, the same slight excess.
That last step is an analogy between two patches, not a measurement,
so it is a candidate name on a confirmed on-death class.

## 5. Netids proposed by fingerprint

Riot renumbers netids every patch but rarely rewrites a class, so what
a class looks like from outside its payload mostly survives: how often
it fires, what its payload lengths are, whether it addresses a unit or
the game, how many entities it ever names, how its traffic spreads over
the game. Matching those against a patch we *can* decode proposes a
name for each netid.

**Payload length does most of the work.** The set of lengths a class
emits is the shape of its fields: 2 bytes on every block is one small
field; 5/6/7/10/11 is a fixed head with a short optional tail.

**How concentrated its bytes are does the next most.** A class that
delta-codes paths spends most of its payload on small signed numbers
that are usually the same one, so a single byte value covers a fifth
of everything it ever sends. A class carrying floats and hashes
spreads out. Measured, on payload bytes alone:

| class | commonest byte | commonest eight |
|-------|---------------:|----------------:|
| 16.9 netid 916, the waypoint class | 20.3 % | 43.0 % |
| 16.19 netid 1267 | 23.8 % | 49.0 % |
| 16.19 netid 1080, deaths | 6.0 % | 24.2 % |
| 16.19 netid 590, the score | 16.4 % | 47.9 % |

How good is it? Split the 16.9 replays in two, fingerprint each half
from different games, and match — the answer is known, so the error
rate is measurable:

| gate (score, margin) | calls | right | precision |
|----------------------|------:|------:|----------:|
| any | 196 | 154 | 78.6 % |
| ≥ 0.70, ≥ 0.05 | 142 | 128 | 90.1 % |
| ≥ 0.85, ≥ 0.10 | 91 | 86 | 94.5 % |
| **≥ 0.85, ≥ 0.15** | **68** | **65** | **95.6 %** |
| ≥ 0.95, ≥ 0.15 | 41 | 39 | 95.1 % |

The margin over the runner-up is what carries the signal; the score
barely moves precision. The shipped gate is the fourth row, and only
matches that clear it are reported.

Adding byte concentration to the score was worth it on the measure
that matters most and not on all of them: netids recovered across the
whole self-test went from 154 to **165** of 254 and the traffic
recovered from 74.8 % to **75.5 %**, with precision unchanged, but the
gate now admits 68 calls where it admitted 72. More of the mapping is
right; slightly less of it is reported as certain.

Against 16.9, the twenty 16.19 replays produce **63 confident matches
out of 273 netids, 62.3 % of blocks.** The ones worth believing,
because something independent agrees:

| 16.19 netid | share | payload | proposed class | why it holds up |
|------------:|------:|---------|----------------|-----------------|
| 539 | 37.4 % | 2 | `PKT_S2C_OnEnterTeamVisibility_s` | same payload, same share, busiest class on both patches |
| 69 | 0.45 % | 2 | `PKT_S2C_OnLeaveTeamVisibility_s` | its pair, same payload, correct relative volume |
| 946 | 10.6 % | 1/2/3/4 | `PKT_S2C_ClearHealthThreshold_s` | identical length set, second busiest on both |
| 925 | 0.56 % | 2–15 | `PKT_CHAR_SetCooldown_Broadcast_s` | identical length set |
| 1148 | 0.91 % | 5/6/7/9/10/11 | `PKT_NPC_BuffRemove2_s` | identical length set |
| 1255 | 0.00 % | 9–33 | `PKT_NPC_Die_Broadcast_s` | identical length range, score 0.993 |
| 601 | 0.01 % | 5 | `PKT_S2C_UpdateDeathTimer_s` | and statsJson agrees (§ 2) |
| 72 | 0.01 % | 9/13 | `PKT_HeroReincarnateAlive_s` | respawns, right order of magnitude |
| 1267 | 3.21 % | 18–352 | the **waypoint class** — movement, where positions and move orders live | three ways: the strongest margin in the whole set (0.41), a payload range of 18–352 against 16.9's 18–367, and the byte concentration above |

**These are hypotheses.** The 95.8 % figure is a ceiling, not an
expectation: the self-test renumbers nothing and changes no class, it
only asks whether a fingerprint taken from two games finds the same
class in three others. A real ten-patch gap also moves the classes
themselves. About one in twenty of the confident calls is wrong even
under ideal conditions, and nobody knows which one.

Where these agree — netid 601, found by § 2, § 4 and § 5 — the answer
is not a hypothesis any more.

## What this does not do *yet*

The goal for a packed patch is the same as for one we can decode: the
whole file, every action in it (README § the goal). This page is how
far along that is, not a smaller ambition.

- **Most payloads stay unread.** § 3 opens the classes whose plaintext
  something outside the file pins down — the visibility pair at 37.9 %
  of traffic, the killer field of the death class, and the movement
  class to within a coordinate's high byte. Damage numbers, item ids,
  spell casts and exact positions are not reachable today.
- **No class is named with certainty by fingerprint alone.** § 2 and
  § 4 produce proof, and only for classes that correspond to a counter
  Riot kept or to an event the kill feed knows about. § 5 produces
  candidates, with a measured error rate.
- **Nothing touches the game.** No binary is unpacked, no process is
  read, and nothing here is any use against a live match. That is a
  constraint on method and it is permanent; the rest of this list is
  just work not done.

## What would make this much stronger

More patches with both a decodable binary *and* replays. The self-test
measures fingerprint stability within one patch; with two decodable
patches the real cross-patch error rate could be measured instead of
bounded, and the weights fitted to it rather than set by hand.
