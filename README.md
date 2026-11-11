# Oxide Hour

An unofficial Rust/Bevy re-implementation of the *Command & Conquer: Generals – Zero Hour* game logic, ported
faithfully from the GPL source that EA released. It reads the game data from your own install; no game assets are
included.

It also has an optional **possession mode** (cargo feature `possession`, off by default): take direct third- or
first-person control of one of your units, then hand it back to the AI. The default build contains no possession code.

> Fan project. Not affiliated with or endorsed by Electronic Arts. *Command & Conquer*, *Generals*, *Zero Hour* and
> *Renegade* are trademarks of their owner and are used here only to say what this project is compatible with.

## How this is written

The code is written by AI coding agents under the owner's direction. The owner sets the goals and the rules, and
reviews the results; the agents port the original C++ subsystem by subsystem. The rules they work under are in
[AGENTS.md](AGENTS.md): the original C++ is the specification, behaviour is ported and not invented, every fix needs a
test that fails without it, and the simulation is compared frame by frame with a build of the original source.

## Status

This is a work in progress and not a finished game.

| Area | State |
|---|---|
| Data loading (archives, maps, INI, models, textures, text, audio) | Works. All 150 shipped maps and 221 of 222 INI files parse; 2115 object templates are typed. |
| Skirmish against the AI | Works. Real main menu, skirmish setup and load screen; control bar, radar, construction, production, economy, power, upgrades, veterancy, special powers, general's promotions, fog of war, audio, effects, a score screen and replays. The AI runs the retail skirmish scripts. |
| Faithfulness to the original | Partly. On five AI-vs-AI matchups on one map the simulation matches the original C++ build on every compared field for the first 3000 frames (100 seconds) and parts from it between frames 3800 and 4900. Other maps and human-input games are not yet compared. |
| Possession mode | Partly. Works for ground, hover and air units in third and first person, with a radial command menu. Verified by tests and scripted input only; no person has playtested it yet. |
| Playtesting | Partly. One human playtest of skirmish so far; many visuals have no side-by-side comparison with the retail game yet. |
| Rendering | Partly. Terrain, models, animation, particles, shadows and water are drawn. Water reflections, cloud shadows and stencil shadow volumes are not. |
| Save / load, campaign | Not yet. The campaign is deferred until skirmish is done. |
| Online multiplayer | Out of scope. |

Details: [docs/ROADMAP.md](docs/ROADMAP.md) (status and open work), [docs/PORTED.md](docs/PORTED.md) (what each
subsystem ports and which C++ it follows), [CHANGELOG.md](CHANGELOG.md) (history).

## Requirements

- Rust 1.86 or newer.
- Your own copy of **Command & Conquer Generals – Zero Hour**. The Steam version is found automatically
  (`Steam\steamapps\common\Command & Conquer Generals - Zero Hour`, which includes the base game archives in
  `ZH_Generals\`). Set `ZR_ZEROHOUR_DIR` to use another install.
- Optional: **Command & Conquer Renegade** (`ZR_RENEGADE_DIR`), used as a reference for the possession camera and
  controller, and by the asset smoke tests.

## Run

Start the game at the main menu:

```bash
cargo run --release -p zr-viewer
```

Skip the menu and start a skirmish on Alpine Assault:

```bash
cargo run --release -p zr-viewer -- --quick
```

Build with possession mode (select a unit, **P** to take control, **E** to release):

```bash
cargo run --release -p zr-viewer --features possession
```

Run an AI-vs-AI game without a window:

```bash
cargo run --release -p zr-game --example skirmish_ai_demo -- FactionChina FactionAmerica
```

Keys, mouse, command-line flags and the other headless demos are in [docs/CONTROLS.md](docs/CONTROLS.md).

## How it is verified

- **Tests and lints.** `cargo test --workspace` and `cargo clippy --workspace --all-targets`, with and without the
  `possession` feature. Tests use synthetic data, or skip when no install is found.
- **Asset smoke tests** against a real install: `cargo run --release -p zr-probe -- parse` and
  `cargo run --release -p zr-ini --example parse_all`.
- **Determinism.** The simulation runs at 30 logic frames per second with a seeded random number generator and no
  platform-dependent maths. The headless demos run every game twice and compare checksums, and replays report any
  desync.
- **Comparison with the original.** A headless build of the community-maintained C++ source
  ([TheSuperHackers/GeneralsGameCode](https://github.com/TheSuperHackers/GeneralsGameCode)) writes a per-frame state
  snapshot and a trace of every random number drawn. `tools/zr_compare.py` runs the same game in this port and prints
  the first frame and field where the two differ. This needs a patched C++ build and a store of reference files that
  are **not** in this repository, so you cannot run it from a clone alone; [docs/HARNESS.md](docs/HARNESS.md)
  describes the formats and the tools.

## Layout

Crates keep the `zr-` prefix of the project's earlier working title.

| Path | What |
|---|---|
| `crates/zr-assets` | Install discovery, `.big` and Renegade `MIX1` archives, a virtual file system with the engine's precedence, RefPack decompression |
| `crates/zr-map` | `.map` reader: heightmap, objects, world info, lighting, triggers and water, scripts |
| `crates/zr-ini` | Parser for the game's INI dialect, with the engine's load order and per-map overrides |
| `crates/zr-text` | String tables (`.csf`, `.str`) |
| `crates/zr-audio` | Audio event definitions, file resolution and a WAV / IMA ADPCM reader |
| `crates/zr-ui` | `.wnd` window layouts, mapped images, command buttons and command sets |
| `crates/zr-templates` | Typed game data built from the INI database (objects, weapons, locomotors, armor, upgrades, object creation lists) |
| `crates/zr-terrain`, `zr-roads`, `zr-water` | Terrain mesh and texture atlas, roads and bridges, water |
| `crates/zr-w3d` | W3D models and animations, DDS/TGA textures |
| `crates/zr-fx` | Effect lists and particle system templates |
| `crates/zr-sim` | The deterministic headless simulation: the 30 Hz logic loop, objects, pathfinding, locomotors and physics, weapons, AI state machines, production and economy, the script engine and the skirmish AI. Possession is behind the `possession` feature. |
| `crates/zr-game` | Glue: loads templates and maps into the simulation, starts a skirmish, records and plays replays |
| `crates/zr-viewer` | The Bevy client: menus, the in-game interface, rendering and audio |
| `crates/zr-probe` | Command-line smoke tests against a real install |
| `crates/zr-harness` | Headless JSON-lines control interface over the simulation (load, step, query, inject commands, snapshots) |
| `tools/` | Development scripts, mainly for comparing a run with the C++ build |
| `docs/` | [Roadmap](docs/ROADMAP.md), [ported systems](docs/PORTED.md), [controls and flags](docs/CONTROLS.md), [possession design](docs/POSSESSION.md), [comparison harness](docs/HARNESS.md), [references and prior art](docs/REFERENCES.md) |

## Licence

GPL-3.0-or-later. The project ports logic from EA's GPL-3.0 source releases, so it must stay GPL.
[LICENSE.md](LICENSE.md) is the licence text as shipped with the source this project ports (TheSuperHackers'
`GeneralsGameCode`, unmodified): the GPL v3 followed by EA's additional terms under GPL section 7 (no trademark
rights, no claimed affiliation, modified versions marked as such). EA's Renegade release carries the same additional
terms. This project is a modified work and not the original program.
