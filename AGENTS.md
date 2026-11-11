# AGENTS.md

Guidance for AI coding agents (and humans) working in this repo. Read [README.md](README.md),
[docs/ROADMAP.md](docs/ROADMAP.md) (status and open work) and, for gameplay work, [docs/POSSESSION.md](docs/POSSESSION.md)
first. The roadmap's "Order of work" is the authoritative order to proceed. [docs/PORTED.md](docs/PORTED.md) says what
each subsystem already ports; read the section for the area you touch.

Three working files are git-ignored and exist only in the owner's checkout: `LOG.md` (the detailed development log),
`docs/ROADMAP-internal.md` (per-batch leftovers behind the roadmap's open gaps) and `docs/AUDIT.md` (the 2026-11-11
audit). Read them when they are present; a fresh clone does not have them.

## Project in one paragraph

A Rust re-implementation of *C&C Generals – Zero Hour* with 100% original gameplay, plus an
optional possession mode (cargo feature `possession`, off by default) that lets the player take
direct third- or first-person control of a unit, Renegade-style. Possession code stays behind
`#[cfg(feature = "possession")]`; the default build is the plain port. The original C++ is the spec. We port behaviour faithfully, and we don't
redesign game rules unless a doc says so.

## Hard rules

- **No game assets in the repo.** All data comes from the user's installs at runtime via
  `zr-assets`. Don't commit extracted files, dumps, screenshots of game art, or test fixtures
  copied from game archives. Tests use synthetic data, or skip gracefully when no install is found.
- **GPL-3.0-or-later.** Code ported from EA's source stays GPL. Dependencies must be GPL-compatible
  (MIT/Apache/BSD/zlib/MPL are fine).
- **No EA trademarks** in crate names, binary names, or UI. The project is named "Oxide Hour"; the `zr-` crate prefix
  keeps the earlier working title's initials.
- **`references/` is read-only** and git-ignored. Never edit or commit it.
- Don't commit or push unless the user asks. `main` is the only working branch and the only one that is ever pushed.

## Where the truth lives

| Need | Look in |
|---|---|
| Zero Hour behaviour | `references/GeneralsGameCode/GeneralsMD` and `Core` (maintained fork, bug fixes) |
| Original Zero Hour as shipped | `references/CnC_Generals_Zero_Hour/GeneralsMD/Code` |
| Renegade controller, camera, MIX1 | `references/CnC_Renegade/Code` |
| Binary formats in Rust | `references/cnc-formats/src` (`.big` and `.w3d` are good; `map_sage` and `ini` are not usable for Generals) |

When porting, cite the C++ source file in a doc comment (e.g. `/// Ported from Compression/EAC/refdecode.cpp`).
When the original fork and TheSuperHackers disagree, follow TheSuperHackers unless the fix
changes retail gameplay, and note the choice.

## Workspace

The crate list and what each crate does is in the [README layout table](README.md#layout).
In short: data crates (`zr-assets`, `zr-map`, `zr-ini`, `zr-text`, `zr-audio`, `zr-ui`,
`zr-templates`, `zr-terrain`, `zr-roads`, `zr-water`, `zr-w3d`, `zr-fx`), the deterministic
simulation (`zr-sim`), the glue (`zr-game`), the Bevy client (`zr-viewer`) and the smoke-test CLI
(`zr-probe`). The sim never depends on `zr-templates`: typed data enters through seam types in
`zr-sim` and `zr-game/src/adapter.rs` converts it.

Port faithfully; don't invent behaviour. If something doesn't work, find what the original does
rather than adding a workaround (e.g. a non-retail "fallback" AI attack). Additions that the
original doesn't have (such as possession-only fog) must be documented as such.

New crates go in `crates/` (picked up by the `crates/*` glob), use the `zr-` prefix, and
inherit `version`/`edition`/`license`/`rust-version` from the workspace. Shared dependency
versions go in `[workspace.dependencies]`.

## Conventions

- Parsers take untrusted bytes. Use bounds-checked reads, return errors, never panic, and cap allocation sizes.
- In-game paths are compared via `zr_assets::normalize_path`: lowercase, `\` separators.
- Simulation code must be deterministic: identical inputs give identical state on every machine.
  - Numbers: `f32`, matching the original's `Real`. Use plain IEEE `+ - * /` and `sqrt`. Use the
    `libm` crate for `sin`/`cos`/`atan2`/`pow` etc., never `std`'s (those are platform libm). No
    `mul_add`/FMA, no SIMD reductions, no `f32::total_cmp`-dependent sorting of NaNs.
  - Ordering: no `HashMap`/`HashSet` iteration in logic. Use `BTreeMap`, `Vec` indexed by id,
    or `IndexMap` with insertion order.
  - Time: logic runs in frames at 30 Hz (`LOGICFRAMES_PER_SECOND`). No wall-clock time in logic.
  - Randomness: only the sim's seeded RNG, called in a deterministic order.
  - Unit conversions follow the engine's INI field parsers: durations in ms become frames,
    velocities per second become per frame, angles in degrees become radians, percents become fractions.
- The client (viewer) reads sim state; it never writes it. Input reaches the sim only as logic
  messages (see POSSESSION.md).
- Match the surrounding style: concise code, doc comments where the format or logic isn't obvious.

## Verify before handing back

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets
```

Possession is an off-by-default cargo feature, so possession work also needs the feature build:

```bash
cargo test --workspace --features zr-sim/possession,zr-game/possession,zr-viewer/possession
```

```bash
cargo run --release -p zr-probe -- parse
```

```bash
cargo run --release -p zr-ini --example parse_all
```

The probe and example need the Steam installs, or `ZR_ZEROHOUR_DIR` / `ZR_RENEGADE_DIR`. Report its numbers,
and update the numbers in `docs/ROADMAP.md` if they changed.

A fix counts only if a test shows it: the test must fail with the fix reverted (`git stash` the source change, run
the test, restore) and pass with it. Say in the report that you did this. Tests that pass either way prove nothing.

For sim divergences from the C++ build, localise before fixing: find the first frame (and the first RNG draw) where
the two runs differ, and port what the C++ does there. Start from `python -I tools/zr_compare.py <CODE> --seed 1
--frames 3000 --slot <N>` (reference store `Rust-remix\zr-refs\`), then trace the diverging objects on both sides with
`ZR_TRACE` + `tools/trace_diff.py` (docs/HARNESS.md) before adding any new diagnostic. Code reachable from a script
action must use the script engine passed to it, never `GameLogic::scripts` (it is taken out while scripts run). Don't tune the port until a later outcome (a matrix count, a
winner) happens to match.

**The bar is "near bit-exact", not "frame-perfect at any cost"** (decided 2026-10-28). Two rules, and only the second
is relaxed:

1. *Code stays faithful, always.* Whenever the port's logic differs from the C++ you are reading, port the C++,
   however small the difference. No invented behaviour, no tuning, tests that fail without the fix. This is not a
   licence for approximate or sloppy ports.
2. *Chase divergences that matter.* The frame/RNG diff is a bug finder, not the goal. A divergence is **meaningful**,
   and must be localised and fixed, when it: changes a decision or state (AI/dock/guard state, target, build site or
   order, path choice, an object created, destroyed or captured, money); grows over time instead of staying bounded;
   moves a game event (production done, building placed, first attack, a death) by more than a few frames; or is a
   position difference of about a pathfinding cell (10 units) or more. A sub-cell drift or an RNG draw shifted by a
   few frames that changes no decision for a few hundred frames is **minor**: record it in the lane's LOG entry and
   `docs/ROADMAP-internal.md` (frame, field, size, suspected cause) and move on to the next meaningful one rather than spending the lane
   on it. When unsure, check whether it changes a decision before deciding.
   Progress is judged by how closely game timelines and outcomes track the C++ across several matchups (build orders,
   event timing, matrix winners), with the frame diff used to find where they part.

Viewer changes also need a release screenshot of both start paths (`--quick` and
`--menu --auto-start`), saved outside the repo (e.g. `%TEMP%`). Open each image and describe what it
actually shows. A report that calls a broken image "right" is worse than no screenshot. For
comparisons with the retail game, `tools/refshot.ps1` captures its window next to ours; a set of retail reference shots
(menus, in-game bar, construction, rally) lives in `%TEMP%\zr-refshots\*.retail.png` when present (game art: never copy
it into the repo). Never draw something "to match the screenshot": find what the engine draws.

## Parallel agents

Several agents may work at once. Stay inside the crate(s) you were assigned, don't edit others'
crates or the root `Cargo.toml` beyond adding a `[workspace.dependencies]` line, and report back
with: files changed, test/probe results, surprises, and anything left unimplemented.

- **Build directory:** set `CARGO_TARGET_DIR=target/slot-<N>` for every cargo command, using
  the slot number (1–4) the orchestrator assigned you, so parallel agents don't block on the
  shared build lock. Never create other target directories: each one is a full Bevy build
  (6–14 GB), and per-task directories once grew `target/` to ~95 GB. Use the **absolute** path
  (`<repo>/target/slot-<N>`) and run cargo from the repo root: a relative path from a crate
  directory creates a stray `crates/<crate>/target`, and forgetting the variable builds into the
  default `target/debug`. Without parallel agents, use `target/slot-1`.
- **Never search `/` or `C:/`** (it can run for an hour). Bevy sources are in the cargo registry
  (`~/.cargo/registry/src/*/bevy_*-0.19.1`).
- **Test scope:** build and test only your own crates (`cargo test -p <crate>`,
  `cargo clippy -p <crate> --all-targets`) and avoid `--release` unless you need it (screenshots,
  install examples). The orchestrator runs the full workspace checks at integration.
- **Shared files** (e.g. `zr-viewer/src/main.rs`, `zr-map/src/map.rs`, root `Cargo.toml`):
  re-read just before editing, use small targeted edits (never rewrite the whole file), and put
  your logic in your own new module file.
- **Edit source with the `Edit` tool, never with scripts.** No Python/`sed -i`/shell rewrites
  of a source file (`open(p,'w').write(...)`, `s.replace(...)`): one empty match once replaced
  the empty string, inserted a 2 KB block before every character of `skirmish_ai.rs` (161 MB) and
  destroyed every uncommitted edit in it, from two lanes. It had to be rebuilt from agent
  transcripts. If you corrupt a file, stop and report it instead of trying a scripted undo.
- **Hand-back:** send your report once, then stop. Don't re-send it.
- **Verifying hand-backs (for orchestrators):** open every screenshot a report cites and check
  key claims against the C++ before accepting them, wrap-up reports included. A wrap-up once
  shipped a viewer that started with no terrain or HUD while its report described both.
- **Adversarial review (for orchestrators):** before integrating a lane's sim or parser change, have a different
  model (or a fresh session) review the diff against the C++ it cites, with one job: find where the port differs,
  simplifies, special-cases or invents behaviour. Fix or reject what it finds before integrating.
- **Sessions (for orchestrators):** start a fresh agent session per task, or per two or three
  closely related tasks, instead of reusing one long-running session. Keep the build *slot*
  fixed per parallel lane, not the session. Long sessions compact their context, and their
  summaries go stale (claiming work wasn't done when it was, carrying forward claims already
  corrected) and drop rules from earlier prompts. In a 12-hour run with four reused sessions this
  led to redone work and invented behaviour creeping back. Hand off through the repo instead: the
  new session's prompt names the relevant `LOG.md` entries and `docs/` sections, the files to
  read, the C++ sources, the assigned crates/slot and which other agents are active where. It
  should restate the hard rules (faithful port, absolute build slot, no commits) rather than
  assume the agent remembers them. Agents should check `git diff` for their files before
  continuing interrupted work.

## Logging

Append notable progress, decisions and findings to `LOG.md` (git-ignored, local to the owner's checkout): newest entry
at the top, dated, a few bullets each. Decisions that change direction also belong in the relevant doc.

[CHANGELOG.md](CHANGELOG.md) is the public history: at the end of a batch of work, add one paragraph at the top saying
what was ported and what the comparison numbers are. No agent, batch or review numbers and no local paths there or in
any other tracked doc.

`docs/ROADMAP.md` holds only status and open work. When you finish an item, delete it (or shrink it to what is left)
instead of marking it done; the details go in `LOG.md`, and a newly ported system gets a line in `docs/PORTED.md`.
Per-batch leftovers too detailed for the roadmap go in `docs/ROADMAP-internal.md` (git-ignored).
