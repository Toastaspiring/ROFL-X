# Exploring a replay whose client is packed

From some patch after 16.9, `League of Legends.exe` ships with its code
encrypted on disk ([COMPATIBILITY.md](COMPATIBILITY.md)). Without the
game's code there is no deserializer, so no packet payload on those
patches can be decoded, and ROFL-X will not go after the code
(README § Non-goals).

That is not the same as learning nothing. A block's `timestamp`,
`netid`, `param` and length are plaintext even when its payload is not,
and the file also carries Riot's own end-of-game record. `rofl-x
explore` is what those two facts are worth, and this page is the run on
the twenty patch-16.19 replays we hold.

```
rofl-x explore -r <replay> [-r <replay> ...] \
  [--reference <16.9 replay> ...] [--self-test] [-o out.json]
```

Everything below was produced by that command. No game binary is read
and no payload is decoded.

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

On 16.9, where every answer is already known:

| netid | counter | class it turned out to be | replays |
|------:|---------|---------------------------|--------:|
| 472 | `NUM_DEATHS` | `PKT_NPC_Hero_Die_s` | 5 / 5 |
| 715 | `NUM_DEATHS` | `PKT_S2C_UpdateDeathTimer_s` | 5 / 5 |
| 64 | `LEVEL` − 1 | `PKT_NPC_LevelUp_s` | 3 / 4 |

Three proposals, three correct. On 16.19, with no binary at all:

| netid | counter | replays | what else is known about it |
|------:|---------|--------:|------------------------------|
| 1080 | `NUM_DEATHS` | 19 / 19 | payload 16–37 bytes, only ever names the ten champions |
| 601 | `NUM_DEATHS` | 3 / 3 on a 6-replay run, not on all 20 | payload exactly 5 bytes, champions only, 1,962 blocks against 1,080's 1,955 |

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

## 3. Netids proposed by fingerprint

Riot renumbers netids every patch but rarely rewrites a class, so what
a class looks like from outside its payload mostly survives: how often
it fires, what its payload lengths are, whether it addresses a unit or
the game, how many entities it ever names, how its traffic spreads over
the game. Matching those against a patch we *can* decode proposes a
name for each netid.

**Payload length does most of the work.** The set of lengths a class
emits is the shape of its fields: 2 bytes on every block is one small
field; 5/6/7/10/11 is a fixed head with a short optional tail.

How good is it? Split the 16.9 replays in two, fingerprint each half
from different games, and match — the answer is known, so the error
rate is measurable:

| gate (score, margin) | calls | right | precision |
|----------------------|------:|------:|----------:|
| any | 176 | 134 | 76.1 % |
| ≥ 0.70, ≥ 0.05 | 132 | 114 | 86.4 % |
| ≥ 0.85, ≥ 0.10 | 90 | 85 | 94.4 % |
| **≥ 0.85, ≥ 0.15** | **72** | **69** | **95.8 %** |
| ≥ 0.95, ≥ 0.15 | 42 | 40 | 95.2 % |

The margin over the runner-up is what carries the signal; the score
barely moves precision. The shipped gate is the fourth row, and only
matches that clear it are reported.

Against 16.9, the twenty 16.19 replays produce **66 confident matches
out of 273 netids, 41 of them to a named class.** The ones worth
believing, because something independent agrees:

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

**These are hypotheses.** The 95.8 % figure is a ceiling, not an
expectation: the self-test renumbers nothing and changes no class, it
only asks whether a fingerprint taken from two games finds the same
class in three others. A real ten-patch gap also moves the classes
themselves. About one in twenty of the confident calls is wrong even
under ideal conditions, and nobody knows which one.

Where § 2 and § 3 agree — netid 601 — the answer is not a hypothesis
any more.

## What this does not do

- **No payload is decoded.** Positions, damage numbers, item ids, spell
  casts: none of it is reachable on a packed patch, and none of it is
  claimed here.
- **No class is named with certainty by fingerprint alone.** Only § 2
  produces proof, and only for classes that happen to correspond to a
  counter Riot kept.
- **Nothing touches the game.** No binary is unpacked, no process is
  read. That is the constraint this whole page is written under.

## What would make this much stronger

More patches with both a decodable binary *and* replays. The self-test
measures fingerprint stability within one patch; with two decodable
patches the real cross-patch error rate could be measured instead of
bounded, and the weights fitted to it rather than set by hand.
