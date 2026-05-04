#!/usr/bin/env python
"""Upgrade semantic_field_names.json with Riot's real class names.

Key finding (documented in commit 'decrypt: leaked Riot PKT_*_s vocabulary'):
  - Riot stripped most RTTI from the binary; standard library RTTI
    survived but Riot's own classes did not.
  - 320 unique PKT_*_s class names leaked through MakeFunction template
    instantiations (mangled MSVC symbols). These are Riot's exact
    internal names for their packet structs.
  - The strings are passive (zero u64 references in code), so we can't
    auto-map "string -> decoder RVA". But the vocabulary lets us
    replace our hand-made names with Riot's real ones, picked by
    matching decoder shape (field count, type pattern, frequency)
    against the class-name semantics.

Output: scripts/semantic_field_names.json with:
  - decoder.riot_class_primary    : single best guess from PKT_*_s set
  - decoder.riot_class_candidates : ranked list of plausible classes
  - decoder.riot_class_confidence : "shape-match" / "frequency-hint"
                                    / "speculative"
  - All 320 PKT names listed under _riot_class_vocabulary for reference

This is NOT a proof; it's a substantially-better-informed guess than
the previous "Replication_compact" / "TwoField" generic labels. Path
to actual proof: correlate decoded values with replay events
(state-correlation studies, the Zhu approach).
"""

from __future__ import annotations

import json
import os
import sys
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PATCH_PATH = ROOT / "patch" / "16-9.patch"
ANALYSIS = Path(os.environ["USERPROFILE"]) / "Tools" / "analysis" / "16-9"
OUTPUT_JSON = Path(os.environ["USERPROFILE"]) / "AppData" / "Local" / "Temp" / "rofl_out" / "full_decoded_216.json"
SEMANTIC_PATH = ROOT / "scripts" / "semantic_field_names.json"


# ---------- Hand-curated decoder -> Riot class mappings -------------------

# Each entry: decoder RVA -> {primary, candidates, confidence, fields}
# Primary is the single best guess; candidates list ranked alternatives.
RIOT_CATALOG: dict[str, dict] = {
    # === 916 = mov_decrypt ===
    # WaypointGroup-style. fb4070 also matches netids 52, 173, 390 in
    # extras. Riot's name for the "movement update" packet:
    "0xfb4070": {
        "riot_class_primary": "PKT_DirectInputMovementDriverServerTurnData_s",
        "riot_class_candidates": [
            "PKT_DirectInputMovementDriverServerTurnData_s",
            "PKT_S2C_AddFollowTargetPosition_s",
            "PKT_S2C_AddFollowTargetTeleport_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "1 vector + 2 inline scalars (waypoint stream + final pos)",
        "fields": {
            "0x0010": {"name": "waypoint_vector_ptr", "type": "u64"},
            "0x0018": {"name": "waypoint_vector_size", "type": "u32"},
            "0x0020": {"name": "final_position_x_or_tag", "type": "f32_or_i32"},
            "0x0024": {"name": "final_position_y_or_tag", "type": "f32_or_i32"},
        },
    },

    # === Visibility / Fog (single-ID events) ===
    "0xfaffc0": {
        "riot_class_primary": "PKT_OnLeaveVisibilityClient_s",
        "riot_class_candidates": [
            "PKT_OnLeaveVisibilityClient_s",
            "PKT_S2C_OnEnterTeamVisibility_s",
            "PKT_S2C_OnLeaveTeamVisibility_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "single u32 ID + flag (Zhu EnterFog/LeaveFog pattern)",
        "fields": {
            "0x0010": {"name": "entity_net_id", "type": "u32"},
            "0x0014": {"name": "visibility_flag_or_subtype", "type": "u32"},
        },
    },

    # === Cooldown ===
    "0xfdb200": {
        "riot_class_primary": "PKT_CHAR_SetCooldown_Broadcast_s",
        "riot_class_candidates": [
            "PKT_CHAR_SetCooldown_Broadcast_s",
            "PKT_NPC_SetAutocast_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "5 fields (slot u8 + 3 floats + 1 entity id)",
        "fields": {
            "0x000c": {"name": "spell_slot", "type": "u8"},
            "0x0010": {"name": "cooldown_remaining_ms", "type": "f32"},
            "0x0018": {"name": "cooldown_total_ms", "type": "f32"},
            "0x001c": {"name": "target_net_id", "type": "u32"},
            "0x0024": {"name": "display_cooldown_ms", "type": "f32"},
        },
    },

    # === High-frequency complex (1068 = 46.5%) ===
    # 12 netids share f9d4e0; 1068 alone is 46.5% of all blocks. The
    # most plausible high-frequency packet classes are Replicate or
    # MissileReplication (missiles fire constantly).
    "0xf9d4e0": {
        "riot_class_primary": "PKT_S2C_ReplicateFields_s",
        "riot_class_candidates": [
            "PKT_S2C_ReplicateFields_s",
            "PKT_MissileReplication_s",
            "PKT_S2C_AnimationUpdateTimeStep_s",
            "PKT_S2C_AmmoUpdate_s",
        ],
        "riot_class_confidence": "frequency-hint",
        "shape": "11 mixed scalar fields (multi-tag dispatch)",
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

    # === Replication base (51 netids) ===
    # Wins broadly across many netids; matches Replication shape.
    "0xf6ab10": {
        "riot_class_primary": "PKT_S2C_ReplicateField_s",
        "riot_class_candidates": [
            "PKT_S2C_ReplicateField_s",
            "PKT_S2C_ReplicateFields_s",
            "PKT_MissileReplication_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "10 mixed-type fields (variable property dict — Zhu Replication)",
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

    # === Position-heavy (BasicAttackPos / CastSpellAns) ===
    "0x1074580": {
        "riot_class_primary": "PKT_Basic_Attack_Pos_s",
        "riot_class_candidates": [
            "PKT_Basic_Attack_Pos_s",
            "PKT_NPC_CastSpellAns_s",
            "PKT_Basic_Attack_Pos_Minion_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "14 fields w/ 3 position pairs (caster, target, target_end)",
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

    # === Replication variants ===
    "0xfcfd30": {
        "riot_class_primary": "PKT_S2C_ReplicateField_s",
        "riot_class_candidates": [
            "PKT_S2C_ReplicateField_s",
            "PKT_S2C_ReplicateFields_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "7 fields incl. position pair",
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
    "0xeba9e0": {
        "riot_class_primary": "PKT_S2C_ReplicateFields_s",
        "riot_class_candidates": [
            "PKT_S2C_ReplicateFields_s",
            "PKT_NPC_BuffUpdateStatAdjustments_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "12 fields (extended property dict)",
        "fields": {
            "0x0010": {"name": "property_value_a", "type": "u32_or_f32"},
            "0x0014": {"name": "property_value_b", "type": "u32"},
            "0x0024": {"name": "property_value_c", "type": "u32_or_f32"},
            "0x0028": {"name": "property_value_d", "type": "u32"},
            "0x0030": {"name": "property_value_e", "type": "u32"},
        },
    },

    # === Spawn / BasicAttack siblings ===
    "0xf9bae0": {
        "riot_class_primary": "PKT_Basic_Attack_s",
        "riot_class_candidates": [
            "PKT_Basic_Attack_s",
            "PKT_Basic_Attack_Minion_s",
            "PKT_S2C_AddFollowTargetPosition_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "11 fields w/ 2 position pairs + IDs",
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

    # === Item or status (medium 8-9 fields) ===
    "0xfcd970": {
        "riot_class_primary": "PKT_NPC_BuffAdd2_s",
        "riot_class_candidates": [
            "PKT_NPC_BuffAdd2_s",
            "PKT_S2C_AddBuffModifier_s",
            "PKT_BuyItemAns_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "8 fields, mid-sized event (buff add or item buy)",
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

    # === Small events ===
    "0x10843b0": {
        "riot_class_primary": "PKT_NPC_Die_MapView_s",
        "riot_class_candidates": [
            "PKT_NPC_Die_MapView_s",
            "PKT_NPC_Die_Broadcast_s",
            "PKT_NPC_Hero_Die_s",
            "PKT_S2C_RemoveItem_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "3-field (killer + killed + flag)",
        "fields": {
            "0x0014": {"name": "primary_id", "type": "u32"},
            "0x0018": {"name": "secondary_id", "type": "u32"},
            "0x001c": {"name": "tertiary_value", "type": "u32_or_f32"},
        },
    },

    # === Tiny ===
    "0xfd6270": {
        "riot_class_primary": "PKT_NPC_Hero_Die_s",
        "riot_class_candidates": [
            "PKT_NPC_Hero_Die_s",
            "PKT_NPC_LevelUp_s",
            "PKT_NPC_LevelUp_Global_s",
            "PKT_S2C_SetSpellLevel_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "2 fields (id + scalar)",
        "fields": {
            "0x0014": {"name": "primary_value", "type": "u32_or_f32"},
            "0x0020": {"name": "secondary_value", "type": "u32"},
        },
    },

    # === Medium 7-field ===
    "0x107c470": {
        "riot_class_primary": "PKT_S2C_AddBuffModifier_s",
        "riot_class_candidates": [
            "PKT_S2C_AddBuffModifier_s",
            "PKT_NPC_BuffUpdateCount_s",
            "PKT_S2C_AddItemModifier_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "7 fields (modifier-style event)",
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

    # === 4-field position event ===
    "0x10371f0": {
        "riot_class_primary": "PKT_S2C_AddFollowTargetPosition_s",
        "riot_class_candidates": [
            "PKT_S2C_AddFollowTargetPosition_s",
            "PKT_S2C_AddFollowTargetTeleport_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "2 floats + scalar + trailing",
        "fields": {
            "0x0008": {"name": "position_x", "type": "f32"},
            "0x000c": {"name": "position_y", "type": "f32"},
            "0x0018": {"name": "scalar_or_id", "type": "u32"},
            "0x0024": {"name": "trailing_value", "type": "u32_or_f32"},
        },
    },

    # === Mov sibling ===
    "0xfc2fe0": {
        "riot_class_primary": "PKT_DirectInputMovementDriverServerTurnData_s",
        "riot_class_candidates": [
            "PKT_DirectInputMovementDriverServerTurnData_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": "mov-family decoder body (sibling of fb4070)",
        "fields": {},
    },

    # === Netid 19 specific ===
    "0xfbd8f0": {
        "riot_class_primary": "(unidentified — netid 19 only)",
        "riot_class_candidates": [
            "PKT_S2C_AddBuffModifier_s",
            "PKT_S2C_AlwaysFaceTarget_s",
        ],
        "riot_class_confidence": "speculative",
        "shape": "9 fields, netid 19 only (18 blocks)",
        "fields": {},
    },

    # === Honest fallbacks ===
    "0xdb2910": {
        "riot_class_primary": "(class constructor — false-positive)",
        "riot_class_candidates": [],
        "riot_class_confidence": "constructor-fallback",
        "shape": "NOT a decoder — this is a class ctor that initializes ~0x4cc bytes of an instance. The 18 netids matched here lack a real per-class decoder in our prologue+dispatch candidate set.",
        "fields": {},
    },
    "0xe9dd20": {
        "riot_class_primary": "(tag-only stub — netid 543 has no payload to decode)",
        "riot_class_candidates": [],
        "riot_class_confidence": "tag-only",
        "shape": "19-byte init stub. netid 543 is signaled by its netid alone.",
        "fields": {},
    },
}


# ---------- Auto-classification of remaining decoders ---------------------

def auto_classify(rva, netid_count, offset_types):
    """Generate a generic Riot-class candidate set based on shape."""
    n = len(offset_types)
    has_positions = sum(1 for off in offset_types if 'f32' in offset_types[off]) >= 2
    if n == 0:
        return {
            "riot_class_primary": "(no field writes captured)",
            "riot_class_candidates": [],
            "riot_class_confidence": "shape-match",
            "shape": "no field writes",
        }
    if n == 1:
        return {
            "riot_class_primary": "(single-id event)",
            "riot_class_candidates": [
                "PKT_NPC_Hero_Die_s", "PKT_NPC_Die_Broadcast_s",
                "PKT_OnLeaveVisibilityClient_s", "PKT_S2C_OnEnterTeamVisibility_s",
            ],
            "riot_class_confidence": "shape-match",
            "shape": f"{n}-field event (id-only)",
        }
    if n == 2:
        return {
            "riot_class_primary": "(small 2-field event)",
            "riot_class_candidates": [
                "PKT_S2C_RemoveItem_s", "PKT_NPC_BuffRemove2_s",
                "PKT_S2C_AmmoUpdate_s", "PKT_S2C_SetSpellLevel_s",
            ],
            "riot_class_confidence": "shape-match",
            "shape": f"{n}-field event",
        }
    if 3 <= n <= 5:
        return {
            "riot_class_primary": "(small fixed packet)",
            "riot_class_candidates": [
                "PKT_NPC_BuffUpdateCount_s", "PKT_S2C_AddBuffModifier_s",
                "PKT_NPC_AddFakeBuff_s", "PKT_BuyItemAns_s",
            ],
            "riot_class_confidence": "shape-match",
            "shape": f"{n}-field packet",
        }
    if has_positions:
        return {
            "riot_class_primary": "(position-bearing packet)",
            "riot_class_candidates": [
                "PKT_S2C_AddFollowTargetPosition_s",
                "PKT_S2C_AddFollowTargetTeleport_s",
                "PKT_Basic_Attack_Pos_Minion_s",
                "PKT_S2C_ChangeMissileSpline_s",
            ],
            "riot_class_confidence": "shape-match",
            "shape": f"{n}-field packet w/ position pair",
        }
    return {
        "riot_class_primary": "(medium-large event)",
        "riot_class_candidates": [
            "PKT_S2C_AddBuffModifier_s", "PKT_S2C_AddItemModifier_s",
            "PKT_NPC_BuffUpdateStatAdjustments_s",
        ],
        "riot_class_confidence": "shape-match",
        "shape": f"{n}-field event",
    }


def main() -> int:
    import zipfile
    with zipfile.ZipFile(PATCH_PATH) as z:
        cfg = json.loads(z.read("result.json"))

    netid_to_rva: dict[int, str] = {e["netid"]: e["rva_start"] for e in cfg.get("extra_decoders", [])}
    rva_to_netids: dict[str, list[int]] = defaultdict(list)
    for n, r in netid_to_rva.items():
        rva_to_netids[r].append(n)

    rva_offset_types: dict[str, dict[int, set[str]]] = defaultdict(lambda: defaultdict(set))
    with open(OUTPUT_JSON) as f:
        e2e = json.load(f)
    for label, samples in e2e.get("extra_decoders", {}).items():
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
                f32 = f.get("f32_le", 0)
                if isinstance(f32, (int, float)) and f32 != 0 and abs(f32) < 1e10 and (f32 != int(f32) or abs(f32) > 1e6):
                    rva_offset_types[rva][off].add("f32")
                if isinstance(u32, int) and 0 < u32 < 0x10000:
                    rva_offset_types[rva][off].add("u16ish")
                elif isinstance(u32, int) and u32 < 0x80000000:
                    rva_offset_types[rva][off].add("u32")

    # Load the leaked Riot vocabulary
    pkt_pairs_path = ANALYSIS / "client_packet_pairs.json"
    with open(pkt_pairs_path) as f:
        riot_pairs = json.load(f)
    riot_classes = sorted(set(p["packet"] for p in riot_pairs))

    catalog = {
        "_comment": (
            "Per-decoder semantic catalog with Riot's REAL packet class "
            "names (extracted from leaked MakeFunction template strings). "
            "Each decoder is mapped to a primary Riot class + ranked "
            "candidates. Field names are hand-curated where shape is "
            "distinctive; otherwise auto-generated. Confidence: "
            "'shape-match' = field count + types align with the named "
            "class's known structure; 'frequency-hint' = packet "
            "frequency points to a class category (e.g. 1068=46.5% must "
            "be Replicate or MissileReplication); 'speculative' = "
            "single-netid decoder with no anchor; 'constructor-fallback'/"
            "'tag-only' = honest no-decoder buckets."
        ),
        "patch": "16.9",
        "_riot_class_vocabulary": {
            "_comment": "All 320 Riot packet class names leaked through MakeFunction template instantiations in the binary's mangled MSVC symbols. These are the EXACT names Riot uses internally.",
            "total": len(riot_classes),
            "names": riot_classes,
        },
        "_zhu_classes": [
            "CreateHero", "WaypointGroup", "WaypointGroupWithSpeed",
            "EnterFog", "LeaveFog", "UnitApplyDamage", "DoSetCooldown",
            "BasicAttackPos", "CastSpellAns", "BarrackSpawnUnit",
            "Replication", "SpawnMinion", "CreateNeutral", "CreateTurret",
            "NPCDieMapView", "NPCDieMapViewBroadcast", "HeroDie",
            "BuyItem", "RemoveItem", "SwapItem", "UseItem",
        ],
        "decoders": {},
        "netid_to_decoder": {},
    }

    for rva, netids in sorted(rva_to_netids.items(), key=lambda x: -len(x[1])):
        offset_types = rva_offset_types.get(rva, {})
        if rva in RIOT_CATALOG:
            entry = dict(RIOT_CATALOG[rva])
        else:
            auto = auto_classify(rva, len(netids), offset_types)
            # Auto-generated field names
            fields = {}
            for off in sorted(offset_types):
                types = offset_types[off]
                if "f32" in types and "u32" not in types:
                    t = "f32"
                elif "u32" in types or "u16ish" in types:
                    t = "u32"
                else:
                    t = "u32_or_f32"
                fields[f"0x{off:04x}"] = {
                    "name": f"field_at_0x{off:02x}",
                    "type": t,
                }
            entry = {
                **auto,
                "fields": fields,
            }
        entry["netids"] = sorted(netids)
        entry["netid_count"] = len(netids)
        catalog["decoders"][rva] = entry
        for n in netids:
            catalog["netid_to_decoder"][str(n)] = rva
    catalog["netid_to_decoder"][str(cfg["mov_decrypt"]["netid"])] = "0xfb4070"
    catalog["mov_decrypt_netid"] = cfg["mov_decrypt"]["netid"]

    with open(SEMANTIC_PATH, "w") as f:
        json.dump(catalog, f, indent=2)

    # Stats
    primary_named = sum(1 for d in catalog["decoders"].values()
                        if d.get("riot_class_primary", "").startswith("PKT_"))
    candidates_total = sum(len(d.get("riot_class_candidates", []))
                           for d in catalog["decoders"].values())
    print(f"wrote {SEMANTIC_PATH}")
    print(f"  decoders: {len(catalog['decoders'])}")
    print(f"  primary mapped to a named Riot class: {primary_named}")
    print(f"  total candidate Riot classes listed: {candidates_total}")
    print(f"  Riot vocabulary embedded: {len(riot_classes)} classes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
