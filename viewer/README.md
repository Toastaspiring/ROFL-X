# Rift position check

A single page for looking at what ROFL-X reads out of a replay, drawn on
the Summoner's Rift minimap. It exists because a statistic cannot tell
you whether a champion walked down a lane or through a wall, and a
picture can — and because several of the mistakes in `docs/EXPLORE.md`
were caught by looking at this rather than by a number.

Open `index.html`. It needs no server and no replay: `viewer_data.json`
beside it holds a finished 16.9 game and a finished 16.19 one.

## What it draws

- **Champions**, named and teamed, walked along the path each movement
  packet asks for at the speed that packet states.
- **Minions, monsters and everything else** the movement class carries,
  as dots sized by what they are.
- **Structures, camps, drakes and Baron**, shown only while they are
  alive, tinted to the side that owns them.
- **Fog of war**, read from the visibility class rather than modelled:
  which units a side could see is in the file. How far a unit *sees* is
  not, so those radii are the game's documented ones and are marked in
  the page as a model.
- **Deaths**, exactly — the death class names its victim in plaintext,
  and its 74 deaths in the sample game split per champion exactly as the
  replay's own scoreboard does.
- **Recalls**, inferred from their shape: a silence that starts away from
  base, ends at the fountain, has no death to account for it, and covers
  more ground than the champion could have walked meanwhile.

Teleports are **not** drawn. The scoreboard says the sample game held
two; 323 jumps in it are unexplained by the packet reporting them, and
nothing separates those two from the ability dashes and table error in
the rest. Drawing a guess would be worse than drawing nothing.

## Art

The objective icons in `obj/` come from
[Code-De-La-League](https://github.com/Toastaspiring/Code-De-La-League).
The minimap and the champion portraits are fetched from Riot's Data
Dragon at run time and are not committed here; drop them in as
`map11.png` and `champ/<Name>.png` to work offline:

```bash
V=16.20.1
curl -o viewer/map11.png "https://ddragon.leagueoflegends.com/cdn/$V/img/map/map11.png"
```

If the minimap cannot be reached at all, the lanes are sketched from the
map's own geometry so the check still works.

## Rebuilding its data from a replay

`viewer_data.json` is produced by the `scripts/viewer_*.py` set. Each
script's docstring lists the files it reads; they run in the working
directory and expect those names. In outline:

```bash
# 1. positions, and the report that goes with them
rofl-x explore -r <replay>.rofl --table table1619.json \
    --positions-netid <movement netid> \
    --dump-positions pos1sr_t.jsonl -o e1sr.json

# 2. deaths, which need no table -- the class names its victim in plaintext
rofl-x explore -r <replay>.rofl --dump-netid <death netid> --dump deaths19.jsonl

# 3. the scoreboard, for summoner spells and death counts to check against
rofl-x summary -r <replay>.rofl > sum19.txt

# 4. vision from the things that never move, and the base's own footprint
python scripts/viewer_vision.py
python scripts/viewer_basepad.py

# 5. deaths and recalls
python scripts/viewer_events.py
python scripts/viewer_recalls.py

# 6. the page's data
python scripts/viewer_build.py table
```

## Checking it without looking

`scripts/sanity_check.py` asserts what the game itself guarantees —
minions arrive in waves, nothing outstrips its own stated speed, ten
units stand out as champions, the reconstruction is continuous at 10 ms
except where the game moves a champion — and prints what it measured
rather than a verdict, because a near miss is more informative than a
pass.

```bash
python scripts/sanity_check.py --positions pos1sr_t.jsonl --meta e1sr.json
```
