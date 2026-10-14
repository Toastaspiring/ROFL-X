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

**What those twenty replays are**, because it matters for anything
counter-based: eleven are build `16.19.821.7343` and nine are
`16.19.823.0722`, and the netid registry is the same in both — the two
strict proofs of § 2 hold 10/10 and 9/9 when the builds are scored
apart, so pooling them is sound. By **game mode** they are not
uniform: seven are Summoner's Rift, and thirteen are an augment mode
with no wards, no vision score and no assigned lane positions. So
`WARD_PLACED` and friends are flat zero in most of the set, and a ward
class can only be looked for in the seven. Where a result depends on
the mode, this page says which replays it used.

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

### When equality is the wrong test: spell casts

A death is one packet, so the strict test fits it. A cast is not: a
held ability, a recast, a toggle, a transform or a steerable missile
all send more than one, and how many more depends on the champion. The
identity is perfectly real and the equality is simply false, so a
matcher built on equality is blind to the entire class of events —
which on the goal's terms is most of them.

The weaker question is which netid *moves with* the cast counters, and
the work is then in ruling out the two ways that answer could be
worthless. `scripts/counter_class.py` does both, and refuses to report
anything on a packed patch until it has picked the right class on one
that decodes.

**The activity confound.** A player who casts more also farms, fights
and buys more, so any busy-player class correlates with any
busy-player counter. On 16.19 netid 474 does not behave like that:

| counter | r |
|---------|--:|
| `SPELL1..4_CAST + SUMMON_SPELL1..2_CAST` | **+0.827** |
| `SPELL1..4_CAST` | +0.820 |
| `ITEMS_PURCHASED` | +0.419 |
| `CHAMPIONS_KILLED + NUM_DEATHS + ASSISTS` | +0.338 |
| `TOTAL_TIME_SPENT_DEAD` | +0.185 |
| `MINIONS_KILLED` | **−0.019** |
| `PING` | −0.102 |

Minions killed is the sharp one: farming is almost pure activity and
has nothing to do with casting, and 474 ignores it completely.

**The coincidence.** With a few hundred netids something will
correlate, so the candidate has to satisfy a prediction a coincidence
need not. If 474 is the cast class, its surplus over the counter is
the extra packets a cast can generate — which means the surplus has to
land on the champions whose abilities are held, recast, toggled or
transformed, and not on the point-and-click ones. That is a claim
about names. Measured over 20 games and 57 champions:

| most surplus | | least surplus | |
|---|--:|---|--:|
| Pantheon | +206 % | Volibear | −0.2 % |
| Graves | +177 % | Vel'Koz | −0.6 % |
| Warwick | +105 % | Zeri | −1.2 % |
| Yuumi | +90 % | Cassiopeia | −2.1 % |
| Xin Zhao | +86 % | Jhin | −2.4 % |
| Galio | +80 % | Malphite | −7.3 % |
| Lulu | +74 % | Miss Fortune | −8.7 % |
| Gnar | +67 % | Annie | −9.1 % |

Pantheon, whose every ability is held or channelled, at the top;
Annie, the point-and-click archetype, at the bottom. Nothing about
"busy player" orders a list that way.

**And the method picks the right class where that can be checked.** Run
on 16.9, the same ranking puts the real cast class first:

| netid | r | blocks per cast | class |
|------:|--:|---:|-------|
| 985 | +0.764 | 1.20 | **`PKT_NPC_CastSpellAns_s`** |
| 644 | +0.660 | 0.07 | `PKT_NPC_UpgradeSpellAns_s` |
| 652 | +0.608 | 2.73 | `PKT_UnitApplyDamage_s` |

**So netid 474 on 16.19 is `PKT_NPC_CastSpellAns_s`.** Three
independent things say so: it tracks casts and not activity; its
surplus falls on exactly the champions it should; and the fingerprint
matcher of § 5, which never sees statsJson, proposes that same 16.9
class for it. The packets-per-cast ratio agrees too — 1.20 on 16.9,
1.15 on 16.19. The netid holds in both 16.19 builds scored separately
(r = +0.791 over the ten games of `16.19.821.7343`, +0.848 over the
nine of `16.19.823.0722`), and the two strict proofs above hold 10/10
and 9/9 in those same splits, so the registry really is shared and
pooling the builds is sound.

The cast identity survives the mode split too, which is worth saying
because the surplus table above pools both: ranked within the seven
Summoner's Rift replays alone, 474 is still first at r = +0.721 with
1.17 packets per cast; within the thirteen augment-mode replays alone,
first at +0.856 and 1.14.

### The same question, asked of other counters

`scripts/counter_class.py --field <counter>` points the same machinery
anywhere, and the validation step is what keeps it honest: it first
ranks the counter on 16.9 and prints the name it lands on, so a run on
a packed patch is worth exactly as much as that name is right. Three
more came out of the seven Summoner's Rift replays, each ranked first
for its counter and each with a 16.9 counterpart whose packets-per-event
ratio agrees:

| 16.19 | counter | r | per event | 16.9 picks | r | per event |
|------:|---------|--:|----------:|------------|--:|----------:|
| 890 | `ITEMS_PURCHASED` | +0.907 | 1.28 | 306 `PKT_BuyItemAns_s` | +0.887 | 1.19 |
| 524 | `ITEMS_PURCHASED` | +0.783 | 0.72 | 1045 `PKT_RemoveItemAns_s` | +0.868 | 0.72 |
| 467 | `WARD_PLACED` | +0.820 | 2.43 | 1196 `PKT_UseItemAns_s` | +0.892 | 2.39 |

They pass the confound check as sharply as 474 does — netid 890 tracks
purchases at +0.907 while sitting at −0.086 on minions killed, −0.022
on casts and −0.082 on pings; 467 tracks ward placement at +0.820 and
casts at −0.137.

**Netid 524 is the strongest of the three**, because § 5's fingerprint
matcher calls it `PKT_RemoveItemAns_s` *confidently* — score 0.968,
margin 0.231 — entirely independently of statsJson. Two methods, one
answer. For 890 and 467 the fingerprint proposes something else with a
low score and does not call it confident, so they rest on the counter
evidence alone.

These are thinner than the cast result: four to six games rather than
nineteen, because only seven replays have the mode for it. Wards being
on the goal's list, netid 467 is where to look next, and more
Summoner's Rift replays of this patch would settle all three.

What is *not* claimed: this is not the strict proof that names 1080.
It is a correlation plus two checks it had to survive and one
cross-patch validation, and the honest reading is that 474 carries
casts, not that every one of its blocks is a cast. A few champions sit
a few percent *below* the counter, which the identification does not
explain. Reading the payload — which ability, at whom — needs 474's
substitution table and is not done.

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

### But a table is not always 256 free choices

That comparison asked whether one table *predicts another*. A separate
question is whether a table has internal structure, and for at least
one class it does. `decode --layout` now prints the table it recovers,
and the cast class's is this:

```
    00  e6 f6 66 76 a6 b6 26 36 e4 f4 64 74 a4 b4 24 34
    10  e7 f7 67 77 a7 b7 27 37 e5 f5 65 75 a5 b5 25 35
    ...
```

Take XOR differences from `T(0x00) = 0xe6` and each single bit of the
plaintext maps to a single bit of the payload byte:

| plaintext bit | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
|---|---|---|---|---|---|---|---|---|
| payload bit | 4 | 7 | 6 | 1 | 0 | 3 | 2 | 5 |

So `T(p) = perm(p) XOR 0xe6` — **affine over GF(2)**, and all 256
entries agree with it. `scripts/table_structure.py` is the test.

This is checked rather than admired, because an affine table out of a
recovery procedure could be an artefact of the procedure. The layout
pass only uses blocks of the commonest payload length, which leaves
3,966 blocks of other lengths untouched; the class's three trailing
4-byte fields sit at fixed offsets from the payload *end*, so they are
readable in those blocks too. Decoding them with this table gives
**11,898 of 11,898 fields exactly right**, none of which it was built
from.

**Why it matters.** An affine table is 16 bits of structure, not 256
free choices:

- nine pairs whose plaintext differences span the byte determine every
  entry — measured: fitting from `T(0)` and the eight single-bit
  plaintexts predicts **247 of 247** of the rest;
- the whole family has only `8! * 256 = 10,321,920` members, small
  enough to enumerate against whatever a packed patch does offer — a
  range, an enum, a parse that has to come out exact.

**But it is a one-off, which is the more important half.** The test was
run over all 273 classes. `decode --layout` recovers a usable table
for far fewer than that — 222 classes give none at all, their fields
being too rare or too constant to pin — but of the **23** whose tables
can be tested, exactly **one** is affine:

| netid | affine fit |
|------:|-----------:|
| 985 `PKT_NPC_CastSpellAns_s` | **256 / 256** |
| 201 | 84 / 256 |
| 410 | 64 / 256 |
| 652 | 48 / 256 |
| 876 | 38 / 256 |
| 347 | 34 / 255 |
| 916 WaypointGroup | 10 / 256 |

So this is not a shortcut into the format. It is one class whose
payload-to-object byte relation happens to be a bit permutation, and
whether that is the obfuscation differing or that class serialising
its fields differently cannot be told from the file.

The movement class is the one we most want and the least structured of
all of them: 2 fixed points, and no multiplier, rotation or
bit-reversal explains more than 7 of its 256. That is why its low byte
had to be searched for rather than solved, and it says the search was
not missing an obvious trick.

So the first question to ask of a class on a packed patch is not "what
are its 256 entries" but "is it affine", which a handful of pairs
answers.

### A movement packet is a click

The waypoint class does not report where a unit is. It reports where a
player asked it to go: the path, and the speed to walk it at. The unit
sets off and is usually interrupted by the next click long before
arriving, so the end of a path is a destination and not a position.
Reading only the first waypoint, as this did at first, throws away
everything in between and makes a unit teleport from click to click.

The speed is also a sharp test of the table, because it has to come out
as a speed. On 16.9 it reads a median of **395** map units a second
against a champion's 325 walking and about 500 with boots, peaking at
1780 for dashes. On 16.19 it read **1524**, which is not a walk — and
trying every offset in the entry found none that read better, so the
field had not moved and the table was wrong at its bytes.

**One entry fixed it.** A speed in [256, 512) can only have the
exponent byte `0x43`, and that position shows a single cipher `0xcb`
95 % of the time, so `T(0x43) = 0xcb`. The table had `0x44` there,
one step out, which doubles every speed:

| | median | within 250–650 | commonest |
|---|--:|--:|---|
| before | 1428 | 0.3 % | 1400, 1524, 1576 |
| after | **381** | **95.5 %** | **350**, 381, 394, 337, 357 |

Those are champion and minion speeds. The mantissa needed no forcing:
its commonest cipher `0xe5` was already pinned to `0xaf` by the
champion naming, and `0x43AF0000` is 350 — which is why 350 comes out
commonest. An earlier attempt that forced the mantissa too stole
`0xe5` from Cho'Gath and dropped the naming to 9 of 10, which is how
the overreach was caught.

### Not every position change is a walk

Walking a unit along its path at its own speed predicts where the next
packet finds it, but not exactly, and dumping the leftover at the
boundary is what makes a unit snap. Spreading it across the interval
removes that — while the unit really walked.

It does not always. A recall, a respawn, a flash and a dash are not
movement: the game puts the champion somewhere, and the packet predicts
none of it. Spreading a residual of fifteen thousand units draws a
champion gliding across the whole map over the interval, when what
happened was instant. The smoothing invents the very motion it exists
to reproduce.

So past 600 units — a flash is 400 — the jump is kept as a jump and
happens at the packet reporting it. **2.4 %** of steps are beyond that.
Sampled every 10 ms across all ten champions, the median step is then
**2.07 map units**, the 99.9th percentile **10.6**, and every step
larger than a flash sits exactly on a packet boundary where the game
moved the champion: **0** stray discontinuities in 1.58 million samples.

### The corner outside the map means "not here"

Champions kept appearing at one of two spots outside the map, `(-632,
-558)` and `(15362, 15398)`, one per side. They are not positions. The
stream parks a unit there instead of reporting it, and the reason is
visibility: of the moments a champion sits at its corner **alive**,
**93 %** are moments the enemy could not see it. The rest are deaths.

This had been showing up for a long time as the 728 positions that fell
off the map and would not go away. **516** of them are this, and the
remaining 212 (0.25 % of all positions) are genuine table error. Read as
positions they throw a champion across the map and back, which is where
most of the apparent recalls came from.

The parser still reports them, because the file says them and the 16.9
control is byte-identical to the game's own deserializer. The reading —
that they mean "not here" — belongs downstream, and so does dropping
them.

### Deaths, recalls, and the teleport that cannot be found

statsJson is the referee again. It carries each player's two summoner
spells and how many times each was cast, and `NUM_DEATHS` per player.

**Deaths are exact.** The death class names its victim in a plaintext
`param`, so it needs no table at all. Its **74** deaths split per
champion exactly as `NUM_DEATHS` does — 1 for Darius, 4 for Cho'Gath,
15 for Ahri, all ten matching. That is what says the class is read
correctly, and it gives the viewer a champion that is off the field
rather than loitering at a corner.

**Recalls are found by their shape, and are an inference.** A champion
recalling holds one spot — eight seconds, or four under Baron's
empowered recall — and is then put in its own fountain. So what to look
for is a silence that starts away from base, ends at the fountain, has
no death to account for it, and covers more ground than the champion
could have walked in that time. **37** of them, 1 to 10 per champion.

The channel's own length is *not* measurable here, which is worth being
plain about. Standing still and being out of sight are the same thing in
this class — both are silence — so the gap is the hidden time plus the
channel, and it runs from 4 to 20 seconds with no peak at 8. What the
eight seconds give instead is a floor: a silence shorter than four
seconds cannot be a recall however much it looks like one, and that
rules some out. There is no recall counter in statsJson to check the
rest against.

Looking for the jump instead of the silence was the wrong way round, and
gave 30 events of which 32 of the first 42 landed at a corner rather
than a fountain — champions going out of vision, not recalling.

**Teleports cannot be found at all, and the page draws none.** statsJson
says exactly one player carried Teleport and cast it twice. There are
**323** jumps in that game the reporting packet does not explain, and
nothing separates those two from the rest: the channel is not visible
either, since Gwen's two real teleports are not among the candidates
that stood still beforehand. The same holds for flashes — 80 short
jumps against 33 casts, and Karthus, who had no Flash, contributes four.
Ability dashes, slows and table error all leave the same signature.
Drawing a guess would be worse than drawing nothing.

### Naming a champion, and what that pays for

The movement class calls a unit by its net id, which sits behind the
substitution — so on a packed patch the positions were anonymous. Three
things the replay gives up in plaintext name them anyway:

- **deaths.** The death class fires once per death and addresses its
  victim, and `param` is plaintext, so every champion's exact death
  times read with no table at all. A champion that dies stops sending
  waypoints for the death timer, so its movement stream carries the
  same times as gaps. Exact where it applies and silent where a
  champion never dies — one survivor is fine, since the assignment
  fills the last place by elimination, two is not.
- **lane.** Every player has an assigned position, and early in a game
  a player is in that lane. Always available; blind to the difference
  between a bottom carry and its support, who share a lane.
- **side.** In any lane the blue-side player stands nearer the blue
  base. Always available, and unharmed by a constant error in the
  coordinate origin, because both players in a lane shift together —
  which matters, because 16.19 has one.

None is enough alone, so `scripts/name_champions.py` scores all three
and solves one assignment. On 16.9 all three agree on all ten and
reproduce what the known table decodes. On 16.19 they name all ten, and
a lane check against each player's assigned position passes **10 of
10** — where the table's own decode had managed 2 of 6.

**What it pays for is the table, not the labels.** A named champion
pins its net id's low byte, which is an exact entry at a plaintext the
high-byte chain never reaches. The constant-speed search cannot find
those on its own: it only constrains ciphers that appear as
*coordinates*, so a cipher appearing as a net id is invisible to it and
gets assigned arbitrarily. That is why 16.19's positions could be
physically right — speeds median 331 units/s against the control's 340
— while only **1 of 10** champion ids decoded correctly.

Fed back as ten more anchors, the search returns a table whose own net
id decode agrees with the behavioural naming **10 of 10**, and the
champions land in their assigned lanes 10 of 10. The scaffolding then
comes away: with the table right, the net id names the champion
directly, which is how the control patch does it.

### Fog of war, which does not have to be modelled

The visibility pair is already read (§ 3), and it covers everything,
not only champions: on one 16.19 replay it is **482,518 blocks over
9,941 distinct units**, each saying that a unit entered or left one
team's sight. Both the unit and the time are plaintext, so the windows
need no table.

That means the game's own record of who could see what is in the file,
and a viewer can show the match as one side saw it rather than
modelling vision. Per champion, the share of the game spent visible to
the enemy comes out as:

| | visible to the enemy |
|---|---|
| Cho'Gath, Karthus (junglers) | 36 %, 33 % |
| the six laners | 58 % – 69 % |
| Miss Fortune (bottom) | 71 % |

Junglers spending half as long in enemy sight as laners is the check
that this is real and not an artefact of how the windows are paired.

**Whose unit is it** follows from the same packets, and the rule reads
backwards at first. The visibility pair fires when a team *gains or
loses* sight, so a side generates no events at all for its own units —
it can always see them. The owner is therefore the team with **fewer**
events, not more. That places all ten champions on the right side, and
decides 2,860 of the 9,941 units; the rest see equal traffic from both
sides and stay undecided.

That is what lets the fog be *drawn* rather than merely applied. A
side's lit ground is the ground around the units it owns, so the viewer
fills the map dark and cuts holes at those positions.

Units are not the only source. A turret sees 1350, the same as a
champion, and grants it to its own side. The turret at the spawn point
sees far enough to cover the whole base, which is a statement about an
area rather than a radius: the base is a triangle with three lane
mouths, and a circle wide enough to reach its far corner — the mid
inhibitor turret, 4,587 units out — would spill into the river and
light ground the team cannot see.

So the base is lit as its own shape. The fountain, the nexus, its two
turrets, the three inhibitors and their turrets stand at the base's
corners, and their convex hull is the base.

That hull stops at the structures, though, and the floor runs on past
them to the walls. How much further is measured rather than guessed:
the minimap draws the base floor blue-grey and the open ground outside
it olive, so flood-filling the floor from each fountain and measuring
how far its pixels lie beyond the hull gives **950 units** one side and
**1,050** the other. At those widths each region covers 99 % of its own
floor. About an eighth of what it lights is not floor — the hull's
rounded corners reaching into the base walls, which are inside the base
anyway.

Checked point by point, that lights all nine structures of a base and
nothing beyond them: the inner and outer turrets, the river, the side's
own jungle and mid all stay dark. It covers 10.5 % and 11.1 % of the
map, which is about what a base is.

Adding turrets and the two bases takes the lit share from 20.7 % to
between **31 % and 41 %** depending on the moment, which is what a real
minimap looks like: lanes lit along the tower line, and a base that
glows.

A fallen turret sees nothing, and nothing read so far says when one
falls, since a structure never moves and so never sends a waypoint.
Late in a game this is therefore generous, and the page says so. Two halves of
that are different in kind and the page says so: **which units a side
could see is read from the file; how far a unit sees is not in the file
at all**, so the lit radius is a model, about 1150 map units. The lit
area is also an undercount, since only the 39 % of positions whose
owner could be decided light anything.

### What the movement class already shows besides champions

Monsters walk inside their camp, so a camp is a place where units stay
put. Taking every unit whose positions sit inside a 900-unit radius and
clustering them gives **26 such places** on one replay, and two of them
are recognisable: one at (9842, 4406) against the dragon pit's roughly
(9866, 4414), and one at (4790, 10184) against baron's roughly (5007,
10471). That lead turned out to be the whole story, and it settles the offset.

**There is no offset.** Matching the detected clusters against the
sixteen landmarks whose real positions are known — the twelve camps,
both pits and the scuttle spawns — the best fit is a shift of
**x by −20 and z by +40**, which is nothing, with **15 of 16 landmarks
matched at a mean error of 259 units**. That residual is the spread of
a camp's own monsters, not a systematic shift: gromp matches to 45 and
51 units, wolves to 83, the dragon pit to 128.

So the earlier "constant offset of 400 to 900 units" was wrong. It came
from fountain coordinates that were guesses, and from a table that was
still poor at the extremes. With the 36-anchor table the blue-side
champions register at the blue fountain at last (24 to 91 visits each,
where they had 0), and the four that still read as the wrong side are
the team that *won*: they spent the end of the game standing in the
loser's base. The fountain test was a bad heuristic for a finished
game, and the positions were right all along.

**Turrets are not in this.** A structure never moves, so it never sends
a waypoint, and nothing here gives its position. Plants and wards are
in the spawn class with their coordinates, which `SpawnMinion` gives on
16.9 — on 16.19 that class is not read yet, so neither are they.

### Wards: the class is found, the table is not

Wards are the obvious next thing after positions, and on **16.9 they are
already read** — `rofl-x file` returns 158 of them with name, owner,
position and duration, and its self-check against statsJson passes
`wards_placed 10/10`. The question is only how to get there on a packed
patch.

The scoreboard says what the answer must be: `WARD_PLACED` totals **84**
in the 16.19 sample, split Maokai 24, Karthus 10, Ahri 10, Cho'Gath 9,
down to Qiyana 3, with `WARD_PLACED_DETECTOR` naming the 5 control wards
among them.

**The class is netid 898.** On 16.9 wards live in the spawn class, netid
214, which the packet catalog records as matching `WARD_PLACED` exactly,
50 / 50. Its 16.19 counterpart was proposed by fingerprint at a score of
only 0.647 — not confident — but everything structural agrees:

| | 16.9, netid 214 | 16.19, netid 898 |
|---|---|---|
| blocks | 2,729 | 1,309 |
| distinct net ids | 2,729 — one per block | 1,309 — one per block |
| ids in increasing time order | 97.7 % | 98.7 % |
| first id | `0x400001ff` | `0x40000201` |
| last id | `0x40016c51` | `0x40012965` |
| payload bytes | 58–68 | 58–72 |

One net id per block, handed out in order, starting just past the
`0x200` where the game's own start-of-game entities end: that is a
spawner, and nothing else in either patch looks like it.

**What does not work.** Three routes were tried and are worth recording
so they are not tried again:

- *The exact counter proof.* No netid's per-champion block count equals
  `WARD_PLACED`, `WARD_KILLED` or any other ward counter in three or
  more replays. It cannot: the spawn class addresses the ward, not the
  player who placed it, so the owner is inside the payload where the
  counter machinery cannot see it.
- *Names as cribs.* The class names what it spawns, and a substitution
  keeps a word's shape, so `SRU_ChaosMinionMelee` should have been
  findable by its repeats. It is not there, under any offset — the name
  travels as a hash or an id, not as text. The matches a looser search
  did return were all short weak patterns like `SRU_Red` appearing 1,313
  times by chance.
- *Borrowing the movement table.* Wards do not move, so the hope was
  they still report once, standing still at zero speed. A few do: of the
  1,309 spawned net ids only **133** appear in the movement class at
  all, 59 of them static. Attributing those to the nearest champion
  covers 18 of 84 wards, 21 %, correlating with the scoreboard at
  r = +0.57 — the right shape (Maokai leads, as he should) and nowhere
  near enough to use.

#### The class, confirmed by elimination

The fingerprint would not commit, so the question was asked the other
way round: which 16.19 class *could* be a spawner at all? A spawner
addresses a different entity every time, and its payload has to hold the
struct. Of every class in the replay, exactly one has both properties at
any volume:

| netid | blocks | distinct entities | payload |
|---|---|---|---|
| **898** | **1,309** | **1,309** | **64–77** |
| 111 | 179 | 179 | 97–126 |
| 1014 | 42 | 42 | 71 |
| 205 | 4 | 4 | 93/94 |

That is worth more than the 0.647 the fingerprint gave it. The earlier
test — one net id per block, handed out in time order — turned out not
to discriminate at all: netids 942 and 495 pass it too. Payload size is
what separates them.

#### Where the position is on 16.9, exactly

The layout tool gets 433 blocks of one length and says so: `NOT SETTLED`,
with 2,495 conflicts in the table it offers. So the layout was taken
from the wards instead, which are decoded and therefore known.

A substitution is a function, so if a payload byte carries the top byte
of a coordinate, every ward sharing that top byte must share that
payload byte. Scanning all offsets at both alignments, **158 of 158
wards agree** at one place:

    payload[8:12]   x, big-endian float
    payload[12:16]  z, big-endian float

and the per-offset distinct-value counts confirm the shape — `4, 241,
219, 220` then `4, 233, 219, 214`, an exponent that can only take a
handful of values beside three bytes that take hundreds. Those 158 wards
also hand over 141 cipher bytes of 16.9's spawn table, consistent with
no contradictions, which places any spawn to about fifty units.

#### On 16.19 the position is not at a fixed offset, and that is the finding

The same signature is not there. Looked for pooled and length by length,
from the start and from the end, and mirrored in case the class writes
backwards as the movement class does:

- Pooled, either alignment: no pair of exponent-shaped bytes four apart.
- Length by length: one candidate, offset 19 in the 68-byte blocks.
- That candidate fails. The two exponents of a position are not
  independent — the Rift is a square with its traffic on a diagonal, so
  16.9's spawns land 36.5 % on (x `0x45`, z `0x46`), 34.0 % on (`0x46`,
  `0x45`), and so on. No relabelling of offsets 19 and 23 reproduces
  that shape: the best leaves **66.8 %** total difference, because
  offset 19 is 85 % a single value where a real x exponent splits about
  evenly between two. 141 blocks were enough to throw up a coincidence
  and enough to kill it.

Two guesses that were also wrong and are cheap to repeat: the unit
*names* are not in the payload as text under any offset, so they travel
as a hash; and the 12-byte run constant across 339 spawns is not a
minion barracks, because on 16.9 — where the positions are known —
minions do not spawn at one point. They are jittered, and the commonest
exact position there occurs 18 times, not 300.

**What is left.** 16.19's spawn payload has a variable-length field
ahead of whatever the position is, which is the only thing that explains
a field being absent at every fixed offset at both ends. The movement
class on the same patch does exactly that — a reversed buffer,
right-aligned to the payload end, behind a varint header whose length
changes block by block — so the next thing to try is parsing 898 that
way, finding each block's header length first and only then looking for
the coordinates at a fixed offset *inside* the buffer.

### Five ways of getting known plaintext that do not work

Reading a class needs its table, and a table needs known plaintext.
The replay offers four sources — the framing, statsJson, the game's own
value ranges, and a decodable patch's layout — and the ones that have
worked are recorded above. These did not, and each is cheap enough to
be worth trying again only with a reason:

1. **The payload's game-time float against the framing timestamp.**
   The cast class carries a game time (`+0x088`), and the block's
   timestamp is plaintext, so the float's upper bytes ought to be
   pinned by it. Measured: the two agree to **0.0011 s at the median
   and 0.008 s at the 90th percentile**, which is far inside one step
   of the float's second byte, so the idea is sound. But only 76 of
   3,922 blocks match bit-for-bit — our timestamp accumulates from
   millisecond deltas and drifts — and the field is not at a fixed
   4-byte window, so the pairing never becomes exact. The 22 classes
   whose game-time field *is* at a fixed offset have 2 to 54 blocks
   each.

2. **A payload echo of the sender's net id.** `param` is the sender and
   is plaintext, and `FIELDS.md` shows the cast class has an object
   field that equals it (`+0x044`), which would give exact pairs on
   every block. It is not in the payload: scanning every position and
   both byte orders, in raw and `XOR 0x40000000` form, over 19,102
   blocks, **no position is consistent**. The deserializer fills that
   field from the transport, as it does `+0x0c`.

3. **The spell slot as a payload byte.** If the slot's bits sat in some
   byte, that byte's cipher would determine the slot outright, since
   the substitution is a bijection. No position does, at any single
   payload length. The best, `[8]`, carries 89–95 % of the slot's
   entropy with 34–41 distinct ciphers — but it confuses slots 3 and 4
   systematically, about 80/20 across thirty different ciphers, so it
   is something correlated with the slot rather than the slot. A byte
   of the spell-name hash would behave exactly like that.

4. **A count byte driving the payload length.** The length is
   plaintext, so a field that determines it would be recoverable for
   every length the class takes — dozens of exact entries. No byte
   does: the best holds **21 % of the length's entropy on 16.9 and
   24 % on 16.19**. Several variable-length pieces share the payload,
   so no one count explains it.

5. **Assuming affinity and fitting from the bounded floats.** For an
   affine class nine pairs suffice, and the exponent positions give
   nine known plaintexts (`0x00` and `0x3f`..`0x46`). But their XOR
   differences span only **rank 5 of 8**: bit 7 is clear in all of
   them, and bits 3–5 only ever move together. Three independent
   constraints short.

### Which fields a payload has, without its table

A substitution permutes byte *values*, so it cannot change how many
distinct values a position takes. A bounded field therefore stays
bounded through the obfuscation, and that is readable with no table at
all:

- an enum of two values leaves 2 distinct ciphers at its position;
- a float confined to the map has a leading big-endian byte of sign
  plus the top seven exponent bits, which for a value in [0, 16384)
  can only be `0x00` or `0x3f`..`0x46` — so 9 ciphers;
- a coordinate triple is three such positions, four bytes apart.

Mantissas, net ids, hashes and counters take most of the 256 values
and are invisible to this, which is what makes a narrow position
informative. `scripts/payload_shape.py` does it.

Checked on 16.9 first. Netid 985's three trailing coordinates
(`+0x140`, `+0x144`, `+0x148`: x, a height, z) show up as narrow
positions at exactly `120`, `124` and `128` of its 132-byte payload —
which is where `decode --layout` independently puts them — and nothing
else in the last sixteen bytes is narrow at all.

Run on both sides of the 16.9 / 16.19 gap for the cast class, it says
something the fingerprint could not:

| | narrow positions |
|---|---|
| 16.9 netid 985 | 0, 2, 5, 7, 9, 10, 11, 13, **120, 124, 128** |
| 16.19 netid 474 | 0, 1, 5, 7, 9, 13 |

The heads agree closely — both narrow at 0, 5, 7, 9 and 13, including
a float triple at 5/9/13 — so the field order at the start of the
payload survived the patch gap. But **985's trailing coordinate triple
is simply absent from 474**. (The second half of this paragraph, as
first written, also said 474 had no coordinate anywhere else. That was
wrong, and the section "Wards on a packed patch" below corrects it.)

That cuts both ways and both should be said. The shared head is
independent support for 474 being the same class, from a property
neither the counters nor the fingerprint looks at. The missing tail is
evidence against, or at least evidence that the class changed: a cast's
target position is in 985's payload and is not in 474's where 985 puts
it. Since the fingerprint was never confident about this pair either
(score 0.602, margin 0.018), the honest reading is that **474 carries
spell casts** — which the counters and the champion-surplus test
establish on their own — while **474 being `PKT_NPC_CastSpellAns_s`
specifically is unsettled**, and the payload layout is the reason to
doubt it.

### Wards on a packed patch, through the cast class

The spawn class was the wrong door. Placing a ward is a *cast*, and the
cast class carries both what was cast and where it was aimed — so that
is where the wards are, on a patch with no archive at all.

**A correction first.** This document said of netid 474 that "985's
trailing coordinate triple is simply absent from 474, and 474 has no
three narrow positions four apart anywhere else either". The second half
is wrong. 474 does carry a coordinate, at payload offsets **23–26 (x)
and 27–30 (z)**, little-endian floats. `payload_shape.py` missed it
because its narrowness test wants nine cipher values or fewer and these
offsets show twenty — three of which cover 99.4 % of casts and seventeen
of which are rare. A field can be narrow in substance and wide in count.

It was found by asking the game instead of the payload. Every block is
addressed to the champion that cast, and the movement class says where
that champion was to a couple of units, so a scan of every offset
against that answer key turns the question into a measurement:

| offset | reading | agreement |
|---|---|---|
| 26 | x exponent | **95.8 %** over the recurring values |
| 30 | z exponent | **95.0 %** |

and the same three ciphers mean the same three things at both —
`0x80→0x46`, `0x4d→0x45`, `0x18→0x44` — which one shared table requires
and two unrelated fields would not produce.

**The mantissa, from where casts are aimed.** Those bytes do *not* track
the caster, which is itself the finding: the field is the cast's target,
not its origin. But a good share of casts are aimed at the caster's own
feet, so the most common vote wins while the rest scatter over 255
values. Read against x at offset 25 and against z at offset 29 — a
different set of casts and a different set of numbers — the two agree on
**63.4 % of ciphers, where chance is 0.4 %**. Over seven replays that is
151 entries with no collisions; a bootstrap pass, pinning an unknown
byte by the range a cast can reach, brings it to **163 of 256**.

**Which casts are wards, by the counter.** A 4-byte spell id sits at
offset 16, and one value's per-champion tally is

    found       5  9  3  4   2  7   9  10  7  2
    trinket     5  9  3  4  21  7   9  10  7  4

— **eight of ten champions exactly**, with the scoreboard's own
`WARD_PLACED` minus `WARD_PLACED_DETECTOR`. The two it misses are the
support and his partner, and a second id accounts for nineteen of
Maokai's twenty-two remaining wards with nothing left over: the support
item's ward, which is why the trinket alone could never have explained
him. Two ids, 77 of the game's 84 wards.

**What comes out.** 44 of 84 wards placed and located, the rest held up
by the 93 table entries still missing. Of those 44, **none** falls off
the map and none lands further from its caster than a cast can reach —
which is the check that matters, since a wrong table scatters
coordinates everywhere. A ward's *duration* is not read by any of this,
so the page draws the game's own 120 s and says it is a model.

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
resolution of a high byte**. On its own that puts a unit's position
within about 512 units: a lane-level heatmap, not a trajectory. The
low byte, below, closes most of the rest.

The low byte is what stands between that and a real position, and it
does not yield to the same trick: a unit moves a few hundred units
between packets, which is most of a low byte, so consecutive low bytes
are not neighbours and no chain forms. The next section measures that,
and gets most of the way anyway.

`T(0x40) = 0x29` is known too, from the net id's top byte, but it is
not in the table above and cannot be: a coordinate's high byte never
takes the value `0x40`, so the chain never reaches it.

### The low byte, measured rather than argued

Every claim here is reproduced by `scripts/lowbyte_probe.py`, run on
16.9 — where the true table *is* known, so a method gets a score
instead of an opinion. It parses the entry buffer using the chain's
anchors alone, which reaches **418,473 positions over 19,995 unit
tracks**.

**There is no chain.** Over 707,848 consecutive-packet observations the
step a coordinate takes is bimodal: **37.8 % exactly zero**, 55.2 %
more than six, and only 7.0 % in between — of which ±1 is 16.2 %. The
high byte chains only because crossing a 256 boundary *is* a ±1 step;
the low byte has no such regime, and walking it one step at a time
walks into noise.

On the quarter of the data that lone-waypoint selection sees, the same
measurement reads 64.2 % zero and 33.2 % over six: a unit told to hold
still is over-represented there, because a packet carrying one
waypoint often *is* a unit standing still. The split moves but the
conclusion does not, and it moves the right way — parsing every entry
finds *more* long steps, not fewer.

**Landmarks do not help either.** Positions do not pile up at spawn:
the largest first-position cluster is 29 of 9,468 lone-waypoint
tracks, and 179 of the 256 low bytes turn up across the top 14
clusters.

What does survive is **locality** — two consecutive low bytes under a
shared high byte are within `speed * dt` of each other — and, better,
the **size** of the step. Both were tried and both were scored:

| method | exact | within 4 | within 16 | median | 90th |
|---|---|---|---|---|---|
| circular seriation, spectral | 29 | 64 | — | 15 | 40 |
| the same, plus local search | — | 75 | — | 8 | — |
| constant speed, lone waypoints only | 31 | 127 | 239 | 5 | 15 |
| constant speed, **parser-fed** | **53** | **207** | **256** | **2** | **5** |

The first three rows are on the lone-waypoint selection. Seriation
does not improve when given the parsed set — it drops to a median of
16 — because locality is all it uses and the extra observations are
mostly *long* steps, which tell it nothing. The same observations make
the speed objective better, since a long step with a known `dt` is
exactly what pins a speed.

Of the exact entries, 27 are the chain's own anchors, held fixed; the
search earns the rest. Seriation knows only that two bytes are close.
The winner keeps the magnitudes: a unit travels at one speed, so
between packets `dt` apart it covers `speed * dt`, one speed is fitted
per unit by least squares, and the table searched for is the one that
leaves each unit's own fits agreeing.

The last row is the same objective with more to chew on. Selecting
blocks by payload *shape* finds only single-waypoint entries — 57,398
of them over 9,468 units. Parsing the buffer instead reaches every
entry of every payload that parses, and each entry's first waypoint is
a position: **418,473 positions over 19,995 units**, seven times the
observations and twice the units. That, not a better search, is where
the accuracy came from.

Two things make the result transfer. The error is **bounded**: the
high byte is exact, so however wrong an entry is it cannot move a
position by more than 256 of the map's 15,000 units. And the search
**cost tracks the score** — every run that reached a lower cost scored
better — so on 16.19, where there is no table to check against,
minimising the same cost is a measurable proxy rather than a hope.

So a position reads to a **median of 4 map units, nine times in ten
within 10, and never worse than 32** — every one of the 256 entries is
within 16 steps of right. A champion's hitbox is about 65 units, so
that is trajectory accuracy, not lane-level.

What it still is *not* is a decode. The table is accurate and **not
exact**: 53 of 256 entries are actually right, and the cost stops
falling at about 85,000 swaps.

That plateau has two possible causes needing opposite responses — a
search stuck short of the optimum, or an optimum that is not the
answer — so it was settled rather than assumed. Scoring the *true*
table under the same objective:

| | cost |
|---|---|
| the true table | 1.346472e+09 |
| the best the search found | **1.336770e+09** |

**The search beats the truth by 0.72 %.** So the optimum of the
constant-speed objective is not the true table, and no better
optimiser, longer run or further data of this kind can reach it. A
unit does not quite travel at one speed — it accelerates, stops, is
slowed — and that mis-specification is larger than the signal
separating the true table from its near neighbours. The worst entries
are off by 7 to 10, which is the size of that gap.

This is the useful kind of negative: it ends the search direction
outright. Exactness needs a *different* constraint, not more of this
one — and the obvious candidates are measured and rejected below.

### What the path deltas do not give

Once the chain is in hand the deltas are readable — a step of ±13 or
less has a plaintext the chain already names — so a path's *shape* is
exact even though its origin sits behind the unknown low byte. It is
tempting to think that turns into exact arithmetic on the table. Three
ways of trying were measured and all three failed. The numbers are
here so the next attempt starts after them, not before.

1. **`absolute → delta(s) → absolute` relates nothing.** An absolute
   coordinate *overwrites* the running value rather than continuing it
   (`waypoints.rs`), so the second absolute is not the first plus the
   deltas between them. Asserting it anyway yielded 249,364
   constraints and **111,056 contradictions**, and moved no entry off
   its anchor.

2. **"The unit arrived where its path ended" is false.** Had it
   arrived, the next packet's first waypoint would be the last
   waypoint plus any trailing deltas, which mod 256 is an exact
   two-unknown constraint, `T[lo_next] = T[lo_last] + D`. Across
   517,772 candidates that looks like it holds **77.1 %** of the
   time — until you notice 256,015 of them have `lo_next == lo_last`,
   which holds trivially whenever `D` is zero, as it is for 99.7 % of
   them. On the constraints that say anything about the table it holds
   **0.4 % to 1.3 %**. Its precision also *falls* as the gap grows,
   1.3 % under 0.4 s against 0.5 % beyond 3 s; if arrival were the
   mechanism it would rise. A packet reports where a unit *is*,
   mid-path, not where its path ended.

3. **Displacement-on-the-polyline is true but thin.** The shape being
   exact, the displacement between two packets should land on that
   polyline, which couples both axes and needs no speed. It does:
   under the true table the residual's median is **33.6 units against
   a random permutation's 155**. But only **2,770 pairs** have a
   readable multi-point path, because the bitmap of a long path needs
   table entries we do not have, and 33.6 is not zero. So it is a
   refinement for once the table is better, not a way in.

Taken together: the deltas hand over path shapes for free and no
leverage on the table. The soft, scored route above remains the one
that works.

### Positions, emitted

`explore --dump-positions` writes them as JSON lines, one per unit per
moment, each carrying the accuracy it was read at:

```json
{"replay":0,"t":0.079,"unit":"3b42","x":1982.0,"z":2548.0,"accuracy_map_units":256}
```

`unit` is the cipher of the net id's low two bytes rather than the id,
which cannot be read without more of the table; the same unit always
gives the same pair, which is what following one through a game needs.

**Checked against the game's own answer first.** Given the table that
`decode --layout` recovers on 16.9, the file-only parser is run over
the movement class and compared with the positions the deserializer
produced from its own plaintext buffer:

| | |
|---|---|
| payloads parsed | **46,418 of 46,418** |
| positions | 172,857 over 5,662 units |
| identical to the deserializer's | **172,857 — all of them** |
| in one output but not the other | 0 |

So the wire format and the table handling are right, which is the part
that could not be checked on a patch with no binary.

On **16.19**, with only the chain's 27 entries:

| | |
|---|---|
| payloads parsed | 436,568 of 610,440 (71.5 %) |
| positions | **1,761,827 over 28,508 units** |
| exact | 19,647, where both low bytes fell in the chain |
| the rest | within 256 map units of 15,000 across |

A wrong table does not produce wrong positions quietly — the entry
stream has to parse to exactly its own length, so it produces none at
all, which is why the parsed share is reported and is unit-tested.

Two honest notes. The 28.5 % of payloads that do not parse are ones
whose flags or bitmap bytes fall outside the 27 known entries, so a
fuller table would recover them too. And the class is named by its
shape, which is **patch-fragile**: on 16.9 that test picks netid 1151
and then fails its own checks, so `--positions-netid` exists to say
which class to read — and is what made the validation above possible.

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
  class to within a coordinate's high byte exactly and its low byte to
  a measured median of 4 map units. Damage numbers, item ids, which
  ability a cast was, and *exact* positions are not reachable today.
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
