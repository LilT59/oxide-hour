# Changelog

A short history, newest first: one paragraph per batch of work. Frame numbers such as "f3000" are logic frames (30 per
second) up to which a matchup agrees with the original C++ build; matchup codes are explained in
[docs/ROADMAP.md](docs/ROADMAP.md). Current status and open work are in the roadmap; what each subsystem ports is in
[docs/PORTED.md](docs/PORTED.md).

## 2026-11-11 — Audit and public docs

An outside audit of the project set the order of work (now in the roadmap). Three platform-dependent maths calls
were removed from the simulation logic. `LICENSE.md` was added. The project was renamed Oxide Hour, the README
became a front page, and the roadmap and the other docs were cut down for a public repository. The viewer's user
data folder was renamed to `OxideHour`; a folder left by an older build is moved on first use. The repository
history was restarted from a single commit for publishing.

## 2026-11-10 — Defection, cash bounty, assault transport, exact inverse square root

Ported `Object::defect` with the defector relationship rules, the three cash-bounty hooks, the troop crawler's
`AssaultTransportAIUpdate`, a bit-exact `WWMath::Inv_Sqrt` for vector normalisation, the attack-path flag with all
its set and clear sites, and the partition's box and cylinder contact tests. To frame 9000 the first meaningful
divergences moved to AH7 f3822, AN5 f4060, AN3 f4227 and AN1 f4632.

## 2026-11-04 — Wake-ups, attack exits, assisted targeting

Ported `AIUpdateInterface::wakeUpNow` at every call site, attack exits that follow the attack sub-state, and
`AssistedTargetingUpdate` with the weapon assist. To frame 9000: AH7 f3349, AN5 f3721, AN1 f4632.

## 2026-11-03 — Movement flags, guard areas, contact order

Ported the `m_isMoving` flag across every move state, `moveAllies` calling each ally's move-away per cell, the
straight-line quick path, polygon guard areas, the partition contact pass in engine order, the Chinook combat drop
as a real state, and the `DeliverPayload` transport's state order. All but two of the referenced matchup and seed
pairs matched to frame 3000.

## 2026-11-01 to 2026-11-02 — Line of sight, exact trigonometry, terrain flattening

Ported target-acquisition line of sight, the 32-bit C runtime's `sin` and `cos` results, `flattenTerrain` under new
buildings, the partition's cell-based contact pairs, partition cash values, and `SpecialAbilityUpdate`'s facing
states. C++ references were extended to frame 9000 for the five seed-1 matchups. Four of them matched to frame 3000.

## 2026-10-29 to 2026-10-30 — Object transform, state-owned move fields, spawn replacement

The object's heading became a readback of its transform matrix, as in the engine. Temporary AI states keep their
own move fields. Ported the guard machine's timers and transitions and `SpawnBehavior`'s replacement list. AE1
became identical to the C++ to frame 3000.

## 2026-10-29 — Container exits, hordes, blocked movement

Container exits (tunnels, spawns, healing, garrisons) go through one door-exit path. Ported `HordeUpdate`, the
treads speed order, the blocked-movement branch and collision processing with the path-priority rule. AN3 had no
divergence to frame 3000.

## 2026-10-28 to 2026-10-29 — Comparison tooling, exact hierarchical pathfinding

Decided the "near bit-exact" bar: code stays faithful, and only divergences that change a decision are chased.
Added the one-command comparison (`tools/zr_compare.py`), the C++ reference store, a per-object trace on both
builds, and the event timeline. Ported the hierarchical path search exactly, aircraft paths and destinations, six
upgrade modules, and base-defense placement.

## 2026-10-27 to 2026-10-28 — Sleepy heap completed, docking, retaliation

The sleepy update heap reached the same entries as the C++ on the reference game. Ported the dock machine's halt,
finished-path semantics, container modules as heap entries, `AIGuardRetaliateMachine`, the retaliation assist, the
horde morale bonus, missile move goals and garrison-clearing projectiles. In the client: launch and effect bones
over every draw module, the class-packed terrain atlas, the particle cap and dynamic level of detail. AN3 matched
to frame 1625.

## 2026-10-22 — One sleepy update loop

AI and module updates run from one sleepy heap in the engine's order. Ported the object helpers (firing tracker,
status and subdual damage, temporary weapon bonus), object and player upgrades through the production queue, the
weapon lock, the attack-move sub-machine, group move filters, and the AI's supply-truck and dozer orders as team
queue entries. AN3 matched to frame 1169 with every random-number draw equal.

## 2026-10-21 — Snapshot comparison, zones, turret AI, trains

Both builds write a per-frame state snapshot, and a diff tool finds the first difference. Object ids match the C++
allocation. Ported the pathfinding zone manager, the `TurretAI` state machine, `RailroadBehavior` (trains), object
to team membership, the dozer and worker module's scheduling, and the vehicle physics transform in the client. Added
the `zr-harness` control interface.

## 2026-10-14 to 2026-10-18 — Random-number trace, score keeper, projectiles

A random-number trace on both builds finds the first draw that differs. Ported `ScoreKeeper` and the score screen
columns, missile and shell physics, missile exhaust, projectile streams, lasers and tracers, the AI mood-check
timer, and the options menu's accept / cancel behaviour.

## 2026-10-12 to 2026-10-14 — First human playtest

The first human playtest of skirmish. Its findings were fixed against reference screenshots of the retail game:
start shroud, power bar, control-bar panels, weapon effects, tread and tyre animation, shell-map script actions.
Also ported the worker machine, `AI_DEAD`, special-ability aborts and `ActionManager` checks.

## 2026-10-07 to 2026-10-10 — Playtest readiness, first C++ comparison

Right-click context commands, retail cursors, the meta-key layer, the control-bar button audit, the EVA announcer
and radar events, dozer repair and construction resume, the airfield jet cycle, and a command soak test. The first
comparison with a headless build of the C++ source began; early matchups showed where the AI economies differed.

## 2026-10-05 to 2026-10-06 — AI state machine, collisions, scope decisions

Ported `AIStateMachine` (replacing an ad-hoc move / attack split), the attack sub-machine, `AI_DOCK`,
object-object collisions with the collide modules, sequential scripts, team recruiting and the AI's gatherer model.
Decisions: online multiplayer is out of scope, the campaign is deferred, and possession moved behind an
off-by-default cargo feature.

## 2026-10-04 — Playable skirmish shell

The real main menu, skirmish setup and load screen; a script-only skirmish AI that wins every matchup;
promotions, replays and options; tunnels, garrison fire points, walls as path layers, hierarchical paths, bridges,
superweapons and the remaining logic modules typed.

## 2026-10-03 — Core gameplay systems

Production and money, the base economy, containment, the death pipeline, real projectiles, upgrades and veterancy,
special powers and sciences, locomotors and physics, fog of war, the script engine and the first skirmish AI. In
the client: skeletal animation, particles, the control bar, the radar, audio, the skybox and the possession
cameras.

## 2026-10-02 — Project start

Install discovery, archives, the map reader, the INI parser and typed templates; terrain, roads, water and models
in a Bevy viewer; the simulation core with a ground pathfinder, weapons and the attack command; the first live
skirmish in the viewer with orders and possession.
