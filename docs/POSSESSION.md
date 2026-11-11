# Possession mode — design

The player selects one of their units and takes direct control of it in third-person
(or first-person) view. The rest of the match continues as normal Zero Hour, and releasing
the unit hands it back to the AI.

**Build:** possession is the cargo feature `possession` (off by default; decided 2026-10-06), so
the default build is the plain Zero Hour port. `cargo run --release -p zr-viewer --features possession`
builds it in. The feature gates `zr-sim::possession`, the `Possess`/`PossessInput`/`Release`
commands and their replay codes, and the viewer's `possess.rs`/`radial.rs`.

Names below refer to the original C++ (`GeneralsMD/Code/GameEngine`); the Rust port keeps the
same concepts so possession fits in without special cases elsewhere.

## Principles

1. **Possession is just another input to the simulation.** It never bypasses game rules: speed,
   turn rate, weapon range and reload, armor and damage come from the unit's own INI data.
2. **Deterministic.** Possession input is sent as logic messages, like every other command, so
   replays and future multiplayer stay consistent.
3. **Zero Hour stays intact.** Economy, production, AI opponents, superweapons and general's
   powers are unaffected. Possession is an optional layer on top.

## Simulation side

New logic messages (network range `MSG_BEGIN_NETWORK_MESSAGES`..`MSG_END_NETWORK_MESSAGES`):

| Message | Payload | Effect |
|---|---|---|
| `Possess` | object id | Validates ownership, the unit is alive, and the unit isn't contained or a structure. Marks the unit as possessed. |
| `PossessInput` | move vector (quantized), aim point, fire / alt-fire bits | Sent every logic frame (30 Hz) while possessing |
| `Release` | — | Clears possession; the AI resumes, idle by default |

The `Possessed` state on the object works as follows:

- **AI suspended.** `AIUpdateInterface` stops running its state machine and clears its path and
  goals. Auto-acquire of targets is disabled for the possessed unit only.
- **Movement.** Driven through the unit's existing locomotor:
  - ground units: `Locomotor::locoUpdate_moveTowardsPosition(obj, pos + move*lookahead, …)`, or
    `locoUpdate_moveTowardsAngle`
  - no input: `locoUpdate_maintainCurrentPosition`

  Each locomotor type keeps its feel. Ground, hover and thrust units steer with the world-space move
  vector; fixed-wing aircraft read it as throttle (`move_y`) and turn (`move_x`) and can never stop.
  `ASCEND` / `DESCEND` move the altitude goal for air units (hover, thrust, wings), clamped to the
  locomotor's envelope (surface + 2 up to twice its cruise height). `PhysicsBehavior::applyMotiveForce`
  is the fallback if a locomotor goal proves too sluggish. (Addition: a jet parked or taxiing at its
  airfield switches to its normal locomotor when possessed and starts with a cruise-height altitude
  goal, so it flies off from the parking space without the runway cycle; `jet_begin_possession`. On
  release the jet AI treats it like any airborne jet: it returns after `ReturnToBaseIdleTime` or when out of ammo.)
- **Aiming.**
  - Turreted units: `AIUpdateInterface::setTurretTargetPosition(turret, aim)`. The turret slews
    at its INI turn rate, so a Crusader feels like a Crusader.
  - Non-turreted units (infantry, most aircraft): the body turns toward the aim point.
- **Firing.** On the fire bit, if the current weapon is ready and the aim point is in range,
  `Weapon::fireWeapon(source, &aimPos)` (fire at position). Hitscan-style infantry weapons use
  a deterministic ray from the muzzle through the aim point against the partition manager and
  terrain to pick the victim object, then fall back to the ground position. Projectiles,
  damage, armor, veterancy and OCLs then work unchanged.
- **Abilities.** Alt-fire maps to the unit's special-power or ability buttons, such as Black Lotus
  hacking, Jarmen Kell sniping, or the Burton knife, using the existing command-button
  paths with the aim point as target.
- **Exit conditions.** Death, transport entry, being captured or hijacked, or a player-control
  change all release automatically.

### Decisions made in the implementation (`zr-sim/src/possession.rs`)

- The move vector is **world XY**; the client converts camera-relative input before sending. Its length (clamped to 1) scales the unit's top speed; speeds and turn rates are otherwise the unit's own.
- A frame without `PossessInput` is neutral (no move, no buttons); the last aim point is kept.
- Non-turreted units turn the body toward the aim only when there is no move input; locomotors cannot strafe.
- Firing requires heading alignment (`AcceptableAimDelta`, at least ~2 degrees), range and a ready weapon, and honours pre-attack delay. Projectile weapons fire at the aim point; instant weapons use the hitscan ray (enemy objects only, allies/neutrals do not block, terrain stops it).
- One possessed unit per player; possessing another switches. Possessed units ignore move/stop/attack orders. `ALT_FIRE` fires the possessed unit's first `SpecialAbilityUpdate` ability (or special power) at the aim point through `Command::DoSpecialPower`: Black Lotus capture-building / disable-vehicle / steal-cash, Burton charges, hacker cash, and Jarmen Kell's `DAMAGE_KILL_PILOT` snipe (via the weapon) all run their real effects. Pack/unpack/facing polish is not ported.
- Containment is implemented (`zr-sim::contain`): `Possess` refuses a contained unit and the possession pass auto-releases a unit that boards a transport/garrison (`Object::contained_by`). Passengers that are allowed to fire (garrisons, open transports) shoot from the container (`zr-sim::contain`/`garrison`), so a possessed unit that boards one is released first.
- Aircraft are locomotor-faithful (the open question below, resolved for now): hover and thrust steer with the world move vector and change altitude with `ASCEND`/`DESCEND` (Space/Ctrl button bits); fixed-wing throttle/turn and cannot stop. The altitude goal is hashed into the possession CRC, so replays stay byte-exact.

## Client side

Status: third-person camera (over-the-shoulder offset, terrain + structure collision, optional
`T` auto-follow), first-person bone cameras (`V`: turret / cockpit / head), controls, crosshair and
HUD are implemented in `zr-viewer/src/possess.rs`; the radial command menu is implemented in
`zr-viewer/src/radial.rs` (a possession-only addition, below); the controller layout and
client-side prediction are not (prediction isn't needed offline).

- **Camera.** Third-person spring-arm orbit behind the unit (Renegade `ccamera.cpp` ratios, shifted
  over the shoulder so the crosshair ray clears the unit), collision against terrain and structure
  geometry. The view direction is fixed by yaw/pitch and the arm only slides along it, so pulling in
  near geometry never rotates the camera (and the camera never ends inside the unit or looks at the
  sky). `V` toggles first person:
  - vehicles: a camera on the model's first `CAMERA` / `TURRET` / `DRIVER` / `BARREL` / `MUZZLEFX` /
    `FIREPOINT` bone (`Hierarchy::find_bone`), falling back to a body-height offset
  - infantry: the `BIP01 HEAD` / `HEAD` bone; the unit's own meshes are hidden (there are no viewmodel
    arms in Zero Hour assets)
  Aircraft get a longer third-person arm and are not clamped to the ground.
- **Controls.** Renegade-style WASD + mouse aim, LMB fire, RMB alt/zoom, E release, and a
  controller layout. The RTS UI collapses to a minimap, credits/power, and a radial menu
  for the possessed unit's command buttons.
- **Radial menu** (`zr-viewer/src/radial.rs`, possession-only — the engine has no such menu). Hold
  `Q` (or the middle mouse button) while possessing to open a ring of the possessed unit's
  `CommandSet` buttons, decoded through the same `MappedImage`/`ButtonImage` path as the control bar.
  Mouse motion aims the highlighted sector (the cursor is locked for possession look), and releasing
  the key activates it, dispatching through the control bar's `dispatch_button` so the issued
  `Command`s are identical. Targeted abilities (`SPECIAL_POWER` with `NEED_TARGET_POS`,
  `ATTACK_MOVE`, `GUARD`) use the crosshair's aim point instead of the RTS placement mode. The
  control bar and its `&` hotkeys are suppressed while possessing, and so are the `CommandMap.ini` meta keys
  (`meta_keys.rs`, also for the frame possession ends, so the release key cannot fire a meta).
- **Feel.** Interpolate the possessed unit at render rate between 30 Hz logic frames and
  predict local movement and aim. Reconcile against the sim each frame. This is free
  offline (online play is out of scope).
- **Visual fidelity.** Zero Hour LODs are built for the overhead camera. Implemented: the
  time-of-day skybox (`W3DWater`'s `new_skybox`, `GameData` `DrawSkyBox` / `SkyBoxScale` /
  `SkyBoxPositionZ`) so the black void beyond the map reads as sky; anisotropic trilinear filtering on the terrain
  atlas and on the non-alpha-tested model textures, possession builds only (`cfg(feature = "possession")`; the plain
  build uses the engine's `TextureFilter`). Every other texture family (roads, bridges, decals,
  particles, water, UI, the shroud overlay) keeps its engine sampler in the possession build too, and the
  `TextureFilter`/`TextureReduction` options apply in both builds; decal shadows for decal-typed objects
  (`W3DProjectedShadow`: `ShadowSizeX/Y` / `ShadowTexture`, offset along the map's sun direction);
  and a subtle possession-only distance fog (the engine has no fog setting, so this is a documented
  addition); vehicles and structures cast shadow-map shadows (approximating the engine's stencil
  volumes). **LOD:** the close camera already draws the highest detail — `zr-w3d` assembles only
  the highest-detail HLOD LOD for every model (`model.rs::top_lod`, the largest-`MaxScreenSize`
  array per `WW3D2/hlod.cpp`), so there is no per-distance LOD to force (`HLodClass::Set_LOD_Level`
  is not ported). Still open: higher-detail replacement models as user-provided content.
- **Testing.** All of the above is verified with tests, scripted input and screenshots; it has not
  yet been playtested by a human with mouse and keyboard.

## Gameplay rules (tunable)

- Who can be possessed: any owned non-structure unit. Configurable later, for example "heroes and
  vehicles only".
- Balance: there's no inherent stat bonus, but a human aiming beats the AI, so an optional
  per-template `PossessionDamageScalar` is available if needed.
- Skirmish AI is unaware of possession; consider letting it focus fire on possessed units.

## Renegade as a reference

Renegade's source (`references/CnC_Renegade`) is the reference for:

- character controller and step-up and slope handling
- third-person camera behaviour
- vehicle entry and exit
- hit-detection feel

Behaviour is ported, not assets. Renegade's GDI and Nod content doesn't map onto the USA, China and GLA factions.

## Open questions

- Should possessing a unit spend a resource or have a cooldown, as a general's power, or be free?
- Aircraft control scheme: locomotor-faithful is implemented; whether an arcade (Renegade-style
  flying-vehicle) scheme feels better is still open.
- Multiplayer: out of scope (online play was dropped 2026-10-05, see ROADMAP). Possession stays
  deterministic so replays work.
