# Roadmap

Goal: complete Zero Hour gameplay in Rust/Bevy, plus an optional possession mode (cargo feature `possession`, off by
default, so the default build is the plain port).

This file holds **current status and open work only**. What is already ported, subsystem by subsystem with the C++ it
follows, is in [PORTED.md](PORTED.md); history is in [CHANGELOG.md](../CHANGELOG.md). When an item here is done,
delete it or shrink it to what is left.

## Status (2026-11-11)

| Area | State |
|---|---|
| Data | 150/150 maps, 221/222 INI files, 2115 objects with 0 warnings. 16770 module instances are typed; the other 2782 are client draw/update modules read by `zr-w3d` and the viewer. |
| Skirmish | Playable from the real main menu against the script-driven AI (retail `SkirmishScripts.scb`). Replays record and play back without desync. A headless soak of every player command (6 configurations x 30,000 frames) runs with no panic and matching checksums. |
| Human vs AI | Against a passive human, every AI faction attacks and wins at Easy, Normal and Hard, deterministically. Attack timing is not yet compared with retail. |
| AI vs AI | 21 of 27 matrix games finish, all deterministic, with no unimplemented script condition. The count swings with every change, so it is not a progress measure. |
| Possession | Works for ground, hover and air units in third and first person, with alt-fire and the radial command menu. Not yet playtested by a person. |
| Checks | `cargo test --workspace` and `cargo clippy --workspace --all-targets` are clean with and without `possession` (2116 / 2142 tests, 2026-11-10). |

### Comparison with the C++ build

`python -I tools/zr_compare.py <CODE> --seed <S> --frames <N>` against the stored C++ references
([HARNESS.md](HARNESS.md)). "Exact" is the first frame where any compared field differs; "meaningful" follows the bar
in [AGENTS.md](../AGENTS.md) (a changed decision, a growing difference, or about a pathfinding cell of position).
All matchups are 2-player AI games on Alpine Assault.

| Matchup | To frame 3000: exact / meaningful | To frame 9000 (seed 1): exact / meaningful |
|---|---|---|
| AE1 (USA v China, Easy), seed 1 | none / none | f4908 / f4908 (player 2 money; a Chinook docks one frame early) |
| AN3 (GLA v USA, Normal), seed 1 | none / none | f4207 / f4227 (a GLA Rebel, 10 units) |
| AN1 (USA v China, Normal), seed 1 | none / none | f4102 / f4632 (a Ranger idle vs busy, from a tank's speed at f4099) |
| AH7 (GLA v USA, Hard), seed 1 | none / none | f3805 / f3822 (a worker, one cell) |
| AN5 (GLA v China, Normal), seed 1 | none / none | f3930 / f4060 (an object's health, then a rider's target) |
| AN5 seed 3, AN1 seeds 2-3, AN3 seed 2 | none / none | not referenced |
| AN3 seed 3 | f2961 / none (bounded drift, about 5 units) | not referenced |
| AN5 seed 2 | f2043 / none (sub-cell drift) | not referenced |

After about frame 3000-4000 the game timelines (build orders, event timing) still part by more than 30 frames, and
whether a game reaches the same winner flips with simulation changes. Judge by timelines, not by the outcome.

## Order of work

1. **Publish.** The repository is on GitHub and private; make it public when the owner decides.
2. **Human playtest.** An hour of skirmish and an hour of possession by a person. The findings set the next work.
3. **Review the unreviewed changes.** Some simulation changes from late October and early November were integrated
   without the adversarial review against the C++ that [AGENTS.md](../AGENTS.md) asks for. Review them, and fix or
   reject each finding with a C++ citation.
4. **Remove invented stand-ins**, starting with the spawner placement ring: `SpawnBehavior` places new spawns on a
   12-unit ring where the C++ uses the exit door (`exitObjectByBudding` and the queue exit for the Angry Mob nexus).
5. **Widen the comparison.** Fine C++ references for a second map and for one scripted human-input game; status bit
   fields in `zr_compare`. The divergence work below resumes after this.
6. **One structure pass over the code.** Fold modules named after work batches into topical ones, drop batch numbers
   from code comments and remove leftover debug environment gates.
7. **Remaining divergences on the Alpine matchups** (table above), in order: AN1 f4099 (a guarding tank accelerates
   where the C++ decelerates; lead: the non-blocked speed path of `locoUpdate_moveTowardsPosition`,
   `Locomotor.cpp:951-1060`, and `computePointOnPath`'s look-ahead), AH7 f3822, AN5 f3930 / f4060, AN3 f4207, AE1 f4908.
8. **Open gaps by area** (below), as a playtest or a divergence points at them.
9. **Campaign scripting**: deferred until skirmish, possession and the client are done.

**Out of scope:** online multiplayer. Lockstep determinism and replays stay, because replays need them.

## Open gaps by area

Some lines may be stale: check the code before starting one, and delete the line if it turns out done.

### Movement and pathfinding

- `findSafePath` and `requestSafePath` (the simulation has no repulsor requests).
- Group moves: the infantry and vehicle column moves (`friend_moveInfantryToPos`, `friend_moveVehicleToPos`, which
  need `Pathfinder::findGroundPath`) and formations. `AIData`'s `MinInfantryForGroup`, `MinDistanceForGroup` and
  `DistanceRequiresGroup` are not parsed.
- `AIFollowPathState`'s final adjust, the waypoint `calcExtraPathDistance`, and `m_moveAsGroup` lockstep.
- Blocked movement: `blockedBy`'s wander and moving-backwards terms, the formation factor in
  `calculateMaxBlockedSpeed`, and the speed comparison in `processCollision`.
- Collisions: sphere-sphere contact is 3D in the C++; the z gate's above/below rule and the sphere slice; partition
  range queries still register objects by bounding circle.
- `DeployStyleAIUpdate`'s state machine (only its never-sleep return is ported).
- The other `Vector3::Normalize` call sites still divide by the length where the C++ uses `Inv_Sqrt`.
- Bridges are a ground overlay and not a separate path layer.

### AI states and combat

- Pursue re-checks `canPursue` every update where the C++ checks only on entry (a likely real divergence), and
  `canPursue` lacks the crush clause.
- `AIAttackState::onExit` should also clear `IS_ATTACKING`, `IS_AIMING_WEAPON`, `IGNORING_STEALTH` and the goal object.
- `wantToSquishTarget` in the attack transitions; `computeAttackPath`'s too-soon arm.
- The mood matrix (`getMoodMatrixValue`), `ForbidPlayerCommands` and a general `aiDoCommand` gate.
- Reduced states: `AI_RAPPEL_INTO`, `AI_ATTACK_SQUAD`, the follow-waypoint attack-follow variant, and the `AI_DOCK`
  module wiring (supply, repair and airfield drive their own reduced docking).
- Turrets: `AltTurret`, the aim-veto targeter list, bridge attack points, `ContinueAttackRange`'s retarget.
- Team-level relationship overrides; `Object::defect`'s parking places and mines; `setAttackedBy`.
- Stealth: `STEALTH_ONLY_WITH_BLACK_MARKET`, the not-while-taking-damage / riders-attacking / using-ability rules,
  disguise.
- `SlavedUpdate` is the leash only (no attack, scout or repair priorities). `SpawnBehavior`'s budding exit,
  `maySpawnSelfTaskAI` and `getSlaveCount`.
- The Helix battle bunker lets every passenger fire where the C++ allows infantry only (a known bug).

### Weapons, projectiles and death

- Projectiles, laser beams and projectile streams are simulation records and not `Object`s, so trigger-area and
  object-count checks that see them in the C++ do not see them here.
- `MissileCallsOnDie`; projectile bounce (no shipped projectile uses it).
- `CrushDie`'s visual crush state and partial-crush points (they kill outright).
- `HijackerUpdate`'s in-vehicle ride consumes the hijacker.
- The neutron missile's heading is a simplified mover.

### Containers and exits

- Riders of a destroyed transport are placed on a ring where the C++ leaves or scatters them.
- `RailedTransportContain`'s unload, `exitObjectInAHurry`, `isDelayExitInAir`, garrison left/right evacuation.
- `KeepContainerVelocityOnExit` and orient-like-container on exit.
- The cave network, the Helix portable-structure move, the Overlord redirected contain, `ParachuteContain::update`.

### Production and economy

- Handicap and multiple-factory modifiers, `MaxSimultaneousOfType`, the player template's `ProductionCostChange`.
- Line-build, supply upgrades (`UpgradedSupplyBoost`), power sabotage.
- `setCanBuildUnits`; a simulation-side command-set table for the AI and script reads and for `CommandSetUpgrade`.
- `ReplaceObjectUpgrade` runs only the special-power half of `onBuildComplete`.

### Skirmish AI and scripts

- The dozer dock point differs from the C++ (`findPositionAround` candidate order or the site choice).
- Why attack waves sometimes do not finish a remnant base, and dozer replacement when none is left.
- Polygon-area guard leftovers, the guard-retaliate enter and hijack-guard branches, `TEAM_SET_ATTITUDE`'s
  auto-acquire gate, the `AITNGuard` AI.
- The team state string is stored as an integer.
- Base-defense placement, AI superweapon use and team abilities were once listed as unported, but the AI now does
  all three: verify against the C++ before treating them as gaps.

### Sleepy update heap

- `FireWeaponUpdate`, `DumbProjectileBehavior`, `CountermeasuresBehavior` and `MinefieldBehavior` have no heap slot;
  `HijackerUpdate`, `MobMemberSlavedUpdate`, `PointDefenseLaserUpdate` and `FireOCLAfterWeaponCooldownUpdate` are
  not registered.
- Some sweeps are not gated by their disabled masks (`OCLUpdate`, `ParticleUplinkCannonUpdate`,
  `PropagandaTowerBehavior`, `SpectreGunshipUpdate`).

### Aircraft and special powers

- The aircraft carrier's `FlightDeckBehavior`, `LockonTime` and `SneakyOffsetWhenAttacking`,
  `defectAllParkedUnits` on capture, the Chinook `isIdle` override.
- The pack / unpack / facing polish of `SpecialAbilityUpdate`.
- `DeliverPayload`'s `DoorDelay` is not parsed and per-item drop positions are approximate.

### Shroud

- Spy vision drawing, `UnlookPersistDuration` from the game data (hardcoded to 30 frames), the map-border shroud
  level.

### Client and rendering

- Water reflections, bump and sparkles; environment / reflection passes; cloud-shadow and lightmap terrain passes;
  stencil shadow volumes (shadow maps approximate them).
- `WeaponMuzzleFlash` sub-objects match by name and not per barrel; muzzle flashes look faint (no retail combat
  reference captured yet).
- No screenshot yet shows a tracer, a beam with its models, a missile trail or a dependency rider.
- The terrain's 3-way blend pass is a per-cell composite and not per pixel.
- The object's own shock pitch and roll, and truck cab / trailer steering.
- Options stored without effect: 3D and cloud shadows, light map, soft water, extra animations, heat effects,
  behind-building markers, props. Texture filter and reduction are read at launch only.
- The medal panel, menu transitions, the shell-map camera, the game-speed slider (a frame cap in the C++).
- EVA ally / stolen / sabotage / beacon events have no simulation source.
- Not yet compared with retail side by side: the minimap rectangle, cursor feel, tooltip art, the shroud, the
  construction scaffold at 50%.
- Performance: the radar's per-pixel shroud sampling (about 0.3 ms) and the simulation step (about 1.2 ms per logic
  frame).
- No save / load.

### Possession

- The controller layout, client-side prediction and a human playtest. Open design questions are in
  [POSSESSION.md](POSSESSION.md).

### Comparison tooling

- `zr_compare` excludes bit fields, so status-bit divergences are invisible to it.
- The player radar counts are not in the port's checksum (the C++ hashes them).
- `ZR_TRACE`'s `dbuild`, `victim`, `aiwake`, `dtask` and `dact` fields are documented gaps, skipped by default.
- Not yet done: the harness requests in the viewer, and a tool server over the harness and the diff.

### Project hygiene

- No continuous integration, `rustfmt.toml` or parser fuzzing. Most meaningful tests need a real install.

## Later

Campaign, higher-fidelity close-up assets, mod support.

## Known issues and upstream notes

- `cnc_formats::map_sage` parses 0 of 150 real maps (wrong header and chunk layout); replaced by `zr-map`.
- `cnc_formats::ini` targets the older `[Section] Key=Value` dialect; replaced by `zr-ini`.
- 5 Saboteur animation files (`uisabotr_*`) fail to decode: the retail data itself is malformed (chunk sizes do not
  land on the next header), so the engine would misparse them too. 4090 of 4095 animation files decode.
