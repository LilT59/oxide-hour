# References & prior art

## Local reference clones (`references/`, git-ignored)

| Clone | Source | Use |
|---|---|---|
| `CnC_Generals_Zero_Hour` | [electronicarts/CnC_Generals_Zero_Hour](https://github.com/electronicarts/CnC_Generals_Zero_Hour) | Original GPL-3.0 source |
| `CnC_Renegade` | [electronicarts/CnC_Renegade](https://github.com/electronicarts/CnC_Renegade) | Original GPL-3.0 source; MIX1 format, character controller, camera |
| `GeneralsGameCode` | [TheSuperHackers/GeneralsGameCode](https://github.com/TheSuperHackers/GeneralsGameCode) | Actively maintained fork (weekly releases), bug fixes; primary behavioural reference |
| `OpenW3D` | [w3dhub/OpenW3D](https://github.com/w3dhub/OpenW3D) | Portable Renegade engine fork |
| `cnc-formats` | [iron-curtain-engine/cnc-formats](https://github.com/iron-curtain-engine/cnc-formats) | Rust crate we depend on (MIT/Apache) for `.big` and `.w3d` |

Refresh with `git -C references/<name> pull`. Clone again with `git clone --depth 1 <url>`.

## Source-code landmarks (Zero Hour, `GeneralsMD/Code/`)

| Area | Location | Size (lines, .cpp) |
|---|---|---|
| Game logic | `GameEngine/Source/GameLogic` | ~187k |
| Game client | `GameEngine/Source/GameClient` | ~117k |
| Common (INI, file system, RTS player) | `GameEngine/Source/Common` | ~66k |
| Networking | `GameEngine/Source/GameNetwork` | ~37k |
| Device layer (W3D renderer, Win32) | `GameEngineDevice` | ~84k |
| WWVegas libs (W3D engine shared with Renegade lineage) | `Libraries/Source/WWVegas` | ~182k |

Key entry points:

- `GameLogic::update` (`GameLogic/System/GameLogic.cpp`)
- `AIUpdateInterface::update` (`Object/Update/AIUpdate.cpp`)
- `Locomotor::locoUpdate_*`
- `Weapon::fireWeapon`
- `TurretAI`
- `PhysicsBehavior`
- `logicMessageDispatcher` (`GameLogicDispatch.cpp`)
- Message enum in `Include/Common/MessageStream.h`

## Format notes discovered so far

- **BIG precedence** (`Win32BIGFileSystem::init`): all `*.big` under the install, recursively, in
  case-insensitive alphabetical order, then the base Generals install. The **first** archive
  containing a path wins. A duplicate `Data\INI\INIZH.big` is skipped. Loose files beat archives.
- **Renegade MIX1**: `"MIX1"`, header offset, names offset, reserved. The info table holds
  `{crc, offset, size}` sorted by CRC. The names table is in the same order, each `u8 len` + NUL-terminated name.
- **Maps**: shipped `.map` files are `EAR\0` + u32 size + a RefPack stream, wrapping a `CkMp` DataChunk file.
- **Generals INI**: block-structured (`Object Name … End`, nested modules), not Windows-INI.

## Related projects

- **Generals-FPS** ([ModDB](https://www.moddb.com/games/generals-fps)): a standalone shooter on Renegade's W3D engine with Generals-themed custom assets. It shows the theme works as an FPS.
- **Generals: First Hour** ([ModDB](https://www.moddb.com/mods/generals-first-hour)): an archived Renegade mod with USA/China/GLA factions.
- **Renegade X** ([Wikipedia](https://en.wikipedia.org/wiki/Renegade_X)) and Firestorm by Totem Arts: a closed-source UE3 remake. Firestorm's base building and Battlefield-style hybrid is useful design inspiration.
- **[OpenSAGE](https://github.com/OpenSAGE/OpenSAGE)** (C#, archived Aug 2026): the best reference for loading and rendering SAGE assets in a fresh engine.
- **[Thyme](https://github.com/TheAssemblyArmada/Thyme)**: a C++ reimplementation of Zero Hour.
- **[GeneralsX](https://generalsx.org/)**: a cross-platform build (SDL3 + DXVK + OpenAL).
- **[Iron Curtain](https://github.com/iron-curtain-engine/iron-curtain)**: a Rust/Bevy C&C engine (RA1/TD first). Its `fixed-game-math` and `deterministic-rng` crates are candidates for our sim.
- **[2010-rust-rewrite-mashup](https://github.com/chasmlol/2010-rust-rewrite-mashup)**: the inspiration. It composes existing Rust reimplementations and loads assets from the user's own copies.
- **[universal-modder](https://github.com/rehan-remade/universal-modder)** (MIT): agent skills and a CLI for modding binary games. Evaluated 2026-10-04 and not adopted: we reimplement from source rather than mod a binary. The one useful idea was its `um win shot`, ffmpeg `gfxcapture` window capture, which `tools/refshot.ps1` uses directly. Behaviour comparison should use deterministic replays/scenarios in TheSuperHackers' build, not input-driven retail runs.
- **W3D Hub**: the [modernization discussion](https://w3dhub.com/forum/topic/450024-westwood-3d-and-sage-engine-source-code-modernization/) for the W3D and SAGE engines.
