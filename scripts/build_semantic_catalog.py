#!/usr/bin/env python
"""Build semantic_field_names.json from the e2e output + decompiled C.

Strategy:
  1. Group netids by decoder RVA (from patch/16-9.patch).
  2. For each decoder, look at the decoded_fields[] in the output JSON
     and infer a "shape" (offsets and dominant types).
  3. Classify each decoder as:
       - tag_only (writes <= 1 small field, no semantic content)
       - constructor_fallback (brute-force matched a class ctor, not
         a real decoder — fields are init constants, not packet data)
       - mov_decrypt (waypoint group; offset shape matches 0x10/0x18 +
         0x20/0x24 inline floats)
       - small_event (1-2 fields, ID-only events: EnterFog, LeaveFog,
         HeroDie)
       - cooldown_like (5 fields including slot + duration float)
       - replication (variable property dict; shared decoder across
         many netids; mixed int/float types per offset)
       - position_packet (many float fields, looks like
         BasicAttackPos / CastSpellAns / SpawnMinion)
       - generic (real decoder, type pattern fits multiple Zhu
         classes — name fields with type hints)
  4. Emit field names per offset, scoped to the decoder.

Output: scripts/semantic_field_names.json (overwritten).
"""

from __future__ import annotations

import json
import os
import re
import sys
import zipfile
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PATCH_PATH = ROOT / "patch" / "16-9.patch"
DECOMP = Path(os.environ["USERPROFILE"]) / "Tools" / "analysis" / "16-9" / "decomp"
OUTPUT_JSON = Path(os.environ["USERPROFILE"]) / "AppData" / "Local" / "Temp" / "rofl_out" / "full_decoded_216.json"
SEMANTIC_PATH = ROOT / "scripts" / "semantic_field_names.json"


# -- Hand-curated overrides for decoders we've identified with high confidence ---

# Each entry maps a decoder RVA -> {class, fields: {offset: {name, type}}}
HIGH_CONF: dict[str, dict] = {
    # mov_decrypt: WaypointGroup. fb4070 also matches netids 52/173/390 in
    # the extras (different packet classes that happen to use the same
    # decoder body). For 916 specifically, the typed `mov_decrypt` path
    # takes over and these offsets are interpreted as inline-floats
    # (final_x, final_y) plus the waypoint vector at 0x10/0x18.
    "0xfb4070": {
        "class": "WaypointGroup_mov",
        "comment": "Position/movement decoder. 916=mov (typed); 52,173,390 are sibling packets reusing the body.",
        "fields": {
            "0x0010": {"name": "waypoint_vector_ptr", "type": "u64"},
            "0x0018": {"name": "waypoint_vector_size", "type": "u32"},
            "0x0020": {"name": "final_position_x_or_tag", "type": "f32_or_i32"},
            "0x0024": {"name": "final_position_y_or_tag", "type": "f32_or_i32"},
        },
    },
    # faffc0: 2-field ID-only events. 707 is the highest freq match
    # (5.5%) — fits LeaveFog or EnterFog. 97 and 1055 are similar.
    "0xfaffc0": {
        "class": "FogEvent_or_smallID",
        "comment": "Single-entity event (EnterFog/LeaveFog/HeroDie shape).",
        "fields": {
            "0x0010": {"name": "entity_net_id", "type": "u32"},
            "0x0014": {"name": "event_subtype_or_flag", "type": "u32"},
        },
    },
    # fdb200: 5-field, matches DoSetCooldown shape (net_id, slot, cooldown,
    # display_cooldown). 684 was hypothesised as DoSetCooldown.
    "0xfdb200": {
        "class": "DoSetCooldown",
        "comment": "Spell cooldown update. 5-field shape matches Zhu's DoSetCooldown(net_id, slot, cooldown, display_cooldown) plus an extra status flag.",
        "fields": {
            "0x000c": {"name": "spell_slot", "type": "u8"},
            "0x0010": {"name": "cooldown_remaining_ms", "type": "f32"},
            "0x0018": {"name": "cooldown_total_ms", "type": "f32"},
            "0x001c": {"name": "target_net_id", "type": "u32"},
            "0x0024": {"name": "display_cooldown_ms", "type": "f32"},
        },
    },
    # f9d4e0: 9 offsets including byte fields at 0x2c, 0x38. Complex
    # multi-tag dispatch. 1068 = highest freq packet (46.5%). Interpreting
    # as a state-update packet with multiple property fields per call.
    "0xf9d4e0": {
        "class": "UpdateState_complex",
        "comment": "1068 (heartbeat/UpdateState, 46.5% blocks) plus 11 sibling netids. Complex multi-tag dispatch with mixed scalar fields.",
        "fields": {
            "0x0010": {"name": "primary_value_or_enum", "type": "u32"},
            "0x0014": {"name": "secondary_id_lo", "type": "u32"},
            "0x0018": {"name": "secondary_id_hi", "type": "u32"},
            "0x001c": {"name": "scalar_1", "type": "f32"},
            "0x0020": {"name": "scalar_2", "type": "f32"},
            "0x0024": {"name": "scalar_3", "type": "u32"},
            "0x0028": {"name": "scalar_4", "type": "u32"},
            "0x002c": {"name": "flag_or_byte", "type": "u8"},
            "0x0030": {"name": "scalar_5_lo", "type": "u32"},
            "0x0034": {"name": "scalar_5_hi", "type": "u32"},
            "0x0038": {"name": "trailing_flag", "type": "u8"},
        },
    },
    # f6ab10: 51 netids. Replication-base shape (variable property dict).
    "0xf6ab10": {
        "class": "Replication",
        "comment": "Generic Replication packet — variable per-property updates. Handles 51 entity-class-specific netids that share this decoder. Per-offset semantics depend on which property the decoder selected for this sample.",
        "fields": {
            "0x0008": {"name": "property_index_or_class_id", "type": "u8"},
            "0x0010": {"name": "property_value_a", "type": "u32_or_f32"},
            "0x0014": {"name": "property_value_b", "type": "u32_or_f32"},
            "0x001c": {"name": "property_value_c", "type": "u32_or_f32"},
            "0x0024": {"name": "property_value_d", "type": "u32_or_f32"},
            "0x0028": {"name": "property_value_e", "type": "u32_or_f32"},
            "0x002c": {"name": "property_value_f", "type": "u32_or_f32"},
            "0x0030": {"name": "property_value_g", "type": "u32_or_f32"},
            "0x0038": {"name": "property_value_h", "type": "u32_or_f32"},
            "0x003c": {"name": "property_value_i", "type": "u32_or_f32"},
        },
    },
    # 1074580: 14 offsets, lots of floats — BasicAttackPos / CastSpellAns shape
    "0x1074580": {
        "class": "AttackOrSpellCast",
        "comment": "Position-heavy packet, 14 offsets with multiple f32 groups. Shape consistent with Zhu's BasicAttackPos or CastSpellAns (source_position, target_position, target_end_position = 6 floats + multiple IDs and scalars).",
        "fields": {
            "0x0018": {"name": "caster_or_source_id", "type": "u32"},
            "0x0020": {"name": "source_position_x", "type": "f32"},
            "0x0024": {"name": "source_position_y", "type": "f32"},
            "0x0030": {"name": "target_id", "type": "u32"},
            "0x0038": {"name": "target_position_x", "type": "f32"},
            "0x003c": {"name": "target_position_y", "type": "f32"},
            "0x0040": {"name": "target_end_position_x", "type": "f32"},
            "0x0044": {"name": "target_end_position_y", "type": "f32"},
            "0x0048": {"name": "spell_or_attack_param_a", "type": "f32"},
            "0x0050": {"name": "spell_hash_lo", "type": "u32"},
            "0x0054": {"name": "spell_hash_hi", "type": "u32"},
            "0x0058": {"name": "windup_time_ms", "type": "f32"},
            "0x005c": {"name": "cooldown_ms", "type": "f32"},
            "0x0060": {"name": "mana_cost", "type": "f32"},
            "0x0064": {"name": "level_or_slot", "type": "u32"},
        },
    },
    # eba9e0: 5 netids, 12 offsets. Known shared-loud (was filtered initially).
    "0xeba9e0": {
        "class": "Replication_long",
        "comment": "Replication-style decoder with extended property dict. Wins many netids by writing to many offsets.",
        "fields": {
            "0x0010": {"name": "property_value_a", "type": "u32_or_f32"},
            "0x0014": {"name": "property_value_b", "type": "u32"},
            "0x0024": {"name": "property_value_c", "type": "u32_or_f32"},
            "0x0028": {"name": "property_value_d", "type": "u32"},
            "0x0030": {"name": "property_value_e", "type": "u32"},
        },
    },
    # f9bae0: BasicAttack-like, 11 offsets
    "0xf9bae0": {
        "class": "BasicAttack_or_Spawn",
        "comment": "Multi-position packet (could be BasicAttackPos, SpawnMinion, or BarrackSpawnUnit).",
        "fields": {
            "0x0010": {"name": "source_or_caster_id", "type": "u32"},
            "0x0018": {"name": "position_a_x", "type": "f32"},
            "0x001c": {"name": "position_a_y", "type": "f32"},
            "0x0020": {"name": "position_b_x", "type": "f32"},
            "0x0024": {"name": "position_b_y", "type": "f32"},
            "0x002c": {"name": "scalar_a", "type": "u32_or_f32"},
            "0x0030": {"name": "scalar_b", "type": "u32_or_f32"},
            "0x0034": {"name": "scalar_c", "type": "u32_or_f32"},
            "0x0040": {"name": "trailing_id_or_flag", "type": "u32"},
            "0x0048": {"name": "trailing_value", "type": "u32"},
            "0x004c": {"name": "trailing_value_b", "type": "u32"},
        },
    },
    # fcd970: medium decoder, 8-9 offsets
    "0xfcd970": {
        "class": "ItemOrStatus_change",
        "comment": "Mid-sized event packet. Could be BuyItem, SwapItem, or status-effect application.",
        "fields": {
            "0x0018": {"name": "entity_net_id", "type": "u32"},
            "0x001c": {"name": "field_b", "type": "u32_or_f32"},
            "0x0020": {"name": "field_c", "type": "u32_or_f32"},
            "0x002c": {"name": "field_d", "type": "u32_or_f32"},
            "0x0034": {"name": "field_e", "type": "u32_or_f32"},
            "0x0038": {"name": "field_f", "type": "f32"},
            "0x003c": {"name": "field_g", "type": "u32_or_f32"},
            "0x0040": {"name": "field_h", "type": "f32"},
        },
    },
    # 10843b0: 3 offsets, small packet
    "0x10843b0": {
        "class": "SmallEvent",
        "comment": "Small 3-field event packet (could be SwapItem, NPCDieMapView, etc.).",
        "fields": {
            "0x0014": {"name": "entity_id_or_value", "type": "u32"},
            "0x0018": {"name": "secondary_id", "type": "u32"},
            "0x001c": {"name": "tertiary_value", "type": "u32_or_f32"},
        },
    },
    # fcfd30: 7 offsets
    "0xfcfd30": {
        "class": "Replication_compact",
        "comment": "Compact replication-style packet, 7 fields including position pair.",
        "fields": {
            "0x0010": {"name": "entity_net_id", "type": "u32"},
            "0x0014": {"name": "field_b", "type": "u32_or_f32"},
            "0x0018": {"name": "scalar_a", "type": "u32"},
            "0x001c": {"name": "position_x_or_value", "type": "f32"},
            "0x0020": {"name": "position_y_or_value", "type": "f32"},
            "0x0024": {"name": "extra_value", "type": "f32"},
            "0x0028": {"name": "trailing_value", "type": "u32"},
        },
    },
    # fd6270: 2-field tiny packet
    "0xfd6270": {
        "class": "TinyEvent",
        "comment": "2-field tiny packet (single value at 0x14 and 0x20).",
        "fields": {
            "0x0014": {"name": "primary_value", "type": "u32_or_f32"},
            "0x0020": {"name": "secondary_value", "type": "u32"},
        },
    },
    # 107c470: medium 7-field
    "0x107c470": {
        "class": "MediumEvent",
        "comment": "Mid-sized 7-field event packet.",
        "fields": {
            "0x0014": {"name": "primary_id_or_value", "type": "u32"},
            "0x0018": {"name": "scalar_a", "type": "u32_or_f32"},
            "0x001c": {"name": "scalar_b", "type": "u32_or_f32"},
            "0x0028": {"name": "field_c", "type": "u32"},
            "0x0038": {"name": "field_d", "type": "u32"},
            "0x0040": {"name": "field_e", "type": "f32"},
            "0x0044": {"name": "field_f", "type": "u32_or_f32"},
        },
    },
    # 10371f0: 4-field
    "0x10371f0": {
        "class": "PositionEvent",
        "comment": "Small position-style event (2 floats + scalar).",
        "fields": {
            "0x0008": {"name": "position_x", "type": "f32"},
            "0x000c": {"name": "position_y", "type": "f32"},
            "0x0018": {"name": "scalar_or_id", "type": "u32"},
            "0x0024": {"name": "trailing_value", "type": "u32_or_f32"},
        },
    },
    # fc2fe0: 3-netid sibling
    "0xfc2fe0": {
        "class": "MovementSibling",
        "comment": "3-netid mov-sized decoder; sibling of fb4070 family.",
        "fields": {},
    },
    # 19's actual decoder
    "0xfbd8f0": {
        "class": "Netid19_specific",
        "comment": "Decoder for netid 19 specifically (found via indirect-init secondary vtable).",
        "fields": {},
    },
    # The two non-decoders we wired
    "0xdb2910": {
        "class": "ConstructorFallback",
        "comment": "Class constructor matched as decoder by brute-force; the decoded_fields[] are init constants (zeros), not real packet data. 18 netids wired here have NO real decoder.",
        "fields": {},
    },
    "0xe9dd20": {
        "class": "TagOnlyStub",
        "comment": "19-byte stub function (init returns immediately). netid 543 is tag-only — the netid IS the entire packet content.",
        "fields": {},
    },
}


def auto_label_decoder(rva_hex, netids, offset_types, decomp_text=""):
    """For decoders not in HIGH_CONF, auto-generate a class name + field names
    based on shape heuristics."""
    n_offsets = len(offset_types)
    has_byte_fields = any(off > 0x40 for off in offset_types)
    has_floats = any('f32' in t for t in offset_types.values())
    has_ints = any('u32' in t or 'i32' in t for t in offset_types.values())

    if n_offsets == 0:
        cls = "TagOnlyOrNoFields"
        comment = "No field writes captured — likely tag-only or decoder bailed."
    elif n_offsets == 1:
        cls = "SingleField"
        comment = "Single-field packet (HeroDie / EnterFog / LeaveFog shape)."
    elif n_offsets == 2:
        cls = "TwoField"
        comment = "Two-field packet (SwapItem / RemoveItem shape)."
    elif 3 <= n_offsets <= 5:
        cls = "SmallPacket"
        comment = f"Small packet with {n_offsets} fields."
    elif 6 <= n_offsets <= 10:
        cls = "MediumPacket"
        comment = f"Medium packet with {n_offsets} fields."
    else:
        cls = "LargePacket"
        comment = f"Large packet with {n_offsets} fields (BasicAttackPos / CastSpellAns / Replication-style)."

    fields = {}
    for off in sorted(offset_types):
        types = offset_types[off]
        # Pick most likely type
        if 'f32' in types and 'u32' not in types:
            t = "f32"
        elif 'i32' in types and 'f32' not in types:
            t = "i32"
        elif 'u32' in types or 'u16ish' in types:
            t = "u32"
        else:
            t = "u32_or_f32"
        fields[f"0x{off:04x}"] = {"name": f"field_at_0x{off:02x}", "type": t}

    return {"class": cls, "comment": comment, "fields": fields}


def main() -> int:
    with zipfile.ZipFile(PATCH_PATH) as z:
        cfg = json.loads(z.read("result.json"))

    netid_to_rva: dict[int, str] = {}
    for e in cfg.get("extra_decoders", []):
        netid_to_rva[e["netid"]] = e["rva_start"]

    # Group netids by decoder RVA
    rva_to_netids: dict[str, list[int]] = defaultdict(list)
    for n, r in netid_to_rva.items():
        rva_to_netids[r].append(n)

    # Read e2e output to get actual offset+type observations per decoder
    rva_offset_types: dict[str, dict[int, set[str]]] = defaultdict(lambda: defaultdict(set))
    with open(OUTPUT_JSON) as f:
        e2e = json.load(f)
    ed = e2e.get("extra_decoders", {})
    for label, samples in ed.items():
        if "_netid" not in label:
            continue
        try:
            netid = int(label.rsplit("_netid", 1)[1])
        except ValueError:
            continue
        rva = netid_to_rva.get(netid)
        if not rva:
            continue
        for s in samples:
            for f in s.get("decoded_fields", []):
                off = f.get("offset")
                if off is None:
                    continue
                u32 = f.get("u32_le", 0)
                i32 = f.get("i32_le", 0)
                f32 = f.get("f32_le", 0)
                if isinstance(f32, (int, float)) and f32 != 0 and abs(f32) < 1e10 and (f32 != int(f32) or abs(f32) > 1e6):
                    rva_offset_types[rva][off].add("f32")
                if isinstance(u32, int) and 0 < u32 < 0x10000:
                    rva_offset_types[rva][off].add("u16ish")
                elif isinstance(u32, int) and u32 < 0x80000000:
                    rva_offset_types[rva][off].add("u32")
                if isinstance(i32, int) and i32 < 0:
                    rva_offset_types[rva][off].add("i32")

    # Build the catalog
    catalog = {
        "_comment": (
            "Per-decoder semantic field names for the 16.9 patch. Each entry "
            "maps a decoder RVA to its hypothesised packet class + field "
            "names per offset. Multiple netids may share a decoder when they "
            "represent variants of the same packet shape (e.g. Replication "
            "for different entity classes). High-confidence entries are "
            "hand-curated from decompiled C; the rest are auto-generated "
            "from observed write patterns."
        ),
        "_zhu_classes": [
            "CreateHero", "WaypointGroup", "WaypointGroupWithSpeed",
            "EnterFog", "LeaveFog", "UnitApplyDamage", "DoSetCooldown",
            "BasicAttackPos", "CastSpellAns", "BarrackSpawnUnit",
            "Replication", "SpawnMinion", "CreateNeutral", "CreateTurret",
            "NPCDieMapView", "NPCDieMapViewBroadcast", "HeroDie",
            "BuyItem", "RemoveItem", "SwapItem", "UseItem",
        ],
        "patch": "16.9",
        "decoders": {},
        "netid_to_decoder": {},
    }

    for rva, netids in sorted(rva_to_netids.items(), key=lambda x: -len(x[1])):
        offset_types = rva_offset_types.get(rva, {})
        if rva in HIGH_CONF:
            entry = dict(HIGH_CONF[rva])  # shallow copy
            entry["confidence"] = "hand-curated"
        else:
            entry = auto_label_decoder(rva, netids, offset_types)
            entry["confidence"] = "auto-generated"
        entry["netids"] = sorted(netids)
        entry["netid_count"] = len(netids)
        catalog["decoders"][rva] = entry
        for n in netids:
            catalog["netid_to_decoder"][str(n)] = rva
    # mov_decrypt netid 916 is configured separately in the patch
    catalog["netid_to_decoder"][str(cfg["mov_decrypt"]["netid"])] = "0xfb4070"
    catalog["mov_decrypt_netid"] = cfg["mov_decrypt"]["netid"]

    with open(SEMANTIC_PATH, "w") as f:
        json.dump(catalog, f, indent=2)
    print(f"wrote {SEMANTIC_PATH}")
    print(f"  decoders: {len(catalog['decoders'])}")
    print(f"  netids mapped: {len(catalog['netid_to_decoder'])}")
    print(f"  high-confidence: {sum(1 for d in catalog['decoders'].values() if d.get('confidence') == 'hand-curated')}")
    print(f"  auto-generated: {sum(1 for d in catalog['decoders'].values() if d.get('confidence') == 'auto-generated')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
