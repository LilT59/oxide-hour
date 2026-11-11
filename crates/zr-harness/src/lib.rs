//! Headless control interface over the simulation (ROADMAP step 3.2): one JSON object per input line,
//! one JSON reply per output line, so an orchestrator or a script can load a skirmish, step it, query
//! state, inject [`Command`]s, write the HARNESS.md snapshot and toggle the RNG trace without
//! re-running a whole demo.
//!
//! The binary is [`zr-harness`](../zr_harness/index.html); this library is the same session, so tests
//! and other tools drive it programmatically. The protocol (fields, filters, replies) is documented in
//! `docs/HARNESS.md` under "Control interface (`zr-harness`)".
//!
//! * [`session`]: [`Session`] owns the [`GameData`](zr_game::GameData), the optional install (VFS) and
//!   the loaded [`Game`]; all sim access goes through it. Queries only read.
//! * [`protocol`]: line parsing and dispatch, errors as `{"ok":false,"error":...}`.
//! * [`command_json`]: validated JSON -> [`zr_sim::Command`] decoding (the same enum replays use).

pub mod command_json;
pub mod protocol;
pub mod session;

pub use protocol::handle_line;
pub use session::{
    LoadSpec, PlayerSpec, Query, Session, SnapshotSpec, What,
};
