# ROFL-X replay viewer

Single-file browser viewer for `rofl-x file` output. Drop a JSON in,
scrub through the match, watch players move and wards come and go.

The point of this viewer is visual sanity-checking: if champions walk
through walls, teleport around, or wards appear in impossible places,
the decoded output is wrong. It is not a production visualisation
tool.

## Usage

1. Produce a JSON output:
   ```
   rofl-x file -r <replay>.rofl -o out.json --patch-dir <dir>
   ```
2. Open `viewer/index.html` directly in a browser (no server needed).
3. Drop `out.json` onto the page, or use the file picker.
4. Click play, drag the scrubber, or use arrow keys.

## Controls

- Space: play / pause
- Arrow left / right: seek back / forward 5 s
- Speed selector: 0.5x up to 60x
- Scrubber: drag for direct seek

## What's rendered

- Players as team-coloured circles with a champion name label,
  interpolated between 1-second state snapshots (matched by
  champ + role + team, which uniquely identifies a player within one
  replay).
- Wards as smaller circles, coloured by ward type (yellow/green for
  SightWard/YellowTrinket, pink for JammerDevice, blue for anything
  else). Each ward is drawn only during its active window
  `[timestamp, timestamp + duration]`. Hover for details.
- Background is a stylised Summoner's Rift (corners for bases, a
  diagonal river band). We do not ship a real LoL map image.

## Coordinate assumptions

Summoner's Rift is treated as a 15,000 x 15,000 unit square, origin
at bottom-left, Y growing upward. Incoming JSON uses Mowokuma's
`{pos: [x, y]}` convention. If a future output schema switches to
Zhu-style `{x, z}`, the viewer will need a small adapter.

## Known limitations

- No minion / monster / turret / dragon rendering. Only the two things
  we decode today (players and wards).
- Interpolation assumes straight-line movement between 1-second
  snapshots. Real pathing is curved; long dashes across terrain are
  an interpolation artefact, not necessarily a decode bug.
- Player entity ids are not present in the JSON (only champion
  names), so a champion picked by both players in custom matches
  would confuse the match-by-name logic.

## What this is useful for

- **Sanity checking decoded output on 15.x**: does the early game
  look like 10 champions leaving their bases for lane? Do ward
  placements cluster around jungle entrances? If not, something is
  broken in the emulator path or the post-processing.
- **Regression detection**: after a parser change, loading the same
  replay should produce visually identical playback.
- **Catalog validation on 16.8**: once a 16.8 `.patch` archive exists,
  the first test is loading the 16.8 output here and eyeballing it.
