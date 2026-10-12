# Rift position check

A single page for looking at positions ROFL-X reads out of a replay,
drawn on the Summoner's Rift minimap. It exists because a statistic
cannot tell you whether a champion walked down a lane or through a
wall, and a picture can.

## Use it

```bash
rofl-x explore -r <replay>.rofl --dump-positions positions.jsonl -o /dev/null
```

Open `index.html` and drop `positions.jsonl` on it. No server needed;
the minimap comes from Riot's Data Dragon, and if that cannot be
reached the lanes are sketched from the map's own geometry instead.

## What it shows

- Every position at once (**All positions**), which is the view that
  answers the question: correct positions trace the lanes and the
  jungle paths, wrong ones smear or pile up.
- One moment at a time (**Playback**), with a trail behind each unit
  and, where the low byte of a coordinate is unknown, a ring at the
  stated error radius so the uncertainty is visible rather than
  implied.

## What it infers, and what it does not

The movement class names units by net id, and that id is behind the
substitution, so the page cannot say which champion is which. What it
can do is pick the ten units whose net id is small enough to be a
champion and that walk the whole game, and give each a team from
which fountain it visits. Both are inferences; neither is read from
the file.

That inference is also the page's own check. A real game has five
champions a side, so a page that reports anything other than five and
five is reporting that the positions are wrong — which is exactly what
it reports for patch 16.19 today.
