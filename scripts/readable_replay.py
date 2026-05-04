#!/usr/bin/env python
"""Stage 3: turn the full event timeline into a readable replay.

Two outputs from one run:
  - <out>.json      compact filtered timeline (deduped, interesting-only)
  - <out>.txt       human-readable play-by-play, one line per event

Filtering strategy:
  - Drop events whose decoded_fields are all sentinel defaults
    (0 / -1 / NaN / inline tag-byte constants). Riot's decoders
    fall through to inline constants when the variable-length tag
    byte selects a 'no payload' branch — those samples carry no
    information, just confirm the netid fired.
  - Dedupe by (netid, fields-tuple, source-entity): if 50 samples
    in a row have identical field values, keep one + emit a
    `repeat_count`.
  - Resolve known entity_ids to player names via the registry.
  - Snap timestamps to mm:ss for the text output.

Usage:
  python scripts/readable_replay.py <timeline.json> [--out replay]
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path


PLAYER_PID_START = 0x40000099


def fmt_t(ms_or_s):
    """Pretty mm:ss.s — input is float seconds."""
    s = float(ms_or_s) if ms_or_s is not None else 0.0
    return f"{int(s // 60):02d}:{s % 60:05.2f}"


def is_default_value(v):
    """True when a field value is one of Riot's inline-tag defaults
    OR looks like denormal-float garbage (the u32 reinterpreted as f32
    of a small int gives ~1e-43 which is meaningless)."""
    if v is None:
        return True
    if isinstance(v, dict):
        u = v.get("u32")
        f = v.get("f32")
        # If the u32 is small (< 1000) and the f32 reinterpretation is
        # denormal-tiny, this is almost certainly a u32 value being
        # mis-read as float — treat as default.
        if isinstance(u, int) and 0 <= u < 1000 and isinstance(f, (int, float)) and abs(f) < 1e-30:
            return is_default_value(u)
        return is_default_value(u) and is_default_value(f)
    if isinstance(v, (int, float)):
        # Common defaults: 0, -1 (= 0xFFFFFFFF), inline f32 constants
        if v == 0:
            return True
        if v in (-1, 0xFFFFFFFF, 4294967295):
            return True
        if v in (1.0, -1.0, 2.0, -2.0):
            return True
        # Tag-byte-style small enums (0..7)
        if isinstance(v, int) and 0 < v < 8:
            return True
        # Denormal-tiny floats reinterpreted from small u32s
        if isinstance(v, float) and 0 < abs(v) < 1e-30:
            return True
        # Player_id_start sentinels (0x40000099 + 0..9)
        if isinstance(v, int) and 0x40000099 <= v <= 0x400000a2:
            return False  # actually a player ID — keep
        return False
    return False


def fields_signature(fields):
    """Tuple key for dedup. Coalesces u32_or_f32 -> tuple (u32, f32)."""
    items = []
    for k, v in sorted(fields.items()):
        if isinstance(v, dict):
            items.append((k, "uf", v.get("u32"), v.get("f32")))
        else:
            items.append((k, "v", v))
    return tuple(items)


def fields_signal_score(fields):
    """How 'interesting' is this event? 0 = all defaults, higher = more
    fields with non-default values."""
    score = 0
    for v in fields.values():
        if not is_default_value(v):
            score += 1
    return score


def event_to_text(e, entities):
    """One-line text rendering of an event."""
    t = fmt_t(e.get("t", 0))
    typ = e.get("type", "?")
    cls = e.get("riot_class", "")
    ent_id = e.get("entity_id")
    ent_label = ""
    if ent_id and ent_id in entities:
        ent_info = entities[ent_id]
        ent_label = f" {ent_info.get('team','?')} {ent_info.get('role','?')} {ent_info.get('champion','?')}"

    # Type-specific compact rendering
    fields = e.get("fields") or {}
    repeat = e.get("repeat_count", 1)
    rep_marker = f" (x{repeat})" if repeat > 1 else ""

    if typ == "Movement":
        pos = e.get("final_pos")
        if pos and pos[0] is not None:
            return f"[{t}] Movement{ent_label} -> ({pos[0]:.0f}, {pos[1]:.0f}){rep_marker}"
        return f"[{t}] Movement{ent_label}{rep_marker}"

    if typ == "Cooldown":
        slot = fields.get("spell_slot")
        cd = fields.get("cooldown_total_ms")
        target = fields.get("target_net_id")
        target_str = f" target=0x{target:x}" if isinstance(target, int) and target > 0 else ""
        cd_str = f" {cd:.0f}ms" if isinstance(cd, (int, float)) and cd else ""
        return f"[{t}] Cooldown slot={slot}{cd_str}{target_str}{rep_marker}"

    if typ == "Visibility":
        eid = fields.get("entity_net_id")
        flag = fields.get("visibility_flag_or_subtype")
        return f"[{t}] Visibility entity=0x{eid:x} flag={flag}{rep_marker}" if isinstance(eid, int) else f"[{t}] Visibility{rep_marker}"

    if typ == "Death":
        return f"[{t}] Death {cls.replace('PKT_NPC_', '').replace('_s', '')}{ent_label}{rep_marker}"

    if typ == "BasicAttack":
        src = fields.get("caster_or_source_id") or fields.get("source_or_caster_id")
        tgt = fields.get("target_id")
        src_str = f"0x{src:x}" if isinstance(src, int) and src > 0 else "?"
        tgt_str = f"0x{tgt:x}" if isinstance(tgt, int) and tgt > 0 else "?"
        return f"[{t}] BasicAttack src={src_str} tgt={tgt_str}{rep_marker}"

    if typ == "Replication":
        idx = fields.get("property_index_or_class_id")
        val_a = fields.get("property_value_a")
        idx_str = f" prop={idx}" if idx is not None else ""
        return f"[{t}] Replicate{idx_str} val={val_a}{rep_marker}"

    # Generic
    interesting = {k: v for k, v in fields.items() if not is_default_value(v)}
    interesting_str = " ".join(f"{k}={v}" for k, v in list(interesting.items())[:3])
    return f"[{t}] {typ} {cls[:30]}{ent_label} {interesting_str}{rep_marker}"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("input")
    ap.add_argument("--out", default="replay")
    ap.add_argument("--keep-all", action="store_true",
                    help="Don't drop default-only events")
    ap.add_argument("--max-text-lines", type=int, default=5000,
                    help="Truncate text output at N lines (0 = unlimited)")
    args = ap.parse_args()

    with open(args.input) as f:
        timeline = json.load(f)
    events = timeline.get("events", [])
    entities = timeline.get("entities", {})
    metadata = timeline.get("metadata", {})

    # Filter: drop events with all-default fields (unless --keep-all)
    if not args.keep_all:
        before = len(events)
        events = [e for e in events
                  if e.get("type") in ("Movement",)
                     or fields_signal_score(e.get("fields") or {}) > 0]
        print(f"filtered: kept {len(events)} of {before} ({len(events)/before*100:.1f}%)")

    # Dedup: collapse adjacent events with same (type, decoder, fields)
    deduped = []
    last_key = None
    last_event = None
    last_count = 0
    for e in events:
        sig = (e.get("type"), e.get("decoder"), e.get("netid"),
               fields_signature(e.get("fields") or {}))
        if sig == last_key:
            last_count += 1
            last_event["t_end"] = e.get("t")
        else:
            if last_event is not None:
                last_event["repeat_count"] = last_count
                deduped.append(last_event)
            last_event = dict(e)
            last_key = sig
            last_count = 1
    if last_event is not None:
        last_event["repeat_count"] = last_count
        deduped.append(last_event)
    print(f"deduped: {len(events)} -> {len(deduped)} (collapsed {len(events) - len(deduped)} repeats)")

    # Compute by-type stats
    type_counts = defaultdict(int)
    for e in deduped:
        type_counts[e.get("type", "?")] += 1

    # Write JSON
    out_path = Path(args.out + ".json")
    with open(out_path, "w") as f:
        json.dump({
            "metadata": metadata,
            "entities": entities,
            "stats": {
                "total_unique_events": len(deduped),
                "by_type": dict(sorted(type_counts.items(), key=lambda x: -x[1])),
            },
            "events": deduped,
        }, f, indent=2)
    print(f"wrote {out_path}")

    # Write human-readable text
    txt_path = Path(args.out + ".txt")
    with open(txt_path, "w", encoding="utf-8") as f:
        # Header
        gl = metadata.get("game_length_ms", 0) / 1000 if metadata.get("game_length_ms") else 0
        f.write(f"=== Replay summary ===\n")
        f.write(f"Patch: {metadata.get('patch', '?')}\n")
        f.write(f"Length: {fmt_t(gl)} ({gl:.0f} seconds)\n")
        f.write(f"Winner: {metadata.get('winner', '?')}\n")
        f.write(f"\n--- Entities ({len(entities)} known) ---\n")
        for eid, info in entities.items():
            f.write(f"  {eid}  {info.get('team','?'):5s} {info.get('role','?'):8s} {info.get('champion','?')}\n")
        f.write(f"\n--- Event stats ---\n")
        f.write(f"  total unique events: {len(deduped):,}\n")
        for typ, cnt in sorted(type_counts.items(), key=lambda x: -x[1]):
            f.write(f"  {typ:14s} {cnt:>10,}\n")
        # Per-minute event distribution
        f.write(f"\n--- Per-minute event distribution ---\n")
        per_min_total = defaultdict(int)
        per_min_by_type = defaultdict(lambda: defaultdict(int))
        for e in deduped:
            m = int((e.get("t") or 0) // 60)
            per_min_total[m] += e.get("repeat_count", 1)
            per_min_by_type[m][e.get("type", "?")] += e.get("repeat_count", 1)
        f.write(f"  {'min':>4}  {'total':>7}  major types\n")
        for m in sorted(per_min_total.keys()):
            top = sorted(per_min_by_type[m].items(), key=lambda x: -x[1])[:4]
            top_str = ", ".join(f"{t}={n}" for t, n in top)
            f.write(f"  {m:>4}  {per_min_total[m]:>7}  {top_str}\n")

        # Movement summary per player
        f.write(f"\n--- Movement summary ---\n")
        # Note: Movement events from raw_positions[] don't have entity_id
        # attached (the typed mov_decrypt didn't decode it). Group by
        # rounded position to estimate distinct waypoints per minute.
        mov_per_min = defaultdict(int)
        for e in deduped:
            if e.get("type") == "Movement":
                m = int((e.get("t") or 0) // 60)
                mov_per_min[m] += e.get("repeat_count", 1)
        f.write(f"  total mov packets: {sum(mov_per_min.values()):,}\n")
        f.write(f"  by minute (top 10): {dict(sorted(mov_per_min.items(), key=lambda x: -x[1])[:10])}\n")

        # Notable events: keep only those with high signal (>= 2 non-default fields)
        f.write(f"\n--- Notable events (signal-score >= 2) ---\n")
        notable = [e for e in deduped
                   if e.get("type") not in ("Movement",)
                   and fields_signal_score(e.get("fields") or {}) >= 2]
        f.write(f"  {len(notable):,} notable events; showing up to {args.max_text_lines}:\n")
        line_limit = args.max_text_lines if args.max_text_lines > 0 else len(notable)
        for i, e in enumerate(notable):
            if i >= line_limit:
                f.write(f"... ({len(notable) - i:,} more events truncated; --max-text-lines 0 to see all)\n")
                break
            f.write(event_to_text(e, entities) + "\n")
    print(f"wrote {txt_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
