# Comparison harness: per-frame state snapshots

**What you need to run this.** The comparison needs two things that are not in this repository: a patched headless
build of the C++ source (TheSuperHackers' `GeneralsGameCode`, with read-only printing added; called `zh-cpp-build`
below) and a store of reference files that build produced (`zr-refs`, kept next to the repository). Without them you
can still write and diff snapshots of two runs of the port, and use the control interface. Some environment variables
below are named after the work batch that added them (`ZR_L99_HEAP`, `ZR_L64_ID`); the names are kept because both
builds read them.

Both sides write the same records, so the first frame and field where the port and the C++ build
disagree is found by `tools/snap_diff.py` instead of by matching log lines by hand. The port side is in
`crates/zr-game/src/snapshot.rs`; the C++ side is a read-only printf in the patched build
(`zh-cpp-build/src-patched`, `GeneralsMD/Code/GameEngine/Source/GameLogic/System/GameLogicSnapshot.cpp`).

## Running the port side

```
ZR_SNAPSHOT=<file> [ZR_SNAPSHOT_EVERY=<N>] [ZR_SNAPSHOT_BITS=1] cargo run --release -p zr-game --example skirmish_ai_demo -- FactionGLA FactionAmerica --seed 1
python tools/snap_diff.py PORT.snap CPP.snap [--pos-tol 1.0] [--hp-tol 0.5] [--fields NAME ...] [--max-report 12]
                                        [--key id|tmpl] [--skip TAG.FIELD[=VALUE] ...] [--from-frame N]
```

- `ZR_SNAPSHOT` (a file path) turns the writer on. Unset or empty, nothing runs. `skirmish_ai_demo` writes from its
  first (verbose) run only; the second run has no snapshot, so the two CRCs printed by the demo also check that the
  writer does not change the simulation.
- `ZR_SNAPSHOT_EVERY` is the frame interval (default `30`). A block is written for frame 0 (before any update) and for
  every later frame whose number is a multiple of it, right after the update that reaches that frame (the C++ print point,
  see "When the C++ side prints").
- `ZR_SNAPSHOT_BITS=1` adds the IEEE bit patterns of the floats (`xb yb zb hpb consb`, eight hex digits).
- The writer only reads the simulation. The demo's AN3 run (`FactionGLA FactionAmerica --seed 1`) prints the same CRC
  twice with the snapshot and a separate no-env run prints it too (2026-10-21: `1bf4461d`). At the default
  interval the run writes 1021 blocks, 333308 records, 60 MB with bits. Sim edits move that CRC, so take the demo's own two
  values as the check.

The viewer does not write snapshots yet; `snapshot::run_frames` is the call to use in its sim step.

## Running the C++ side

```
cd zh-cpp-build\zhbuild
ZR_SNAPSHOT=<file> [ZR_SNAPSHOT_EVERY=<N>] [ZR_SNAPSHOT_BITS=1] python -I matrix_run.py 144000 --pairs AN3:1
```

- Build first with `zh-cpp-build\zhbuild\build-patched.bat build` (only one C++ build at a time). The writer is
  `GeneralsMD/Code/GameEngine/Source/GameLogic/System/GameLogicSnapshot.cpp` (declared in
  `Include/GameLogic/GameLogicSnapshot.h`, called from `GameLogic::update`; two lines in that file and the CMake source
  list name it). The only other source changes are two read-only getters: `TeamFactory::zrGetNthTeamPrototype` (Team.h,
  the prototype map is private) and `ProductionEntry::getFramesUnderConstruction` (ProductionUpdate.h, the member is
  protected). Nothing writes game state: the AN3 s1 run still ends at frame 25218 with GLA winning, as without the printfs.
- The same three environment variables as the port, same defaults and meaning; the file is created (truncated) when
  the game starts. The C++ run's stdout keeps its other `ZR_*` diagnostics, so the snapshot goes to its own file.
- The run ends at game over (or the replay horizon), so the C++ file can end before the port file; `snap_diff` then
  reports the port-only frames after it. Use the same horizon on both sides (the port demo runs to a winner or 40
  minutes) and compare the frames both have.

## File format

One record per line. A file may contain other lines (a C++ log, for example): the diff ignores every line that does not
start with `SNAP `.

```
SNAP f=<frame> <TAG> <name>=<value> <name>=<value> ...
```

- `f` is the logic frame: `GameLogic::getFrame()` (the port's `GameLogic::frame`). Each block starts with one `FRAME`
  record, followed by its records in the order below. When a block is written is in "When the C++ side prints".
- Fields are separated by single spaces, so a value never contains a space. String values escape `%` as `%25`, space
  as `%20` and `=` as `%3D` (uppercase hex). Team prototype names are printed lowercase on both sides: the port
  lowercases them, and the C++ printf does too (`TeamFactory::createTeamPrototype` keeps the script's case, and
  `NAMEKEY` is case-sensitive, so the case differs between the builds). Template names keep their INI case.
- Integers are decimal. Floats: `x y z` with 4 decimal places, `hp` and `cons` with 3 (C `%.4f` / `%.3f`). Both
  printers round the exact binary value to the nearest decimal, ties to even. The diff parses the decimals and compares
  within tolerance, which is far larger than the rounding (5e-5), so rounding never makes a difference on its own. Bit
  patterns (`*b` names, `%08x`, lowercase) are compared only when named in `--fields`.
- Booleans are `0` / `1`. A missing id (`atk`, `bld`, `recruit`, `factory`) is `0`. A missing door is `-`. `power` is
  `+` when power is sufficient and `-` when it is not. A missing value that is not an id is `-` (`hp` for an object with
  no damageable body, `ai` for one with no AIUpdate module).
- Record order inside a block: `FRAME`, then `P` for each player in index order, `O` for each live object in id order,
  `Q` for each factory's production queue in object order, `T` for each player's AI build queue then ready queue, `S`
  for each created script team in `TeamFactory` id order.

### Records

| Tag | Key | Fields (in order) | Meaning | C++ source (to add as printf) |
|---|---|---|---|---|
| `FRAME` | - | `crc` `objs` `players` | Block header. `crc` is `Game::crc`: informational, never compared, even with `--fields crc` (the builds hash different state). `objs` is the object-list length: C++ `GameLogic::getObjectCount()` walks `m_objList`, and the port counts every object in its object map (`logic.object_count()`), including objects destroyed earlier in the frame that the frame's `processDestroyList` has not removed yet (that is why frame 0 has the two replaced command centres). `players` is the player-list length. | `TheGameLogic->getFrame()`; `GameLogic::getObjectCount()`; `ThePlayerList->getPlayerCount()` (`PlayerList.h:97`) |
| `P` | `p` | `money` `power` `prod` `cons` `ai` | A player (`p` is the index). `money` is cash. `power` is `+` when power is sufficient, else `-`. `prod` is the power production with the sabotage gate applied (0 while the grid is sabotaged, Energy.cpp:67-73; the port's `printed_production`); the sim's own tally is not changed. `cons` is the consumption. `ai` is 1 for a skirmish AI. | `Player::getPlayerIndex()`; `Money::countMoney` (port `Player::money`); `Energy::hasSufficientPower()`; `Energy::getProduction()` (gated, see the text), `Energy::getConsumption()`; `Player::isSkirmishAIPlayer()` (`m_ai && m_ai->isSkirmishAI()`, Player.cpp:1151). `getPlayerType() == PLAYER_COMPUTER` is true for the neutral and civilian players too (1 for both), so it would not match this field |
| `O` | `id` | `tmpl` `owner` `x` `y` `z` `hp` `ai` `cons` `q` `atk` `bld` | A live object. `tmpl` is the template name. `owner` the player index. `x y z` the position (4 places). `hp` the health (3 places), `-` with no damageable body: the port has no body module, and in C++ every template inherits the default `Body = InactiveBody` (`data/ini/default/object.ini:89`) whose max and current health are 0 (InactiveBody.cpp:143), so a body with no max health also prints `-`. `ai` the AI state name (table below), `-` with no AIUpdate module. `cons` the construction percent while under construction, `-1` (CONSTRUCTION_COMPLETE) otherwise. `q` the production queue length. `atk` the victim object id, `0` for none, `ground` for an attack on a position. `bld` the dozer's build task target, `0` for none. | `Object::getID()`; `Object::getTemplate()->getName()`; `Object::getControllingPlayer()->getPlayerIndex()`; `Object::getPosition()`; `Object::getBodyModule()` (null or `getMaxHealth() == 0`: `-`) then `->getHealth()`; `Object::getAIUpdateInterface()` (null: `-`) then `getCurrentStateID()`; `Object::getConstructionPercent()` (`CONSTRUCTION_COMPLETE` = -1 when complete: Object.cpp:283, DozerAIUpdate.cpp:568); queue length by walking `ProductionUpdate::firstProduction()` / `nextProduction()`; `atk`: `AIUpdateInterface::getCurrentVictim()` (`m_currentVictimID`, AIUpdate.cpp:4274); when there is no victim, `ground` only in `AI_ATTACK_POSITION` (the attack-ground state) and `0` in every other attacking state (`getCurrentVictimPos()` alone is non-null for attack-move and guard too, where the port has no ground order) — see the mapping gap below; `bld`: `DozerAIUpdate::getTaskTarget(DOZER_TASK_BUILD)` (`m_task[DOZER_TASK_BUILD].m_targetObjectID`, DozerAIUpdate.h:294; `0` when `INVALID_ID`) |
| `Q` | `obj` `i` | `kind` `tmpl` `total` `done` `frames` `door` | One production queue entry of factory `obj`, at queue index `i`. `kind` is `unit` or `upgrade`; `tmpl` the unit template or upgrade name; `total` the requested count (`ProductionEntry::m_productionQuantityTotal`: 1 or the quantity modifier for a unit, **0 for an upgrade**, ProductionUpdate.cpp:163 leaves the constructor value), `done` the produced count; `frames` the frames under construction; `door` `ProductionEntry::getExitDoor()`: `DOOR_1` = 0 while a unit waits for its factory exit (stored by `ProductionUpdate.cpp:710-716`) and for an upgrade entry (the constructor leaves the field to the zeroed allocation); `-` for `DOOR_NONE_AVAILABLE`/`DOOR_NONE_NEEDED`. | `ProductionUpdate::firstProduction()` and `nextProduction(p)` (ProductionUpdate.h:226); `ProductionEntry::getProductionType()`, `getProductionObject()` / `getProductionUpgrade()` (`getUpgradeName()` for the `UpgradeTemplate`), `getProductionQuantity()` (`total`), `done` = `getProductionQuantity() - getProductionQuantityRemaining()`, `getExitDoor()` (`-1` DOOR_NONE_AVAILABLE and `-2` DOOR_NONE_NEEDED both print `-`; `DOOR_1` = 0, UpdateModule.h:330). `frames` = `m_framesUnderConstruction`, protected (ProductionUpdate.h:104): **patch added** `ProductionEntry::getFramesUnderConstruction()` |
| `T` | `p` `q` `i` | `key` `prio` `started` `reinf` `recruit` `sent` `work` | One AI team queue entry for player `p`: `q` is `build` (`AIPlayer` `m_teamBuildQueue`) or `ready` (`m_teamReadyQueue`), `i` its index. `key` is the team's prototype name, lowercase: `m_team->getPrototype()->getName()`, where `m_team` is the team the entry fills (the port resolves it from `TeamInQueue::key`, its instance key, through `ScriptEngine::team_proto_name`). For a supply-truck entry that is the default team (AIPlayer.cpp:409, `m_team = m_player->getDefaultTeam()`), so the key is the default team's prototype, not the entry's own name. `prio` `m_priorityBuild`, `started` `m_frameStarted`, `reinf` `m_reinforcement`, `recruit` `m_reinforcementID`, `sent` `m_sentToStartLocation`. `work` is a comma list of `thing:completed/required:req:factory` (`WorkOrder` `m_thing`, `m_numCompleted`, `m_numRequired`, `m_required`, `m_factoryID`; `factory` `0` for none). | `TeamInQueue` fields (`AIPlayer.h`, public section from line 129) and `WorkOrder`; `TeamInQueue::m_team->getPrototype()->getName()` for the key |
| `S` | `id` | `proto` `members` | A created script team. `id` is `Team::getID()` (`m_id`, from `++m_uniqueTeamID`, Team.cpp:353 and :388). `proto` is the prototype name, lowercase: `Team::getName()` (= `getPrototype()->getName()`). `members` the member object ids in ascending order, comma separated, empty for none. **The team state is not printed** (see the gaps below). Created means `Team::isActive()` (`m_active`, set by `setActive()`); build teams from `createInactiveTeam` (AIPlayer.cpp:2547) are not printed until they are activated (AIPlayer.cpp:2847, :3526). Both sides print every active team, the *default* teams (`team`, `teamplyrcivilian`, `teamskirmishgla0`, ...) included, and their real member lists: the port assigns every object to exactly one team (`Object::m_team`, `Object.cpp:196`) — map objects by their `originalOwner` (`PlayerList::validateTeam`), produced/spawned/created objects to their owner's default team (`ProductionUpdate.cpp:773`, `Object.cpp:312`), and script/AI teams through `Object::setTeam` (`Object.cpp:851-948`) — and keeps `Team::m_TeamMemberList` in the engine's prepend order. | `Team::getID()`, `Team::getName()`, `Team::getPrototype()->getName()`, `Team::isActive()`; the member list is `iterate_TeamMemberList()`; sort the ids (the list is built by prepending, Object.cpp:898, and the dump at Team.cpp:599 reverses it, so its order is not a game fact); teams are enumerated through the read-only `TeamFactory::zrGetNthTeamPrototype` helper |

Name tables: `ai` values are the port's `AiStateType` names (`crates/zr-sim/src/ai_states.rs`); "AI state names" below
gives the port name of every C++ `AI_*` id. A C++ id that is not in that table prints as `Unknown(<id>)`, which the diff
then reports.

Not in the schema yet (add when a divergence needs it): the sleep and goal fields of the state machine, weapon and
target detail, body damage state, model conditions, shroud, the scheduler heap, and the AI's `SkirmishAi` counters
(`base_defenses_built`, `powers_fired`, ...). Each would be one more field on an existing record.

## Object ids (`ZR_ID_DUMP` / `ZR_L64_ID`)

`O` records are matched by `id`, so both builds must hand out the same ids in the same order. `GameLogic::allocateObjectID`
(`GameLogic.cpp:4069-4075`) is one counter starting at 1; the port's `GameLogic::allocate_object_id` copies it, and also
numbers the things the sim keeps as records (projectiles, laser beams, projectile streams: `Weapon.cpp:1105`, `:2515`,
`:2543`). `ZR_ID_DUMP=<file>` (port, `crates/zr-sim/src/id_dump.rs`) writes one line per allocation, `ZR_ID_DUMP_MAX` caps it
(default 400): `ZR_L64_ID id=<id> f=<frame> tmpl=<name> at=<file>:<line>`. The patched C++ build prints the same tokens from
`Object::Object` (`tmpl` = `getTemplate()->getName()`, `at` = a creation-path tag: `mapbridge`, `bridgetower`, `mapobj`,
`startbld`, `startunit`, else `other`), and `GameLogic::destroyObject` prints `ZR_L64_DEST id= f= tmpl=` for lifetimes.
`python tools/id_diff.py PORT CPP` aligns the two. AN3 seed 1: the whole start-up and early game (ids 1-238:
the map's 207 objects incl. the landmark bridge and its four towers, both starting sets, the AI's rebuilt command centers)
match in id and template; the first difference is a later production-order divergence.

## When the C++ side prints

- **Port:** a block is written for frame 0 (the state before any update, counter 0), and then after each update that
  leaves the counter on a multiple of N. `snapshot::run_frames` does this for any chunking, including single steps
  (`ZR_FINE` in the demo), and writes each due frame once.
- **C++ print point:** the end of `GameLogic::update`, after `m_frame++` (inside `if (!m_startNewGame); GameLogic.cpp:4035`
  before the ZR patch). `zrSnapshotPrintFrame()` is called there. The printed frame is the counter after that update, the
  same as the port's frame after `step()`.
- **Frame 0 is compared.** The C++ update never prints frame 0 (the counter is still 0 when the first update starts), so
  `zrSnapshotPrintFrameZero()` prints it once, right after `startNewGame()` returns and before the first update's work.
  For a fresh game that state includes what `startNewGame` created and destroyed: the two starting command centers it
  deletes are still on the object list (they leave at the first `processDestroyList`), and the default teams it activates
  are listed. The port keeps destroyed objects in its object list until the end of the frame too, and its writer walks
  that list (`logic.object_list()`), so the frame-0 `FRAME objs` and `O` records match, and the
  `S members` lists match too (object→team assignment below).
- Both sides print the same records in the same order. Each side prints nothing else into the snapshot file.
- A run that ends on game over ends the snapshot there. Compare only the frames both files have, or the diff reports the
  port-only tail as differences.

## C++ accessors and fields

The printf uses public getters where they exist and changes nothing else. The two read-only helpers marked **patch** were
added to the patched build; no game logic changed.

- Energy (`Energy.h`): `getProduction()` (gated, Energy.cpp:67-73), `getConsumption()`, `hasSufficientPower()`.
- `ProductionEntry`: `getProductionType()`, `getProductionObject()`, `getProductionUpgrade()` (its name is
  `getUpgradeName()`), `getProductionQuantity()`, `getProductionQuantityRemaining()`, `getExitDoor()`. **Patch:**
  `getFramesUnderConstruction()` (the member is protected, ProductionUpdate.h:104).
- Team (`Team.h`): `getID()` (:249), `getName()` (:279), `getPrototype()` (:246), `isActive()` (:314), the member list
  `iterate_TeamMemberList()` (DLINK, public), plus **patch** `TeamFactory::zrGetNthTeamPrototype(Int)` to enumerate the
  prototypes (the map is private; the snapshot then sorts the teams by id).
- `TeamInQueue`, `WorkOrder` (`AIPlayer.h`): public fields as in the table.
- `AIUpdateInterface` (`AIUpdate.h`): `getCurrentStateID()` (:638), `getCurrentVictim()` (:533). `DozerAIUpdate`
  (`DozerAIUpdate.h`): `getTaskTarget(DOZER_TASK_BUILD)`.
- `PlayerList::getPlayerCount()` (`PlayerList.h:97`), `GameLogic::getObjectCount()` (`GameLogic.h:222`), `Player::getDefaultTeam()`.

## Sleepy heap (`ZR_L99_HEAP`)

The scheduler heap is compared by a separate dump, not by the snapshot (the snapshot schema has no heap field yet).
Other read-only C++ gates: `ZR_L160_HEAP` (AN1 frames 1900-2071), `ZR_L166_HEAP` (AN5 2480-2710), `ZR_L167_HEAP` (AH7
1900-2124, with pops), `ZR_L167_PHYS` (`calcSleepTime`), `ZR_L159_CD` (`checkDestination`), `ZR_L161_CAND` (`checkForAdjust`),
`ZR_L162_ATK`/`_AM`/`_PM`/`_ADJ`/`_GATE`, `ZR_L170_HEAP`/`_PATH`, `ZR_L171B_POP`, `ZR_L169_LEGAL`/`_BSWD`/`_MOOD`/`_LOS`,
`ZR_L175_*` (`ZR_L175_ADD`: contact-pair cells), `ZR_L191_ATKPOP=<from>-<to>:<id>` (`findAttackPath` pops and
result; the port has the same dump under the same name) and `ZR_L191_FPOS` (`tryPosition` per-candidate results; C++ only,
the port-side copy was temporary). `zr_compare`'s "first exact" uses `--pos-tol 1.0`, so a sub-unit drift can start
before the frame it reports.

- **Port:** `ZR_L99_HEAP=<frames>` (comma list, e.g. `1000,1163`) prints the heap in array order on stderr, at the start
  of the frame's sleepy loop (`GameLogic::dump_sleepy_heap`, before the AI pass): `ZR_L99_HEAP f= i= id= key= mod= tmpl=`.
  `ZR_L99_POP=<frames>` prints each AI pop of those frames (`f= id= key=`) in pop order.
- **C++:** the patched build prints the same format on stdout (`ZR_L99_HEAP`, GameLogic.cpp, before the sleepy loop, and
  `ZR_L99_POP` for each pop of frames 1160-1166). `mod` is `TheNameKeyGenerator`'s class name (`getModuleNameKey`).
- **Compare:** `python -I tools/heap_diff.py <port stderr> <C++ stdout> <frame> [-v]`: class counts, matching (id, class)
  pairs and keys, and the AI modules of the earliest key in heap order. Key = `(wake << 2) | phase`; `FOREVER` is
  `0x3fffffff` (keys `0xFFFFFFFE` and `0xFFFFFFFF`).
- AN3 s1 at f1163: both builds hold 1357 entries (same classes, pairs and keys, except the AI sleep of the seven
  civilian cars).
- The C++ `ZR_L99_HEAP` dump is hard-coded to frames 1000/1100/1163. Gated variants cover other frames:
  `ZR_L156B_HEAP` (frames 1991, 2008, 2020, 2038) and `ZR_L157_POP` (the heap and every pop of frames
  1872-1874). AN5 s1 matches the C++ entry for entry at f1000, 1163, 1991, 2008, 2020 and 2038; the
  only label difference is `SupplyTruckAIUpdate` (C++) vs `AIUpdateInterface` (port) for the supply trucks, same keys.
- More C++ gates: `ZR_L154_PATHN` (computePath requests), `ZR_L154_COLLIDE` (PhysicsBehavior::onCollide),
  `ZR_L158_CONTACT` (partition contact pairs).

## AI state names

`ai=` prints the port's `AiStateType` name. The C++ side maps the `AI_*` id (AIStateMachine.h:58-110, 44 entries; the
index is the enum value) to that name. A C++ state the port does not have is still printed by C++ under its name, so
the blocks differ at that object, which is the point of the diff.

| id | C++ | port `ai=` |
|---|---|---|
| 0 | AI_IDLE | Idle |
| 1 | AI_MOVE_TO | MoveTo |
| 2 | AI_FOLLOW_WAYPOINT_PATH_AS_TEAM | FollowWaypointPath (collapsed) |
| 3 | AI_FOLLOW_WAYPOINT_PATH_AS_INDIVIDUALS | FollowWaypointPath (collapsed) |
| 4 | AI_FOLLOW_WAYPOINT_PATH_AS_TEAM_EXACT | FollowWaypointPath (collapsed) |
| 5 | AI_FOLLOW_WAYPOINT_PATH_AS_INDIVIDUALS_EXACT | FollowWaypointPath (collapsed) |
| 6 | AI_FOLLOW_PATH | FollowPath |
| 7 | AI_FOLLOW_EXITPRODUCTION_PATH | FollowExitProductionPath |
| 8 | AI_WAIT | Wait |
| 9 | AI_ATTACK_POSITION | AttackPosition |
| 10 | AI_ATTACK_OBJECT | AttackObject |
| 11 | AI_FORCE_ATTACK_OBJECT | ForceAttackObject |
| 12 | AI_ATTACK_AND_FOLLOW_OBJECT | AttackAndFollowObject |
| 13 | AI_DEAD | Dead |
| 14 | AI_DOCK | Dock |
| 15 | AI_ENTER | Enter |
| 16 | AI_GUARD | Guard (C++ sub-states are not printed; see gaps) |
| 17 | AI_HUNT | Hunt |
| 18 | AI_WANDER | Wander |
| 19 | AI_PANIC | Panic |
| 20 | AI_ATTACK_SQUAD | AttackSquad |
| 21 | AI_GUARD_TUNNEL_NETWORK | missing in port |
| 22 | AI_GET_REPAIRED | missing in port |
| 23 | AI_MOVE_OUT_OF_THE_WAY | MoveOutOfTheWay |
| 24 | AI_MOVE_AND_TIGHTEN | MoveAndTighten |
| 25 | AI_MOVE_AND_EVACUATE | MoveAndEvacuate |
| 26 | AI_MOVE_AND_EVACUATE_AND_EXIT | MoveAndEvacuateAndExit |
| 27 | AI_MOVE_AND_DELETE | MoveAndDelete |
| 28 | AI_ATTACK_AREA | AttackArea |
| 29 | AI_HACK_INTERNET | missing in port (`hack_internet.rs` is not an AI state) |
| 30 | AI_ATTACK_MOVE_TO | AttackMoveTo |
| 31 | AI_ATTACKFOLLOW_WAYPOINT_PATH_AS_INDIVIDUALS | AttackFollowWaypointPath (collapsed) |
| 32 | AI_ATTACKFOLLOW_WAYPOINT_PATH_AS_TEAM | AttackFollowWaypointPath (collapsed) |
| 33 | AI_FACE_OBJECT | FaceObject |
| 34 | AI_FACE_POSITION | FacePosition |
| 35 | AI_RAPPEL_INTO | missing in port (the Chinook has its own port, `zr-sim::chinook`) |
| 36 | AI_COMBATDROP | missing in port |
| 37 | AI_EXIT | Exit |
| 38 | AI_PICK_UP_CRATE | missing in port |
| 39 | AI_MOVE_AWAY_FROM_REPULSORS | missing in port |
| 40 | AI_WANDER_IN_PLACE | WanderInPlace |
| 41 | AI_BUSY | Busy |
| 42 | AI_EXIT_INSTANTLY | ExitInstantly |
| 43 | AI_GUARD_RETALIATE | `GuardRetaliate` |

Port states with no `AI_*` id here: `GuardInner`, `GuardOuter`, `GuardReturn`, `GuardIdle`, `GuardGetCrate`,
`GuardAttackAggressor` (the guard sub-machine, `AIGuard.h`), and the jet and helicopter states (`JetAIStateType`, a
separate enum). The C++ `getCurrentStateID()` of a guarding unit is `AI_GUARD`, so these print under a different name
than C++ and will show as a difference until one side is changed.

Jet and helicopter states: `JetAIStateType` ids start at 1000 (`JetAIUpdate.cpp:62`: 1001 `TAXI_FROM_HANGAR`, 1002
`TAKING_OFF_AWAIT_CLEARANCE`, 1003 `TAXI_TO_TAKEOFF`, 1004 `PAUSE_BEFORE_TAKEOFF`, 1005 `TAKING_OFF`, 1006
`LANDING_AWAIT_CLEARANCE`, 1007 `LANDING`, 1008 `TAXI_FROM_LANDING`, 1009 `ORIENT_FOR_PARKING_PLACE`, 1010 `RELOAD_AMMO`,
1011 `RETURNING_FOR_LANDING`, 1012 `RETURN_TO_DEAD_AIRFIELD`, 1013 `CIRCLING_DEAD_AIRFIELD`). The C++ printf maps them to
the port's names (`JetTaxiFromHangar`, ... `JetCirclingDeadAirfield`), and for a helicopter (`m_needsRunway` false,
`JetAIUpdate.cpp:1862`) 1005/1007 print `HeliTakingOff`/`HeliLanding`, which is what the port calls them.

## First comparison (AN3 seed 1)

The first snapshot diff (2026-10-21) could not align the two files by object id, and differed at frame 0. Fixing the
map load, the id allocation, the default teams and the start-up timing (the "fixed" entries in the next section) made
frame 0 identical on the compared fields. The first simulation divergence then moved from frame 30 to frame 3000 and
beyond over the following weeks; the current numbers are in the [roadmap](ROADMAP.md). While the two builds still
diverge early, these options keep the diff readable:

```
python tools/snap_diff.py PORT CPP --key tmpl --from-frame 30 --skip O.atk O.bld T.recruit T.work
```

`--key tmpl` aligns objects by template and per-template creation order instead of by id.

## Fidelity gaps the snapshot exposes

These come from checking the schema against the C++ and from the first AN3 comparison. The simulation changes they
call for are roadmap items.

- **Team state.** C++ `Team::m_state` is an AsciiString set only by `doSetTeamState` (ScriptActions.cpp:486-496), empty by
  default. The port stores a `TeamState` enum set by `TEAM_SET_STATE` from an integer, with a `_ => Idle` fallback
  (`script/actions.rs:262-272`). So the state is not printed (the `S` record has no `state` field).
- **Team ids.** C++ increments `m_uniqueTeamID` once per instance created (Team.cpp:353 and :388). The port's
  `create_team` does the same. Its `on_team_created` also increments `next_team_id` when the instance is missing; C++ has
  no such path, so ids drift after it. Normal creation goes through `create_team`.
- **Victim (`atk`).** C++ `getCurrentVictim()` is `m_currentVictimID` (AIUpdate.cpp:4274); `ground` is printed only in
  `AI_ATTACK_POSITION` (the attack-ground state), `0` in every other state without a victim. The port keeps the same variable (`current_victim`, set by the attack update) and `atk` reads it, so the gap is
  closed. `tools/trace_diff.py` still lists `victim` in its default skip set; pass `--fields victim` or `--all-fields`
  to compare it.
- **Missing AI states:** 8 of 44 (table above), and the four `FOLLOW_WAYPOINT_PATH` and two `ATTACKFOLLOW_WAYPOINT_PATH`
  variants collapse into one port state each.
- **Guard sub-states** print under their own names (see above).
- **Object ids** *(fixed 2026-10-21).* The port now allocates the same ids as the C++: `ZR_ID_DUMP` / `ZR_L64_ID`
  (§ "Object ids" above). The three causes were the AI's placed starting command center being replaced instead of reused
  (`AISkirmishPlayer::adjustBuildList` destroys it and `newStructureNow` builds a new one), the landmark bridge creating its
  towers inside its own creation (`Bridge::Bridge(Object*)`), and 55 map objects the C++ loads as client props
  (`CLEARED_BY_BUILD` fluff, below). AN3 seed 1: the first 238 allocations (ids 1-238, the whole start-up and early game)
  are identical in id and template; the first id difference is a later production-order divergence, not an allocation one.
  `--skip O.atk O.bld T.recruit T.work` is still useful while the sim diverges, but `--key tmpl` is no longer needed for
  the ids. (The frame-0 `FRAME objs` difference this bullet used to mention is fixed: the writer counts the
  object list, including objects destroyed earlier in the frame.)
- **Map props** *(fixed 2026-10-21).* The 55 extra objects were `KINDOF_CLEARED_BY_BUILD` with `FenceWidth = 0`
  (StreetLamp, CrossingGuard, ParkBench, crates, ...): in a multiplayer game `forceFluffToProp` is set (GameLogic.cpp:1840-1843)
  and the engine registers them with `TheTerrainVisual->addProp` (:1919-1921), never as `Object`s. That is why the C++ has no
  ids for them. They are still drawn: the viewer's static map-object pass (`zr-viewer` `Stage::Objects`) draws every map
  object the sim does not own. The map chunk is read correctly; nothing was deleted from `zr-map`.
- **Bridge towers** *(fixed 2026-10-21)*: the four `BridgeTowerConcrete*` objects of the train bridge are now
  created inside the bridge object's creation (their ids follow the bridge) at the C++ positions — the corners come from the
  bridge object's box and angle and each tower is offset by its own major radius (`Bridge::Bridge(Object*)`,
  Core/.../Map/TerrainLogic.cpp:329-408). The two `FROM_` towers take `orientation + PI` and the two `TO_` towers the bridge
  orientation (`Bridge::createTower`, :143-160), so AN3's towers are `-1.482+PI` / `-1.482` rather than four times the bridge
  angle. A real-data test pins the four AN3 frame-0 positions from the C++ snapshot; the creation is per corner (a missing
  `TowerObjectName*` key or an unknown tower template only drops that corner) and no longer needs the port's terrain rectangle.
- **Bodyless objects and `EFFECTIVELY_DEAD`** *(fixed 2026-10-21)*: a template without an `ActiveBody` gets the
  default `Body = InactiveBody` (`data/ini/default/object.ini:89`), and that constructor sets `EFFECTIVELY_DEAD`
  (`InactiveBody.cpp:47-50`). The port now sets `Object::effectively_dead` at creation for every bodyless object, so the
  `isEffectivelyDead()` readers see them as dead: the mood scan/targeting skip them, `count_owned` /
  `count_owned_in_area` skip them (except `KINDOF_CRATE`, `ScriptConditions.cpp:494-497`), `NAMED_DESTROYED` /
  `NAMED_NOT_DESTROYED` follow `evaluateNamedUnitDestroyed`/`Exists` (`:295-330`), and
  `BuildAssistant::isRemovableForConstruction` clears them under a footprint (`BuildAssistant.cpp:1381-1382`) without the
  earlier special case in `removable_for_construction`. AN3: the ambient sound markers, hedges and rocks are bodyless;
  the USA barracks at frame 1025 lands at the C++ position (1099.6906,135.6625) because the marker no longer blocks the
  site search.
- **Start-up AI timing** *(fixed 2026-10-21)*: `AIIdleState::doInitIdleState` now runs at the C++ frame (the
  `AIUpdate` module's creation wake frame, `AIUpdate.cpp:299-301`), so the cars snap at frame 2 like C++. The
  dozer/worker first build task is on the C++ frame too: the engine's dozer update *is* the `AIUpdateInterface`
  module, so `DozerAIUpdate::update` runs the AI update and then the dozer machine in one call (`DozerAIUpdate.cpp:1619`,
  `:1666`), and `newTask`'s `resetToDefaultState` enters BUILD but leaves `PickActionPos::update` for the module's next
  update (`:2044`, `StateMachine.cpp:644-650`, `DozerAIUpdate.cpp:1230-1240`). At frame 2 both the GLA worker and the
  US dozer are `ai=Idle` with their build target set, and the move starts at frame 3, as C++ (fine snapshot).
- **Civilian cars** *(fixed 2026-10-21)*: the six cars left their `.map` position because of
  `AIIdleState::doInitIdleState`'s start-up cheat move (`AIStates.cpp:1358-1382`): an idle unit that is doing ground
  movement and is not `ULTRA_ACCURATE` calls `Pathfinder::updateGoal` on its own position (`:1370`) and then
  `Pathfinder::goalPosition` (`AIPathfind.cpp:5303`), and at `getFrame() <= 1` it does `setPosition` to that goal cell's
  world position (`:1372-1374`). The car footprint is not centred in a cell, so `adjustCoordToCell` returns
  `cell * 10 + 0.5` (`AIPathfind.cpp:9127`) — ~5 units away. The port now does the same, at the same frames (the AI
  module's creation wake frame, `AIUpdate.cpp:299-301`, is ported too), so the cars match at every sampled frame.
- **Start-up split** *(fixed 2026-10-21)*: the port now does what the C++ does — `AISkirmishPlayer::adjustBuildList`
  destroys the placed command center (`score_structure_undone`, `destroyObject`) and `newMap`'s `buildStructureNow` builds a
  fresh one, so the ids are the C++'s (208/210 placed and destroyed, 212/213 rebuilt). The frame-0 difference this bullet
  used to list (C++ `getObjectCount()` counting the two destroyed command centers) is gone since the writer
  walks the object list; the default-team `S` records exist on both sides too (below).
- **Civilian and observer money** *(fixed 2026-10-21)*: `Player::init` (`Player.cpp:427-443`) gives every player
  whose `PlayerTemplate` `Money` is 0 the game's starting cash, so the civilian (p1) and the replay observer (p4) have 10000;
  the port created them with 0. Both now follow the same rule as the skirmish slots.
- **Upgrade `Q total`** *(fixed 2026-10-21)*: `ProductionEntry`'s constructor leaves `m_productionQuantityTotal` at 0
  (`ProductionUpdate.cpp:163`) and `queueUpgrade` never sets it, so an upgrade entry prints `total=0`; the port set 1.
- **`Q door`** *(fixed 2026-10-21)*: the field is `ProductionEntry::m_exitDoor`. For a unit, `queueCreateUnit` stores
  `DOOR_NONE_AVAILABLE` unless the factory is an airfield, and `ProductionUpdate::update` stores the reserved door on the entry
  (`:710-716`) — `DOOR_1` (0) for every non-airfield exit (`QueueProductionExitUpdate.cpp:191`,
  `SpawnPointProductionExitUpdate.cpp:117`) — and clears it after a successful exit (`oneProductionSuccessful`, `:89`). The port
  only kept the airfield parking door and printed `-`. An upgrade entry keeps the constructor's value: the entry comes from the
  zeroed pool (`MemoryPool::allocateBlockImplementation`, `GameMemory.cpp:1708`) and no upgrade path writes `m_exitDoor`, so it
  prints `0`.
- **`T key`** *(fixed 2026-10-21)*: the entry prints `m_team->getPrototype()->getName()`, the prototype of the team the
  entry fills (`AIPlayer.cpp:2593`, `:1665`); the port printed its `proto#id` instance key. `TeamInQueue` keeps the instance key
  (the handle the AI uses) and the writer resolves the prototype through `ScriptEngine::team_proto_name`.
- **`S` default teams** *(fixed 2026-10-21)*: `TeamFactory::initFromSides` (`Team.cpp:217-241`) creates an inactive
  instance of every singleton prototype, then `Player::setDefaultTeam` (`Player.cpp:760-770`) activates each player's
  `team` + player name; the port now does the same at start-up, with the neutral/civilian/replay-observer prototypes from
  `SidesList::validateSides` and `GameLogic::startNewGame` (`GameLogic.cpp:1543-1550`). AN3 seed 1 has the same active teams as
  C++, with the same ids: 1 `teamreplayobserver`, 2 `team`, 3 `teamplyrcivilian`, 4 `teamskirmishgla0`, 6
  `teamskirmishamerica1` (5 is the inactive `GLA Worker` singleton).
- **`S members`** *(fixed 2026-10-21)*: `Object::m_team` and the team membership lists are ported, so the default
  teams print their real members (C++: every map object on the team named by its `originalOwner`,
  `PlayerList::validateTeam`, `PlayerList.cpp:294-308`; produced units on the owner's default team, `ProductionUpdate.cpp:773`;
  script/AI teams through `Object::setTeam`). The map objects' `originalOwner` names resolve through `TeamFactory::findTeam`
  and an unknown name falls back to the neutral default team. `--skip S.members` is no longer needed.

## Diff tool

`python tools/snap_diff.py PORT CPP` aligns the two files by frame (the `FRAME` headers) and by record key (`P p`,
`O id`, `Q obj i`, `T p q i`, `S id`), then compares fields:

- `--key tmpl` keys `O` records by template + per-frame creation ordinal instead of id (`Q` follows its factory's key,
  `S` its prototype + ordinal), for runs whose ids differ (see the id gap above). The records' own id fields are then
  not compared, and the ordinal shifts when an earlier object of the same template died on one side only.
- `--skip TAG.FIELD` ignores one field on both sides; `--skip TAG.FIELD=VALUE` drops the records with that value, and
  a trailing `*` in VALUE matches a prefix (`O.tmpl=Car*`). For the known gaps above: the output names what was skipped
  and how many records.
- `--from-frame N` compares frames >= N only (for the frame-0 start-up gaps). Frames before N are dropped before
  comparing, so a frame missing on one side there is not a difference either.
- Exact: every field except the floats: decisions and timing (states, owners, templates, queue contents, counts,
  team members, power sign).
- `x y z` within `--pos-tol` (default 1.0 world units); `hp` and `cons` within `--hp-tol` (default 0.5). The builds are
  not bit-identical, so these tolerances absorb float drift and nothing else.
- `crc` is never compared, not even with `--fields crc`. The `*b` bit fields are compared exactly only when named in
  `--fields` (`--fields pos` means `x y z`).
- A record present on one side only, and a frame present on one side only, are differences.
- The first divergence is the earliest frame that differs (frames are visited in ascending order).

Output: the first frame that differs, its differing records and fields (`O id=7: field hp: port 400.000, cpp 380.000`),
at most `--max-report` of them (default 12, then `... N more in this frame`), the first differing record's raw line on
each side, and the count of differing frames.

Input rules: lines that do not start with `SNAP ` are skipped. A malformed `SNAP` line that ends with a newline is
exit 2, as is a key repeated inside one frame. A final line without a newline is a write cut short by a killed run: it
is dropped together with its frame block, with a warning. A file with no `SNAP` records, on either side, is exit 2, and
so is an unreadable file, a bad `--skip` or `--from-frame` beyond the last frame.

Exit status (as `tools/rng_diff.py`): `0` identical over every frame, `1` a divergence, `2` unreadable, malformed, or
without records.

The self-test `python tools/test_snap_diff.py` (25 cases on synthetic snapshots) checks the exit codes, the reported
frame and field, both key modes, `--skip` and `--from-frame`.

## Writer tests

`crates/zr-game/tests/snapshot.rs` (synthetic INI and map, no game data): the header and record layout and their key
order, object id order, `-1` for complete objects and `-` for missing bodies and AI, the cadence (only the due frames,
once each), chunked runs of 20 and 30 frames (no double write at a chunk boundary), a CRC check that the writer leaves a
300-frame run unchanged (every frame, with bits), no writer is the plain run, two runs of one seed give byte-identical
snapshots, the sabotage gate, the `S` record (members sorted, proto lowercase), the escaping (uppercase hex), the
environment settings (`ZR_SNAPSHOT_EVERY` must be positive, `ZR_SNAPSHOT_BITS` is on only for `1`), and the object list
of a frame (`the_object_list_counts_objects_destroyed_earlier_in_the_frame`: a destroyed object keeps its `O` record and
its `objs` count until the frame's `processDestroyList`).

Not covered by tests: `Energy::is_power_sabotaged` itself (no sabotage setter exists, so only the gate formula is
tested), and the `ZR_FINE` single-step path in the demo (it needs game data; it is compiled and runs through
`snapshot::run_frames(..., 1, ...)`).

Python: `python tools/test_snap_diff.py` (25 cases on synthetic files): identical, the hp and position tolerances, the
earliest of two differing frames, `--max-report`, `--fields` and `crc`, a cut-off final line, a repeated key, an
unreadable file, a side with no records, a missing frame, a team member change, `--key tmpl` (objects, queues and teams
with different ids; a field difference; an extra object by ordinal), `--skip` (a field and a record value, with the
skipped counts) and `--from-frame` (drops earlier frames, and errors with no frame left).

## C++ printer checks

There is no unit test for the printf (it needs a game): the evidence is the AN3 comparison above. At frame 0 every
non-skipped object's `xb yb zb hpb consb` equals the port's bit for bit (`--fields xb yb zb hpb consb --key tmpl`), which
checks the `%.4f`/`%.3f` rounding and the `%08x` bit patterns; `hp` for a body-less object prints `-` on both sides. The
run with the snapshot ends on the same frame and winner as the run without it (`ZR_GAMEOVER frame=25218 winners=GLA(p2)`),
so it changes no game state. Two mistakes the first runs caught: the `InactiveBody` health rule (before it, `hp` was
`0.000` against the port's `-`) and the `ground` mapping (before restricting it to `AI_ATTACK_POSITION`, 14 frames printed
`atk=ground` where the port had no ground order).

## Control interface (`zr-harness`)

`crates/zr-harness` drives one game over JSON lines, so an orchestrator or a script can load a skirmish, step it, query
state, inject the same `Command`s replays use, write the snapshot above and toggle the RNG trace, without re-running a
whole demo. The binary owns the stdio loop; the library (`Session`, `protocol::handle_line`) is the same code, so tests
drive it directly.

```
cargo run -p zr-harness                 # install auto-detected, or ZR_ZEROHOUR_DIR
cargo run -p zr-harness -- --help       # the same summary as the `help` request
```

One JSON object per line on stdin, one JSON reply per line on stdout. Nothing else is written to stdout (diagnostics go
to stderr). An empty line is ignored. A request that fails is `{"ok":false,"error":"..."}` and the session keeps
running; an unknown field is ignored, an unknown `cmd`, a missing/wrong-typed field, a non-finite number or an unknown
command type is an error. A panic while handling a line is caught and reported the same way (with `"session stopped"`)
because the sim may be half-updated.

The session scans the installation and loads `GameData` once, on the first `load`; later `load`s reuse it.

### Requests

| `cmd` | Fields | Reply payload |
|---|---|---|
| `load` | `map` (default `Alpine Assault`), `seed` (default `1`), `cash` (the starting-cash option, applied to every slot without its own), `players` (required, 1-8 objects): `faction` (required, a `PlayerTemplate` name), `ai` (default `true`), `difficulty` (`easy`/`normal`/`hard`, default `normal`), `start` (default: the slot's array index), `team` (default `0`), `color` (default `-1`), `cash` (starting cash override, default: the game default); optional `snapshot` object (see `snapshot`) to open a streaming writer before frame 0 | `map`, `seed`, `frame`, `crc`, `crc32`, `objects`, `snapshot` (path or null), `players` (as `query players`) |
| `step` | exactly one of `frames` (a count) and `until` (a frame number; must not be before the current frame) | `frame`, `crc`, `crc32`, `objects` |
| `query` | `what`: `objects` / `players` / `ai` / `scripts` / `all` (required); `id` (an id or an array), `owner` (an index or an array), `template` (exact name), `kind` (a `KindOf` name, case-insensitive: `STRUCTURE`, `DOZER`, `VEHICLE`, ...), `player` (for `players`/`ai`), `limit` (objects); see below | `what` plus `objects`/`count`, `players`, `ai`, `scripts` |
| `command` | `player` (the issuing player index) and `command` (the command object, see below) | `player`, `frame` |
| `snapshot` | `path` (omit/null to stop a stream), `bits` (default `false`), `once` (default `false`), `every` (default 30). No `every` (or `once: true`) writes **one** block for the current frame; `every` opens a **streaming** writer (`ZR_SNAPSHOT` cadence) and the current frame is written now when it is due | `snapshot` (path or null), `frame`, and `every`/`bits`/`once` as applicable |
| `rng_trace` | `path` (start: creates/truncates the file) or `off: true` (stop); neither stops too | `rng_trace` (path or null), `traced`, `frame` |
| `crc` | - | `frame`, `crc` (8 lowercase hex), `crc32` (the number), `objects` |
| `quit` | - | `{"ok":true,"cmd":"quit"}` then the process exits |
| `help` | - | `protocol` and the command list |

`load` is `Game::new_skirmish_with`, the same start path as `skirmish_ai_demo` and the viewer; the crate does not
duplicate start-up logic. `player` must be an existing player index (0 is neutral, the skirmish slots follow). Commands
are queued (`Game::submit`) and applied at the start of the next `step`, exactly like a replay event at that frame. Note
that `GameLogic::crc` hashes the length of the queued message list, so a `crc` read between a `command` and the `step`
that applies it differs from the post-step state; compare CRCs after steps.

### Object, player, AI and script fields

`query objects` returns live objects in id order, after the filters:

```
{"id": 211, "template": "AmericaVehicleDozer", "owner": 2, "pos": [x, y, z], "angle": 1.2,
 "health": 300.0,                 // null when the object has no body module
 "ai": "Idle",                    // AiStateType name, null without an AIUpdate
 "attack": {"target": 7} | {"target": "ground", "pos": [x,y,z]} | null,   // current attack order
 "attack_phase": "Aim", "guard": {"target": 7} | {"pos": [x,y,z]} | null, "guard_mode": "normal",
 "move": [x,y,z],                 // current move destination (AIUpdate move goal), null when none
 "attack_move": false, "hunt": false,
 "build": 12,                     // dozer build-task target id, 0 for none
 "under_construction": false, "construction_percent": null,
 "queue": 0,                      // factory production queue length
 "contained_by": 0, "boxes": 0}
```

Positions have 4 decimals and health/construction 3 (like the snapshot). `query players` returns, for every player
(neutral/civilian/observer included): `index`, `name`, `kind` (`neutral`/`civilian`/`skirmish`/`observer`), `faction`,
`side`, `money` (the sim's current `Money::countMoney`), `is_ai`, `team`, `color`, `start`, `power`
(`production`, `consumption`, `sufficient`, `sabotaged`), `rank`, `skill_points`, `science_points`, `sciences`, `score`
(`ScoreKeeper::calculateScore`). `query ai` returns one entry per skirmish-AI player: `base_center`, `base_radius`,
`current_enemy`, `structures_built`, `attacks_launched`, `teams_built`, `base_defenses_built`, `powers_fired`,
`sciences_bought`, the candidate/failure counters, the structure/team timers and ready flags, `attack_group`, `built`,
`build_queue`/`ready_queue` (key, priority, started, reinforcement, recruit id, sent flag, work orders with their
`resource_gatherer` flag: the supply-truck orders are team-queue entries, `AIPlayer::queueSupplyTruck`) and `supply`. `query scripts` returns the script engine's `counters` (`name` -> value), `flags`
(`name` -> bool), `named_objects`, `actions_executed`, `conditions_evaluated`, `warnings`, `teams_created`,
`team_prototypes` and `trigger_areas`. `all` returns all four parts with the object/player/AI filters applied.

### Command JSON

`{"type": "<name>", ...}`. Type names are the `zr_sim::Command` variant names in `snake_case`; matching ignores case
and `_`/`-`, so `MoveTo`, `move_to` and `moveto` are the same. Positions are `[x, y, z]` or `{"x":..,"y":..,"z":..}`;
ids are unsigned 32-bit integers; `ids` lists are capped at 4096 entries and names at 256 bytes, and every float must
be finite after the `f32` conversion. Types and fields:

- `move_to`, `attack_move`, `force_move`, `add_waypoint`, `attack_ground`: `ids`, `pos`.
- `attack`, `force_attack`: `ids`, `target` (object id).
- `stop`, `scatter`, `toggle_overcharge`, `hack_internet`, `set_mine_clearing_detail`: `ids`.
- `guard`: `ids`, one of `target` (object id) and `pos`, optional `mode`
  (`normal`/`without_pursuit`/`flying_units_only`, default `normal`).
- `sell` (`building`), `evacuate` (`container`), `cancel_construct` (`dozer`), `repair`/`resume_construction`
  (`dozer`, `target`), `enter` (`ids`, `container`), `exit` (`container`, `ids`), `dock`/`get_repaired`/`get_healed`
  (`ids`, `target`), `queue_unit` (`factory`, `template`), `queue_upgrade` (`building`, `upgrade`), `cancel_unit`
  (`factory`, `index`), `cancel_upgrade` (`building`, `upgrade`), `set_rally_point` (`factory`, `pos`), `construct`
  (`dozer`, `template`, `pos`, optional `angle` in radians, default 0), `switch_weapons` (`ids`, `slot`),
  `do_weapon` (`ids`, `slot`, `max_shots`), `do_weapon_at_object` (`ids`, `slot`, `target`, `max_shots`),
  `do_weapon_at_location` (`ids`, `slot`, `pos`, `max_shots`), `combat_drop` (`ids`, one of `target` and `pos`),
  `purchase_science` (`science`), `do_special_power` (`source`, `power`, optional `target` or `pos`; neither is
  `SpecialPowerTarget::None`), `do_special_power_at_location` (`source`, `power`, `pos`, optional `angle`; absent/null
  is `INVALID_ANGLE`).
- `max_shots` is an `i32` (`-1` is the sim's unlimited). `"slot"`/`"index"` are integers in 0..=1,000,000.
- Possession messages (`Possess`/`PossessInput`/`Release`) are not exposed: the crate has no `possession` feature,
  which matches the default build.

### Snapshots and the RNG trace

`snapshot` and `load`'s `snapshot` option use `zr_game::snapshot` (`Snapshot`/`write_frame`), the same writer as
`ZR_SNAPSHOT`: a frame block is written for frame 0 before the first update and for every later multiple of `every`,
right after the update that reaches it. The file is byte-identical to a direct `snapshot::run_frames` run with the same
commands, which the synthetic test below asserts (including the `FRAME crc=` field). `snapshot` with no `every` writes
only the current frame (`snapshot::write_frame`), for a one-off check.

`rng_trace` starts or stops the per-draw trace at runtime (`GameRandom::set_trace_file`, the runtime counterpart of
`ZR_RNG_TRACE`); lines have the documented `RNG f=<frame> n=<draw> k=I|R ... at=<file>:<line>` format and start at the
frame the command arrives. The sink is flushed at the end of each logic frame (as `GameLogic::update` does), so the tail
of the current frame's draws reaches the file on the next step or when the trace is stopped. One trace per process: a
second `rng_trace` (or a second session in the same process) replaces the previous file. It reads and writes no
simulation state.

### Determinism

Only `step` (which advances time) and `command` (which queues a message) change the game; every query reads it (the
client-only-reads rule). A harness session that loads, commands and steps gives the same CRC as the equivalent direct
run: `crates/zr-harness/tests/synthetic.rs` builds two games from the same synthetic INI/map fixture as
`crates/zr-game/tests/synthetic.rs`, drives one through `Session::submit`/`Session::step` and the other through
`Game::submit`/`Game::run_frames`, compares the CRCs at every checkpoint and the streamed snapshot byte for byte. The
real-data test (`tests/real_data.rs`) loads AN3 (GLA vs America, seed 1), steps 300 frames, queries dozers, checks
players/AI/scripts, the snapshot cadence and the RNG trace, and compares the CRC with a direct `Game::new_skirmish_with`
run; it skips with a message when no installation is found.

## One-command compare (`tools/zr_compare.py`)

Builds the harness, loads a matrix matchup, streams a bits snapshot and the RNG trace, steps, and
prints the first exact divergence, the first **meaningful** divergence, the RNG mismatch with call sites, the first
divergent object's preceding frames and the port CRC. It replaces the hand sequence of build → `load`/`snapshot`/
`step` JSON lines → `snap_diff.py` → `rng_diff.py` → grep context.

```
python -I tools/zr_compare.py CODE [--seed N] [--frames N] [--every N] [--ref-dir DIR]
    [--cpp-snap FILE] [--cpp-rng FILE] [--cpp-coarse FILE] [--slot N] [--no-build]
    [--harness EXE] [--out DIR] [--skip TAG.FIELD[=VALUE] ...] [--no-skip]
    [--pos-tol X] [--hp-tol X] [--key id|tmpl] [--from-frame N] [--max-report N] [--max-minor N]
    [--rng-context N] [--timeline] [--tol-frames N] [--max-events N] [--timeline-every N]
    [--timeout SECONDS]
```

- `CODE` indexes `tools/matchups.json`, copied from `zh-cpp-build/zhbuild/matrix_run.py`'s table (AN1-AN7, AE1-AE3,
  AH1-AH7, TU1-TU8, FF1-FF2): port map display name, C++ replay map, difficulty and one faction per slot. Slots use
  start position = slot index and team 0 (the matrix's `-1`, free-for-all), so the port loads the same setup as the
  C++ replay. The seed is a separate argument.
- **Build:** `cargo build --release -p zr-harness` with `CARGO_TARGET_DIR=<repo>/target/slot-<N>` (absolute, default
  slot 1), from the repo root. `--no-build` uses the existing `target/slot-N/release/zr-harness[.exe]`; `--harness`
  points at another binary.
- **Run:** one harness process, requests piped in order: `load` (streaming snapshot at `--every`, bits, plus
  `ZR_RNG_TRACE` in its environment so start-up draws are recorded), `step until --frames`, stop the snapshot, `crc`,
  `quit`. Outputs go to `--out` (default `zr_compare\<CODE>_s<SEED>` in the system temporary directory): `port_fine.snap`, `port_rng.txt` and,
  with `--timeline`, `port_coarse.snap`. Only the frames both snapshots have are compared; the reference may run one
  frame past `--frames` (the C++ writer prints the frame after the update), and that tail is reported as a horizon
  note, not a divergence.
- **Report:**
  - (a) first exact divergence: frame, records and fields exactly as `tools/snap_diff.py` prints them, with the first
    raw line from each side;
  - (b) first meaningful divergence (see below), with every earlier difference listed as minor (frame, field,
    `port -> cpp`, size as a distance for positions/health or `exact`) and a count if `--max-minor` cuts the list
    short;
  - (c) first RNG mismatch: draw, frames, differing part, both full `at=` call sites (the C++ one included), and
    `--rng-context` draws (default 5) of context on both sides. A mismatch that differs only in the frame (same kind,
    range and value) is labelled a timing difference, otherwise a value difference that changes decisions;
  - (d) the first divergent object's `O` record for the 10 frames before its divergence, port and C++ side by side;
  - (e) the port CRC at the end.
- **Skip defaults:** `T.q=build T.q=ready` (the known AI-team-queue gap waves 14-16 carried) so the baseline is
  reproducible; `--skip` replaces them, `--no-skip` clears them. `crc` is never compared, as in `snap_diff`.
- **Exit status:** 0 identical on the compared frames and RNG, 1 a divergence, 2 an error (missing reference, build
  failure, harness error, malformed snapshot/trace).
- **Timeline (`--timeline`):** when `<ref-dir>/cpp_coarse.snap` (or `--cpp-coarse`) exists, runs the port again at
  `--timeline-every` (default 30, no bits) to the reference's last frame and runs `tools/timeline.py` on the two
  coarse files. The timeline matches structures placed/completed, units produced, first attack per owner, deaths
  (last seen), money at each minute and game end by `(owner, template, ordinal)`; a shift beyond `--tol-frames`
  (default 30 = one second) is flagged, and events on one side only are listed. Units are templates a factory is
  ever seen producing (`Q kind=unit`); structures are objects that ever show `cons >= 0`; debris, projectiles and
  other never-produced objects are only counted (`other objects created`), not listed. It can also be run directly:

  ```
  python -I tools/timeline.py PORT.snap CPP.snap [--tol-frames 30] [--max-events N]
  ```

### Reference store

The reference files live in `zr-refs/<CODE>_s<SEED>/` (default root `zr-refs` in the repo's parent directory,
i.e. next to the repo; not part of this repository). The names are fixed, so the tools agree:

| File | Content |
|---|---|
| `cpp_fine.snap` | every frame from one patched C++ run, bits on, to the run horizon (may end one frame after it) |
| `cpp_rng.txt` | the same run's `ZR_RNG_TRACE` (1964 draws for AN3 seed 1 to f3000) |
| `cpp_fine_f9000.snap`, `cpp_rng_f9000.txt` | the same recipe to f9000, seed 1 of the five matchups: `zr_compare` uses them automatically for `--frames` above 3000 when they exist |
| `cpp_coarse.snap` | optional: every 30 frames to game end, for `--timeline` |
| `manifest.json` | optional: what produced the run (code, seed, map, difficulty, factions, horizon, command, ...); the tool prints it and warns when code or seed disagrees |

`--cpp-snap`/`--cpp-rng`/`--cpp-coarse` override individual files, for a reference that is not in the store yet. `zr_compare.py` never writes
into the store.

### Meaningful divergences

Per AGENTS.md ("near bit-exact", 2026-10-28), the exact first divergence is a bug finder; the one to chase is the
first **meaningful** one. `zr_compare.py` calls a difference meaningful when it is:

- a decision/state field: `O.ai`, `O.atk`, `O.bld`, `O.owner`, `O.tmpl`, `O.q`; **any** difference in a `Q`, `T` or
  `S` record; `P.money`, `P.power`; `FRAME.objs`/`FRAME.players`;
- a record present on one side only (an object/queue/team created, destroyed or captured);
- an `O` position whose 3D `x`/`y`/`z` distance is at least 10 world units (one pathfinding cell).

Everything else is minor: `hp`, `cons`, `P.prod`/`P.cons`, the non-listed `P` fields, one position component below a
cell, the bit fields unless selected. The tool stops scanning at the first meaningful frame; later differences are
not classified (run again with `--from-frame` or a larger horizon). Minor differences are recorded and parked, not
fixed (AGENTS.md). Keep `--pos-tol` below 10: a larger tolerance hides a meaningful position drift from the diff.

### Checks

`python -I tools/test_zr_compare.py` (16 cases: meaningful vs minor, object presence, Q/T/S, hp/position
tolerances, common horizon, skip defaults, the report and object context, RNG mismatch/frame-only/tail) and
`python -I tools/test_timeline.py` (12 cases: extraction, ordinals, unit/debris classification, shifts
past/within the tolerance, one-sided events, first attack, money, deaths, CLI) use synthetic snapshots/traces only.
Both, plus `python -I tools/test_snap_diff.py` (25 unchanged cases), run without game data.

An example of the output, AN3 seed 1 to frame 3000 on the tree of 2026-10-28 (the current numbers are in the
roadmap): first exact divergence f1626 `O id=243`
(AmericaVehicleChinook) `y` 199.8795 vs 198.8220; first meaningful f2012 `O id=247` (AmericaVehicleChinook) at
10.7212 units; RNG draws 0-1688 match, first mismatch draw 1689 (same value, port f2042 vs C++ f2045,
`AIStates.cpp:1331`); port CRC f3000 `b963c29d`. Warm run (no build) 12.9 s (3.0 s harness, 7.3 s snapshot load);
a cold run adds the ~27 s release build. The tool follows the sim: after later `ai_states`/`locomotor`
edits were compiled that day, the same command gave first exact f1658 (`O id=243` y 170.3498 vs 169.3339),
first meaningful f2012 at 10.2193 units, draw 1689 at port f2043, CRC `02e87ba9`. Moving the first difference
*later* is the improvement; an edit that moves it earlier is the regression to report.

## Per-object trace (`ZR_TRACE`)

A standing per-object trace on both sides, so that localising a divergence does not need new printfs and a rebuild
of both builds. Both writers produce the same line for
every traced object every frame and `tools/trace_diff.py` aligns them by `(frame, id)`. It is read-only (the two
pre-existing cache writes below aside): the C++ side is a diagnostic in the patched build
(`GeneralsMD/Code/GameEngine/Source/GameLogic/System/GameLogicTrace.cpp`), the port side is
`crates/zr-game/src/trace.rs`.

```
cd zh-cpp-build\zhbuild
ZR_TRACE=<file> [ZR_TRACE_IDS=<ids|all>] [ZR_TRACE_FRAMES=<a-b>] python -I matrix_run.py 144000 --pairs AN3:1

# the port: the harness, either with ZR_TRACE set (opened by `load`) or with the `trace` request:
ZR_TRACE=<file> [ZR_TRACE_IDS=<ids|all>] [ZR_TRACE_FRAMES=<a-b>] zr-harness   # then the usual JSON lines
{"cmd":"trace","path":"obj.trace","ids":"243,209","frames":"1550-1700"}
```

- `ZR_TRACE` (a file path) turns the writer on; unset or empty, nothing runs. The file is created (truncated) at the
  first print. `ZR_TRACE_IDS` is `all` (ASCII case-insensitive, surrounding whitespace allowed) or a comma-separated
  object-id list (decimal, `1..2^32-1`; empty tokens skipped, a list with no id is malformed); `ZR_TRACE_FRAMES` is an
  inclusive `a-b` range or a single frame (`0` allowed at either end; an empty value is every frame). Both sides use
  this one grammar and behave the same on a bad value: one warning on stderr and the trace is off (no file). An
  unwritable `ZR_TRACE` path is also one warning and no trace, so the run continues. A large range with all ids
  writes one line per object per frame (gigabytes), so always set `ZR_TRACE_IDS`.
- **Print point:** the same as the snapshot. In C++, `GameLogic::update` calls `zrTracePrintFrameZero()` right after
  `startNewGame()` and `zrTracePrintFrame()` after `m_frame++`; the port writes frame 0 before the first step and every
  due frame after a step (`trace::run_frames`, the same loop as `snapshot::run_frames`). A frame is written once, even
  across chunked steps. Objects are listed in id order on both sides.
- **Line:** `TRACE f=<frame> id=<id> tmpl=<name> <name>=<value> ...`. Fields are separated by single spaces; string
  values escape `%`, space and `=` as the snapshot does (uppercase hex). Floats have 4 decimal places; the `*b` fields
  are the IEEE bit patterns (`%08x`, lowercase) of `x y z`, `ang` and `vx vy vz`. `x,y,z` packed values (path nodes,
  goal position) are three 4-place floats separated by commas. A value one side has no state for is `-`.
- **Fields, in order** (`port` = the expression in `crates/zr-game/src/trace.rs`, `C++` = the getter in
  `GameLogicTrace.cpp`):

| Field | Meaning | Port | C++ |
|---|---|---|---|
| `f`, `id` | logic frame and object id | `Game::frame`, `Object::id` | `GameLogic::getFrame`, `Object::getID` |
| `tmpl` | template name, INI case | `Object::template.name` | `Object::getTemplate()->getName()` |
| `x y z` `xb yb zb` | position (4 places, bits) | `Object::position` | `Object::getPosition()` |
| `ang` `angb` | heading in radians (4 places, bits) | `Object::angle` | `Object::getOrientation()` |
| `vx vy vz` `vxb vyb vzb` | physics velocity (4 places, bits) | `Physics::velocity` (`-` without a physics module) | `PhysicsBehavior::getVelocity()` (`-` without one) |
| `vmag` | velocity magnitude | `Physics::velocity_magnitude()` | `PhysicsBehavior::getVelocityMagnitude()` |
| `pfall pffree pfstun pfcoll` | `ALLOW_TO_FALL`, `IS_IN_FREEFALL`, `IS_STUNNED`, `ALLOW_COLLIDE_FORCE` (`0`/`1`) | the `Physics` flags | `getAllowToFall`, `getIsInFreeFall`, `getIsStunned`, `getAllowCollideForce` |
| `pfturn` | `PhysicsBehavior::getTurning()` (`-1`/`0`/`1`) | `Physics::turning` | `getTurning()` |
| `lbrake lbf ldonut` | `Locomotor::IS_BRAKING` (`0`/`1`), `m_brakingFactor`, `m_donutTimer`, printed when the object has an `AIUpdateInterface` **and** a current locomotor (and, on the port, a physics module) | the port keeps them on `Physics` (`braking`, `braking_factor`, `donut_timer`) | **patch** `Locomotor::zrGetIsBraking/zrGetBrakingFactor/zrGetDonutTimer` |
| `ai` | the AI state name (`docs/HARNESS.md`, "AI state names"). While an attack-move/hunt/attack-area/attack-follow machine runs its attack, C++'s `getCurrentStateID()` is the enclosing state (the attack runs in a sub-machine, `AIStates.cpp:1999-2006`, `:7122`, `:7329`); the port's main state is the attack, so it prints `attack_return` there | `AiMachine::current` (mapped through `shown_state`) | `getCurrentStateID()` + the snapshot's name table |
| `aisleep` | `StateMachine::m_sleepTill` of the AI machine | `AiMachine::sleep_till` | **patch** `StateMachine::zrGetSleepTill` |
| `aiwake` | the AI update module's sleepy-heap frame (`m_nextCallFrameAndPhase >> 2`) | `-` (gap) | `UpdateModule::friend_getNextCallFrame()` |
| `aiphase` | the update module's heap phase (`0`-`3`) | `-` (gap) | `UpdateModule::friend_getNextCallPhase()` |
| `sub` | the sub-machine state of the current AI state: dock (`Approach`, `WaitForClearance`, `Advance`, `MoveToEntry`, `MoveToDock`, `ProcessDock`, `MoveToExit`, `MoveToRally`), guard (`GuardInner`, `GuardOuter`, `GuardReturn`, `GuardIdle`, `GuardGetCrate`, `GuardAttackAggressor`) or the four `AI_ATTACK_*` states' attack machine (`Pursue`, `Approach`, `Aim`, `Fire`; an attack on a position prints `Pursue` while closing, because its `CHASE_TARGET` state *is* the approach state); `-` elsewhere, including `AI_ATTACK_MOVE_TO`/`AI_HUNT`/`AI_ATTACK_AREA`, whose immediate sub-machine is not the attack machine | `AiMachine::dock_sub`, `guard_sub`, `AttackOrder::phase` | **patch** `AIDockState/AIGuardState/AIAttackState::zrGetSubStateID()` |
| `goaltype` | `Locomotor` goal: `None`, `PositionOnPath`, `PositionExplicit`, `Angle` | `AiState::locomotor_goal_type` | **patch** `AIUpdateInterface::zrGetLocomotorGoalType` |
| `gx` | the locomotor goal data as `x,y,z` | `AiState::locomotor_goal_data` | **patch** `AIUpdateInterface::zrGetLocomotorGoalData` |
| `pcount` | path node count | `AiState::raw_path` nodes | `AIUpdateInterface::getPath()` walked with `getFirstNode`/`getNext` |
| `pfirst` `plast` `pnext` | first node, last node and the first optimised node after the first (`x,y,z` each) | the same `Path` (`nodes[0]`, `nodes.last()`, `nodes[0].next_optimized`) | `getFirstNode`, `getLastNode`, `getFirstNode()->getNextOptimized()` |
| `victim` | the snapshot's `atk` mapping: victim object id, `ground` in `AI_ATTACK_POSITION`, else `0` | `current_victim` (see the snapshot's "Victim (atk)") | `getCurrentVictim()` / `getCurrentVictimPos()` |
| `wslot` `wstat` `wnext` | current weapon slot, `WeaponStatus` name (`ReadyToFire`, `BetweenFiringShots`, `ReloadingClip`, `PreAttack`, `OutOfAmmo`) and the next frame it can fire; `-` with no weapon in the current slot (even when other slots are filled) | `WeaponSet::weapon(current_weapon_slot)`, `Weapon::status(frame)`, `when_we_can_fire_again()` | `Object::getCurrentWeapon(&slot)`, `Weapon::getStatus()`, `getPossibleNextShotFrame()` |
| `dtask` | the dozer task: `invalid`, `build`, `repair`, `fortify` | `-` (gap: the port does not store `m_currentTask`) | `DozerAIInterface::getCurrentTask()` |
| `dbuild` | the dozer build sub-task: `SelectBuildDock`, `MovingToBuildDock`, `DoBuildAtDock`; `-` otherwise. From the action dock on, both sides print the same value; during the walk to the action position C++ prints the **stale** `m_buildSubTask` (the setter only runs in the action machine's do-action state, `DozerAIUpdate.cpp:462-467`), which the port does not store | `AiState::dozer_action.sub` (the build variants) | `DozerAIInterface::getBuildSubTask()` |
| `dact` | the port's dozer action sub-machine state (`Inactive`, `PickActionPos`, `MoveToActionPos`, the three `dbuild` names) | `AiState::dozer_action.sub` | `-` (gap: `DozerActionStateMachine` is private to `DozerAIUpdate.cpp`) |
| `ssub` | the supply-truck state machine state (`Idle`, `Busy`, `Wanting`, `Regrouping`, `Docking`) | `Object::supply_ai.state` (the supply machine: a plain truck or the Chinook; `-` for a worker) | **patch** `SupplyTruckAIInterface::zrGetSupplyStateID()` (the `m_supplyTruckStateMachine` id; `-` for a worker, whose interface has no supply machine) |
| `cont` | the container id, `0` for none | `Object::contained_by` | `Object::getContainedBy()` |

- **Gaps** (a field one side cannot read is `-`): `aiwake` and `aiphase` are the C++ sleepy module's heap key; the port's
  AI runs from per-frame passes with a one-frame creation gate, so it has no equivalent (`o.ai.wake_frame` is `0` while
  awake, not the module's next call frame). `dtask` is not stored by the port. `dact` is not reachable in C++. `dbuild`
  matches from the action dock on, but not during the walk to it (the C++ value is then the previous task's leftover).
  `victim`'s gap is closed but it stays in the default skip set. `tools/trace_diff.py` skips exactly these six fields by default
  (`--all-fields` compares them, `--fields` names them explicitly), so a default diff's first difference is a real one.
  While an attack-move, hunt or area unit is fighting, both the trace (`shown_state`) and the snapshot's `O.ai` print
  the enclosing state, as the C++ does.
  `pcount`/`pfirst`/`plast`/`pnext`: the port's `set_ai_quick_path`/`compute_quick_path` builds the same two-node
  `getAircraftPath` `Path` as C++ (start node at the unit's position with the goal's z, then the goal), so the old
  `0/-` air gap is gone; the remaining difference is real: C++ `getAircraftPath` (`AIPathfind.cpp:5955-6019`) clips a
  wings destination around tall buildings and adds detour nodes, and the f379 `pcount 57->56` repath (`pfirst` becoming
  its own position, seen in AH7 seed 1) is a real C++ `doPathfind` re-path, not a trace artifact.
- **Objects not traced:** the port keeps some engine Objects as plain records (projectiles, laser beams, projectile
  streams), so the C++ writer omits them: every `KINDOF_PROJECTILE` Object, and every Object whose template name is one
  of the loaded weapons' `LaserName`/`ProjectileStreamName` templates (`Weapon::createLaser`/`newProjectileFired` create
  exactly those, `Weapon.cpp:2511-2560`). Their ids are still allocated on both sides; they simply have no `TRACE` line,
  so a shot in flight is not a one-sided record. Ambient `Amb_*` map markers are traced (they inherit
  `DefaultThingTemplate`'s `InactiveBody`, so a "no body" test would have dropped them wrongly).
- **C++ getters added** (const, read-only; no game logic changed): `Locomotor::zrGetIsBraking`,
  `zrGetBrakingFactor`, `zrGetDonutTimer` (private members); `AIUpdateInterface::zrGetStateMachine` (protected member),
  `zrGetLocomotorGoalType`, `zrGetLocomotorGoalData` (private members); `StateMachine::zrGetCurrentState`,
  `zrGetSleepTill` (private members; `peekSleepTill()` is `RTS_DEBUG` only); `AIDockState`, `AIAttackState` and
  `AIGuardState` `zrGetSubStateID()` (private sub-machines; the guard one is defined in `AIGuard.cpp` because
  `AIGuardMachine` is only forward-declared in `AIStateMachine.h`); `SupplyTruckAIInterface::zrGetSupplyStateID()`
  (the private `m_supplyTruckStateMachine`; the interface default returns `-1`, so a worker's supply interface prints
  `-`). The two calls in `GameLogic::update` and the two CMake source-list lines are the only other changes.
  Caveat: the trace calls two pre-existing non-const-clean getters that write mutable caches — `getVelocityMagnitude()`
  caches `m_velMag` (`PhysicsUpdate.cpp:941-947`) and `Weapon::getStatus()` may write `m_status` (`Weapon.cpp:2854`).
  Both are in the savegame `xfer` but not in either CRC (`Weapon.cpp:3281-3291`, `PhysicsUpdate.cpp:1939`), so the
  trace changes no sim CRC/RNG, but a savegame written immediately after a trace could differ.
- **Harness request:** `{"cmd":"trace","path":"obj.trace","ids":"243,209","frames":"1550-1700"}` starts the streaming
  writer (the current frame is written when due); `ids`/`frames` are strings in the environment-variable format and
  both default to everything; `{"cmd":"trace"}` or `{"cmd":"trace","off":true}` stops it. A `load` with `ZR_TRACE` set
  starts the same writer before frame 0 (the reply's `trace` field names the path), so a run can be traced without a
  request. `FileTrace::from_env()` is the same writer for any other caller.
- **Diff tool:** `python -I tools/trace_diff.py PORT CPP [--ids 1,2] [--fields x,y,z,ai] [--all-fields] [--tol X]
  [--window N] [--max-report N]`. It aligns by `(frame, id)`, compares every **non-gap** field by default (the six gap
  fields above are skipped unless `--fields` names them or `--all-fields` is given; two values that both parse as floats
  are compared numerically; packed positions component by component; `--tol` is the absolute tolerance, default `0` =
  the same printed value), and prints the first differing frame and field per object, the first difference over all
  objects, and with `--window N` a side-by-side window of N frames either side of it showing only the fields that
  differ there. A frame missing for an object, or an object present on one side only, is a difference. Lines that do
  not start with `TRACE ` are ignored; a final line without a newline is dropped with a warning; a malformed line or a
  repeated `(frame, id)` is exit 2. Exit status: `0` identical, `1` a divergence, `2` unreadable/malformed/empty or a
  bad option. Self-test: `python -I tools/test_trace_diff.py` (16 cases, including the default gap skip).
- **Writer tests:** `crates/zr-game/tests/l141_trace.rs` (synthetic INI and map): the field set and order, id order, the
  `-` gaps for a bodyless/no-AI object, the id and frame filters (each due frame once across chunked steps), a CRC check
  that a 300-frame run with the trace on every frame is unchanged, two runs of one seed byte-identical, a moving
  tank's path/heading fields, the position-attack `Pursue` and attack-move/hunt/area `sub=-` mappings (with a real
  attack order acquired), an empty current slot printing `wslot=-`, the `lbrake` AI gate and a flush per written frame.
  `cargo test -p zr-game --test l141_trace`.
- **C++ check (AN3 seed 1, 2026-10-28; re-checked after a review's fixes).** `ZR_TRACE=… ZR_TRACE_IDS=209,243
  ZR_TRACE_FRAMES=1550-1700 ZR_RNG_TRACE=… ZR_SNAPSHOT=… ZR_SNAPSHOT_EVERY=1 ZR_SNAPSHOT_BITS=1 python -I matrix_run.py 3000
  --pairs AN3:1` wrote the 272 expected lines and produced a snapshot byte-identical to the stored `cpp_fine_f3000.snap`;
  the RNG draws are identical to `cpp_rng_f3000.txt` (only the `at=` line numbers in `GameLogic.cpp` shifted,
  because the two trace calls add two lines). After the review fixes (the `ssub` getter, the `GuardRetaliate`
  name in both tables, the projectile/INERT/bodyless-Object filter and the strict env grammar) the same run with
  `ZR_TRACE_IDS=209,246 ZR_TRACE_FRAMES=2000-2060` wrote 122 lines, its snapshot is still **byte-identical** to the
  reference and `rng_diff` reports **all 1964 draws identical**: the trace changes no game state. The trace-diff check
  over that window now reports a real first difference (f2000 `id=209` `y` 2210.3765 vs 2210.3767, the documented
  ~2e-4 drift; `id=246` `wnext` 1848 vs 1847) instead of the old `aiwake`/`dtask`/`dbuild`/`victim` false positives. A
  separate run with `ZR_TRACE_IDS=209,243,246` shows the Chinook's `ssub=Docking` on both sides (the supply machine,
  not the standard AI machine: finding 1 on real data).
- **Sub-tolerance drift:** the default `--tol 0` is much stricter than the snapshot diff's `1.0` position tolerance, so
  the trace shows drift the snapshot calls identical. On the 2026-10-28 tree object 209
  differed in `y`/`ang` by ~2e-4 from frame 1550, and object 243's first movement difference was `x` at f1640
  (765.6747 vs 765.6727); at `--tol 1.0` the same run's first difference was f1658, matching `tools/zr_compare.py`.

## Hierarchical path diagnostic (`ZR_HPATH`)

`ZR_HPATH=<first>-<last>` (or a single frame) dumps every ground path request's hierarchical
corridor on both sides, in the same shape, read-only. Unset means off.

```
# C++ (src-patched, read-only printf): one run, diagnostics into runs/matrix/out/<CODE>_s<SEED>_h<H>.out
cd zh-cpp-build\zhbuild
ZR_HPATH=2010-2030 python -I matrix_run.py 2025 --pairs AN3:1

# port: the harness writes them to stderr
ZR_HPATH=2010-2030 zr-harness   # then the usual JSON lines
```

- **Lines.** `req=findPath|findClosestPath|findAttackPath|patchPath|getMoveAwayFromPath` (object id, from/to),
  `req=hier` (from/to, surfaces, crusher, closestOK, the start/goal effective zones), `hierpop`/`hierpush` (cell,
  block, block zone, `g`, total), `hier=ok|closest|fail`, `req=hier id=0 corridor <bx,by>...` (the blocks whose
  `ZoneBlock::m_markedPassable` the search set, i.e. the corridor the fine A* will use) and
  `req=findPath id=<id> fine=ok n=<nodes> first=(x,y) next=(x,y) last=(x,y)` / `fine=fail`.
- **Reading a mismatch.** Compare the request, then the first `hierpop`/`hierpush` that differs (a missing pop is
  the interesting one: a rejected scan cell usually means the `newCell->hasInfo()` gate or a zone comparison), then
  the corridor block set, then the fine path. The block set is only meaningful after a `hier=ok`; on `hier=fail`
  `findPath` calls `setAllPassable` and the corridor is every block.
- **Cost.** One line per request plus one per coarse pop/push; a busy frame with a 300-cell path is a few hundred
  lines. It changes no state (verified: the AN3 s1 snapshot is byte-identical and all 1964 RNG draws match).
