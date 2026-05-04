# Riot's leaked PKT_*_s class vocabulary (16.9)

The League of Legends 16.9 binary has stripped most C++ RTTI — a scan
finds only 11 `.?AV` mangled type names, all from `std::*`
(`std::runtime_error`, `std::bad_cast`, etc.). Riot's own packet
classes have no surviving Type Descriptors, so the standard
"vtable[-1] → Complete Object Locator → mangled name" trick that
works on most MSVC binaries returns garbage on this one.

But **320 unique Riot packet class names leaked** through a different
channel: `MakeFunction` template instantiations. Every callback
registered via Riot's event system creates a unique mangled symbol
of the form

```
.?$MakeFunction@VAIBaseClient@@V1@_NAEBVPKT_NPC_CastSpellAns_s@@@Riot@@...
```

These symbol names sit in `.data` as static type info embedded by the
MSVC linker. They aren't referenced by code at runtime (zero u64
references back to the strings — confirmed via byte-aligned scan), so
we can't auto-link "this string → this decoder RVA". But the
vocabulary itself is gold: it lists Riot's *exact* internal names for
every packet class in the binary.

## Extraction

Reproducible via:

```bash
python -c "
import re
data = open(r'~/Tools/analysis/16-9/league_16-9.exe', 'rb').read()
pat = re.compile(rb'V([A-Za-z0-9_]{4,80}?)@@V1@_NAEBV(PKT_[A-Za-z0-9_]+_s)@@')
print(sorted({(m.group(1).decode(), m.group(2).decode()) for m in pat.finditer(data)}))
"
```

The full extraction including (ClientType, PacketType) pairs is at
`~/Tools/analysis/16-9/client_packet_pairs.json` (321 unique pairs)
and the deduped class list is embedded in
[scripts/semantic_field_names.json](../scripts/semantic_field_names.json)
under `_riot_class_vocabulary.names`.

## ClientTypes (which subsystems handle which packets)

| ClientType                          | Packet count |
|-------------------------------------|--------------|
| AIBaseClient                        | 127          |
| AIHeroClient                        | 102          |
| HeroInventoryClient                 |  23          |
| MissileClient                       |  15          |
| AttackableUnit                      |  11          |
| BuffManagerClient                   |  11          |
| AIMinionClient                      |   3          |
| NetVisibilityObjectClient           |   3          |
| SpellbookClient                     |   3          |
| AITurretClient                      |   2          |
| AnimatedBuildingClient              |   2          |
| DirectMovementComponentClient       |   2          |
| JunglePathComponentClient           |   2          |
| ObjectAttacher                      |   2          |
| VisibleNetworkedObject              |   2          |
| (16 more singleton clients)         |  16          |

Total: 321 (ClientType, PacketType) bindings from 26 client classes.

## Packet classes by category

Below is the full 320-name vocabulary, grouped by domain. Many
packet classes register on multiple ClientTypes; the per-class
naming reflects Riot's intended use, not the listening side.

### Replication (3)

```
PKT_MissileReplication_s
PKT_S2C_ReplicateField_s
PKT_S2C_ReplicateFields_s
```

### Position / Movement (21)

```
PKT_Basic_Attack_Pos_Minion_s            PKT_Basic_Attack_Pos_s
PKT_DirectInputMovementDriverServerTurnData_s
PKT_S2C_AddFollowTargetPosition_s        PKT_S2C_AddFollowTargetTeleport_s
PKT_S2C_CameraPosition_s                 PKT_S2C_CharacterPosition_s
PKT_S2C_CursorPositionUpdate_s           PKT_S2C_DirectInputForceMovement_s
PKT_S2C_DirectInputForcePosition_s       PKT_S2C_FollowTargetMovement_s
PKT_S2C_LookAtPosition_s                 PKT_S2C_MoveCamera_s
PKT_S2C_MoveCameraToPosition_s           PKT_S2C_MovementCompleteCount_s
PKT_S2C_RemoveFollowTargetPosition_s     PKT_S2C_SetCameraPosition_s
PKT_S2C_SetClientCharacterPosition_s     PKT_S2C_SyncMovementCompleteCount_s
PKT_S2C_TeleportCharacter_s              PKT_S2C_UpdateCursorPosition_s
```

### Spell / Cast / Cooldown (15)

```
PKT_CHAR_CancelTargetingReticle_s        PKT_CHAR_SetCooldown_Broadcast_s
PKT_ChangeSlotSpellData_OwnerOnly_s      PKT_ChangeSlotSpellData_Summoner_s
PKT_ChangeSlotSpellData_s                PKT_NPC_CastSpellAns_s
PKT_NPC_SetAutocast_s                    PKT_NPC_UpgradeSpellAns_s
PKT_S2C_AutoAimTarget_s                  PKT_S2C_AddSpellModifier_s
PKT_S2C_RemoveSpellModifier_s            PKT_S2C_SetSpellLevel_s
PKT_S2C_SetSpellSlotData_s               PKT_S2C_SpellChainOwner_s
PKT_S2C_UpdateAvailableSpells_s
```

### Buff / Status (15)

```
PKT_NPC_AddFakeBuff_s                    PKT_NPC_AddFakeBuffs_s
PKT_NPC_BuffAdd2_s                       PKT_NPC_BuffLockFacing_s
PKT_NPC_BuffRemove2_s                    PKT_NPC_BuffReplace_s
PKT_NPC_BuffUpdateCount_s                PKT_NPC_BuffUpdateNumCounter_s
PKT_NPC_BuffUpdateStatAdjustments_s      PKT_NPC_RemoveFakeBuff_s
PKT_NPC_RemoveFakeBuffs_s                PKT_S2C_AddBuffModifier_s
PKT_S2C_AddDamagePredictionByItem_s      PKT_S2C_AddDamagePredictionBySpell_s
PKT_S2C_AddDamagePredictionByValue_s
```

### Death / Reincarnate (7)

```
PKT_Building_Die_s          PKT_HeroReincarnate_s        PKT_HeroReincarnateAlive_s
PKT_NPC_Die_Broadcast_s     PKT_NPC_Die_MapView_s        PKT_NPC_ForceDead_s
PKT_NPC_Hero_Die_s
```

### Item / Inventory (27)

```
PKT_BuyItemAns_s                         PKT_RemoveItemAns_s
PKT_S2C_AddItemModifier_s                PKT_S2C_RemoveItemModifier_s
PKT_S2C_RemoveItem_s                     PKT_S2C_SetItemHover_s
PKT_S2C_SetItemAnnouncement_s            PKT_S2C_ShowItemSpellGlowDecal_s
PKT_S2C_AddDamagePredictionByItem_s      ...
(27 entries — BuyItem, RemoveItem, item modifiers, decals, etc.)
```

### Visibility / Fog (7)

```
PKT_OnLeaveVisibilityClient_s
PKT_S2C_OnEnterTeamVisibility_s
PKT_S2C_OnLeaveTeamVisibility_s
PKT_S2C_ExtraActionsSetVisibilityOfButtonById_s
PKT_S2C_ExtraActionsSetVisibilityOfGroup_s
PKT_S2C_SetClientVisibilityOfNeutralObjective_s
PKT_S2C_VisibilityOnEnterClient_s
```

### Damage prediction (7)

```
PKT_S2C_AddDamagePredictionByItem_s   PKT_S2C_AddDamagePredictionBySpell_s
PKT_S2C_AddDamagePredictionByValue_s  PKT_S2C_RemoveDamagePredictionByItem_s
PKT_S2C_RemoveDamagePredictionBySpell_s
PKT_S2C_RemoveDamagePredictionByValue_s
PKT_S2C_OverrideDamageDisplay_s
```

### Health bar (11)

```
PKT_HealthBar_Icon_Add_S2C_s             PKT_HealthBar_Icon_Clear_S2C_s
PKT_HealthBar_Icon_Remove_S2C_s          PKT_HealthBar_Icon_State_Set_S2C_s
PKT_S2C_AddHealthThreshold_s             PKT_S2C_ChangeHealthBarStyle_s
PKT_S2C_RemoveHealthThreshold_s          PKT_S2C_ResetHealthBar_s
PKT_S2C_SetHealthBarLockedToOwner_s      PKT_S2C_SetHealthBarPosition_s
PKT_S2C_ShowHealthBar_s
```

### AI / Targeting (16)

```
PKT_AI_TargetHeroS2C_s                   PKT_AI_TargetS2C_s
PKT_CHAR_CancelTargetingReticle_s        PKT_S2C_AlwaysFaceTarget_s
PKT_S2C_AutoAimTarget_s                  PKT_S2C_RemoveTarget_s
PKT_S2C_SetAITargetingPolicy_s           PKT_S2C_SetTargetableArea_s
PKT_S2C_SetTargetableForTeam_s           PKT_S2C_SetTargetableToTeam_s
PKT_S2C_SetTargetableToTeamFlags_s       PKT_S2C_SetTargetingPolicyForUnit_s
... (16 total)
```

### Animation / VFX (18)

```
PKT_S2C_AnimationUpdateTimeStep_s        PKT_S2C_CopyAnimationState_s
PKT_S2C_DisableAnimationOnDeath_s        PKT_S2C_EnableDeathEffectForChampionKills_s
PKT_S2C_PauseAnimation_s                 PKT_S2C_PerformAttackForAnimation_s
PKT_S2C_PlayAnimation_s                  PKT_S2C_RemoveAnimation_s
PKT_S2C_RestartAnimation_s               PKT_S2C_SetAnimSpeed_s
PKT_S2C_SetExtraAnim_s                   PKT_S2C_StopAnimation_s
... and VFX/effect packets
```

### Audio (2)

```
PKT_S2C_ChangeCharacterVoice_s   PKT_S2C_SetAudioLinkedAlly_s
```

### Missile (16)

```
PKT_MissileReplication_s                 PKT_S2C_ChangeMissilePhysics_s
PKT_S2C_ChangeMissileSpline_s            PKT_S2C_DampenerMissileMode_s
PKT_S2C_DestroyClientMissile_s           PKT_S2C_MissileForceMaxSpeed_s
PKT_S2C_MissileReInitVFX_s               PKT_S2C_MissileScriptTrigger_s
PKT_S2C_MissileScriptedVisible_s         PKT_S2C_MouseTrackMissile_s
PKT_S2C_PauseMissile_s                   PKT_S2C_SetMissileOrbit_s
... (16 total)
```

### Minion / Wave / Barrack (3)

```
PKT_Basic_Attack_Minion_s
PKT_Basic_Attack_Pos_Minion_s
PKT_S2C_IncrementMinionKills_s
```

### Building / Tower (2)

```
PKT_Building_Die_s   PKT_UpdateTurretFlags_s
```

### Camera (10)

```
PKT_S2C_CameraFOV_s                      PKT_S2C_CameraLockTarget_s
PKT_S2C_CameraLock_s                     PKT_S2C_CameraPosition_s
PKT_S2C_CameraRotation_s                 PKT_S2C_CameraZoom_s
PKT_S2C_MoveCamera_s                     PKT_S2C_MoveCameraToPosition_s
PKT_S2C_SetCameraPosition_s              PKT_S2C_SetCameraZoom_s
```

### Modifier (11)

```
PKT_S2C_AddBuffModifier_s                PKT_S2C_AddItemModifier_s
PKT_S2C_AddSpellModifier_s               PKT_S2C_RemoveBuffModifier_s
PKT_S2C_RemoveItemModifier_s             PKT_S2C_RemoveSpellModifier_s
PKT_S2C_AddPredicateOnTargetModifier_s   PKT_S2C_RemovePredicateOnTargetModifier_s
PKT_S2C_AddSpellGlobalCooldownModifier_s PKT_S2C_RemoveSpellGlobalCooldownModifier_s
PKT_S2C_ResetSpellModifiers_s
```

### Hero / Champion (11)

```
PKT_AI_TargetHeroS2C_s                   PKT_HeroReincarnateAlive_s
PKT_HeroReincarnate_s                    PKT_NPC_Hero_Die_s
PKT_S2C_ChangeCharacterData_s            PKT_S2C_ChangeCharacterVoice_s
PKT_S2C_HeroSelected_s                   PKT_S2C_PlayCharacterIntroVO_s
PKT_S2C_SetClientCharacter_s             PKT_S2C_SetClientCharacterPosition_s
PKT_S2C_SetClientCharacterUnitTarget_s
```

### Other (~149 — game-mechanic / UI / debug / housekeeping)

The complete unmodified list lives in
[scripts/semantic_field_names.json](../scripts/semantic_field_names.json)
under `_riot_class_vocabulary.names`.

## How this maps to our 53 unique decoders

We've extracted Riot's vocabulary but **the strings have zero
runtime references** in the binary, so there's no automatic
"name → decoder RVA" link. Instead, each of our 53 unique decoder
RVAs is matched by **shape** (offset count, field types, payload
size pattern, frequency) to a primary candidate Riot class plus a
ranked candidate list, in
[scripts/semantic_field_names.json](../scripts/semantic_field_names.json):

| Confidence tier        | Decoders | What it means                                                          |
|------------------------|----------|------------------------------------------------------------------------|
| `shape-match`          |  ~30     | Field count + type pattern is distinctive enough to narrow to 1-3 names |
| `frequency-hint`       |   ~3     | Class category inferred from how often the netid fires                  |
| `speculative`          |   ~6     | Single-netid decoder with no anchoring signal                           |
| `constructor-fallback` |    1     | brute-force matched a class ctor; 18 wired netids have no real decoder |
| `tag-only`             |    1     | netid is the entire packet content (no payload bytes to decode)         |
| auto-generated         |  ~12     | Generic `field_at_0xNN` names + observed type                          |

## What it would take to lift confidence to "proven"

Three options, in increasing cost:

1. **Cross-reference Riot debug strings** (cheap, partial). Some
   game-event strings (e.g. `evtCastSpell1`) survive in `.rdata`. If
   a decoder body references one near a netid constant, that's a
   crisp link. Ghidra symbol scan finds 53 such matches; manual
   pairing could promote 5-10 decoders to "proven".

2. **State-correlation studies** (expensive, proves fields). Take
   replays where you know the outcome (e.g. you played them).
   Correlate decoded values to observed game state to pin down each
   field's meaning. Henry Zhu did this manually for ~9 classes from
   1.4 M replays. For 53 decoders, weeks of effort.

3. **A debug build of League** (impossible). Would carry full RTTI
   + symbol table. Doesn't exist publicly.

The catalog as it stands now is **shape-anchored guesses against
Riot's real class-name vocabulary** — strictly better than picking
names ourselves, but not RTTI-proof.
