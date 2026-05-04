#!/usr/bin/env python
"""Translate rofl-x output JSON into a typed event timeline.

Goal: take the per-decoder samples from `extra_decoders[]` + the
typed `raw_positions` (mov) + ward packets, and emit a flat
chronological list of typed events. Each event has a timestamp,
a type, the entity it relates to (when known), and the relevant
decoded fields.

Output JSON layout:

  {
    "metadata":  { game length, players, ... },
    "entities":  { "<net_id_hex>": {"name", "role", "team", "champion", ...} },
    "stats":     { event counts per type, total events, etc. },
    "events":    [
      { "t": 12.345, "type": "Movement",
        "entity_id": "0x40000099",
        "final_pos": [5000.0, 6000.0],
        ... },
      { "t": 12.4, "type": "Replication",
        "decoder": "0xf6ab10",
        "riot_class": "PKT_S2C_ReplicateField_s",
        "fields": { "property_value_a": 2.0, ... }
      },
      ...
    ]
  }

Usage:
  python scripts/build_event_timeline.py <rofl-x output.json> [--out events.json]
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from collections import defaultdict
from pathlib import Path


SEMANTIC_PATH = Path(__file__).resolve().parent / "semantic_field_names.json"


def fmt_t(t):
    """ms -> 'mm:ss.s' for human reading."""
    if t is None:
        return "?"
    s = float(t)
    return f"{int(s // 60):02d}:{s % 60:05.2f}"


def classify(decoder_rva, riot_primary):
    """Map decoder RVA + riot class to a top-level event type."""
    if "Replicate" in (riot_primary or "") or "Replication" in (riot_primary or ""):
        return "Replication"
    if "Movement" in (riot_primary or "") or "FollowTarget" in (riot_primary or "") or "TurnData" in (riot_primary or ""):
        return "Movement"
    if "Visibility" in (riot_primary or "") or "Fog" in (riot_primary or ""):
        return "Visibility"
    if "Cooldown" in (riot_primary or ""):
        return "Cooldown"
    if "Die" in (riot_primary or "") or "Reincarnate" in (riot_primary or ""):
        return "Death"
    if "BasicAttack" in (riot_primary or "") or "Basic_Attack" in (riot_primary or ""):
        return "BasicAttack"
    if "CastSpell" in (riot_primary or ""):
        return "SpellCast"
    if "Buff" in (riot_primary or ""):
        return "Buff"
    if "Item" in (riot_primary or ""):
        return "Item"
    if "Damage" in (riot_primary or ""):
        return "Damage"
    if "Sound" in (riot_primary or "") or "Voice" in (riot_primary or ""):
        return "Audio"
    if "Camera" in (riot_primary or ""):
        return "Camera"
    if "Animation" in (riot_primary or "") or "Anim" in (riot_primary or ""):
        return "Animation"
    if "Health" in (riot_primary or ""):
        return "HealthBar"
    if "Constructor" in (riot_primary or "") or "tag-only" in (riot_primary or ""):
        return "TagOnly"
    return "Unknown"


def build_entity_registry(metadata):
    """Player roster from replay metadata. Player net_ids are
    `player_id_start + slot_index` (slot 0..9). Riot uses
    0x40000099 + N for the ten human player slots, in metadata
    order (Blue then Red, by role).
    """
    entities = {}
    pid_start = 0x40000099
    players = metadata.get("players", []) if isinstance(metadata, dict) else []
    if isinstance(players, str):
        try:
            players = json.loads(players)
        except Exception:
            players = []
    for i, p in enumerate(players[:10]):
        entity_id = pid_start + i
        entities[f"0x{entity_id:x}"] = {
            "kind": "Hero",
            "team": p.get("team") or p.get("TEAM", "?"),
            "champion": p.get("skin") or p.get("SKIN") or p.get("champion") or "?",
            "role": p.get("position") or p.get("POSITION", "?"),
            "name": p.get("name") or p.get("NAME") or "(masked)",
        }
    return entities


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("input")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    with open(args.input) as f:
        rofl = json.load(f)
    with open(SEMANTIC_PATH) as f:
        catalog = json.load(f)
    netid_to_decoder = catalog.get("netid_to_decoder", {})
    decoders = catalog.get("decoders", {})

    metadata = rofl.get("metadata", {})
    entities = build_entity_registry(metadata)

    events = []

    # 1) Movement events from raw_positions[]
    for rp in rofl.get("raw_positions", []):
        ts = rp.get("timestamp")
        x = rp.get("x")
        y = rp.get("y")
        events.append({
            "t": ts,
            "type": "Movement",
            "decoder": "0xfb4070",
            "riot_class": "PKT_DirectInputMovementDriverServerTurnData_s",
            "final_pos": [x, y] if (x is not None and y is not None) else None,
            "kind_detail": "mov_decrypt_inline_floats",
        })

    # 2) Ward events
    for w in rofl.get("wards", []):
        events.append({
            "t": w.get("timestamp"),
            "type": "Ward",
            "ward_id": w.get("id"),
            "owner_id": w.get("owner_id"),
            "name": w.get("name"),
            "pos": [w.get("x"), w.get("y")],
        })

    # 3) Per-netid extra_decoders[] -> classified events
    for label, samples in (rofl.get("extra_decoders") or {}).items():
        if "_netid" not in label:
            continue
        try:
            netid = int(label.rsplit("_netid", 1)[1])
        except ValueError:
            continue
        decoder_rva = netid_to_decoder.get(str(netid))
        decoder_info = decoders.get(decoder_rva, {})
        riot_primary = decoder_info.get("riot_class_primary", "")
        riot_candidates = decoder_info.get("riot_class_candidates", [])
        confidence = decoder_info.get("riot_class_confidence", "")
        ev_type = classify(decoder_rva, riot_primary)
        fields_map = decoder_info.get("fields", {})
        for s in samples:
            ts = s.get("timestamp")
            decoded = s.get("decoded_fields", []) or []
            named_fields = {}
            entity_id = None
            for f in decoded:
                off = f.get("offset")
                off_hex = f"0x{off:04x}" if isinstance(off, int) else off
                fdef = fields_map.get(off_hex, {})
                fname = fdef.get("name") or f"field_at_{off_hex}"
                ftype = fdef.get("type", "")
                # Pick the right interpretation
                if ftype == "f32":
                    val = f.get("f32_le")
                elif ftype in ("u32", "u16ish", "u8"):
                    val = f.get("u32_le")
                elif ftype == "i32":
                    val = f.get("i32_le")
                elif ftype == "u64":
                    val = f.get("u32_le")
                else:
                    # u32_or_f32 — keep both
                    val = {
                        "u32": f.get("u32_le"),
                        "f32": f.get("f32_le"),
                    }
                named_fields[fname] = val
                if entity_id is None and "entity" in fname.lower() and isinstance(val, int):
                    entity_id = f"0x{val:x}"
            ev = {
                "t": ts,
                "type": ev_type,
                "decoder": decoder_rva,
                "netid": netid,
                "riot_class": riot_primary or "(unidentified)",
                "riot_confidence": confidence,
                "fields": named_fields,
            }
            if riot_candidates and len(riot_candidates) > 1:
                ev["riot_candidates"] = riot_candidates
            if entity_id:
                ev["entity_id"] = entity_id
            events.append(ev)

    # Sort by timestamp
    events.sort(key=lambda e: (e.get("t") or 0.0))

    # Stats
    type_counts = defaultdict(int)
    for e in events:
        type_counts[e["type"]] += 1

    out = {
        "_comment": (
            "Typed event timeline derived from rofl-x output + the "
            "semantic catalog. Each event is one decoded packet "
            "classified by the leaked Riot vocabulary (shape-anchored, "
            "not RTTI-proven). Use 'riot_class' for the best class "
            "guess, 'riot_candidates' for the ranked alternatives, "
            "'riot_confidence' for the heuristic strength."
        ),
        "metadata": {
            "game_length_ms": metadata.get("game_len") or metadata.get("gameLength"),
            "patch": metadata.get("version") or metadata.get("gameVersion"),
            "winner": metadata.get("winning_team") or "?",
        },
        "entities": entities,
        "stats": {
            "total_events": len(events),
            "by_type": dict(sorted(type_counts.items(), key=lambda x: -x[1])),
        },
        "events": events,
    }

    out_path = Path(args.out) if args.out else Path(args.input).with_suffix(".timeline.json")
    with open(out_path, "w") as f:
        json.dump(out, f, indent=2)
    print(f"wrote {out_path}")
    print(f"  total events: {len(events)}")
    print(f"  by type: {dict(sorted(type_counts.items(), key=lambda x: -x[1]))}")
    print(f"  entities: {len(entities)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
