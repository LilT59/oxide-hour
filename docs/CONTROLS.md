# Controls and command-line flags

How to drive the viewer (`zr-viewer`), and the flags and headless programs used for testing. For what the project is
and how to build it, see the [README](../README.md).

## Starting

With no arguments the viewer opens the real start screen (`Window\Menus\MainMenu.wnd`, drawn over the 3D shell map
like retail). **Solo Play** opens the single-player submenu; **Skirmish**, **Options** and **Exit Game** work.
Skirmish opens `SkirmishGameOptionsMenu.wnd`: the map list with previews, a human and up to 7 AI slots with
difficulty, faction (including the generals), colour, team and start position, starting cash and the
superweapon-limit toggle. **PLAY GAME** shows the load screen and then the map.

- Any command-line argument skips the menu and starts the default Alpine Assault skirmish. `--quick` is the explicit
  form; `--menu` forces the start screen.
- Another map: `cargo run --release -p zr-viewer -- "tournament desert"`.

## In a skirmish

Bindings are the retail ones from `Data\INI\CommandMap.ini`.

### Camera

- Arrow keys pan; the screen edges scroll; the wheel zooms.
- Keypad 4/6 rotate, keypad 8/2 zoom, keypad 5 resets.
- Middle-drag rotates; middle-click resets the angle, pitch and zoom.
- Right-drag scrolls the map (with Alternate Mouse only if `UseRightMouseScrollWithAlternateMouse` is set).
- `--dev` enables a non-retail development camera: WASD pan, Q/E rotate, F free-fly. It is off by default because
  retail binds those letters to other commands.

### Selection

- Left click selects a unit (Shift toggles); left drag box-selects.
- Double-click selects all units of that type on screen (with Alt: on the whole map).
- **Q** select all, **W** select all aircraft, **E** select matching units.
- Ctrl+Left/Right previous/next unit; Ctrl+Up/Down previous/next worker; Ctrl+H select hero.
- **H** centres on the command centre.
- Control groups: Ctrl+0..9 creates, 0..9 selects (twice to centre the camera), Shift+N adds, Alt+N centres.

### Orders

- Default mouse scheme (retail's): a left click on an enemy attacks it, on terrain moves, on an own unit selects it.
  A right click deselects. A double left click guards the point.
- With **Alternate Mouse** in the options, the right button orders and the left only selects.
- With a factory selected, an order click on terrain sets its rally point.
- Hold **Ctrl** to force-attack (attack the ground, or fire on any object).
- Hold **Alt** for waypoint mode: each order click appends a waypoint.
- Hold **Shift** to prefer selection, so own objects are not order targets.
- Attack-move is the control bar's `A` button or hotkey, followed by a left click. Force-move has no default key
  (it is commented out in retail's `CommandMap.ini`).
- **S** stop, **X** scatter.
- Control bar buttons work by click or by their hotkey letter: build units and upgrades, construct (green/red
  placement ghost, Q/E rotate), attack-move, guard, sell, stop, rally point and special powers.
- On the radar, the scheme's order button orders the selection to move and the other button moves the camera.

### Interface

- **Space** centres on the last radar event. **F9** toggles the control bar.
- **Esc** cancels a pending order or placement first, then closes the options or science panel, and otherwise opens
  the pause menu (Resume / Restart / Options / Quit to Setup).
- **F10** (or the pause menu) opens the options: resolution, fullscreen, music / effects / voice volumes and scroll
  speed, saved to `options.ini` in the user data directory.
- The star button on the control bar opens the general's promotions panel; the shortcut bar lists the purchased
  powers with their cooldowns.
- When the last player loses their buildings, the victory or defeat screen appears.

## Possession mode

Only in builds with `--features possession`.

- Select a unit and press **P** to take control; **E** releases it.
- Mouse look, wheel zoom, crosshair in the screen centre. **V** toggles first person. **T** toggles auto-follow.
- WASD moves relative to the camera. Space / Ctrl change altitude for aircraft.
- Left mouse fires; right mouse uses the unit's special ability.
- Hold **Q** (or the middle mouse button) for the radial command menu: a ring of the possessed unit's command
  buttons. Move the mouse to highlight one and release to issue it. Targeted abilities fire at the crosshair.

Design and rules: [POSSESSION.md](POSSESSION.md).

## Replays

- `--record [name]` records the command stream, the setup and seed, and periodic checksums to the user data
  directory.
- `--replay <file>` plays a recording back and reports any checksum desync.

The user data directory is `%APPDATA%\OxideHour` on Windows and `~/.oxide-hour` elsewhere. A directory left by an
older build under the earlier working title (`ZeroHourRenegade`) is renamed on first use.

## Verification flags

Used for scripted runs and screenshots.

| Flag | Effect |
|---|---|
| `--screenshot out.png` | Save a screenshot and exit (mutes audio). `--shot-frame N` picks the app frame (default 90, counted from app start including menu frames). |
| `--camera x,y,zoom`, `--camera overview` | Start camera |
| `--sim-frames N` | Run N logic frames before the scripted action or screenshot |
| `--menu`, `--quick` | Force the start screen / skip it |
| `--menu-single`, `--menu-setup` | Open the single-player submenu / stop on the skirmish setup screen |
| `--load-screen` | Hold the load screen (with `--screenshot`) |
| `--auto-start`, `--auto-quit`, `--auto-pause` | After a short delay: click Start / quit to the setup screen / open the pause menu |
| `--open-science`, `--open-options` | Open the promotions panel / the options |
| `--force-end win\|lose` | Show the victory or defeat screen |
| `--no-sim` | Do not step the simulation |
| `--no-game-end` | Never run the victory check (for long `--sim-frames` runs) |
| `--ai-view` | Start with the camera on the AI player |
| `--dev` | The non-retail development camera |
| `--mute` | No audio |
| `--spawn <template>` | Frame one animated instance of a template |
| `--fx <FXListName>` | Play one effect list |
| `--select-first-structure` | Select a structure so the control bar shows its commands |
| `--duel` | A Crusader and a Battlemaster fight in the open. `ZR_DUEL_A=<template>` / `ZR_DUEL_B=<template>` swap the units. |
| `--chinook-demo` | Board Rangers and order a combat drop |
| `--superweapon-demo` | Spawn the particle uplink cannon and fire |
| `--construct-demo` | The dozer starts an American power plant beside it |
| `--possess` | Possess the America dozer after `--sim-frames N` (possession builds) |
| `--script-input "W:60,D:30"` | Held keys per logic frame, with `--possess`: W A S D move, F fire, R alt-fire, `-` idle (possession builds) |
| `--radial-demo` | Hold the radial menu open (possession builds) |

Environment variables:

| Variable | Effect |
|---|---|
| `ZR_ZEROHOUR_DIR`, `ZR_RENEGADE_DIR` | Override install detection |
| `ZR_PERF=1` | Print frame time, simulation cost per logic frame and asset counts every 5 s. `ZR_PERF=uncapped` also turns vsync off. |
| `ZR_RNG_TRACE=<file>` | Write one line per logic random-number draw |
| `ZR_SNAPSHOT=<file>` | Write a per-frame state snapshot (`ZR_SNAPSHOT_EVERY=<N>` frames, default 30; `ZR_SNAPSHOT_BITS=1` adds float bit patterns) |
| `ZR_TRACE=<file>` | Per-object trace, frame by frame |

The snapshot, trace and diff tools are described in [HARNESS.md](HARNESS.md).

## Headless programs

All run against a real install.

- `cargo run --release -p zr-game --example skirmish_ai_demo -- FactionChina FactionAmerica` runs an AI-vs-AI game to
  a winner. It also takes a difficulty (`easy` / `hard`; omit for Normal), `--seed N` (default 1) and `--map NAME`
  with more than two factions (4-player free-for-all games are played to elimination). It runs each game twice,
  asserts identical checksums, and prints per-minute money, units, dozers, supply centres and team/attack counts.
- Single-system examples in `zr-game`: `tank_duel`, `possess_duel` (possession builds), `economy_demo`,
  `production_demo`, `repair_demo`, `special_power_demo`, `contain_demo`, `tunnel_demo`, `garrison_demo`,
  `gla_hole_demo`, `air_demo`, `bridge_demo`, `veterancy_demo`, `chinook_demo`, `superweapon_demo`, `collide_demo`,
  `wall_demo`, `dock_actions_demo`, `rider_demo` and `human_vs_ai_demo`.
- `script_dump <filter>` prints matching scripts from `SkirmishScripts.scb`.
- `command_soak [--frames N] [--seed S] [--replay]` stress-tests every player command, including invalid ones, for
  panics, hangs and desyncs.
- `cargo run --release -p zr-probe -- summary` and `-- parse` smoke-test asset loading;
  `cargo run --release -p zr-ini --example parse_all` parses every INI file.
- `python -I tools/zr_compare.py AN3 --seed 1 --frames 3000` compares a matchup with the C++ reference
  (see [HARNESS.md](HARNESS.md); needs the reference store, which is not in the repository).

## Retail reference screenshots

To compare with the retail game: start Zero Hour windowed (Steam launch option `-win`), set up the scene, then

```powershell
pwsh tools/refshot.ps1 -Name alpine -Ours -ViewerArgs '--quick'
```

captures the game window and this viewer side by side into a folder outside the repository. The images contain game
art, so never commit them.
