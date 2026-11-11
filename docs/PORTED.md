# Ported systems

What each subsystem ports, with the C++ it follows. Moved out of [ROADMAP.md](ROADMAP.md) on 2026-10-14 so the roadmap
holds only open work; this file is the reference for what is already done. Open items that were listed here are now in
the roadmap's "Open gaps by area". History is in [CHANGELOG.md](../CHANGELOG.md).

## Phase 1 — Data (nearly complete)

- [x] Install discovery (Steam + env overrides) for Zero Hour and Renegade
- [x] `.big` archives (via `cnc-formats`), engine-accurate precedence (`Win32BIGFileSystem::init`)
- [x] Renegade `MIX1` archives (`.dat`/`.mix`, from `wwlib/mixfile.cpp`)
- [x] RefPack / `EAR\0` decompression (`Compression/EAC/refdecode.cpp`)
- [x] `.w3d` chunk parsing (via `cnc-formats`): 8896/8901 ZH files, 3976/3976 Renegade files
- [x] `.map` DataChunk reader (`zr-map`): 150/150 maps. Decodes heightmap, world info, objects, sides and waypoints.
- [x] BlendTileData (terrain textures), via `zr-terrain`
- [x] GlobalLighting (`MapFile::lighting`)
- [x] `.map` remaining chunks: PolygonTriggers (water areas, rivers; 2488 triggers) and the script tree (23906 scripts, structural decode only, no execution). All 8 chunk kinds in the 150 shipped maps are typed. Water areas feed `TerrainLogic::is_underwater` / `water_height` and the pathfinder's `CELL_WATER` (120 of 150 maps have water cells).
- [x] W3D models + DDS/TGA textures + template→model resolution (`zr-w3d`)
- [x] Generals INI parser + merged database (`zr-ini`): 221/222 files; the remaining one is never loaded by the engine
- [x] Engine-accurate INI load order (`zr_ini::LOAD_ORDER`, 133 files; `INI::loadFileDirectory` semantics) and per-map `map.ini`/`solo.ini` overlay (`zr_ini::load_map_ini`, with `IniDb::rollback`). Wired into `zr-game` start-up: `GameData::for_map` overlays it, rebuilds the templates (~50-120 ms) and rolls back; 82 shipped maps have one. Overrides follow `INI_LOAD_CREATE_OVERRIDES` (`Templates::from_ini_overlay`: copy of the base, `AddModule`/`ReplaceModule`, last Body wins, Weapon overlay)
- [x] Typed templates (`zr-templates`): `ThingTemplate`, Weapon, Locomotor, Armor, Upgrade and ObjectCreationList (294 OCLs, 716 typed nuggets).
  - Round 1: bodies, AI family, PhysicsBehavior, die modules (`DieMuxData`), upgrade modules (`UpgradeMuxData`), `ProductionUpdate`, the common behaviors (`StealthUpdate`, `AutoHealBehavior`, `FlammableUpdate`, `FireWeaponWhenDeadBehavior`, `PoisonedBehavior`, `LifetimeUpdate`, `StealthDetectorUpdate`, the OCL/command-set/XP/weapon-bonus upgrades).
  - Round 2 (crush `SquishCollide`/`CrushDie`, crates `CreateCrateDie`/`CrateCollide` (base + every subclass: `Money`/`Heal`/`Veterancy`/`Unit`/`Salvage`/`Shroud`/`ConvertToCarBomb`/`ConvertToHijackedVehicle`/`Sabotage*`) and `FireWeaponCollide`, `EjectPilotDie`, `VeterancyGainCreate`, `ToppleUpdate`, `SpawnBehavior`, `SlavedUpdate`, `HordeUpdate`, `AutoDepositUpdate`, `BaseRegenerateUpdate`, `PropagandaTowerBehavior`, `FireWeaponUpdate`, `RepairDockUpdate`, `HealContain`/`TunnelContain`, `OverchargeBehavior`, `MinefieldBehavior`, `ProneUpdate`/`DemoTrapUpdate`/`CleanupHazardUpdate`/`CommandButtonHuntUpdate`).
  - Round 3 (aircraft `JetAIUpdate`/`ParkingPlaceBehavior`/`CountermeasuresBehavior`, `TransitionDamageFX`, `GenerateMinefieldBehavior`, `RebuildHoleBehavior`/`RebuildHoleExposeDie`, `KeepObjectDie`, `ModelConditionUpgrade`).
  - Round 4 (all remaining GameLogic `Update`/`Behavior`/`Die`/`Create`/`Upgrade`/`Contain`/`SpecialPower` families via `ModuleData::Logic`, plus the `W3DTankDraw`/`W3DTruckDraw`/`W3DTankTruckDraw` tread/tire fields via `ModuleData::Draw`).
  - Totals: 2115 objects, 363 weapons, 182 locomotors, 53 armors, 82 upgrades, 0 warnings; 16770 typed / 2782 `ModuleData::Unparsed` module instances (the rest are client draw/update modules handled by `zr-w3d`/the viewer)
- [ ] Typed templates, remaining: the other module families (still `ModuleData::Unparsed`)
- [x] Textures (`.dds`, `.tga`), via `zr-w3d`
- [x] String tables (`zr-text`): `.csf` and `.str`, case-insensitive labels, map string overlay; english `Generals.csf` has 6422 labels and every object `DisplayName` resolves. `zr-game` loads it into `GameData::text` (+ the map's `map.str`) with `display_name(template)`.
- [x] Audio lookup (`zr-audio`): `AudioEventInfo` field table (`INIAudioEventInfo.cpp`) for `AudioEvent` (1410) / `MusicTrack` (69) / `DialogEvent` (2569), `Default*` seeding, `AudioSettings` and `MiscAudio`; filename resolution like `AudioEventRTS::generateFilename` / `adjustForLocalization` (`AudioRoot`/`SoundsFolder`/`MusicFolder`/`StreamingFolder`/`SoundsExtension` + speech language folder); deterministic variant pick (client RNG, never the sim RNG); bounds-checked PCM / IMA ADPCM WAV header reader plus an IMA ADPCM decoder to PCM.
  - On the retail install 7842/7928 sound references resolve (98.9%) and all 8475 `.wav` headers decode (5250 PCM, 3225 IMA ADPCM).
  - The viewer plays audio (`audio.rs`): `AudioEvent` names resolve through the VFS, IMA decodes to PCM, and Bevy audio plays voices (`VoiceSelect`/`VoiceMove`/`VoiceAttack`), `FireSound`, `SoundDie`, FXList `Sound` nuggets, `UnitSpecificSound`/`GUIClick` and a music playlist, honouring volume / range fade / `Limit` / `Priority` / `Control` / RNG (`--mute` silences).

## Phase 2 — Headless simulation (core gameplay done; module coverage continues)

Done so far:

- `zr-sim` core: 30 Hz frame loop with the sleepy-update heap, objects with deferred destroy,
  ActiveBody, reduced AI/locomotor `MoveTo`, terrain height, a uniform-grid partition, the
  `RandomValue.cpp` port and a CRC.
- Sleepy heap, one loop: AI and module updates run in key order (`GameLogic::update`, GameLogic.cpp:3851-3905);
  the object helpers (`object_helpers.rs`: ObjectSMCHelper, ObjectDefectionHelper, StatusDamageHelper, SubdualDamageHelper,
  ObjectWeaponStatusHelper, FiringTracker, TempWeaponBonusHelper) are registered in the `Object` constructor's order with their
  C++ phases and first wakes, with the idle sleeps; their feature wake calls and the other sleepy classes are not ported yet.
- `zr-game` skirmish start (port of `startNewGame`): Alpine Assault, America vs China. Two runs
  agree on the CRC.
- Object ids and the map-object loop: one counter from 1 (`GameLogic::allocateObjectID`) shared by objects and
  the record-only projectiles/beams/streams. The map load reproduces `forceFluffToProp` (`CLEARED_BY_BUILD` with
  `FenceWidth = 0` is a client prop in multiplayer, still drawn by the viewer's static map-object pass), a landmark bridge
  creates its four towers inside its own creation (`Bridge::Bridge(Object*)`), and `AISkirmishPlayer::newMap` destroys the
  placed starting command center before `buildStructureNow` rebuilds it (both ids spent, as in C++). Bodyless map objects
  (`InactiveBody`) carry `EFFECTIVELY_DEAD` from creation (`InactiveBody.cpp:47-50`), as in C++: `Object::effectively_dead`
  is set for every template without an `ActiveBody`, so the mood scan/targeting, the script counts and
  `isRemovableForConstruction` (`BuildAssistant.cpp:1381-1382`) see them as dead. `ZR_ID_DUMP` + `tools/id_diff.py`
  compare the allocations with the C++ `ZR_L64_ID`; AN3 seed 1 matches for the first 238 ids (start-up and early game).
- Ground pathfinder (`zr-sim::pathfind`): cell grid with cliff / structure-footprint classification, the
  engine's A* (costs, open-list order), closest-cell fallback and path smoothing; water cells from the map's water areas. Bridges
  (`zr-sim::bridge` + `zr-game::bridges`) are reduced to one grid: a healthy bridge overlays its cells
  as a passable deck (a bridge over water/cliff becomes crossable at the deck height, a land bridge
  still allows pass-under), the locomotor snaps to `TerrainLogic::surface_height`, and destroying the
  bridge re-blocks the crossing. Pathing uses the locomotor's real `Surfaces` mask (climber = cliff,
  boat = water, hover/air). A bridge object's `BODY_RUBBLE` kills the units on it and re-blocks the
  crossing (`zr-sim::bridge_logic`), corner towers are created for landmark bridges, and a dozer can
  repair it (`Command::Repair`). Zones (`zr-sim::zone`) are ported from the engine's
  `PathfindZoneManager::calculateZones`: the raw per-cell zone (same-*cell-type* components inside each
  10x10 `ZONE_BLOCK_SIZE` block, collapsed), the six equivalency tables (`m_hierarchicalZones`,
  `m_terrainZones`, `m_groundWaterZones`, `m_groundRubbleZones`, `m_groundCliffZones`, `m_crusherZones`,
  built by `applyZone`/`resolveZones`/`flattenZones` with the engine's `waterGround`/`terrain`/
  `crusherGround` predicates, in the ladder this Zero Hour build compiles — the `#else` of
  `RTS_GENERALS && RETAIL_COMPATIBLE_PATHFINDING`) and `getEffectiveZone`/`getEffectiveTerrainZone`;
  on the AN3 map the port's raw zone numbers and effective/terrain zones match the C++ cell for cell.
  `clientSafeQuickDoesPathExist` is ported with its `validMovementPosition` gate, its `doingTerrainZone`
  remap and the `UNINITIALIZED_ZONE` parent arm (`crates/zr-sim/src/pathfind.rs`). The tables are
  **persistent**, as in the engine: `calculateZones` is the only writer, called from `classifyMap` and
  from `processPathfindQueue` on the frame `markZonesDirty` scheduled (`ZONE_UPDATE_FREQUENCY` 300, or
  frame 2 before frame 2). A footprint change marks each cell it actually changed
  `UNINITIALIZED_ZONE` and runs `updateZonesForModify` over the touched bounds, which gives each one a
  same-type neighbour's zone inside its block (unassignable cells stay 0) without rebuilding the tables.
  The coarse search is the engine's `internal_findHierarchicalPath`/`processHierarchicalCell` over the `ZoneBlock` tables
  (2026-10-28), including the `hasInfo` border-cell gate (a block is left only through a cell that keeps a
  `PathfindCellInfo`: obstacle, unit or goal cells, `AIPathfind.cpp:1395`, `:7464-7467`) and `buildHierarchicalPath`'s
  21x21 start expansion; `findPath` clears and rebuilds the corridor for every request (`setAllPassable` when the coarse
  search fails, `:6455-6461`), and the corridor restricts the fine A* (`isPassable` in `examineCellsCallback`, and the engine's
  `100*COST_ORTHOGONAL` surcharge for a neighbour outside the corridor); `getMoveAwayFromPath` and
  `patchPath` lift the corridor with `setAllPassable` first. `checkDestination`
  and `checkForMovement` use the *unit's* ignored obstacle (`AIUpdateInterface::getIgnoredObstacleID`),
  while `validMovementPosition`/`isLinePassable` use the pathfinder's search-scoped one, as in the engine;
  `validMovementPosition` also takes the engine's `isCrusher` arm (`isCrusher && isObstacleFence`) at
  every call site (computePath passes `CrusherLevel > 0`, `clientSafeQuickDoesPathExist` `false`).
  **Walls as layers** are ported: `addWallPiece`/
  `removeWallPiece`, the wall cell layer (four-corner classification, one-cell edge tightening) and
  wall-layer paths at the wall height for units on a wall (`Iron Dragon` has 116 pieces). Unit
  avoidance is ported (`checkForMovement`'s ally/enemy tests with the relationship table,
  `blockedByAlly` cells, `moveAllies` + `getMoveAwayFromPath` pushing idle allies aside, with the
  ZH vetoes — attacking, using an ability or busy allies are left alone — and the mover's *own*
  ignored obstacle, and the `isLinePassable` fixed-unit test); `privateMoveAwayFromUnit` records the
  two avoided ids before the search and retries once with `setCanPathThroughUnits(true)` (left set);
  `patchPath` is wired through `AIUpdateInterface::computePath`
  when the unit is blocked-and-stuck. `checkDestination` group tightening (`tightenPath`/
  `checkPathCost`) and `findAttackPath` (with the `isAttackViewBlockedByObstacle` obstacle pass) are
  ported; fences and `KINDOF_MINE` exclusions are modelled. The **async pathfind queue + per-frame
  cell budget** are ported (`queueForPath`/`processPathfindQueue`, `PATHFIND_CELLS_PER_FRAME`)
  and `GameLogic::process_pathfind_queue` services them after the sleepy updates in engine order;
  the fine A*'s neighbour expansion hard-aborts on a fixed enemy like the engine
  (`AIPathfind.cpp:6332`), and the AIUpdate movement machinery (`zr-sim::ai`: `requestPath` /
  `requestAttackPath` / `requestApproachPath`, `AIInternalMoveToState`'s waiting/repath,
  `processCollision` blocked-and-stuck detection, `patchPath`) keeps blocked units moving.
  `findSafePath`/aircraft paths remain. `Player::onStructureConstructionComplete` re-classifies the
  finished structure's footprint (`removeObjectFromPathfindMap` + `addObjectToPathfindMap`,
  `Player.cpp:1640-1641`): the touched cells end `UNINITIALIZED_ZONE`, which is what the C++ leaves
  until the next `calculateZones` (AN3's GLA supply stash at f1121). A `PathfindCell` carries the
  engine's flag word (`CellFlags`: `NO_UNITS`/`UNIT_GOAL`/`UNIT_PRESENT_MOVING`/
  `UNIT_PRESENT_FIXED`/`UNIT_GOAL_OTHER_MOVING`) derived from `setPosUnit`/`setGoalUnit`
  (`AIPathfind.cpp:1420-1531`, including the "a unit already on its own goal keeps the cell" rule), so
  `UNIT_PRESENT_FIXED` means "standing on its own goal", not "not moving"; `checkDestination` blocks
  on it and `checkForMovement` treats the other present states as transient (`:5081-5086`,
  `:5146-5159`). `updateGoal` dispatches on `isDoingGroundMovement`: an air unit takes
  `updateAircraftGoal` and its separate aircraft goal marks (`:9816-9827`, `:9905-9911`), and
  `doPathfind`'s failed attack path runs `adjustToPossibleDestination` on the victim's position before
  `computePath` (`AIUpdate.cpp:450-457`). The build tests' `CollideInfo` pair procs are literal ports
  (`PartitionManager.cpp:479-486`: rect vs a *squared* circle, `:552-601` corner-in-rect,
  `:699-724` sphere-sphere 3D), which fixed the dock approach ring's borderline samples.
  Reduced: `computePath`'s `!m_isFinalGoal && isLinePassable`
  quick-path shortcut (`AIUpdate.cpp:1705-1709`) is not ported, so the `m_ignoreObstacleID` it leaves
  set (it is only cleared after the search, `:1755`) never leaks in the port — it is always cleared;
  `moveAllies` computes each ally's move-away path itself and `GameLogic` applies it next frame
  instead of calling the ally's `aiMoveAwayFromUnit` (so a `moveAllies` push has no ids/retry);
  the per-endpoint
  `getLayerForDestination` in `clientSafeQuickDoesPathExist` (unobservable here: walls have no ground
  `connectLayer` and bridges are a ground overlay).
- Weapons, damage and armor (`zr-sim::{weapon,armor,damage,combat,projectile}`): weapon state machine,
  radius damage, armor scalars, the attack command (chase, aim, fire, driven by the `AI_ATTACK_*`
  states of `zr-sim::ai_states`), idle auto-acquire, turret
  yaw + pitch, and real `ProjectileObject`s: `DumbProjectileBehavior`'s Bezier ballistic arc with
  collision en route and `MissileAIUpdate`'s launch/ignition/attack/out-of-fuel states homing on a
  moving victim. The **`TurretAI` state machine** (`zr-sim::turret_ai`) is ported:
  `IDLE -> IDLESCAN -> HOLD -> RECENTER -> IDLE` plus `AIM`/`FIRE` with the machine's sleep, the
  idle-scan draws (`resetIdleScan`'s interval, the scan angle and sign), `TurretAI.cpp:1299`'s mood
  reset, the `outOfWeaponRangeObject -> AIM` fire condition, and the AI aim state's delegation
  (a turret that can turn aims and fires through its own machine; `choose_weapon` deprioritises a
  weapon whose turret is aiming at the victim). Not modelled: `AltTurret` (the 4 battleship
  templates), the `addTargeter`/`isTemporarilyPreventingAimSuccess` aim veto, the bridge attack
  points and the view-blocked test in the aim state, `ContinueAttackRange`'s retarget, and
  `FireWeaponPower`'s `setTurretTargetPosition`. Crusader vs Battlemaster with real data ends in 375 frames (the shells launch
  from the model's `WeaponLaunchBone` and fly the real arc). Weapon bonuses and barrels are done
  (see below); `MissileCallsOnDie` is still to do (the projectile stream draw is done, see the projectile streams bullet
  below). Missile exhaust
  (`MissileAIUpdate::doIgnitionState` / `tossExhaust`, `Weapon.cpp` per-veterancy `ProjectileExhaust`):
  the sim picks the level's system at fire time (`combat.rs`, `ProjectileSnapshot::exhaust` / `exhaust_lit`),
  and `fx.rs` `sync_projectile_fx` attaches it at ignition and destroys it on toss or removal. Dumb shells
  take the parameter and drop it in C++ (`DumbProjectileBehavior.cpp`), so they get none.
  Missile and shell physics (`combat.rs` `step_missile` / `dumb_physics`, tests `missile_physics.rs`):
  the cruise height and dive (`PreferredHeight`, `PreferredHeightDamping`, `DistanceToTargetBeforeDiving`:
  `Locomotor.cpp:1935-1943`, `MissileAIUpdate.cpp:551-574`), the braking slowdown to `MinSpeed`
  (`Locomotor.cpp:81-90, 1917-1936`; `AuroraBomb` is the only shipped braking missile), the `KILL` detonation gated
  by `CloseEnoughDist` (`doKillState`'s `isIdle`, a 3D distance for a projectile, AIUpdate.cpp:2468-2472, with the
  bounding-sphere gap of both radii), and the launch heading from the barrel bone's rest rotation
  (`W3DModelDraw::validateWeaponBarrelInfo`, 208 of 379 retail barrels turn; the rocket buggy's about 27 degrees).
  Update order is by sleepy phase, not module order (`UpdateModule.h:80-87`): a missile's AI and drive (PHASE_INITIAL)
  run before its physics (PHASE_PHYSICS); a shell's physics always runs before its own flight step (PHASE_NORMAL,
  `DumbProjectileBehavior.cpp:579-715`), which then sets the absolute path position, so the launcher's velocity moves
  the landing only. A shell faces its step (`:643-683`; step 0 uses path[1]-path[0]), unless `TumbleRandomly`
  (`:381-387`), which draws three rates at launch and turns the shell each physics frame. Its friction is the
  PhysicsBehavior INI values (all 31 shipped shells use the 0.15/0.15/0 defaults). A shell with no physics has no
  ground test. Missile speed is `getForwardSpeed3D` (per-axis, `PhysicsUpdate.cpp:988-1000`); the braking distance is
  the flight distance (2D, `computeFlightDistToGoal`). Turret art (`TurretArtAngle`/`Pitch`) is applied to the
  default state's barrels (no shipped state has both a launch bone and art angles; the 24 art-angle fields are on
  default states of modules without launch bones there). Water surface for the cruise
  height (`getSurfaceHtAtPt`, `Locomotor.cpp:2028-2040`). The missile's AI move goal
  (`AIMoveToState::update`, `AIStates.cpp:2085-2125`) lifts a goal object to its half height and leads a moving,
  non-immobile goal; the `KILL` arrival and the drive both use that goal. A projectile detonates positionally at its
  own position (`DumbProjectileBehavior.cpp:531-539`), not at the object hit; a `Dumb` shell's flight path ends at the
  victim's geometry centre (`DumbProjectileBehavior.cpp:355-360`). `DAMAGE_KILL_GARRISONED` (`ActiveBody.cpp:445-478`) and
  the `GarrisonHitKill*` projectile clears (`DumbProjectileBehavior.cpp:485-515`, `MissileAIUpdate.cpp:333-364`) are
  ported, with `ImmuneToClearBuildingAttacks` carried in `ContainTemplate` and checked at every C++ read site (the
  damage cases, the projectile blocks, `ActiveBody::estimateDamage` and the `AllowAttackGarrisonedBldgs` weapon-estimate
  branch). Launch bones are resolved per model condition state with `findBestInfo` (the object's default conditions pick
  the rest state) and selected at fire time from the object's model conditions (`LaunchBones::select`), so the 22 objects
  whose first state is `ConditionState = NONE` get bones. A contact weapon's `FireFX` plays at its target when there is no
  `WeaponFireFXBone` (`Weapon.cpp:963`, viewer `fx.rs`). No shipped projectile sets `ALLOW_BOUNCE`; bridge layers are
  not modelled.
- Projectile streams, laser beams and tracers: `zr-sim::projectile_stream` ports
  `ProjectileStreamUpdate` (the 20-slot id ring, target-change holes, cull, `considerDying`, roof-skimmed
  `getAllPoints`) fed by `Weapon::newProjectileFired` (one stream per weapon slot); `zr-sim::laser_beam` keeps
  `Weapon::createLaser`'s beams (source, `LaserBoneName`, the fixed end, the lifetime rolls in template module
  order, `LifetimeUpdate` kills and `DeletionUpdate` destroys, removed at the earlier frame). Both are sim records,
  not `Object`s: they, and the projectiles, take their ids from the shared `allocateObjectID` counter at the
  original's creation points (Weapon.cpp:1105, 2515, 2543) so later ids match; they are CRC-hashed only while they
  exist. The laser branch (Weapon.cpp:1035-1057) deals its damage the same frame (victim on a hit, `INVALID` on a
  miss at the scattered point) and has no delayed damage. Known divergences, documented not modelled: C++ streams and
  beams are `Object`s, so in C++ the non-`INERT` beams are seen by the `Team.cpp` trigger-area and `hasAnyObjects`
  checks (Team.cpp:1787, 1964-2212) and re-evaluate the script counts (Object.cpp:576) for their lifetime; here they
  are not. The viewer's tracer steps once per logic frame, like the engine's one draw per loop, and holds while
  paused. `zr-viewer::projectile_draw` draws them as
  `SegLineRendererClass` ribbons (`W3DProjectileStreamDraw.cpp`, `W3DLaserDraw.cpp`, `seglinerenderer.cpp`: the
  V-by-point tiling, the [0, 50] tile clamp, scroll, the arc and beam spread) and the `W3DTracerDraw` tracer
  (`TracerFXNugget::doFXPos`: the flight to the shot's target, the linear fade). The weapon FX event carries the
  target and the weapon speed (`FxEvent::target` / `weapon_speed`).
- Death pipeline (`zr-sim::{die,ocl}`): `Object::onDie` runs the typed die modules filtered by
  `DieMuxData` (`DestroyDie`, `InstantDeathBehavior`, `CreateObjectDie`, `FXListDie` and
  `SlowDeathBehavior` with its weighted `ProbabilityModifier` choice and sink/midpoint/destruction
  delays); OCLs execute `CreateObject`/`CreateDebris` (owner, offset, disposition RNG) and
  `FireWeapon`. A killed Battlemaster now lingers through its real SlowDeath and spawns its debris
  OCL. `DeliverPayload` is done (see special powers); `ApplyRandomForce` (physics force + yaw/roll/
  pitch rates) and `Attack` (weapon lock + `aiAttackPosition` for `NumberOfShots`) are done too.
  `UpgradeDie`, `SpecialPowerCompletionDie`, `DamDie`, `HeightDieUpdate`, `NeutronBlastBehavior`,
  the three slow-death subclasses (`HelicopterSlowDeathBehavior`, `BattleBusSlowDeathBehavior`,
  `NeutronMissileSlowDeathBehavior`) and the missile `MissileCallsOnDie` data are ported too
  (2026-10-04). `SlowDeathBehavior`'s fling physics is ported
  (`FlingForce`/`FlingPitch`, `setAllowToFall`/`setAllowBouncing`/extra friction, the
  `FLUNG_INTO_AIR` timer push-out); `CrushDie`'s visual crush state is still to do.
- AI mood-check timer (`zr-sim::mood`, `combat.rs::ai_scan`, `ai_states.rs` idle state):
  - `m_nextMoodCheckTime` / `m_randomlyOffsetMoodCheck` (`AIUpdate.cpp:4483-4515`, `:4574-4674`).
  - Idle entry resets it to `ForceIdleMSEC` (default 1 frame) with a random offset (`AIStates.cpp:1321`); `getNextMoodTarget` checks when due and schedules `MoodAttackCheckRate` on, with one `GameLogicRandomValue(-rate/2, rate/2)` offset; the idle update sleeps to the next check and restores its sleep offset only then (`AIStates.cpp:1400-1466`); the transport exit wakes an idle rider (`TransportContain.cpp:407`).
  - `setNextMoodCheckTime` (`AIUpdate.cpp:4506`) is the `AIGroup` stealth rule: a player move (`AIGroup.cpp:1759-1781`) or stop (`:2105-2127`) of a combat stealth unit that is neither stealthed nor detected, and does not auto-acquire while stealthed, delays the mood check by `StealthDelay + GameLogicRandomValue(0, 30)` with no offset; a script order (`CMD_FROM_SCRIPT`) and a scatter (a direct `aiMoveToPosition`) skip it.
  - `markAsDetected` (`StealthUpdate.cpp:866-932`) now does its reveal wake: with `OrderIdleEnemiesToAttackMeUponReveal`, every enemy object whose own vision range reaches the revealed unit is woken (`setWakeupIfInRange`, `:837-862`) and, if idle, checks for a target at once.
  - `isIdle` is the literal check (`AIUpdate.cpp:3120-3128`: `AI_IDLE` or the state's `isIdle`, which only `AIIdleState` overrides, `AIStateMachine.h:228` — a sub-machine's idle state does not count), plus the `JetAIUpdate`/`HackInternetAIUpdate` pending-command vetoes; `isAttacking` includes a guard with an active attack (`AIGuard.h:144,194`).
  - The early returns before the timer are `getNextMoodTarget`'s (dead, `IS_USING_ABILITY`, `NOTWHILEATTACKING` while attacking, and for the idle caller `AAS_Idle` and the stealth rule); the team victim, the container check (after the timer, as C++ findClosestEnemy), and the passive AI answering its last attacker (`MM_Mood_Passive`, AIUpdate.cpp:4626) follow; `calledByAI` false (the attack-move's post-attack scan) skips the team victim, the timer and the human-only flags.
  - The attack-move's sub-machine `AI_IDLE` entries (a fresh entry, its exit, and each attack's end: `AIAttackMoveToState::onEnter`/`onExit`, `AIStates.cpp:3584-3601`, and `AIIdleState::onEnter`, `:1313`) reset the timer and draw the sleep offset (`ai_states.rs::ai_attack_sub_idle_enter`); the scan-to-attack hand-off draws nothing (`AIStates.cpp:3655`).
  - The attack-follow machine (`AIAttackFollowWaypointPathState`) does the same at its own entries — ctor `initDefaultState` (`:4392`, counted in the creation idles), `onEnter` (`:4447`), an attack's end (its `AI_IDLE` transition), and `onExit` (`:4520`) — and its post-attack scan is `getNextMoodTarget(false, false)` (`:4493`, `ai_attack_follow_resume`); the aim/pursue states also run their incidental turret scan while the attack is in its initial approach (`getNextMoodTarget(true, false)` + `setTurretTargetObject`, `AIStates.cpp:2783-2794`, `:3108-3123`, `combat.rs::ai_attack_incidental_turret_scan`), and an attack order writes the AI's last command source like every other `private*` order (`AIUpdate.cpp:3442`, `:3460`, `:3577`), as does the attack-move's own pickup (`AIStates.cpp:3658`, restored at `:3631`).
  - The idle update skips mood targeting while paralyzed, unmanned, EMP, subdued or hacked (AIStates.cpp:1432). The hunt and guard scans keep their own gates. A moving unit's scan is not withheld (no C++ gate; the old one was a leftover of the pre-`isIdle` workaround).
  - The turret idle reset (`TurretAI.cpp:1299`) runs on every turret `IDLE` entry since the turret state machine was ported (`zr-sim::turret_ai`).
  - Not ported: `AIAttackPickUpCrateState`'s scan (the port never creates crates to pick up) and the `m_isInitialApproach` re-arming of a *second* pursuit in the same attack (the port keeps one flag per attack).
- AIGroup move orders (`zr-sim::{player_orders,orders}`, `AIGroup.cpp:1591-1974`, `:514-551`, `:2327-2559`): `groupMoveToPosition` for `MSG_DO_MOVETO`, `MSG_DO_FORCEMOVETO` and `MSG_ADD_WAYPOINT` (near-to-far order, first-unit centre, `KINDOF_IMMOBILE`/`DISABLED_HELD`/AI filters, `clampWaypointPosition` with the aircraft and helicopter margins, the `groupTightenToPosition` path for a player click inside the group bounds, each unit's destination at the layer height); attack-move, attack-area and guard hand the clicked point to every member unchanged. Not ported: the infantry/vehicle column moves and formations (see ROADMAP).
- `RailroadBehavior` (`zr-sim::railroad`, `RailroadGuideAIUpdate.cpp`): the whole train.
  - A locomotive's first update (`IsLocomotive`) builds its `TrainTrack` from the map waypoints (`loadTrackData`, :498-634: the anchor is the nearest waypoint, then each waypoint's link 0; a point's flags come from the *target* waypoint's name suffix `Tunnel`/`Station`/`Disembark` and the *source* waypoint's `PingPong`, the handle is the source waypoint, a link back to the anchor makes it looping), then hitches the free allied carriages with a `RailroadBehavior` in reach behind it (`hitchNewCarriagebyProximity`, following the chain), or creates its `CarriageTemplateName` list front to back, each produced by the locomotive and hitched; a missing template or a carriage without the module ends the list. Carriages are created at the origin, as C++ leaves an unpositioned `Thing`.
  - Every update (:670-851) runs the conductor's state machine (`Accelerate`: `speed += 0.02 * direction; speed *= Acceleration`, clamped to `±SpeedMax`; `ApplyBrakes`/`WaitAtStation` for a station, from `WaitAtStationTime`), advances the lead car's `trackDistance` (wrapped only on a looping track), places every car by `FindPosByPathDistance` (:1377-1510) `2 * its majorRadius` behind its puller, rotates it towards its puller's tow point (`updatePositionTrackDistance`, :1255-1341, the engine's `Rz * Translate` matrix composition), snaps its z to the ground under its rear hitch point unless the point is a tunnel/bridge, and pulls the carriages recursively (`getPulled`, :1170-1218).
  - The module's first update is one frame after creation (`PhysicsBehavior::setWakeFrame(now + 1)`), so carriages created during a frame first run on the next.
  - `m_waitingInWings` only hides a car on a non-looping track (`setDrawableHidden(m_waitingInWings || m_endOfLine)`, :834-837); a looping track never hides it and wraps a negative distance instead.
  - The state is per object in `GameLogic::railroad_states` (the C++ module's state; not in the port CRC, as `RailroadBehavior::crc` hashes nothing either).
  - AN3 seed 1: every train object matches the C++ snapshot at every sampled frame (0..25200, worst position delta 0.0008 units) and the C++ per-frame state trace (speed, track distance, tow point, position, angle; 0.001 units / 1.5e-5 rad).
  - Not ported: `disembark` (:859-880), the `m_held` script flag, the audio events and the `OVER_WATER` drawable condition (client-only), and the `xfer` save/load path.
  - The ally test uses the port's `relationship` for the two owners.
  - `RailroadBehavior` also *is* the train's `CollideModule` (`onCollide`, :210-420; `isRailroad`, :188-205): a locomotive at or above `KillSpeedMin` kills what it hits (two `GameLogicRandomValueReal(-0.03, 0.03)` draws for the victim's pitch and roll), below it crushes with `DAMAGE_CRUSH` for `speed * 10`, and either way shoves the victim out of the way (`((usR+themR)-dist+1)/4`), lifts it to `ground+2` and throws it (`GameLogicRandomValueReal(0.05, speed/10)` in z, `setAllowToFall/Bouncing/AirborneFriction`, a yaw rate of `deviationCOG * -0.06 * speed`); FS_ power/factory/base-defense/technology/rebuild-hole structures and ready `DemoTrap`s are killed on contact; a train car only counts as a crash target at the ends of the train (`isRailroad`: the lead carriage or the last car), so a locomotive that rear-ends a stranded train kills that car, and a lead carriage that coasts into one kills both.
  - `RailroadBehavior` derives from `PhysicsBehavior` (`RailroadGuideAIUpdate.h:186`), so its INI block also carries the `PhysicsBehaviorModuleData` fields (`Mass`, `AllowBouncing`, ...) parsed into `Railroad::physics`; `Object::getPhysics()` stays null for a train, though (`Object.cpp:445-449` only fills `m_physics` for the module named `PhysicsBehavior`), and `RailroadBehavior::update` replaces `PhysicsBehavior::update`, so a train is insubstantial to other modules' physics and its own physics state is never integrated.
  - At a station the stopped train's whole footprint becomes a pathfinding obstacle (`makeAWallOutOfThisTrain`, :640-663, on `APPLY_BRAKES` -> `WAIT_AT_STATION` :713 and removed on the restart :733, via `Pathfinder::createAWallFromMyFootprint`/`removeWallFromMyFootprint`), and `m_waitAtStationTimer` is an `UnsignedInt` (`RailroadGuideAIUpdate.h:85`), so `WaitAtStationTime = 0` wraps and the locomotive never departs.
  - The collision pass mirrors the engine's COI rule that an object outside the partition grid has no cells and therefore no collisions (AN3's Alpine train runs at `x < 0`, where the C++ reports no train contacts). Neither build draws a random number for the train.
- ScoreKeeper (`zr-sim::score`, `Common/RTS/ScoreKeeper.cpp`, one per `Player`):
  - Built / lost / destroyed / captured counts (units and buildings, by template name and by victim player), money earned and spent, `calculateScore`, and `ENABLE_SCORING` / `DISABLE_SCORING` (`GameLogic::m_isScoringEnabled`, which gates the built, lost, destroyed and captured counts only).
  - Hooks at the C++ call sites: production and spawns (`onUnitCreated`), construction completion (build cost as money spent), `scoreTheKill` (lost and destroyed, the cash bounty), the Chinook combat-drop kills (`killEnemiesInContainer`), `Object::onCapture`, the crates, sabotage, cash hack / steal cash, hack internet, auto-deposit and supply-dock income, and finished upgrades (their cost).
  - Kill rewards follow `Object::scoreTheKill`: enemy, playable, not `IGNORED_IN_GUI`, not under construction; skill points use `SkillPointValue` (experience value at the `-999` default).
  - `Player::addSkillPoints` scales by the per-player modifier (`PLAYER_AFFECT_RECEIVING_EXPERIENCE`, rounded up) and caps at the highest allowed rank (`PLAYER_SET_RANKLEVELLIMIT`, default 1000).
  - Not ported: the battle-honour and medal stats in `ScoreScreen.cpp`, the isRebuild flag (rebuilds count as builds), the AI start-base command-centre removal (`onStructureUndone`), the instant-build paths (`buildObjectNow` without a dozer, `AIPlayer::buildStructureNow`, `ReplaceObjectUpgrade`: none exist in the port), the DumbProjectile garrison-clearing kills (`GarrisonHitKillCount` parsed, not simulated), two build-cost factors (`calcCostToBuild` applies `CostModifierUpgrade`; the player template's `ProductionCostChange` and handicap are 1.0), the save-game `xfer` (the CRC is empty in C++ too, so the score is not in the CRC).
- Production and money (`zr-sim::{player,production}`): `Money` deposit/withdraw, player starting
  cash in the sim, `Command::{QueueUnit,CancelUnit,SetRallyPoint}`, the `ProductionUpdate` queue
  (`MaxQueueEntries`, cost withdrawn at queue time, refund on cancel/death, `BuildTime * 30` frames,
  `QuantityModifier`), `Prerequisites` (`Object` OR groups; sciences skipped) and the exit updates
  (`Default`/`Queue`/`SupplyCenter`: create point, exit delay, rally move). A Command Center builds a
  Dozer (1000, 150 frames) in `production_demo`. Upgrade research in the queue, the low-power
  factor and dozer construction are done (below); handicap/multiple-factory modifiers and
  `MaxSimultaneousOfType` are still to do.
- Base economy loop (`zr-sim::{build,supply,energy,dozer,repair}`): dozer/worker construction
  (`Command::{Construct,CancelConstruct}`, legality, cost at placement, `UNDER_CONSTRUCTION` progress
  and cancel refund), supply trucks auto-cycling warehouses -> own supply centers
  (`MaxBoxes`/action delays/`ValuePerSupplyBox`, `DeleteWhenEmpty`), and power
  (`EnergyProduction` tally, low-power build-time factor). `economy_demo` builds a Power Plant and a
  Supply Center and harvests (money 6900 -> 13200). **The dozer/worker module's scheduling is the
  engine's** (2026-10-21): `DozerAIUpdate::update` is the `AIUpdateInterface` module, so the
  AI state machine, `doLocomotor` and the dozer's primary/action machines run in one update at one wake
  frame (`DozerAIUpdate.cpp:1619-1668`), and a new task's `PickActionPos` runs on the module's next
  update (`:2044`, `StateMachine.cpp:644-650`); a worker runs its supply machine in `AS_SUPPLY_TRUCK`
  and its dozer machine in `AS_DOZER` (`WorkerAIUpdate.cpp:271-309`). Not modelled: the fortify /
  going-home primary states, the mine-clearing branch, and the repair task's own `PickActionPos` walk.
  **Dozer/worker repair and resume** are done
  (2026-10-07): `ActionManager::canRepairObject` / `canResumeConstructionOf`, `Command::Repair` /
  `Command::ResumeConstruction`, `RepairHealthPercentPerSecond` healing (a second dozer can resume a
  scaffold whose builder died/left), and the `DozerPrimaryIdleState` idle auto-repair
  (`BoredTime`/`BoredRange`, `AIDozerBoredRadiusModifier` for AI players). `repair_demo` exercises
  all of it on real data (structure repair, resume, idle auto-repair, GLA worker, AI base repair).
  Line-build, supply upgrades and power sabotage are still to do (the Chinook combat-drop path is now
  done, see above).
- Locomotors and physics (`zr-sim::{locomotor,physics}`): the appearance kernels (legs/wheels/treads,
  hover, thrust, wings with min speed + circling, climber), `handleBehaviorZ` (surface-relative /
  absolute preferred height), all eight `Locomotor = SET_x` sets with a stable `set_locomotor_set` (a real switch
  draws the ctor RNG, the AI chooser included), the body damage state (`*Damaged` speed/turn/acceleration/lift from
  the GameData `MovementPenaltyDamageState`, default `BODY_REALLYDAMAGED`, `locomotor_now`),
  `OBJECT_STATUS_AIRBORNE_TARGET` after each `doLocomotor` (read by countermeasures, the jet idle order and the
  anti-air victim mask),
  and `PhysicsBehavior` (mass/velocity/gravity, `applyForce`, `AllowBouncing` with `GroundStiffness`,
  `KillWhenRestingOnGround`). Air locomotors ignore the ground pathfinder; `air_demo` flies a Comanche
  and Raptor across Alpine at altitude 100. **Motive-force ground physics** is ported: ground
  locomotors with a `PhysicsBehavior` apply `mass * acceleration` along their facing vector
  (`applyMotiveForce` + `MOTIVE_FRAMES`, the `isMotive()` lateral-only force filter), and
  `PhysicsBehavior::update` integrates gravity, forward/lateral/Z/aerodynamic friction, the tiny-
  velocity clamp, the `OBJECT_STATUS_BRAKING` z-only step, ground slam/bounce and the
  `HAS_PITCHROLLYAW` angular-rate rotation (yaw/pitch/roll are all applied now). Wheels port the
  `MOVING_BACKWARDS`/`DOING_THREE_POINT_TURN`/donut-timer behaviour and the impassable-terrain
  projection (`Locomotor.cpp:1368-1432`: more than 15 degrees off, half a second of travel is
  projected along the turn and an illegal terrain cell there turns the unit in place instead of
  driving). `moveTowardsPositionClimb` keeps the engine's persistent `CLIMBING` flag (set past one
  pathfind cell of height difference, cleared under one unit, `:1732-1737`), reads the ground height one
  unit toward the goal for the slope slow-down and the "step down, back up" case (`:1743-1761`, reversed
  desired angle `:1773-1776`, negated actual speed `:1795-1797`). A locomotor-set change rebuilds the
  `Locomotor`s (`AIUpdate.cpp:830-847`), which resets their flags (`IS_BRAKING`, `MOVING_BACKWARDS`,
  `DOING_THREE_POINT_TURN`, `CLIMBING`), `m_brakingFactor` and the donut timer. The donut timer is
  re-armed at all four engine sites (ctor `:676`, `startMove` `:782-786`,
  `maintainCurrentPosition` `:2441`, wheels kernel `:1426-1432`) and the persistent
  `Locomotor::m_brakingFactor` (`:445`, `:966`, `:1218-1231`, `:1255`, `:1417-1441`, capped at
  `MAX_BRAKING_FACTOR` 5.0) is per object; both are in the physics CRC. `locoUpdate_maintainCurrentPosition`
  dispatches by appearance (`:2480-2503`): HOVER brakes with a motive force (`:2567-2597`), TWrust/wings keep
  moving, and every ground appearance (two-legs, climber, wheels, motorcycle, treads, other) takes
  `maintainCurrentPositionOther` = `scrubVelocity2D(0)` (`:2601-2608`, `Locomotor.h:382-384`), so a move
  that ends stops dead instead of coasting; the return value is the engine's `requiresConstantCalling`.
  `doLocomotor` only maintains
  position for the `NONE` goal (it does nothing while a `POSITION_ON_PATH` move waits for its path,
  `AIUpdate.cpp:2214-2228`). `computeQuickPath` builds the engine's two-node `Path` (start + destination,
  `AIUpdate.cpp:1676-1721`), so a unit leaving production is visible to other units' `getMoveAwayFromPath`,
  and `privateMoveAwayFromUnit` runs the temporary `AI_MOVE_OUT_OF_THE_WAY` state (`:3333-3341`,
  `AIStates.cpp:2149-2174`) rather than a bare path in `AI_IDLE`. `applyForce` uses the engine's `1/mass`
  reciprocal (no mass clamp).
  **Sleepy physics** is
  ported: a module's first update is creation frame + 1 (`setWakeFrame(UPDATE_SLEEP_NONE)` in the
  constructor), `calcSleepTime` sleeps only when at rest, not spinning, not motive, on
  `LAYER_GROUND` (no bridge deck), not above the terrain and not overlapping (the crusher's
  `m_currentOverlap`/`m_previousOverlap`, reset each update), and an external force wakes a sleeping
  module for the next frame. Turn pivots are applied on every `rotateTowardsPosition` path
  (`locoUpdate_moveTowardsAngle`, legs/climb through it, wheels/treads/other), the pivot move goes
  through `Object::set_position` so the partition follows it, and the very-close-to-goal early return
  reports `TURN_NONE`. The **`THRUST` 3D kernel** is
  ported (`calcDirectionToApplyThrust`, `tryToRotateVector3D`, `tryToOrientInThisDirection3D`,
  `MaxThrustAngle`, `moveTowardsPositionThrust`, `maintainCurrentPositionThrust`) and thrust units
  are force-driven. Terrain-normal slope tilt and the suspension pitch/roll/wobble are a client
  `Drawable::calcPhysicsXform*` feature, not simulated (out of the sim's lane); the viewer ports it
  (`zr-viewer/src/physics_xform.rs`: treads, wheels, motorcycle, hover/wings and thrust, the
  wheel steering angle and the suspension height offsets).
- Object-object collisions (`zr-sim::collide`, new): `PartitionManager`'s contact list reduced to a
  partition scan; dirty non-immobile objects collide (`PartitionData::collidesWith`'s z + geometry
  test), both directions get `Object::onCollide`, and contact objects re-dirty. The `Collide`
  modules are ported: `PhysicsBehavior::onCollide` (push-apart + `checkForOverlapCollision` crush,
  structure bounce via `StructureStiffness`), `SquishCollide`, `FireWeaponCollide` (`FireOnce`,
  `AFLAME` status) and every shipped `CrateCollide` subclass (`Money`/`Heal`/`Veterancy`/`Unit`/
  `Salvage`/`Shroud`/`ConvertToCarBomb`/`ConvertToHijackedVehicle`/the eight `Sabotage*`). The
  sim's kinematic hover/wings skip the collide force (their kernels position-step; the engine's air
  is force-driven). `collide_demo` (GLA vs China) picks up a `SalvageCrate` (weapon-set upgrade) and
  a money crate and crushes infantry. Reduced: `HijackerUpdate`'s in-vehicle ride consumes the
  hijacker; `CrushDie`'s partial-crush points kill outright. The enter-type abilities
  (hijack / carbomb / sabotage) are now ordered through `Command::Enter` and their `ActionManager`
  checks are ported (`zr-sim::enter_actions`), so weaponless saboteurs no longer need the attack
  path (whose goal gate required a weapon).
- 2026-10-29: AI base defenses are checked at the base centre's z = 0
  (`AISkirmishPlayer.cpp:688`, ground height added in `buildStructureWithDozer`, `AIPlayer.cpp:530`). OpenContain exits
  (`zr-sim::contain_exit`): tunnel/spawn, AIExit passengers, heal and garrison burst exits go through
  `exitObjectViaDoor` (OpenContain.cpp:1013-1125: ExitStart/ExitEnd bones with the `m_whichExitPath` cycle, the idle-vehicle
  step, one second of ignored collisions, `adjustToPossibleDestination`, the `[end, end, rally]` path) and the transport's
  exit-busy window (TransportContain.cpp:410, :649-656). `GenerateMinefieldBehavior` and `HordeUpdate` are sleepy-heap
  entries; `HordeUpdate` has its ctor draw (HordeUpdate.cpp:169), the 3D bounding-sphere scan over the `FASTER_GCO` cells,
  the ALLIES/exact/KindOf/HordeUpdate filter, the rub-off pass and the AI-gated morale bonus. The queue production exit
  snaps its natural point (QueueProductionExitUpdate.cpp:130-132). Treads read the forward speed after the turn
  (Locomotor.cpp:1185, :1207); `locoUpdate_moveTowardsPosition`'s blocked branch (:1036-1058); `blockedBy` with
  `canCrushOrSquish`, the state-machine-goal near-goal test (AIUpdate.cpp:1292-1303) and `hasHigherPathPriority`;
  `processCollision`'s moving branch with `needToRotate` and `calculateMaxBlockedSpeed`; contacts from the geometry test.
  `AIUpdateInterface::update` sleeps for the least of the state, turret and locomotor sleeps (AIUpdate.cpp:1090-1116), and
  `AIFollowPathState::onEnter` adjusts a plain path's first point (AIStates.cpp:3276-3310).
- 2026-11-04: `AssistedTargetingUpdate` (`zr-sim::assist`): a heap slot from the creation
  frame (the constructor's FOREVER wake is compiled out in Zero Hour, `AssistedTargetingUpdate.cpp:72-75`,
  `RETAIL_COMPATIBLE_CRC`), and the weapon assist (`Weapon::processRequestAssistance`, Weapon.cpp:2593, :2960-3031: the
  player's team walk in engine order, equivalence by template name, `FROM_CENTER_2D` range, `isFreeToAssist`, `assistAttack`'s
  lock and clip-size shot limit, the two feedback lasers as beam records starting at the assistant, LaserUpdate.cpp:295-313).
  `AIUpdateInterface::wakeUpNow` (AIUpdate.cpp:973-988) as `wake_up_now`: pulls only a module asleep past next frame, a no-op
  inside its own update, called from the AI machine's `setState`/`clear` notify (AIStates.cpp:1017-1058), the `doPathfind`
  tail, `markAsDead`, the move-away and the order paths; `setQueueForPathTime`'s pull (:955-968); the attack sub-machine
  (a plain `StateMachine`) does not notify. Attack exits follow the sub-state: an approach/pursue exit ends its move
  (`friend_endingMove`, the completed-move block drops path, goal and request, AIUpdate.cpp:1035-1041), aim/fire exits leave
  it, a new move cancels the block (:2034-2036); the guard's attack entry keeps the unit moving. `canCrushOrSquish` in
  `checkDestination` and `checkForMovement` (AIPathfind.cpp:5082-5086, :5201-5205); the search limit counts only cells
  without `PathfindCellInfo` (:6344-6349).
- 2026-11-10: `Object::defect` (`defect_l193.rs`, Object.cpp:6239-6370: the
  under-construction/sold guards, production refund, rider kick-out, the one-frame undetected-defector flag) with
  `Object::getRelationship`'s defector rules (Object.cpp:1595-1613) read by `SpecialAbilityUpdate` and weapon targeting, and
  team (not player) colleague tests; `CashBountyPower`'s three hooks (`cash_bounty_l193.rs`, CashBountyPower.cpp:129-181,
  Player.cpp:2590-2600); map and starting objects run every create module's `onBuildComplete` (GameLogic.cpp:1946-1953);
  `AssaultTransportAIUpdate` (`assault_transport.rs`: the DEPLOY shot's `beginAssault`, the member list, riders' `aiExit`,
  `isAttackPointless`, retrieve, `giveFinalOrders`, the `aiDoCommand` resets at the dispatch point, `isAllowedToChase`);
  `WWMath::Inv_Sqrt` bit-exact under `setFPMode`'s 24-bit precision (`vector_math.rs`, wwmath.h:614-660) for
  `Vector3::Normalize`, the factory exit point (`getNaturalRallyPoint`) and `normalized_cross`; `m_isAttackPath` with every
  set/clear site, `computeAttackPath`'s in-range and airborne arms, the approach's `m_stopIfInRange = !isAttackPath` and the
  contact-weapon success (AIStates.cpp:2673-2750); the attack range read at compute time; `AIAttackState::onEnter` on every
  entry path with no stop on a command or a pickup (AIUpdate.cpp:3426-3446); `DeployStyleAIUpdate`'s never-sleep return;
  the partition box/cylinder contact tests and the nonsphere closeEnough preflight (PartitionManager.cpp:479-693); the
  snapshot's `ai`/`atk` print the enclosing state (GameLogicSnapshot.cpp:263-309).
- Possession (`zr-sim::possession`, feature `possession`), see Phase 4.
- Skirmish AI phase 1 (now only the fallback for maps without scripts; retail skirmish is
  script-driven, see the script engine below) (`zr-templates::ai_data`, `zr-sim::skirmish_ai`): typed `AIData` (12 `SideInfo`
  gatherer counts, 12 `SkirmishBuildList`s / 214 structures, global tunables), and a deterministic
  `AISkirmishPlayer`-derived player that dozer-builds its rotated build list (power first when
  underpowered, rebuild delay), keeps `ResourceGatherersNormal` supply trucks, and every
  `TeamSeconds` sends a small group from its war factory at the nearest enemy structure. `zr-game`
  marks skirmish AI slots and builds their configs at start. `skirmish_ai_demo` (Alpine, China vs
  America) is deterministic. Phase-1 bridges (the script layer isn't ported): the shipped lists mark
  structures `AutomaticallyBuild = No` (retail map scripts enable them) so the AI builds the whole
  list when no scripts load, and the omitted supply center is inserted after the command center.
  Team prototypes/priorities, reinforcements and base defense are still to do.
- Script engine (`zr-map::scripts::parse_scripts_file`, `zr-sim::script`): the `TheScriptEngine`
  port — per-side lists run in side order with the side's player as "this player", free scripts then
  active non-subroutine groups, `active`/one-shot/easy-normal-hard/`EvaluationInterval`, OR-of-ANDs,
  true/false actions, counters (countdown timers), flags, named timers,
  `ENABLE_SCRIPT`/`DISABLE_SCRIPT`, `CALL_SUBROUTINE`, named objects and object-type lists. `zr-game`
  loads the map's scripts or the shared `SkirmishScripts.scb` (14 lists / 3957 scripts / 1340 teams)
  matched by faction plus `MultiplayerScripts.scb`; the skirmish AI is script-driven (builds only
  `buildSpecificAIBuilding`/`buildBySupplies`/base-defense orders, whole-list fallback otherwise).
  Phase 2 adds teams (`TeamPrototype`/`TeamInstance`, per-slot min/max, owner, generic/on-create
  scripts run every frame with `<This Team>`, AI `selectTeamToBuild`/`queueUnits`/recruitment and
  production-priority feedback), the `TEAM_*` order actions, attack-priority sets, trigger-area
  conditions with the `[Skirmish]` perimeter rewrite, skirmish value/supply/tech conditions,
  `<This Player's Enemy>`, and `VictoryConditions` (last skirmish player standing wins). Phase 3
  makes the waves fight: `AIHuntState`/`AIAttackAreaState` (`command_hunt`/`command_attack_area`,
  whole-map acquisition that ignores fog for AI players and targets enemy buildings), the
  `TEAM_EXECUTE_SEQUENTIAL_SCRIPT`/`<This Team>`/subroutine fixes, `TeamPrototype::
  evaluateProductionCondition` (difficulty gating), `SKIRMISH_FIRE_SPECIAL_POWER_AT_MOST_COST`,
  `MULTIPLAYER_*`, the team `OnEnemySighted`/`OnAllClear`/`OnIdle`/`OnDestroyed` scripts, and the
  skirmish `VICTORY_NOBUILDINGS` rule. Scripts are **per-player qualified**
  (`Parameter::qualify`/`duplicateAndQualify`): each skirmish player gets a duplicated list and team
  copies with the qualifier (`m_mpStartIndex`) appended to SCRIPT/COUNTER/FLAG/SCRIPT_SUBROUTINE/
  TEAM names, so counters/flags are not shared. GLA's economy is fixed (its worker is the harvester;
  `ChinookAIUpdate`/`WorkerAIUpdate` are supply trucks; `UNLIMITED_REBUILDS` rebuilds; blocked
  rebuild spots try offsets). Supply is now faithful: `buildBySupplies` treats the parameter as a
  warehouse-supply threshold and builds near a `KINDOF_SUPPLY_SOURCE` warehouse, the phase-1
  inserted supply center / whole-list build / attack group are disabled when scripts drive the AI
  (kept only for script-less maps), `ai_build_teams` builds any team whose
  `evaluateProductionCondition` passes (`IsAIRecruitable` only gates unit stealing),
  `teamGenericScriptHook0` is read, and AI gatherers use `2 *` the warehouse scan distance
  (`SupplyTruckAIUpdate::getWarehouseScanDistance`), which is what stopped the economies stalling.
  `skirmish_ai_demo [fa fb [easy|hard]]` reaches a winner for **all twelve Easy/Normal/Hard Alpine
  matchups** (6–25 min), driven only by `SkirmishScripts.scb`, deterministic. Hard used to stall
  because teams were activated the moment they were selected: on Hard the wave production conditions
  are all true early, so every team was created empty and `maxInstances` blocked it, and production
  stopped. Ported `AIPlayer`'s `TeamBuildQueue`/`TeamReadyQueue` (`selectTeamToBuild`, `queueUnits`,
  `checkQueuedTeams`, `checkReadyTeams`): a selected team is built (produced/recruited) while
  inactive and only activated once full (or at `teamInitialIdleFrames`), and `findFactory` now picks
  the factory named in the unit's `Prerequisites` (infantry from the barracks, not the command
  center). Implemented team actions `TEAM_CAPTURE_NEAREST_UNOWNED_FACTION_UNIT`,
  `TEAM_USE_COMMANDBUTTON_ABILITY`/`TEAM_PARTIAL_USE_COMMANDBUTTON`, `TEAM_LOAD_TRANSPORTS`,
  `SKIRMISH_FOLLOW_APPROACH_PATH`/`SKIRMISH_MOVE_TO_APPROACH_PATH` (waypoint path labels + links),
  `TEAM_GARRISON_NEAREST_BUILDING` and `SET_ATTACK_PRIORITY_KIND_OF`. Every condition/action used
  by `SkirmishScripts.scb` + `MultiplayerScripts.scb` is now implemented: `UNIT_HEALTH`,
  `SUPPLY_SOURCE_SAFE`, `PLAYER_TRIGGERED/COMPLETED_SPECIAL_POWER`, `TEAM_*_HAVE_OBJECT_STATUS`,
  `VICTORY/DEFEAT/LOCALDEFEAT`, `PLAYER_SELL_EVERYTHING`, `TEAM_SET_ATTITUDE`,
  `TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_OBJECTTYPE`, `TEAM_GUARD_IN_TUNNEL_NETWORK`,
  `TEAM_EXIT_ALL` (evacuate) — unimplemented runtime invocations dropped from ~15.8k to 0. The
  engine's `AISkirmishPlayer::acquireEnemy` (closest enemy every 5 s, anti-ganging) drives
  `<This Player's Enemy>`. 4-player FFA now finishes several maps/sets (`VictoryConditions::update`
  + `Player::killPlayer` ported). The player default teams exist from start-up
  (`PlayerList::newGame` -> `TeamFactory::initFromSides` for every singleton prototype, then
  `Player::setDefaultTeam` activates `team` + player name), with the C++ team ids. Object→team
  membership is ported (2026-10-21): `Object::m_team` on every object, the prepend-ordered
  `Team::m_TeamMemberList`, map objects on the team their `originalOwner` names
  (`PlayerList::validateTeam`, `TeamFactory::findTeam`), produced/spawned/created objects on their
  owner's default team, script/AI teams through `Object::setTeam`/`setTemporaryTeam` (controlling
  player, initial attitude, attack-priority set, `onCapture`), `Team::transferUnitsTo`/`disband`,
  `TEAM_STOP_AND_DISBAND` (re-idle + merge into the default team), the team create script in
  `Team::updateState`'s pass (`ScriptEngine::update`), `AIPlayer::checkReadyTeams`' `joinTeam`, and
  `Team::tryToRecruit` reading the object's team (default team always recruitable, other teams by
  `ProductionPriority`/`IsAIRecruitable`/per-team `setRecruitable`). Still to do: the remaining
  economy/attack-deadlock sets, `TEAM_SET_ATTITUDE`'s auto-acquire gate, the `AITNGuard` AI, and
  campaign scripting.

- 2026-10-22: the `AIAttackMoveStateMachine` sub-machine and its `AI_IDLE` entries
  (AIStates.cpp:1993-2000, 3584-3601); `privateIdle`'s passenger loop; `Pathfinder::adjustToPossibleDestination`
  (AIPathfind.cpp:5672) for the dozer/worker end point (workers keep END = START, WorkerAIUpdate.cpp:650-654); object vs
  player upgrades through `queueUpgrade` (ProductionUpdate.cpp:257-321, completion to the object, `RequiresAllTriggers`,
  the `PLAYER_BUILT_UPGRADE*` events); every `Object::doCommandButton*` branch for team script actions (Object.cpp:5503-5830);
  the temporary/permanent weapon lock in the weapon set (WeaponSet.cpp:1078-1133) with the 11 `releaseWeaponLockForGroup`
  dispatch sites and the reload release; the stored `ActiveBody` damage state (ActiveBody.cpp:901-930, 1217-1237); the full
  `getVictimAntiMask` (WeaponSet.cpp:389-433); `isSignificantlyAboveTerrain`; `setGoalPositionClipped` for player moves;
  the `isMobile` order gates; `canMakeUnit`'s factory checks in `queue_unit` (the unit cap is not ported); INI-only
  `StickToGround`; spawn creation before exit (SpawnBehavior.cpp:635). Reduced: the sleepy heap holds the AI modules, the
  seven object helpers and the train, not yet the other ~550 C++ entries (added later, see the next entries).

- 2026-10-22, later: the sleepy heap runs module families dispatched with the whole `GameLogic`
  (`sleepy_families.rs`): AutoHeal, Flammable, Poisoned, SlowDeath/JetSlowDeath, Topple, Stealth, StealthDetector,
  Production, Physics (PHASE_PHYSICS; the `update_physics` id walk is gone) and GarrisonContain, each with its C++ ctor wake,
  update sleep and wake sites, the disabled-types gate (GameLogic.cpp:3851-3885), destroy-time erase in heap order
  (GameLogic.cpp:2578-2609), and registration in the template's INI behaviour order (`update_module_order`, Object.cpp:320-410);
  AutoHeal activation is an upgrade event (UpgradeModule.cpp:111-150), healing goes to `onHealing` (poison cure). The object
  helpers do their work: `FiringTracker` (continuous fire MEAN/FAST bonus and model conditions, PerAttack pre-fire count),
  `TempWeaponBonusHelper`, `StatusDamageHelper` (DAMAGE_STATUS, applied in the weapon damage loop), `SubdualDamageHelper` +
  `ActiveBody` subdual damage (`DISABLED_SUBDUED`); turrets pre-fire on entering fire (`Object::preFireCurrentWeapon`).
  `AIUpdateInterface::update` order (completed-move block before turrets and `doLocomotor`, AIUpdate.cpp:1035-1114), pop to
  goal, the pathfinder goal cells (`updateGoal`/`removeGoal`, `snapClosestGoalPosition`, the ally test in `checkDestination`,
  `computeIndividualDestination`), the AI supply-truck and dozer orders as `TeamInQueue`s (AIPlayer.cpp:223-416, 3189-3236).
  Viewer: tracer timing/start (`W3DTracerDraw`), retail mouse scheme incl. Alternate Mouse and double-click guard
  (`CommandXlat.cpp`, `SelectionXlat.cpp`).

- 2026-10-23: the rest of the AN3 sleepy heap. Families for `TechBuildingBehavior` (captured model
  condition, capture/die wake), `DefaultProductionExitUpdate` / `SupplyCenterProductionExitUpdate` / `SpawnPointProductionExitUpdate`
  (asleep forever) and `QueueProductionExitUpdate` (its delay ticks in its own slot, no longer inside `ProductionUpdate`),
  `PowerPlantUpdate` (asleep until `extendRods`), `AutoDepositUpdate` (the ctor's `m_depositOnFrame` and the per-frame
  deposit), `BaseRegenerateUpdate` (ctor wake, `onDamage` delay, the 3-frame heal, `DISABLED_UNDERPOWERED`),
  `SupplyWarehouseDockUpdate` / `SupplyCenterDockUpdate` / `RepairDockUpdate` (the `DockUpdate::update` entry grant;
  `update_docks` is gone), `RadarUpdate`, `SpawnBehavior` (the 15-frame countdown, `update_spawners` is gone),
  `SpectreGunshipDeploymentUpdate`, `HealContain` (the per-frame heal and the ejection at `TimeForFullHeal`; the old
  `update_contain_heal` sweep is gone and `TransportContain`'s `HealthRegen%PerSec` is healed at the engine's rate) and
  `BridgeBehavior`. At f1163 the heap is 1357 entries like C++ (all classes, pairs and keys except the seven civilian cars'
  AI sleep). `ProductionExitClass` / `DockClass` carry the C++ class name for the heap dump. Still open: the heap array
  order, `PowerPlantUpdate`'s `RodsExtendTime` and `TechBuildingBehavior`'s `PulseFX` (adapter fields), `OverchargeBehavior`
  as a family.

- 2026-10-27: the RMB/damage retaliation assist and the horde morale bonus. `Command::EnableRetaliationMode`
  (replay tag 40, format version 3) sets `Player::m_logicalRetaliationModeEnabled` for the message's own player
  (`GameLogicDispatch.cpp:1082-1108`, default off `Player.cpp:498`); `ActiveBody::attemptDamage`'s tail runs the
  `shouldRetaliateAgainstAggressor` / `shouldRetaliate` filters and orders the idle allies within
  `RetaliationFriendsRadius` + the hit object's bounding circle to guard-retaliate (`ActiveBody.cpp:688-792`; the
  `AIGuardRetaliateMachine` is ported, see below). The viewer posts the
  mode once a second when `Retaliation` differs (`Player.cpp:704-714`). `AIUpdateInterface::evaluateMoraleBonus`'s classic
  action (HORDE/NATIONALISM/FANATICISM from `AllowedNationalism` + `Upgrade_Nationalism`/`Upgrade_Fanaticism`,
  `AIUpdate.cpp:4741-4824`) runs from `HordeUpdate`'s refresh instead of `weapon_bonus_flags` deriving the conditions from
  the player's upgrade. `JetSlowDeathBehavior` now runs the base `SlowDeathBehavior` INITIAL phase (its FX/OCL draws)
  before its own initial FX/OCL (`JetSlowDeathBehavior.cpp:180-193`).

- 2026-10-27: the stealth/attack leftovers. `StealthUpgrade` (`StealthUpgrade.cpp:51-70`) is an
  `UpgradeEffect::Stealth` module: the first activation sets `OBJECT_STATUS_CAN_STEALTH` (and gives it to the slaves of a
  `KINDOF_SPAWNS_ARE_THE_WEAPONS` spawner, `SpawnBehavior::giveSlavesStealthUpgrade`), which the awake GLA camouflage
  `StealthUpdate` reads in `allowedToStealth`. That function's `STEALTH_NOT_WHILE_MOVING` now tests the physics velocity
  against the module's `MoveThresholdSpeed` (`StealthUpdate.cpp:399-402`) instead of the AI move order, and its
  `NOT_WHILE_ATTACKING` / `NOT_WHILE_FIRING_WEAPON` tests `OBJECT_STATUS_IS_FIRING_WEAPON` (`:283`, `:340-397`), which the
  attack sub-machine now sets in `AIAttackFireWeaponState::onEnter` and clears in its `onExit`/`AIAttackState::onExit`
  (`AIStates.cpp:5203`, `:5347`, `:5727`). Attack orders record the engine's command source (see the mood bullet at the
  top of this list); not ported yet: `STEALTH_ONLY_WITH_BLACK_MARKET`, `NOT_WHILE_TAKING_DAMAGE`,
  `NOT_WHILE_RIDERS_ATTACKING`, `NOT_WHILE_USING_ABILITY`, the garrisonable-container test and `SCRIPT_UNSTEALTHED`.
- 2026-10-28: the A*'s dozer exception for a friendly
  obstacle (`AIPathfind.cpp:6303-6317`); `internalFindPath`'s zone screen, the construction-completion footprint
  re-classification (`Player.cpp:1640-1641`), the `PathfindCell` flag model from posUnit/goalUnit (`:1420-1531`), the
  aircraft goal arm (`updateAircraftGoal`) and the `CollideInfo` pair procs with the ring sample's angle
  (`PartitionManager.cpp:479-724, 3880, 5267-5296`); `requestPath` leaves the locomotor goal until `computePath` installs a
  path (`AIUpdate.cpp:480-521, 1740-1745`); the `ObjectHelper` ctor sleep, the creation-time mood reset, the Deletion /
  DynamicShroud heap slots and the ramp, Overcharge/PowerPlant families, creation-time ctor draws in INI module order
  (`Object.cpp:300-491`), AutoDeposit capture bonus, TechBuilding pulse, spawn slave notifications, the defection timer;
  the subdual/status/shotFired/strafe/FireWeaponPower fixes; the stealth-detector, goal-bookkeeping, ULTRA_ACCURATE
  and topple fixes; the jet slow death's base INITIAL phase. Viewer: the global particle cap, field cap and dynamic LOD
  (`ParticleSys.cpp:1867-1921, 3554-3576`).
- 2026-10-28, later: the `AI_DOCK` machine's `halt` (no sub-state `onExit`, no
  owner-machine lock, `StateMachine.cpp:745-755`, `AIDock.cpp:70-90, 515`) and `SupplyTruckAIUpdate::privateIdle`'s busy latch
  (not for workers, `WorkerAIUpdate.cpp:531-540`); `AIInternalMoveToState`'s finished-path semantics (the path and goal are
  kept, a start-only `Path` counts, `doLocomotor` obeys the goal type, `AIStates.cpp:1786-1943`, `AIUpdate.cpp:1035-1061,
  2161-2286`) with the guard-return and enter-state move exits; supply trucks and Chinooks run `SupplyTruckAIUpdate` inside
  their own AI module (Chinook work → supply → base AI, `ChinookAIUpdate.cpp:1078-1230`); the AI heap slot's
  `DISABLED_HELD` gate (`AIUpdate.h:305`); heap slots for `CommandButtonHuntUpdate`, `SpecialAbilityUpdate`,
  `AutoFindHealingUpdate`, `DemoTrapUpdate`, `CleanupHazardUpdate`, `ProneUpdate` and one per contain class (all 13
  `Object/Contain/` classes typed; Overlord/Helix/MobNexus/Garrison payloads in `onObjectCreated`), with duplicate INI kinds
  registered in INI order; `OBJECT_STATUS_NO_ATTACK` in `isAbleToAttack`; `AIEnterState`'s enemy-attack fallback and
  contained-airborne bail; the AI build-site spiral's skirmish double increment (`AIPlayer.cpp:563-566`);
  `AIGuardRetaliateMachine` (new `ai_guard_retaliate.rs`, `AIGuardRetaliate.cpp`, `AIStates.cpp:6798-6936`; never
  `isAttacking()`). Client: launch and fire-FX/recoil/muzzle bones over every `W3DModelDraw` module (frame-0 pristine pose,
  `AttachToBoneInAnotherModule` pivots, the map's `NIGHT`/`SNOW`), every extra draw module drawn (attach offsets,
  `MinLODRequired`, `OkToChangeModelColor`, construction lift, dependency riders), and the class-packed terrain atlas
  (`updateTileTexturePositions`, `getUVForNdx`, two-pass base + blend; the 3-way pass is an opaque per-cell composite).
- 2026-10-28 to 2026-10-29: movement — an air move's quick path inside `requestPath`
  before the anti-spin guard, `computeQuickPath`'s early return and `destroyPath`, `getAircraftPath` (wings clip and
  `KINDOF_AIRCRAFT_PATH_AROUND` detours), the hover/wings kernel's pre-turn force direction, aircraft destinations through
  `adjustDestination`/`checkDestination`'s aircraft branch with `isAircraftThatAdjustsDestination` from the current
  locomotor, `NO_SLOW_DOWN_AS_APPROACHING_DEST`, `doPathfind`'s `m_queueForPathFrame` waiting tail (`AIUpdate.cpp:463-465`),
  the C++ locomotor-goal setters, the weapon-set rebuild's PRIMARY reset; the exact hierarchical search (above); skirmish
  AI — `isLocationSafe` through the partition (same-frame objects invisible until the end-of-frame partition update),
  `AIPlayer::buildUpgrade`'s object-upgrade refusal, base-defense placement along `Center/Flank/Backdoor<n>`, the cluster-mine
  superweapon target; script engine — code reachable from an action uses the engine passed in (the engine is taken out of
  `GameLogic` while scripts run), so script-fired special-power events, script-created team members and their owner are
  immediate as in C++; upgrades — `RadarUpgrade` (player radar counts, `RadarUpdate` extend, disable-proof, brownout),
  `GrantScienceUpgrade`, `CostModifierUpgrade` with `calcCostToBuild` at every money site, `PassengersFireUpgrade`,
  `ReplaceObjectUpgrade`, `SubObjectsUpgrade` (carried), and `GrantUpgradeCreate` (`onCreate`/`onBuildComplete`).
  Tooling: `tools/zr_compare.py`, `timeline.py`, the `zr-refs` reference store, `ZR_TRACE` on both sides, `ZR_HPATH`.

Still to port:

Port `GameLogic` (~187k lines of C++ in `GeneralsMD/Code/GameEngine/Source/GameLogic`):
- Module system: every GameLogic module instance is now typed (`ModuleData::Logic` and the
  extended families); 2782 client draw/update instances remain `Unparsed` but their data is handled
  by `zr-w3d` / the viewer. Open gaps are listed in the roadmap. Containment (`OpenContain` / `TransportContain` /
  `GarrisonContain`: `Enter`/`Exit`/`Evacuate`, slot capacity, kind-of and relationship rules,
  damage-on-death, contained units hidden from targeting) is done; `PassengersAllowedToFire`
  passengers fire from the container, and enclosing garrisons assign each passenger to the free
  model `FIREPOINT` bone closest to its target (`GarrisonContain::matchObjectsToGarrisonPoints`,
  pristine/damaged/really-damaged arrays resolved by `zr-game` from the W3D art). Non-enclosing
  garrisons (firebases) keep passengers on the model `STATION` bones (`loadStationGarrisonPoints` /
  `positionObjectsAtStationGarrisonPoints`), `PassengersInTurret` containers fire from their turret's
  `FIREPOINT` bones, `WeaponBonusPassedToPassengers` passes the container's bonus flags to its
  riders, `KINDOF_NO_GARRISON` and really-damaged garrisons reject/eject occupants, and a captured
  transport ejects its riders (`TransportContain::onCapture`; garrisons keep theirs). `RiderChangeContain` (GLA
  Combat Cycle) is ported 2026-10-09: the rider sets the bike's `WEAPONSET_RIDERn` / `STATUS_RIDERn` / model condition /
  locomotor and `CommandSet` override (kept after exit, as in C++), a riderless bike scuttles, and the bike stealths
  through its rider (`UseRiderStealth`, `receiveGrant`). `TunnelContain`/`TunnelTracker`
  share one per-player passenger list (capacity from `MaxTunnelCapacity`, healing, last-tunnel
  cave-in, enter-at-one/leave-through-another). Of 196 enclosing garrisons, 32 have no resolvable
  `FIREPOINT` bones; that is a shipped art bug (the bones do not start at `01` and both the engine
  and the sim stop at the first missing bone), so the centre fallback matches retail.
- Pathfinding leftovers: `findSafePath` / aircraft paths (the sim has no repulsor requests).
  `moveAllies`/`getMoveAwayFromPath`, `patchPath` (now wired), `checkDestination` group tightening
  (`tightenPath`/`checkPathCost`) and `findAttackPath` are done (see above); the async pathfind
  queue + per-frame cell budget (`queueForPath`/`processPathfindQueue`) is now done and the
  `AIStateMachine` (`zr-sim::ai_states`: `AI_IDLE`, `AI_MOVE_TO`/`AIInternalMoveToState`,
  `AIMoveOutOfTheWay`, `AIMoveAndTighten`, the `AI_ATTACK_*` sub-machine, `AI_ATTACK_MOVE_TO`,
  `AI_HUNT`, `AI_ATTACK_AREA`, `AI_GUARD`/`AIGuardMachine`, `AI_ENTER`/`AI_EXIT`,
  `AI_FACE_*`, primitives) now owns `requestPath`/`requestAttackPath`/`requestApproachPath`,
  `AIInternalMoveToState`, `processCollision`, `patchPath` and `doLocomotor`, run once per object per
  frame by `GameLogic::update_ai_state_machines`; `zr-sim::ai` keeps the movement kernels and the
  pathfinder seams. `AIMoveOutOfTheWay` is the real temporary state (`privateMoveAwayFromUnit`,
  `AIUpdate.cpp:3210-3345`: the move-away `Path`, then `AIMoveOutOfTheWayState::onEnter`'s existing-path
  `computePath` override, `AIStates.cpp:2149-2174`) and `moveAllies`' pending pushes carry the `Path`.
  Income modules skip neutral-controlled objects as the engine does (`AutoDepositUpdate.cpp:161-162`). The attack sub-machine is the real `AttackStateMachine` (`AIAttackPursueTargetState`
  / `AIAttackApproachTargetState` / `AIAttackAimAtTargetState` / `AIAttackFireWeaponState` with
  `AttackExitConditions`); the guard attack-aggressor retaliate is ported (clearable last attacker +
  `AIGuardAttackAggressorState`); `AI_FOLLOW_PATH`/`AI_FOLLOW_WAYPOINT_PATH*`,
  `AI_WANDER_IN_PLACE`/`AI_PANIC`, the production exits `AI_MOVE_AND_EVACUATE`/`AI_MOVE_AND_DELETE`
  and `AI_DOCK`'s state machine are ported; idle AI sleeps (`UPDATE_SLEEP_FOREVER`/`wakeUpNow`).
  The `AIUpdateInterface` module's creation wake frame (`setWakeFrame(now + 1)`, `AIUpdate.cpp:299-301`) gates the first
  update of every AI-driven pass, and `AIIdleState::doInitIdleState` (`AIStates.cpp:1334-1385`) does its start-up cheat
  move: an idle ground unit at frame <= 1 moves to `Pathfinder::goalPosition` (`AIPathfind.cpp:5303`), which is how the
  map's civilian cars leave their `.map` position by one cell corner (`adjustCoordToCell`, `:9127`). The goal-cell
  bookkeeping (`Pathfinder::updateGoal`/`removeGoal` and `AIUpdateInterface::m_pathfindGoalCell`) is not kept: the cell is
  derived from the object's position at the one place that needs it.
  Reduced: `AI_RAPPEL_INTO` (the Chinook keeps its own port), `AI_ATTACK_SQUAD` (no `Squad`),
  the follow-waypoint *attack-follow* variant, and the `AI_DOCK` module wiring (supply / repair /
  airfield still drive their own reduced docking). Walls as
  layers and `findHierarchicalPath` were already done, as were zones, bridges and bridge
  damage/repair. The fine A*'s neighbour expansion now hard-aborts on a fixed enemy like the
  engine. Motive-force ground physics is now done (see the locomotors bullet); terrain-normal slope
  tilt is a client `Drawable` feature, not simulated. Object-object collisions and the `Collide`
  module families are now done (see the collide bullet above).
- Modules simulated by the sweeps: stealth/detection, auto-heal, flammable, poison, lifetime,
  deletion (`DeletionUpdate`: destroys after its rolled lifetime, e.g. the spy-satellite ping),
  fire-weapon-when-dead, crush/topple, crates (all shipped `CrateCollide` subclasses), pilot eject, veterancy-gain, spawners/slaves,
  hordes, income (`AutoDepositUpdate`), base regeneration, propaganda towers, repair docks, heal
  containers, overcharge, minefield detonation, prone, transition damage FX, keep-object die,
  model-condition upgrades and a (since replaced, see above) reduced jet/airfield cycle. Round 4 (`zr-sim::modules4`): GLA
  rebuild holes (`RebuildHoleBehavior`/`RebuildHoleExposeDie`: hole health, worker respawn,
  reconstruction, `TransferAttackers`), aircraft countermeasures (volleys, reaction/reload,
  `EvasionRate` diverting `KINDOF_SMALL_MISSILE` projectiles to flares), `DemoTrapUpdate`
  proximity detonation, `CleanupHazardUpdate` (`KINDOF_CLEANUP_HAZARD` targeting),
  `CommandButtonHuntUpdate` and `GenerateMinefieldBehavior` (real geometry); `JetSlowDeathBehavior` lives with the
  die modules (`zr-sim::die`: on-ground vs in-air death, jet timers, hit-ground/final blow-up).
- Upgrades and progression (`zr-sim::{upgrade,experience,bonus}`): object/player upgrade sets,
  `ArmorUpgrade`/`WeaponSetUpgrade`/`MaxHealthUpgrade` (set selection by condition specificity),
  `ExperienceTracker` veterancy (kill points, `ExperienceValue`/`ExperienceRequired`, promotions,
  `HealthBonus_<Level>` max-health scaling), research cost/time/refund through the production queue,
  and the global `GameData` `WeaponBonus` set plus per-weapon entries applied to
  DAMAGE/RADIUS/RANGE/RATE_OF_FIRE/PRE_ATTACK (VETERAN/ELITE/HERO/PLAYER_UPGRADE/GARRISONED,
  NATIONALISM/FANATICISM), Horde counting and `LocomotorSetUpgrade`. NATIONALISM/FANATICISM come from
  `AIUpdateInterface::evaluateMoraleBonus`'s classic action (HORDE, `AllowedNationalism`,
  `Upgrade_Nationalism`/`Upgrade_Fanaticism`), which `HordeUpdate` evaluates on each horde refresh;
  they are not derived from the player's upgrade directly. The die classes are now ported
  (see the death-pipeline bullet).
- Special powers (`zr-sim::{special_power,science}`): `SpecialPower` / `Science` / `Rank` templates,
  `Command::DoSpecialPower` with per-object and shared cooldowns and `RequiredScience` gating,
  `OCLSpecialPower` (OCL at the target) / `FireWeaponPower` / `CashHackSpecialPower`, science
  purchase points and general's promotions from kill skill points. `DeliverPayload` flies the
  transport with its air locomotor, drops the payload (paratroopers descend under a parachute,
  bombs detonate through their die OCL) and strafes for self-delivering jets (A-10); the Daisy
  Cutter and A-10 damage their targets. `SpecialAbilityUpdate` runs the approach / preparation /
  effect state machine: Black Lotus capture-building (change owner) / disable-vehicle / steal-cash,
  Jarmen Kell's `DAMAGE_KILL_PILOT` (unmanned neutral vehicle), Burton charges, hacker cash; ALT_FIRE
  routes through it. `LocomotorSetUpgrade` swaps to `SET_NORMAL_UPGRADED`. The other power modules
  are now ported too (`zr-sim::superweapon`): `CashBountyPower` (passive + `doBountyForKill`),
  `CleanupAreaPower`, `BaikonurLaunchPower`, `SpyVisionSpecialPower`/`SpyVisionUpdate` (reveals the
  owner's shroud through enemy `SpyOnKindof` units), the `ParticleUplinkCannonUpdate` moving-beam
  swath (orbit-to-target, S-curve or manual drive, width grow/decay, scorch marks, damage pulses,
  remnant objects, reveal), the `SpectreGunship` orbit insertion + gattling strafe + howitzer
  follow-up, the `NeutronMissileUpdate` launch/attack flight and the `NeutronBlastBehavior`
  infantry/vehicle wipe, and `MissileLauncherBuildingUpdate`'s door model conditions
  (2026-10-04). `Defector`/`Demoralize` are absent from the shipped data; the pack/unpack/facing polish
  remains. Line-build / supply upgrades / power sabotage; production queues, construction, repair/resume
  and the base economy loop are done
- AI players: skirmish AI and the script engine are done (build list, gatherers, map/`.scb` scripts
  driving building and teams, script teams with AI recruitment and orders, hunt/attack-area, trigger
  areas and `VICTORY_NOBUILDINGS`; per-player qualified scripts/teams). Their orders run through the
  real `AIStateMachine` (`zr-sim::ai_states`, `AI_HUNT`/`AI_ATTACK_AREA`/`AI_GUARD` and the move/attack
  states). Ported 2026-10-05: `AI_DOCK` (`zr-sim::dock`: `DockUpdate`/`AIDockMachine`, supply and repair docks,
  model dock bones), `SupplyTruckStateMachine`, the `AIPlayer` gatherer model (`queueSupplyTruck`,
  `findSupplyCenter`/`buildBySupplies`, `onUnitProduced`), team work orders (`queueUnits`/`startTraining`),
  `buildSpecificAITeam`, `tryToRecruit`, `TeamInQueue::disband`, team generic hook scripts
  (`Team::updateGenericScripts`), `CALL_SUBROUTINE` over group scripts, the sequential-script engine
  (`zr-sim::script::sequential`), retail `Team::updateState` idle / enemy-sighted, team attitude and mood,
  attack-priority sets, hunt re-scan during attack, `PartitionFilterRejectBuildings`, guard inner/outer
  ranges, and orders to contained passengers. Base-defense placement, AI use of the superweapon powers,
  team abilities and campaign scripting are still to port
- **2026-10-30:**
  - Object transform: `Thing::m_transform` is `Object::transform` (`transform.rs`, Matrix3D in f32 C++ operation order).
    Pivot turns, physics yaw and `setAngles`, 3D turns and the railroad write it, and the heading is its readback.
  - State objects: a temporary move keeps its own move fields (`AIStateMachine.h:247-300`); the main's are live between
    frames and for every main-state change. The attack-move has its own `m_pathGoalPosition` (its retry test).
  - Locomotor: `locoUpdate_moveTowardsPosition` has no arrival step (the port's old snap is gone); blocked flow, `*blocked`
    output, `m_blockedFrames` clamp and `m_curMaxBlockedSpeed` reset; `handleBehaviorZ` for every kind.
  - Guard: timers (`m_guardEnemyScanRate`, `m_guardEnemyReturnScanRate`) and the `defineState` transitions of `AIGuard.cpp`.
  - Follow path: the leg's adjust flag, `ignoreObstacleID` clear on advance, last-point ground test.
  - Flight paths: `computeFlightDistToGoal`'s first node and the `m_cpopRecentStart` latch on steer and distance query.
  - Skirmish AI: recruit home and enemy distance use `m_baseCenter`, `hasHome` -> `aiIdle`; `RECRUIT_TEAM` moves to `m_homeLocation`.
  - `SpawnBehavior`: replacement list, one-shot, aggregate-health death, `onEnslave` draws on every exit and OCLs;
    `SlavedUpdate` as a sleepy family (leash only). `PowerPlantUpgrade` extends the rods.
  - Contain exits take the `RETAIL_COMPATIBLE_CRC=1` branches (the build the port is compared with); `allowToFall`.
- **2026-11-01 to 2026-11-02:**
  - Special abilities: `startFacing`, `AI_FACE_OBJECT`/`AI_FACE_POSITION` with `getRelativeAngle2D`, `aiBusy`,
    `IS_USING_ABILITY` with `endPreparation`, `resetDynamicPhysics`.
  - Target acquisition line of sight (`zr-sim::los`): `PartitionFilterLineOfSight`, `isClearLineOfSight` terrain march,
    `isAttackViewBlockedByObstacle` obstacle march, `AttackUsesLineOfSight`, `KINDOF_CAN_SEE_THROUGH_STRUCTURE`.
  - Partition contact pairs: `updateCellsTouched`'s COI fills, `addPossibleCollisions`, the 0.01 dirty thresholds and
    `REAL_TO_INT_CEIL`'s rounding of an exact integer. Range queries still use bounding circles.
  - Physics: `Rotate_X/Y/Z` rates in the transform, the bounce re-right, friction through `applyForce`, `getMass` with
    contained items at every caller, the HELD tail, the null `onCollide` landing path and `ParachuteContain`'s null branch.
  - Terrain: `TerrainLogic::flattenTerrain` + `setRawMapHeight` from dozer/worker builds and `LIKE_EXISTING` OCL structures.
  - Transcendentals (`zr-sim::crtmath`): the 32-bit UCRT `sin`/`cos` results (f32 equal over every exponent field tested);
    the table is generated from mathematics by `tools/gen_crtmath_table.py`. `atan2`/`asin`/`acos` are double libm rounded to f32.
  - Partition cash values: `doValueAffect` cells, `getNearestGroupWithValue`, `getMostValuableLocation` (re-derived at frame end, not event-driven).
  - Scripts and AI: `SKIRMISH_ATTACK_NEAREST_GROUP_WITH_VALUE`, `AI_PLAYER_BUILD_TYPE_NEAREST_TEAM`
    (`buildSpecificBuildingNearestTeam`), `buildStructureWithDozer`'s funds check, `WeaponSet::getAttackResult`'s
    `MASKED`/`UNATTACKABLE` victim tests, `privateIdle` at every old `stop_object_ai` caller.
  - Guard: idle sleep propagation, `attackAggressors` on IDLE/RETURN/INNER, the zeroed `m_guardeePos`.
  - Attack move: `forceRepath` on the retry wake and the resume, each state's own `m_pathGoalPosition`.
  - `SpawnBehavior::computeAggregateStates`, `onEnslave`'s master; `SlavedUpdate`'s pinned guard point and stray rule
    (`:255-278`, `:406-427`) from the slave's own module data. `ParkingPlaceBehavior` and the second `ProductionUpdate` in the heap.
- **2026-11-03:**
  - Movement: `m_isMoving` with `friend_startingMove`/`friend_endingMove` at every move state (approach and pursue entries
    included); `moveAllies` with `aiMoveAwayFromUnit` at each cell visit; `computePath`'s straight-line quick path; the
    step-aside's IMMOBILE test.
  - Temporary states: a temporary of the main state's own ID shares its state object (`internalGetState`,
    AIStates.cpp:919); other IDs keep their own fields.
  - Attack: the quarter-cell `UNDERSIZE` on the goal range (Weapon.cpp:469-487), `findAttackPath`'s vehicle strip-back
    and `ATTACK_CELL_LIMIT`, `canPursue` on the current weapon's turret, a stuck pursuer approaching, the approach
    refusals (computer vs airborne aircraft, a human's AI-commanded chase), the attack fallback's ignored obstacle.
  - Guard areas: polygon guards, the trigger-area clock, `PolygonTrigger`'s radius, the area guard's zero
    `m_positionToGuard` (AIGuard.cpp:168).
  - Partition contact pass in engine order; every `AutoHealBehavior` module of a template (a pilot snipe resets the first).
  - Chinook combat drop as `ChinookMoveToBldgState`; the DeliverPayload transport's state order (per-item drops, the exit
    move, removal off the bordered map, `DelayDeliveryMax`); a boneless dock returns the docker's position.

The Rust sim only needs to be deterministic against itself, not bit-compatible with the
retail binaries. To verify behaviour, run the same scripted scenarios in TheSuperHackers'
C++ build and in Rust, and compare outcomes such as unit counts, health and positions at
fixed frames.

## Phase 3 — Client

Done: `zr-viewer` renders Alpine Assault with textured terrain, real models, roads and bridges
(`zr-roads`), map lighting, team colours and the faded map border, and runs the live `zr-game`
skirmish at 30 Hz with interpolation: select / box-select, move, attack and attack-ground orders.
Water (`zr-water` + viewer `water.rs`): standing water and rivers are drawn textured, translucent and depth-faded over the terrain, with wobble / scroll animation; reflections, bump and sparkles are not ported.

UI groundwork (`zr-ui`):

- All 80 `Window\*.wnd` layouts parse into `Window` trees (1667 windows), 1324 `MappedImage`s with engine UV/rotated-size maths, and image → texture / text resolution; `Window\ControlBar.wnd` dumps as a tree.
- The in-game control bar is drawn (`zr-ui::command` + viewer `hud.rs`): `CommandButton`/`CommandSet` parse (820 buttons / 472 sets; 3107 slot refs, 0 missing) and `ControlBar.wnd` is laid out with `bevy_ui`, scaled from its 800×600 creation resolution, drawing window draw-data images from `MappedImage` UV sub-rects, the selected unit's command grid (icons + `GameText` labels, `&` hotkey markers stripped) and money / power readouts; `UNIT_BUILD` clicks submit `Command::QueueUnit`, the bar hides while possessing, and `--select-first-structure` frames it for screenshots.
- The minimap radar (`radar.rs`) samples the terrain (height/water/cliff) like `W3DRadar::buildTerrainTexture`, overlays shroud (`GameLogic::cell_shroud_status`), draws object dots in player colours and the camera view trapezoid, and left/right clicks move/order the selection.
- `placement.rs` gives a `DOZER_CONSTRUCT` ghost (the real model, tinted green/red by the now-public `GameLogic::is_location_legal_to_build`) committed with `Command::Construct`, and the same pending mode covers `ATTACK_MOVE`, `GUARD` and `SPECIAL_POWER` (`NEED_TARGET_POS` -> a ground radius-cursor ring); `zr-sim::orders` ports `AttackMove` / `Guard` / `Sell`; `SPECIAL_POWER` buttons get a cooldown wipe.
- The `ProductionQueueWindow` shows the selected factory's queue with progress (click cancels via `Command::CancelUnit`), and the right HUD shows the unit `SelectPortrait`.
- Animation is wired (`zr-w3d` `Hierarchy::pose_trs` + viewer `anim.rs`): a Bevy `SkinnedMesh` over per-pivot joint entities (W3D stores one bone per vertex), driven by the condition-state animation with LOOP/ONCE and state hide/show; condition flags now come from the simulation (`zr-sim::condition::ModelConditionFlags`, derived each frame from locomotor/weapon/body/construction/supply state and hashed into the object CRC) and the chosen state swaps the model too (damaged/rubble); `--spawn <template>` frames one animated instance and `--duel` runs a Crusader-vs-Battlemaster fight for screenshots.
- Effects are wired (`zr-fx` templates + viewer `fx.rs`): a CPU port of `Particle::update` / `ParticleSystem::update` (keyframed alpha/color, size rates, damping, gravity, bursts, the emission velocity/volume shapes) rendered as camera-facing textured billboards, played from `FXList`s, from the sim's per-frame `FxEvent`s (death, weapon `FireFX` at the muzzle, `ProjectileDetonationFX`) and from `--fx <FXListName>`; projectile models are drawn from `GameLogic::projectiles()` interpolated along velocity; terrain scorch is drawn (`scorch.rs`: the `W3DScorch` grid, alpha-blended and shaded by the terrain ambient/diffuse; tracers, rays, light pulses and sound also play).
- Particles obey the engine's global cap (`fx.rs` `ParticleManager`): one count and a per-`ParticlePriorityType` oldest-first list, removal from the lowest priority below the new particle, the `ALWAYS_RENDER` exemption, a cap of 0 refusing everything, the `m_maxFieldParticleCount` field-particle cap, and the dynamic-LOD priority floors / skip mask; the cap is the detail level's `MaxParticleCount` (`GameLOD.cpp:257`, `Custom` from `options.ini`) and the dynamic level follows the 30-frame average FPS (`W3DDisplay.cpp:1857-1866`, `GameLOD.ini`'s `DynamicGameLOD` blocks). Open work: see the roadmap.

Shell / playable skirmish (viewer `shell.rs` + `menu.rs`): with no arguments `zr-viewer` opens the skirmish setup screen rendered from the real `Window\Menus\SkirmishGameOptionsMenu.wnd` (background panels, labels and mapped images) with functional overlays — a scrollable map list built from the `MapCache` blocks (`INIMapCache.cpp`) with `.tga` previews, 8 player slots (human + AI with Easy/Normal/Hard difficulty, faction incl. the generals' `Side`s, `MultiplayerColor` colour, `GameInfo` team and start position), `MultiplayerStartingMoneyChoice` cash and the superweapon-limit toggle.

Start builds the chosen `PlayerSetup`s (extended in `zr-game` with difficulty/team/colour/cash; same-team slots become allies, difficulty reaches the sim through `ResourceGatherersEasy/Normal/Hard` and the per-player script flags), and the world is torn down and rebuilt cleanly for Restart / Quit to Setup.

Esc opens a pause overlay (Resume / Restart / Options / Quit) and `VictoryConditions::checkForGameEnd` drives a victory/defeat score screen. The score screen (`ScoreScreen.cpp` `grabMultiPlayerInfo` / `populatePlayerInfo`, skirmish) sorts its rows by each player's `ScoreKeeper::calculateScore` and fills the seven stat columns (units created / lost / destroyed, buildings constructed / lost / destroyed, money earned as "Supplies Collected") in the slot colour.

Any command-line argument still starts a game immediately (`--quick`), so the scripted/debug flags keep working.

Generals promotions (viewer `promotions.rs`): the control-bar rank/experience bar uses `ControlBarScheme`'s `ExpBarForegroundImage` and the player's `Rank` band (`Player::getSkillPointsLevelDown/Up`); `ControlBar.wnd:ButtonGeneral` opens the real `GeneralsExpPoints.wnd` purchase-science panel, whose `ButtonRank1/3/8NumberN` slots come from the `PlayerTemplate`'s `PurchaseScienceCommandSetRank1/3/8` and submit `Command::PurchaseScience` (enabled when the prerequisites are met and the points suffice).

The `SpecialPowerShortcut` bar (`SpecialPowerShortcutWinName` + `SpecialPowerShortcutCommandSet`) shows the sciences' powers with cooldown wipes.

Options (`options.rs`, `options/settings.rs`) render `OptionsMenu.wnd` from the draft of an ACCEPT / CANCEL session (`OptionsMenu.cpp`: nothing applies until ACCEPT), with the C++ `options.ini` keys and value formats (`OptionPreferences`, `saveOptions`), the detail presets from the install's `StaticGameLOD` blocks, and the SFX 2D/3D split; the detail level's particle cap and dynamic-LOD switch reach `fx.rs`, the `DynamicGameLOD` blocks come from the install's `GameLOD.ini`; the roadmap lists the controls that are stored without a viewer effect.

Replays (`zr-game::replay`): `--record` stores the command stream + setup/seed + periodic CRCs in a small versioned binary file in the user data dir, and `--replay` plays it back headless or in the viewer, reporting CRC desyncs; `zr-game/tests/replay.rs` records a 2-minute AI-vs-AI + human-commands run and asserts the replayed CRCs match.

Shroud / fog of war (`zr-sim::shroud` + viewer `shroud.rs`): a per-player partition-cell grid ports
`PartitionCell`'s look/shroud counters, the `DiscreteCircle` reveal and `PartitionData::getShroudedStatus`;
the viewer draws it as a per-cell texture multiplied over the terrain (`W3DShroud` / `W3DDisplay::setShroudLevel`)
using `GameData` `ShroudColor`/`ClearAlpha`/`FogAlpha`/`ShroudAlpha`, hides enemy objects fogged or shrouded
for the local player (`GameClient.cpp`, 2-second grace), and renders in RTS and possession cameras. Terrain-attached
geometry is shaded too (`terrain_shroud.rs`): roads and water multiply the local cell level into their per-vertex
colours, and bridges / props / trees swap to a darkened material while fogged and hide once fully shrouded
(`W3DPropBuffer::drawProps`, `W3DTreeBuffer`, `W3DBridgeBuffer`, `W3DWater::getRiverVertexDiffuse`). The idle
auto-acquire honours `PartitionFilterFreeOfFog`; explicit player attack commands remain authoritative.
Ghost objects are client-side (`W3DGhostObject` / `W3DRenderObjectSnapshot`): an enemy structure seen at
least once and then fogged is frozen at its last-seen transform, model and animation (a `GhostSnapshot`
marker that `sim_sync` / `animate_rigs` skip), survives its own death, and is released when the cells clear
or fall to full shroud (`PartitionData::getShroudedStatus` transitions). Not ported: spy vision,
stealth/disguise, `UnlookPersistDuration` from `GameData` (hardcoded 30 frames) and the map-border shroud
level.

Close-up visuals (`zr-viewer`): the time-of-day skybox (`W3DWater`'s `new_skybox`, `GameData`
`DrawSkyBox`/`SkyBoxScale`/`SkyBoxPositionZ`) follows the camera so the void beyond the map is sky;
W3D textures (model meshes, sky, roads) sample with the engine's `TextureFilter` mode, default BILINEAR with
point-mip (`GlobalData.cpp:943`, `texturefilter.cpp:125-274`; read from `options.ini` at launch), their wrap (REPEAT
unless `W3DTEXTURE_CLAMP_U/V`, `texture.cpp`) and their mip cap (`MIP_LEVELS_2/3/4`, NO_LOD = one level).
Every other texture family takes its own `TextureClass` settings through `convert::TextureUse`: particles,
lasers/streams and the water surface use the mode with all levels and REPEAT; bridges keep `MIP_LEVELS_3`;
the scorch decal follows `GameData`'s `BilinearTerrainTex`/`TrilinearTerrainTex` (`ScorchTextureClass::Apply`);
decal shadows clamp and disable mip mapping; radar, shell/UI images and the shroud overlay are `MIP_LEVELS_1`
(the shroud also clamps and disables the mip filter, `W3DShroud.cpp:241-257` with min/mag `FILTER_TYPE_DEFAULT`).
DDS textures keep the levels their file has minus the two lowest (`ddsfile.cpp:107-109`, one level when none
are left), for model meshes and the other families alike; TGA gets a generated chain. The `TextureReduction` option
(`GameLOD`/`options.ini`, read at launch) drops that many top levels off **DDS** file textures the way
`Get_Requested_Reduction`/`Apply_Mip_Reduction` do (no reduction for `MIP_LEVELS_1` or a side of 32 or less; TGA files
stay at full resolution, TheSuperHackers' `Begin_Uncompressed_Load` sets `Reduction = 0`, `textureloader.cpp:1602-1603`)
and starts the terrain atlas sampler at a coarser level (`TerrainTextureClass::setLOD`). The atlas layout is the
engine's (`TerrainTextureClass::update`): each block is a map cell's composited 32x32 quadrant surrounded by
`TILE_OFFSET`'s 4-texel border (`TerrainTex.h:38`) holding the class content just outside the cell - the adjacent
quadrants of the same tile - wrapped around the class block at the class edge exactly as the engine copies the block's
last 4 columns/rows around it (`TerrainTex.cpp:155-193`); the cell's UV rect covers the tile only, so the filter
footprint's extra texels are the border, and the atlas has the engine's three mip levels (`MIP_LEVELS_3`,
`TerrainTex.cpp:70-74`, box-filtered like `D3DXFilterTexture`, `:198`). Cells in different texture classes still meet
at a content edge (our blocks are per cell, the engine's per class), but no sample reaches an unrelated block. The
possession build adds anisotropic trilinear for non-alpha-tested model textures and the terrain atlas only
(POSSESSION.md); roads keep the plain-build sampler there;
construction sites' extra `W3DModelDraw` modules (scaffold, fence, crane; viewer `construct_modules.rs`) run their own
`setModelState`: `TransitionState`s between condition states, `WaitForStateToFinishIfPossible` / `AllowToFinish`, the
per-frame `ADJUST_HEIGHT_BY_CONSTRUCTION_PERCENT` lift, a random pick among a state's animations and
`RANDOMIZE_START_FRAME` (client RNG) (`W3DModelDraw.cpp:2027-2220, 2905-3173`); decal-typed
objects (`SHADOW_DECAL`, e.g. infantry) get a `W3DProjectedShadow`-style quad from `ShadowSizeX/Y` /
`ShadowTexture`, offset along the map's sun direction; and possession adds a subtle distance fog
(documented possession-only; the engine has no fog setting). Model-attached FX: `Turret`/`TurretPitch`
bones rotate from the sim's turret state; `WeaponMuzzleFlash` sub-objects flash on firing;
`ParticleSysBone` systems follow their bones (damaged smoke/fire, exhaust, dust); `Tracer`/`RayEffect`
beams and `LightPulse` nuggets play. `SHADOW_VOLUME` objects cast Bevy cascade shadow maps. The
terrain keeps the engine's baked `doTheLight` vertex lighting, drawn unlit through a small custom
material (`terrain.rs` + `terrain.wgsl`) that only darkens shadowed texels toward
`GlobalLighting::shadow_color`, so unshadowed terrain matches the earlier verified look. Weapon
bones (`W3DModelDraw` `ModelConditionInfo`): per-slot/-barrel `WeaponFireFXBone` (recoiled muzzle
`FireFX`), `WeaponRecoilBone` (the `handleClientRecoil` shift along the bone's local X),
`WeaponLaunchBone` (resolved at load into the sim, which computes the launch position from the
turret/barrel bones) and `FxListAtBonePos` are applied; `WeaponMuzzleFlash` sub-objects still match
by name rather than per barrel. Stencil shadow volumes are not applied.

Known visual issues: env/reflection (second) passes ignored (TODO); no cloud-shadow / lightmap
terrain passes. Alpha-test cut-outs fixed; the lavender on snow is the map's own fill lights.

Bevy-based renderer: terrain and blend tiles, W3D meshes/HLOD/animations, model condition states,
particles/FX, shroud/fog of war, skybox, decal + shadow-map shadows, model-attached FX, UI (`.wnd`), audio.

## Phase 4 — Possession

See [POSSESSION.md](POSSESSION.md). Third-person and first-person bone cameras are implemented.
Everything in this phase is behind the cargo feature `possession` (off by default, 2026-10-06), so
the default build stays the plain Zero Hour port.

- Done: sim side (`zr-sim::possession`: possess / drive / aim / fire / hitscan / auto-release, deterministic, `possess_duel` example). Ground, hover and thrust units steer with the world move vector; fixed-wing throttle/turn and cannot stop; air units change altitude with `ASCEND`/`DESCEND` (Space/Ctrl) within the locomotor envelope, hashed into the possession CRC.
- Done: client (`zr-viewer` `possess.rs`): P / E, third-person spring-arm camera (Renegade `ccamera.cpp` ratios, terrain + structure collision, over-the-shoulder offset, optional `T` auto-follow; the view direction is arm-independent, so pulling in near geometry never rotates the camera), `V` first-person bone cameras (turret / cockpit / head via `Hierarchy::find_bone`, own model hidden), a longer ground-free arm for aircraft, camera-relative WASD sent as `PossessInput` every logic frame, crosshair and HUD. RTS box-select excludes structures when units are in the box.
- Done: radial command menu (`zr-viewer` `radial.rs`, possession-only): hold `Q` / middle mouse for a ring of the possessed unit's `CommandSet` buttons (icons from the `CommandButton` `ButtonImage`), aim with mouse motion, release to dispatch through the control bar's `dispatch_button`; targeted abilities use the crosshair aim point.
- Remaining: the controller layout, client-side prediction, interactive testing. LOD
  needs no forcing: `zr-w3d` always assembles the highest HLOD LOD (`model.rs::top_lod`).

