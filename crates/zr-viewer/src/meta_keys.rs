//! The retail meta-key layer: `Data\INI\CommandMap.ini` bindings and the `MetaEventTranslator`
//! that turns keystrokes into meta messages, plus the `CommandXlat` meta handlers a skirmish
//! player uses.
//!
//! Retail does not read keys directly: `MetaEventTranslator` (priority 20 in the message stream,
//! `GeneralsMD/.../GameClient.cpp:299`) looks each key + modifier + transition up in `TheMetaMap`
//! and appends the bound meta message, which `CommandTranslator` (priority 70) then handles
//! (`Core/.../MessageStream/CommandXlat.cpp:2553-3760`). `TheMetaMap` is built from
//! `Data\{lang}\CommandMap.ini` then `Data\INI\CommandMap.ini` (`MetaMap::parseMetaMap`,
//! `Core/.../MessageStream/MetaEvent.cpp:786`).
//!
//! This module ports that table ([`MetaBinding`], [`Meta`], [`resolve`]) and dispatches the metas a
//! skirmish player uses. The translator's key handling is faithful in a simplified form: normal
//! keys match on the exact modifier state; the `KEY_NONE` + modifier rows (`BEGIN_FORCEATTACK`,
//! `BEGIN_WAYPOINTS`, `BEGIN_PREFER_SELECTION`) fire on the modifier going down, and their `END_*`
//! rows on a modifier release. (Retail tracks each held key's modifier combos in `KeyDownInfo` to
//! get the release order exactly; the shipped table only has single-modifier `KEY_NONE` rows, so
//! the simpler model gives the same result. Command-bar `&` hotkeys keep their own path: retail
//! fires metas on key *down* and the `HotKeyTranslator` (priority 25) fires the button on the key
//! *up*, so both run; `hud.rs` still fires its hotkeys on the press.)
//!
//! The viewer only ever issues `zr_sim::Command`s; selection is client state. Metas run only for the
//! non-possessing local player, and are suppressed while a modal overlay is open (retail's
//! `TheInGameUI->getInputEnabled()`), mirroring `ui_block`.
//!
//! Approximations, all noted where they live: `SELECT_NEXT/PREV_UNIT`/`WORKER` cycle the
//! deterministic object-id order (the engine walks its prepended drawable list) and
//! `SELECT_NEXT_IDLE_WORKER` selects the next worker rather than consulting the idle-worker list;
//! `VIEW_COMMAND_CENTER`'s most-expensive fallback is not modelled; `BEGIN_FORCEMOVE` is bound in
//! the code but the shipped `CommandMap.ini` comments it out, so it has no default key.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use zr_assets::Vfs;
use zr_sim::template::kind_of;
use zr_sim::{Command, Coord3D, ObjectId, PlayerIndex};

use crate::camera::{FreeCam, Interactive, RtsCam};
use crate::convert;
use crate::radar::RadarEvents;
use crate::sim_view::{is_structure, SimRes};

/// `MappableKeyModState` (`MetaEvent.h:285`): the ctrl/alt/shift bits a meta requires.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mods(pub u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const CTRL: Mods = Mods(1);
    pub const ALT: Mods = Mods(2);
    pub const SHIFT: Mods = Mods(4);

    /// Parse a `Modifiers` token (`MetaEvent.h:297`).
    pub fn from_name(name: &str) -> Option<Mods> {
        Some(match name.to_ascii_uppercase().as_str() {
            "NONE" => Mods::NONE,
            "CTRL" => Mods::CTRL,
            "ALT" => Mods::ALT,
            "SHIFT" => Mods::SHIFT,
            "CTRL_ALT" => Mods(Mods::CTRL.0 | Mods::ALT.0),
            "SHIFT_CTRL" => Mods(Mods::SHIFT.0 | Mods::CTRL.0),
            "SHIFT_ALT" => Mods(Mods::SHIFT.0 | Mods::ALT.0),
            "SHIFT_ALT_CTRL" => Mods(Mods::SHIFT.0 | Mods::ALT.0 | Mods::CTRL.0),
            _ => return None,
        })
    }
}

/// `MappableKeyTransition` (`MetaEvent.h:265`). `DOUBLEDOWN` is unused by the shipped table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transition {
    Down,
    Up,
}

/// A meta a skirmish player can trigger. Team/view/camera metas carry their index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Meta {
    Stop,
    Scatter,
    BeginForceAttack,
    EndForceAttack,
    BeginForceMove,
    EndForceMove,
    BeginWaypoints,
    EndWaypoints,
    BeginPreferSelection,
    EndPreferSelection,
    SelectAll,
    SelectAllAircraft,
    SelectMatchingUnits,
    SelectNextUnit,
    SelectPrevUnit,
    SelectNextWorker,
    SelectPrevWorker,
    SelectNextIdleWorker,
    SelectHero,
    ViewCommandCenter,
    ViewLastRadarEvent,
    BeginCameraRotateLeft,
    EndCameraRotateLeft,
    BeginCameraRotateRight,
    EndCameraRotateRight,
    BeginCameraZoomIn,
    EndCameraZoomIn,
    BeginCameraZoomOut,
    EndCameraZoomOut,
    CameraReset,
    ToggleControlBar,
    SaveView(u8),
    ViewView(u8),
    CreateTeam(u8),
    SelectTeam(u8),
    AddTeam(u8),
    ViewTeam(u8),
    /// Recognised but not implemented (chat/beacon/replay/observer/debug). Kept so the key is
    /// consumed exactly as retail consumes it.
    Ignored,
}

impl Meta {
    /// The CommandMap name of this meta, or `None` for [`Meta::Ignored`].
    pub fn name(self) -> Option<&'static str> {
        Some(match self {
            Meta::Stop => "STOP",
            Meta::Scatter => "SCATTER",
            Meta::BeginForceAttack => "BEGIN_FORCEATTACK",
            Meta::EndForceAttack => "END_FORCEATTACK",
            Meta::BeginForceMove => "BEGIN_FORCEMOVE",
            Meta::EndForceMove => "END_FORCEMOVE",
            Meta::BeginWaypoints => "BEGIN_WAYPOINTS",
            Meta::EndWaypoints => "END_WAYPOINTS",
            Meta::BeginPreferSelection => "BEGIN_PREFER_SELECTION",
            Meta::EndPreferSelection => "END_PREFER_SELECTION",
            Meta::SelectAll => "SELECT_ALL",
            Meta::SelectAllAircraft => "SELECT_ALL_AIRCRAFT",
            Meta::SelectMatchingUnits => "SELECT_MATCHING_UNITS",
            Meta::SelectNextUnit => "SELECT_NEXT_UNIT",
            Meta::SelectPrevUnit => "SELECT_PREV_UNIT",
            Meta::SelectNextWorker => "SELECT_NEXT_WORKER",
            Meta::SelectPrevWorker => "SELECT_PREV_WORKER",
            Meta::SelectNextIdleWorker => "SELECT_NEXT_IDLE_WORKER",
            Meta::SelectHero => "SELECT_HERO",
            Meta::ViewCommandCenter => "VIEW_COMMAND_CENTER",
            Meta::ViewLastRadarEvent => "VIEW_LAST_RADAR_EVENT",
            Meta::BeginCameraRotateLeft => "BEGIN_CAMERA_ROTATE_LEFT",
            Meta::EndCameraRotateLeft => "END_CAMERA_ROTATE_LEFT",
            Meta::BeginCameraRotateRight => "BEGIN_CAMERA_ROTATE_RIGHT",
            Meta::EndCameraRotateRight => "END_CAMERA_ROTATE_RIGHT",
            Meta::BeginCameraZoomIn => "BEGIN_CAMERA_ZOOM_IN",
            Meta::EndCameraZoomIn => "END_CAMERA_ZOOM_IN",
            Meta::BeginCameraZoomOut => "BEGIN_CAMERA_ZOOM_OUT",
            Meta::EndCameraZoomOut => "END_CAMERA_ZOOM_OUT",
            Meta::CameraReset => "CAMERA_RESET",
            Meta::ToggleControlBar => "TOGGLE_CONTROL_BAR",
            Meta::Ignored => return None,
            Meta::SaveView(_) | Meta::ViewView(_) | Meta::CreateTeam(_) | Meta::SelectTeam(_) | Meta::AddTeam(_) | Meta::ViewTeam(_) => return None,
        })
    }

    /// Map a CommandMap name (`MetaEvent.cpp`'s `GameMessageMetaTypeNames`) onto a [`Meta`]. Returns
    /// `None` for names outside the shipped table; `Some(Meta::Ignored)` for known-but-skipped rows.
    pub fn from_name(name: &str) -> Option<Meta> {
        if let Some(n) = name.strip_prefix("SAVE_VIEW").and_then(parse_index) {
            return Some(Meta::SaveView(n));
        }
        if let Some(n) = name.strip_prefix("VIEW_VIEW").and_then(parse_index) {
            return Some(Meta::ViewView(n));
        }
        if let Some(n) = name.strip_prefix("CREATE_TEAM").and_then(parse_index) {
            return Some(Meta::CreateTeam(n));
        }
        if let Some(n) = name.strip_prefix("SELECT_TEAM").and_then(parse_index) {
            return Some(Meta::SelectTeam(n));
        }
        if let Some(n) = name.strip_prefix("ADD_TEAM").and_then(parse_index) {
            return Some(Meta::AddTeam(n));
        }
        if let Some(n) = name.strip_prefix("VIEW_TEAM").and_then(parse_index) {
            return Some(Meta::ViewTeam(n));
        }
        Some(match name {
            "STOP" => Meta::Stop,
            "SCATTER" => Meta::Scatter,
            "BEGIN_FORCEATTACK" => Meta::BeginForceAttack,
            "END_FORCEATTACK" => Meta::EndForceAttack,
            "BEGIN_FORCEMOVE" => Meta::BeginForceMove,
            "END_FORCEMOVE" => Meta::EndForceMove,
            "BEGIN_WAYPOINTS" => Meta::BeginWaypoints,
            "END_WAYPOINTS" => Meta::EndWaypoints,
            "BEGIN_PREFER_SELECTION" => Meta::BeginPreferSelection,
            "END_PREFER_SELECTION" => Meta::EndPreferSelection,
            "SELECT_ALL" => Meta::SelectAll,
            "SELECT_ALL_AIRCRAFT" => Meta::SelectAllAircraft,
            "SELECT_MATCHING_UNITS" => Meta::SelectMatchingUnits,
            "SELECT_NEXT_UNIT" => Meta::SelectNextUnit,
            "SELECT_PREV_UNIT" => Meta::SelectPrevUnit,
            "SELECT_NEXT_WORKER" => Meta::SelectNextWorker,
            "SELECT_PREV_WORKER" => Meta::SelectPrevWorker,
            "SELECT_NEXT_IDLE_WORKER" => Meta::SelectNextIdleWorker,
            "SELECT_HERO" => Meta::SelectHero,
            "VIEW_COMMAND_CENTER" => Meta::ViewCommandCenter,
            "VIEW_LAST_RADAR_EVENT" => Meta::ViewLastRadarEvent,
            "BEGIN_CAMERA_ROTATE_LEFT" => Meta::BeginCameraRotateLeft,
            "END_CAMERA_ROTATE_LEFT" => Meta::EndCameraRotateLeft,
            "BEGIN_CAMERA_ROTATE_RIGHT" => Meta::BeginCameraRotateRight,
            "END_CAMERA_ROTATE_RIGHT" => Meta::EndCameraRotateRight,
            "BEGIN_CAMERA_ZOOM_IN" => Meta::BeginCameraZoomIn,
            "END_CAMERA_ZOOM_IN" => Meta::EndCameraZoomIn,
            "BEGIN_CAMERA_ZOOM_OUT" => Meta::BeginCameraZoomOut,
            "END_CAMERA_ZOOM_OUT" => Meta::EndCameraZoomOut,
            "CAMERA_RESET" => Meta::CameraReset,
            "TOGGLE_CONTROL_BAR" => Meta::ToggleControlBar,
            // Known rows the viewer does not act on (skips still consume the key, as retail does).
            "ALL_CHEER"
            | "CREATE_FORMATION"
            | "CHAT_ALLIES"
            | "CHAT_EVERYONE"
            | "DIPLOMACY"
            | "PLACE_BEACON"
            | "DELETE_BEACON"
            | "OPTIONS"
            | "TOGGLE_FAST_FORWARD_REPLAY"
            | "TOGGLE_CAMERA_TRACKING_DRAWABLE"
            | "TAKE_SCREENSHOT"
            | "TAKE_SCREENSHOT_PNG"
            | "DEMO_INSTANT_QUIT"
            | "BEGIN_PATH_BUILD"
            | "END_PATH_BUILD"
            | "DEPLOY"
            | "FOLLOW" => Meta::Ignored,
            _ => return None,
        })
    }
}

fn parse_index(rest: &str) -> Option<u8> {
    rest.parse::<u8>().ok().filter(|n| *n < 10)
}

/// One `CommandMap` row: the meta, its key (`None` = `KEY_NONE`, a modifier-only row) and the exact
/// state it fires in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MetaBinding {
    pub meta: Meta,
    pub key: Option<KeyCode>,
    pub mods: Mods,
    pub transition: Transition,
}

/// A synthetic key event fed to [`resolve`]: a normal key (with its held modifiers) or a
/// modifier-only row (`key = None`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeyEvent {
    pub key: Option<KeyCode>,
    pub mods: Mods,
    pub transition: Transition,
}

/// `MetaEventTranslator::onKeyPressed`'s lookup: the first binding whose key, modifier state and
/// transition match fires (`MetaEvent.cpp:606-654`). One meta per event.
pub fn resolve(bindings: &[MetaBinding], events: &[KeyEvent]) -> Vec<Meta> {
    let mut out = Vec::new();
    for ev in events {
        for b in bindings {
            if b.key == ev.key && b.mods == ev.mods && b.transition == ev.transition {
                out.push(b.meta);
                break;
            }
        }
    }
    out
}

/// `KeyNames` (`MetaEvent.h:166`) -> Bevy `KeyCode`. `Some(None)` is `KEY_NONE`.
fn parse_key(name: &str) -> Option<Option<KeyCode>> {
    let k = name.to_ascii_uppercase();
    if k == "KEY_NONE" {
        return Some(None);
    }
    if let Some(rest) = k.strip_prefix("KEY_F") {
        if let Ok(n) = rest.parse::<u8>() {
            return Some(match n {
                1 => Some(KeyCode::F1),
                2 => Some(KeyCode::F2),
                3 => Some(KeyCode::F3),
                4 => Some(KeyCode::F4),
                5 => Some(KeyCode::F5),
                6 => Some(KeyCode::F6),
                7 => Some(KeyCode::F7),
                8 => Some(KeyCode::F8),
                9 => Some(KeyCode::F9),
                10 => Some(KeyCode::F10),
                11 => Some(KeyCode::F11),
                12 => Some(KeyCode::F12),
                _ => return None,
            });
        }
    }
    if let Some(rest) = k.strip_prefix("KEY_KP") {
        return Some(match rest {
            "0" => Some(KeyCode::Numpad0),
            "1" => Some(KeyCode::Numpad1),
            "2" => Some(KeyCode::Numpad2),
            "3" => Some(KeyCode::Numpad3),
            "4" => Some(KeyCode::Numpad4),
            "5" => Some(KeyCode::Numpad5),
            "6" => Some(KeyCode::Numpad6),
            "7" => Some(KeyCode::Numpad7),
            "8" => Some(KeyCode::Numpad8),
            "9" => Some(KeyCode::Numpad9),
            "PLUS" => Some(KeyCode::NumpadAdd),
            "MINUS" => Some(KeyCode::NumpadSubtract),
            "ENTER" => Some(KeyCode::NumpadEnter),
            "STAR" => Some(KeyCode::NumpadMultiply),
            "SLASH" => Some(KeyCode::NumpadDivide),
            "DEL" => Some(KeyCode::NumpadDecimal),
            _ => return None,
        });
    }
    let short = k.strip_prefix("KEY_").unwrap_or(&k);
    Some(match short {
        "A" => Some(KeyCode::KeyA),
        "B" => Some(KeyCode::KeyB),
        "C" => Some(KeyCode::KeyC),
        "D" => Some(KeyCode::KeyD),
        "E" => Some(KeyCode::KeyE),
        "F" => Some(KeyCode::KeyF),
        "G" => Some(KeyCode::KeyG),
        "H" => Some(KeyCode::KeyH),
        "I" => Some(KeyCode::KeyI),
        "J" => Some(KeyCode::KeyJ),
        "K" => Some(KeyCode::KeyK),
        "L" => Some(KeyCode::KeyL),
        "M" => Some(KeyCode::KeyM),
        "N" => Some(KeyCode::KeyN),
        "O" => Some(KeyCode::KeyO),
        "P" => Some(KeyCode::KeyP),
        "Q" => Some(KeyCode::KeyQ),
        "R" => Some(KeyCode::KeyR),
        "S" => Some(KeyCode::KeyS),
        "T" => Some(KeyCode::KeyT),
        "U" => Some(KeyCode::KeyU),
        "V" => Some(KeyCode::KeyV),
        "W" => Some(KeyCode::KeyW),
        "X" => Some(KeyCode::KeyX),
        "Y" => Some(KeyCode::KeyY),
        "Z" => Some(KeyCode::KeyZ),
        "0" => Some(KeyCode::Digit0),
        "1" => Some(KeyCode::Digit1),
        "2" => Some(KeyCode::Digit2),
        "3" => Some(KeyCode::Digit3),
        "4" => Some(KeyCode::Digit4),
        "5" => Some(KeyCode::Digit5),
        "6" => Some(KeyCode::Digit6),
        "7" => Some(KeyCode::Digit7),
        "8" => Some(KeyCode::Digit8),
        "9" => Some(KeyCode::Digit9),
        "ESC" => Some(KeyCode::Escape),
        "BACKSPACE" => Some(KeyCode::Backspace),
        "ENTER" => Some(KeyCode::Enter),
        "SPACE" => Some(KeyCode::Space),
        "TAB" => Some(KeyCode::Tab),
        "UP" => Some(KeyCode::ArrowUp),
        "DOWN" => Some(KeyCode::ArrowDown),
        "LEFT" => Some(KeyCode::ArrowLeft),
        "RIGHT" => Some(KeyCode::ArrowRight),
        "HOME" => Some(KeyCode::Home),
        "END" => Some(KeyCode::End),
        "PGUP" => Some(KeyCode::PageUp),
        "PGDN" => Some(KeyCode::PageDown),
        "INS" => Some(KeyCode::Insert),
        "DEL" => Some(KeyCode::Delete),
        "MINUS" => Some(KeyCode::Minus),
        "EQUAL" => Some(KeyCode::Equal),
        "LBRACKET" => Some(KeyCode::BracketLeft),
        "RBRACKET" => Some(KeyCode::BracketRight),
        "SEMICOLON" => Some(KeyCode::Semicolon),
        "APOSTROPHE" => Some(KeyCode::Quote),
        "TICK" => Some(KeyCode::Backquote),
        "BACKSLASH" => Some(KeyCode::Backslash),
        "COMMA" => Some(KeyCode::Comma),
        "PERIOD" => Some(KeyCode::Period),
        "SLASH" => Some(KeyCode::Slash),
        _ => return None,
    })
}

/// Parses the `CommandMap` rows of `lang_text` (`Data\{lang}\...`) then `main_text`
/// (`Data\INI\...`), with the later file overriding the same meta (`MetaMap::getMetaMapRec` reuses
/// the record). Only `UseableIn = ... GAME ...` rows are kept; the viewer is never in the shell here.
/// Unknown metas and unparsable keys are skipped.
pub fn parse_command_map(lang_text: &str, main_text: &str) -> Result<Vec<MetaBinding>, zr_ini::ParseError> {
    let mut out: Vec<MetaBinding> = Vec::new();
    let mut index: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for text in [lang_text, main_text] {
        let blocks = zr_ini::parse("CommandMap.ini", text)?;
        for b in blocks.iter().filter(|b| b.kind.eq_ignore_ascii_case("CommandMap")) {
            let Some(name) = b.header.first() else { continue };
            let Some(meta) = Meta::from_name(name) else { continue };
            let usable = b.value("UseableIn").unwrap_or("GAME");
            if !usable.split_whitespace().any(|t| t.eq_ignore_ascii_case("GAME")) && !usable.contains("EVERYWHERE") {
                continue;
            }
            let Some(key) = b.value("Key").and_then(|v| parse_key(v.trim())) else { continue };
            let Some(mods) = b.value("Modifiers").and_then(|v| Mods::from_name(v.trim())) else { continue };
            let transition = match b.value("Transition").unwrap_or("DOWN").trim().to_ascii_uppercase().as_str() {
                "UP" => Transition::Up,
                _ => Transition::Down,
            };
            let binding = MetaBinding { meta, key, mods, transition };
            // `Meta::Ignored` rows are kept so their keys are consumed, but they have no name to key on.
            let key_name = meta.name().unwrap_or(name).to_owned();
            match index.get(&key_name) {
                Some(&i) => out[i] = binding,
                None => {
                    index.insert(key_name, out.len());
                    out.push(binding);
                }
            }
        }
    }
    Ok(out)
}

/// The transient modes the meta layer drives (`InGameUI`'s `m_forceAttackMode`, `m_waypointMode`,
/// `m_forceMoveMode`, `m_preferSelection`, camera rotate/zoom flags, control-bar visibility) plus
/// the camera bookmarks (`m_viewLocation[8]`).
#[derive(Resource, Default)]
pub struct MetaModes {
    pub force_attack: bool,
    pub force_move: bool,
    pub waypoints: bool,
    pub prefer_selection: bool,
    pub control_bar_hidden: bool,
    pub rotate_left: bool,
    pub rotate_right: bool,
    pub zoom_in: bool,
    pub zoom_out: bool,
    pub saved_views: [Option<SavedView>; 8],
}

/// A saved camera view (`TheTacticalView::getLocation`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedView {
    pub focus: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
}

/// Applies the mode-only metas (no sim/camera needed), for `resolve` tests and the dispatcher.
/// Returns true when `meta` changed a mode.
pub fn apply_mode(meta: Meta, modes: &mut MetaModes) -> bool {
    match meta {
        Meta::BeginForceAttack => modes.force_attack = true,
        Meta::EndForceAttack => modes.force_attack = false,
        Meta::BeginForceMove => modes.force_move = true,
        Meta::EndForceMove => modes.force_move = false,
        Meta::BeginWaypoints => modes.waypoints = true,
        Meta::EndWaypoints => modes.waypoints = false,
        Meta::BeginPreferSelection => modes.prefer_selection = true,
        Meta::EndPreferSelection => modes.prefer_selection = false,
        Meta::BeginCameraRotateLeft => modes.rotate_left = true,
        Meta::EndCameraRotateLeft => modes.rotate_left = false,
        Meta::BeginCameraRotateRight => modes.rotate_right = true,
        Meta::EndCameraRotateRight => modes.rotate_right = false,
        Meta::BeginCameraZoomIn => modes.zoom_in = true,
        Meta::EndCameraZoomIn => modes.zoom_in = false,
        Meta::BeginCameraZoomOut => modes.zoom_out = true,
        Meta::EndCameraZoomOut => modes.zoom_out = false,
        _ => return false,
    }
    true
}

// ---- selection helpers -------------------------------------------------------------------------

/// `Object::isMassSelectable` for a locally controlled, uncontained, selectable non-structure.
fn selectable_non_structure(sim: &SimRes, id: ObjectId, local: PlayerIndex) -> bool {
    let Some(o) = sim.game.logic().find_object(id) else { return false };
    o.player == local
        && !o.is_destroyed()
        && !o.is_dying()
        && o.is_kind_of(kind_of::SELECTABLE)
        && !o.is_kind_of(kind_of::STRUCTURE)
        && !sim.game.logic().is_contained(id)
}

fn disqualified(o: &zr_sim::Object) -> bool {
    o.is_kind_of(kind_of::DOZER) || o.is_kind_of(kind_of::HARVESTER)
}

fn on_screen(camera: &Camera, gt: &GlobalTransform, view: Vec2, pos: Coord3D) -> bool {
    camera
        .world_to_viewport(gt, convert::to_bevy([pos.x, pos.y, pos.z]))
        .is_ok_and(|p| p.x >= 0.0 && p.y >= 0.0 && p.x <= view.x && p.y <= view.y)
}

/// `command_translator`'s `SELECT_ALL` / `SELECT_ALL_AIRCRAFT` (`CommandXlat.cpp:3089`):
/// `selectAllUnitsByType`, which is across the screen and falls back to across the map; first
/// deselects all when an incompatible object is selected.
fn select_all(sim: &mut SimRes, camera: &Camera, gt: &GlobalTransform, view: Vec2, aircraft: bool) {
    let local = sim.local;
    let incompatible = sim.selected.iter().any(|&id| {
        sim.game.logic().find_object(id).is_some_and(|o| {
            if aircraft {
                !o.is_kind_of(kind_of::AIRCRAFT) || disqualified(o)
            } else {
                o.is_kind_of(kind_of::STRUCTURE) || disqualified(o)
            }
        })
    });
    if incompatible {
        sim.selected.clear();
    }
    let matches = |sim: &SimRes, id: ObjectId| {
        sim.game.logic().find_object(id).is_some_and(|o| {
            selectable_non_structure(sim, id, local) && !disqualified(o) && (!aircraft || o.is_kind_of(kind_of::AIRCRAFT))
        })
    };
    let mut add: Vec<ObjectId> = sim
        .cur
        .iter()
        .filter(|(id, s)| matches(sim, **id) && on_screen(camera, gt, view, Coord3D::new(s.pos[0], s.pos[1], s.pos[2])))
        .map(|(id, _)| *id)
        .collect();
    if add.is_empty() {
        add = sim.cur.iter().filter(|(id, _)| matches(sim, **id)).map(|(id, _)| *id).collect();
    }
    for id in add {
        if !sim.selected.contains(&id) {
            sim.selected.push(id);
        }
    }
}

/// `InGameUI::selectUnitsMatchingCurrentSelection` (`CommandXlat.cpp:2556`): adds every object of
/// the same template as the selection, across the screen then (if none) across the map.
fn select_matching(sim: &mut SimRes, camera: &Camera, gt: &GlobalTransform, view: Vec2) {
    let local = sim.local;
    let templates: Vec<String> = sim
        .selected
        .iter()
        .filter_map(|&id| sim.game.logic().find_object(id))
        .filter(|o| o.player == local && !o.is_kind_of(kind_of::STRUCTURE))
        .map(|o| o.template.name.clone())
        .collect();
    if templates.is_empty() {
        return;
    }
    let same = |sim: &SimRes, id: ObjectId| {
        sim.game.logic().find_object(id).is_some_and(|o| templates.contains(&o.template.name)) && selectable_non_structure(sim, id, local)
    };
    let already: Vec<ObjectId> = sim.selected.clone();
    let mut add: Vec<ObjectId> = sim
        .cur
        .iter()
        .filter(|(id, s)| same(sim, **id) && !already.contains(id) && on_screen(camera, gt, view, Coord3D::new(s.pos[0], s.pos[1], s.pos[2])))
        .map(|(id, _)| *id)
        .collect();
    if add.is_empty() {
        add = sim.cur.iter().filter(|(id, _)| same(sim, **id) && !already.contains(id)).map(|(id, _)| *id).collect();
    }
    sim.selected.extend(add);
}

/// `SELECT_NEXT/PREV_UNIT` / `..._WORKER`: cycle the deterministic (object-id) order. The engine
/// iterates its prepended drawable list; we use the id order, which is creation order. When nothing
/// is selected the first press picks from the far end (the engine's list is newest-first).
fn select_next(sim: &mut SimRes, forward: bool, workers: bool) -> Option<ObjectId> {
    let local = sim.local;
    let candidates: Vec<ObjectId> = sim
        .cur
        .keys()
        .copied()
        .filter(|&id| {
            selectable_non_structure(sim, id, local)
                && sim.game.logic().find_object(id).is_some_and(|o| !workers || o.is_kind_of(kind_of::DOZER))
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let next = match sim.selected.first().and_then(|id| candidates.iter().position(|c| c == id)) {
        Some(i) if forward => candidates[(i + 1) % candidates.len()],
        Some(i) => candidates[(i + candidates.len() - 1) % candidates.len()],
        None if forward => *candidates.last().unwrap(),
        None => candidates[0],
    };
    sim.selected = vec![next];
    Some(next)
}

/// `SELECT_HERO` (`CommandXlat.cpp:3028`): select the local player's `KINDOF_HERO` object.
fn select_hero(sim: &mut SimRes) -> Option<ObjectId> {
    let local = sim.local;
    let hero = sim
        .cur
        .keys()
        .copied()
        .find(|&id| sim.game.logic().find_object(id).is_some_and(|o| o.player == local && o.is_kind_of(kind_of::HERO)));
    if let Some(id) = hero {
        sim.selected = vec![id];
    }
    hero
}

/// `viewCommandCenter` (`CommandXlat.cpp:930`): the local command center, else an owned structure.
/// (`findCommandCenterOrMostExpensiveBuilding`'s build-cost tie-break is not modelled; the first
/// owned structure is used.) Returns its position.
fn command_center(sim: &SimRes) -> Option<Coord3D> {
    let local = sim.local;
    let mut cc: Option<Coord3D> = None;
    for id in sim.cur.keys() {
        let Some(o) = sim.game.logic().find_object(*id) else { continue };
        if o.player != local || !o.is_kind_of(kind_of::STRUCTURE) {
            continue;
        }
        if o.is_kind_of(kind_of::COMMANDCENTER) {
            cc = Some(o.position);
        }
    }
    if cc.is_some() {
        return cc;
    }
    // `findCommandCenterOrMostExpensiveBuilding`: fall back to any owned structure.
    sim.cur.iter().find_map(|(id, _)| {
        sim.game.logic().find_object(*id).filter(|o| o.player == local && o.is_kind_of(kind_of::STRUCTURE)).map(|o| o.position)
    })
}

// ---- control groups ----------------------------------------------------------------------------

/// The ten hotkey squads of the local player (`Player::m_squads`, `NUM_HOTKEY_SQUADS`). Client-side:
/// the sim has no squads, and they only matter to selection.
#[derive(Resource, Default)]
pub(crate) struct ControlGroups {
    groups: [Vec<ObjectId>; 10],
    /// Real time and index of the last group key press (`SelectionTranslator::m_lastGroupSel*`).
    last: Option<(f64, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TeamAction {
    Create,
    Select,
    Add,
    View,
}

/// `SelectionTranslator`'s control-group handlers. `look_at` is the camera focus setter.
#[allow(clippy::too_many_arguments)]
fn apply_team(
    action: TeamAction,
    n: usize,
    now: f64,
    sim: &mut SimRes,
    groups: &mut ControlGroups,
    look_at: &mut impl FnMut(Coord3D),
) {
    if n >= 10 {
        return;
    }
    let local = sim.local;
    let live: Vec<ObjectId> = groups.groups[n].iter().copied().filter(|&id| sim.game.object(id).is_some_and(|o| o.owner == local)).collect();
    groups.groups[n] = live.clone();
    let centre = |sim: &SimRes, list: &[ObjectId], look_at: &mut dyn FnMut(Coord3D)| {
        if let Some(o) = list.last().and_then(|&id| sim.game.object(id)) {
            look_at(o.position);
        }
    };
    match action {
        TeamAction::Create => {
            let members: Vec<ObjectId> = sim.selected.iter().copied().filter(|&id| sim.game.object(id).is_some_and(|o| o.owner == local)).collect();
            for g in groups.groups.iter_mut() {
                g.retain(|id| !members.contains(id));
            }
            groups.groups[n] = members;
        }
        TeamAction::View => centre(sim, &live, look_at),
        TeamAction::Add => {
            // A selected structure can't be combined with units (`onMetaAddTeam`).
            if sim.selected.first().is_some_and(|&id| is_structure(&sim.game, id)) || live.first().is_some_and(|&id| is_structure(&sim.game, id)) {
                sim.selected.clear();
            }
            for id in live {
                if !sim.selected.contains(&id) {
                    sim.selected.push(id);
                }
            }
            groups.last = Some((now, n));
        }
        TeamAction::Select => {
            let double = groups.last.is_some_and(|(t, g)| g == n && now - t < DOUBLE_CLICK_SECS);
            let mut select = true;
            if double {
                centre(sim, &live, look_at);
                // Re-select unless the whole group is still selected (`areAllObjectsSelected`).
                select = live.is_empty() || !live.iter().all(|id| sim.selected.contains(id));
            }
            if select {
                sim.selected = live;
            }
            groups.last = Some((now, n));
        }
    }
}

/// `GlobalData::m_doubleClickTimeMS` (OS default 500 ms).
const DOUBLE_CLICK_SECS: f64 = 0.5;

// ---- viewer glue -------------------------------------------------------------------------------

/// The loaded binding table (or empty when no install / parse failure).
#[derive(Resource, Default)]
pub struct MetaMapRes(pub Vec<MetaBinding>);

/// Synthetic key events injected by the `ZR_META_SEQ` debug aid, drained by [`dispatch`].
#[derive(Resource, Default)]
struct InjectedEvents(Vec<KeyEvent>);

pub fn add_systems(app: &mut App) {
    app.init_resource::<MetaMapRes>();
    app.init_resource::<MetaModes>();
    app.init_resource::<ControlGroups>();
    app.init_resource::<InjectedEvents>();
    app.add_systems(OnEnter(crate::shell::ShellState::InGame), setup.after(crate::hud::setup_hud));
    app.add_systems(
        Update,
        (dispatch, inject_debug, animate_control_bar)
            .chain()
            .before(crate::camera::rts_camera)
            .run_if(in_state(crate::shell::ShellState::InGame)),
    );
}

/// Reads `Data\{lang}\CommandMap.ini` then `Data\INI\CommandMap.ini` from the mounted VFS.
fn setup(mut commands: Commands, hud: Option<Res<crate::hud::HudData>>) {
    commands.insert_resource(MetaModes::default());
    commands.insert_resource(ControlGroups::default());
    let owned;
    let vfs: Option<&Vfs> = if let Some(h) = &hud {
        Some(&h.vfs)
    } else {
        owned = zr_assets::Installs::detect().zero_hour.and_then(|i| Vfs::zero_hour(&i).ok());
        owned.as_ref()
    };
    let Some(vfs) = vfs else {
        warn!("meta: no Zero Hour install; meta keys disabled");
        commands.insert_resource(MetaMapRes::default());
        return;
    };
    let lang = zr_ini::detect_language(vfs);
    let lang_path = format!(r"data\{lang}\commandmap.ini");
    let lang_text = vfs.read(&lang_path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let main_text = vfs.read(r"data\ini\commandmap.ini").map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    match parse_command_map(&lang_text, &main_text) {
        Ok(bindings) => {
            info!("meta: {} CommandMap bindings loaded", bindings.len());
            commands.insert_resource(MetaMapRes(bindings));
        }
        Err(e) => {
            warn!("meta: CommandMap.ini parse failed: {e}");
            commands.insert_resource(MetaMapRes::default());
        }
    }
}

fn meta_log() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ZR_META_LOG").is_some())
}

/// `--dev`: re-enables the non-retail viewer camera (WASD pan, Q/E rotate, F free-fly). Read once.
pub(crate) fn dev_mode() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::args().any(|a| a == "--dev"))
}

/// `ZR_META_SEQ="STOP,SCATTER,SELECT_ALL"`: fires the named metas, one every 30 frames from frame
/// 60, by synthesising the key event of the first binding that maps to each meta. This exercises the
/// real table lookup and [`resolve`], not just the apply path. Debug aid only.
fn inject_debug(meta_map: Res<MetaMapRes>, mut inject: ResMut<InjectedEvents>, mut frame: Local<u32>) {
    *frame += 1;
    let Ok(seq) = std::env::var("ZR_META_SEQ") else { return };
    if seq.is_empty() || *frame < 60 {
        return;
    }
    if (*frame - 60) % 30 != 0 {
        return;
    }
    let idx = ((*frame - 60) / 30) as usize;
    if let Some(name) = seq.split(',').nth(idx) {
        if let Some(meta) = Meta::from_name(name.trim()) {
            if let Some(b) = meta_map.0.iter().find(|b| b.meta == meta) {
                inject.0.push(KeyEvent { key: b.key, mods: b.mods, transition: b.transition });
            }
        }
    }
}

/// Recomputes the current modifier state.
fn held_mods(keys: &ButtonInput<KeyCode>) -> Mods {
    let mut m = 0u8;
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
        m |= Mods::CTRL.0;
    }
    if keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight) {
        m |= Mods::ALT.0;
    }
    if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
        m |= Mods::SHIFT.0;
    }
    Mods(m)
}

#[allow(clippy::too_many_arguments)]
fn dispatch(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    interactive: Res<Interactive>,
    free: Res<FreeCam>,
    modal: crate::ui_block::ModalOpen,
    meta_map: Res<MetaMapRes>,
    mut modes: ResMut<MetaModes>,
    mut groups: ResMut<ControlGroups>,
    mut rts: ResMut<RtsCam>,
    radar_events: Res<RadarEvents>,
    window: Query<&Window, With<PrimaryWindow>>,
    cam: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut inject: ResMut<InjectedEvents>,
    sim: Option<NonSendMut<SimRes>>,
    mut was_possessed: Local<bool>,
) {
    let Some(mut sim) = sim else { return };
    // Possession owns the keyboard. On the frame it ends, its release key (`E`) must not also fire
    // `SELECT_MATCHING_UNITS`, whichever order `possess_control` runs in.
    let possessed_now = sim.possessed().is_some();
    if *was_possessed && !possessed_now {
        *was_possessed = false;
        return;
    }
    *was_possessed = possessed_now;
    if meta_map.0.is_empty() {
        return;
    }
    // Modal overlays and the free camera own all input. The `ZR_META_SEQ` debug injection still
    // runs in non-interactive screenshot builds.
    let input_ok = interactive.0 && !free.active && !possessed_now && !modal.get();

    // Build this frame's synthetic key events (normal keys + modifier-only rows), then resolve them
    // against the table. Injected debug events go through the same lookup.
    let mut events: Vec<KeyEvent> = Vec::new();
    if input_ok {
        let mods_now = held_mods(&keys);
        for b in &meta_map.0 {
            if let Some(k) = b.key {
                let hit = match b.transition {
                    Transition::Down => keys.just_pressed(k),
                    Transition::Up => keys.just_released(k),
                };
                if hit && mods_now == b.mods {
                    events.push(KeyEvent { key: Some(k), mods: b.mods, transition: b.transition });
                }
            }
        }
        for (bit, pressed, released) in [
            (Mods::CTRL, keys.just_pressed(KeyCode::ControlLeft) || keys.just_pressed(KeyCode::ControlRight), keys.just_released(KeyCode::ControlLeft) || keys.just_released(KeyCode::ControlRight)),
            (Mods::ALT, keys.just_pressed(KeyCode::AltLeft) || keys.just_pressed(KeyCode::AltRight), keys.just_released(KeyCode::AltLeft) || keys.just_released(KeyCode::AltRight)),
            (Mods::SHIFT, keys.just_pressed(KeyCode::ShiftLeft) || keys.just_pressed(KeyCode::ShiftRight), keys.just_released(KeyCode::ShiftLeft) || keys.just_released(KeyCode::ShiftRight)),
        ] {
            if pressed {
                events.push(KeyEvent { key: None, mods: mods_now, transition: Transition::Down });
            }
            if released {
                events.push(KeyEvent { key: None, mods: bit, transition: Transition::Up });
            }
        }
    }
    events.append(&mut inject.0);
    let fired = resolve(&meta_map.0, &events);
    if fired.is_empty() {
        return;
    }

    let now = time.elapsed_secs_f64();
    let (cams, view) = match (cam.single(), window.single()) {
        (Ok((c, g)), Ok(w)) => (Some((c, g)), w.size()),
        _ => (None, Vec2::ZERO),
    };

    for meta in fired {
        if meta_log() {
            info!("meta: {}", meta.name().unwrap_or("(ignored)"));
        }
        if apply_mode(meta, &mut modes) {
            continue;
        }
        match meta {
            Meta::Stop => {
                if !sim.selected.is_empty() {
                    let (local, ids) = (sim.local, sim.selected.clone());
                    sim.game.submit(local, Command::Stop { ids });
                }
            }
            Meta::Scatter => {
                if !sim.selected.is_empty() {
                    let (local, ids) = (sim.local, sim.selected.clone());
                    sim.game.submit(local, Command::Scatter { ids });
                }
            }
            Meta::SelectAll | Meta::SelectAllAircraft => {
                if let Some((c, g)) = cams {
                    select_all(&mut sim, c, g, view, meta == Meta::SelectAllAircraft);
                }
            }
            Meta::SelectMatchingUnits => {
                if let Some((c, g)) = cams {
                    select_matching(&mut sim, c, g, view);
                }
            }
            Meta::SelectNextUnit | Meta::SelectPrevUnit => {
                if let Some(id) = select_next(&mut sim, meta == Meta::SelectNextUnit, false) {
                    focus_object(&mut rts, &sim, id);
                }
            }
            Meta::SelectNextWorker | Meta::SelectNextIdleWorker | Meta::SelectPrevWorker => {
                if let Some(id) = select_next(&mut sim, meta == Meta::SelectNextWorker || meta == Meta::SelectNextIdleWorker, true) {
                    focus_object(&mut rts, &sim, id);
                }
            }
            Meta::SelectHero => {
                if let Some(id) = select_hero(&mut sim) {
                    focus_object(&mut rts, &sim, id);
                }
            }
            Meta::ViewCommandCenter => {
                if let Some(p) = command_center(&sim) {
                    rts.focus = convert::to_bevy([p.x, p.y, p.z]);
                }
            }
            Meta::ViewLastRadarEvent => {
                // `MSG_META_VIEW_LAST_RADAR_EVENT` (`CommandXlat.cpp:3073`): `userLookAt` the last
                // event's location. Nothing happens when no event has been created yet.
                if let Some(p) = radar_events.last_pos() {
                    rts.focus = convert::to_bevy([p.x, p.y, p.z]);
                    if meta_log() {
                        info!("meta: view last radar event at {:.0},{:.0}", p.x, p.y);
                    }
                }
            }
            Meta::SaveView(n) => {
                modes.saved_views[n as usize] = Some(SavedView { focus: rts.focus, yaw: rts.yaw, pitch: rts.pitch, dist: rts.dist });
            }
            Meta::ViewView(n) => {
                if let Some(v) = modes.saved_views[n as usize] {
                    rts.focus = v.focus;
                    rts.yaw = v.yaw;
                    rts.pitch = v.pitch;
                    rts.dist = v.dist.clamp(rts.min_dist, rts.max_dist);
                }
            }
            Meta::CameraReset => {
                rts.yaw = 0.0;
                rts.pitch = crate::camera::RTS_PITCH;
                rts.dist = DEFAULT_DIST.clamp(rts.min_dist, rts.max_dist);
            }
            Meta::ToggleControlBar => {
                modes.control_bar_hidden = !modes.control_bar_hidden;
            }
            Meta::CreateTeam(n) => apply_team(TeamAction::Create, n as usize, now, &mut sim, &mut groups, &mut |p| rts.focus = convert::to_bevy([p.x, p.y, p.z])),
            Meta::SelectTeam(n) => apply_team(TeamAction::Select, n as usize, now, &mut sim, &mut groups, &mut |p| rts.focus = convert::to_bevy([p.x, p.y, p.z])),
            Meta::AddTeam(n) => apply_team(TeamAction::Add, n as usize, now, &mut sim, &mut groups, &mut |p| rts.focus = convert::to_bevy([p.x, p.y, p.z])),
            Meta::ViewTeam(n) => apply_team(TeamAction::View, n as usize, now, &mut sim, &mut groups, &mut |p| rts.focus = convert::to_bevy([p.x, p.y, p.z])),
            // Modes handled above; metas the viewer does not act on.
            Meta::BeginForceAttack
            | Meta::EndForceAttack
            | Meta::BeginForceMove
            | Meta::EndForceMove
            | Meta::BeginWaypoints
            | Meta::EndWaypoints
            | Meta::BeginPreferSelection
            | Meta::EndPreferSelection
            | Meta::BeginCameraRotateLeft
            | Meta::EndCameraRotateLeft
            | Meta::BeginCameraRotateRight
            | Meta::EndCameraRotateRight
            | Meta::BeginCameraZoomIn
            | Meta::EndCameraZoomIn
            | Meta::BeginCameraZoomOut
            | Meta::EndCameraZoomOut
            | Meta::Ignored => {}
        }
    }
}

/// The viewer's default RTS camera distance (`build.rs`), used by `CAMERA_RESET`/middle-click.
pub(crate) const DEFAULT_DIST: f32 = 450.0;

fn focus_object(rts: &mut RtsCam, sim: &SimRes, id: ObjectId) {
    if let Some(o) = sim.game.object(id) {
        rts.focus = convert::to_bevy([o.position.x, o.position.y, o.position.z]);
    }
}

/// `ToggleControlBar` hides/shows the control bar (`ControlBar::toggleControlBar`).
fn animate_control_bar(modes: Res<MetaModes>, mut root: Query<&mut Visibility, With<crate::hud::HudRoot>>) {
    let Ok(mut vis) = root.single_mut() else { return };
    let want = if modes.control_bar_hidden { Visibility::Hidden } else { Visibility::Inherited };
    if *vis != want {
        *vis = want;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "\
CommandMap STOP
  Key = KEY_S
  Transition = DOWN
  Modifiers = NONE
  UseableIn = GAME
End

CommandMap SCATTER
  Key = KEY_X
  Transition = DOWN
  Modifiers = NONE
  UseableIn = GAME
End

CommandMap SELECT_ALL
  Key = KEY_Q
  Transition = DOWN
  Modifiers = NONE
  UseableIn = GAME
End

CommandMap CREATE_TEAM0
  Key = KEY_0
  Transition = DOWN
  Modifiers = CTRL
  UseableIn = GAME
End

CommandMap SELECT_TEAM0
  Key = KEY_0
  Transition = DOWN
  Modifiers = NONE
  UseableIn = GAME
End

CommandMap BEGIN_FORCEATTACK
  Key = KEY_NONE
  Transition = DOWN
  Modifiers = CTRL
  UseableIn = GAME
End

CommandMap BEGIN_CAMERA_ROTATE_LEFT
  Key = KEY_KP4
  Transition = DOWN
  Modifiers = NONE
  UseableIn = GAME
End

CommandMap END_CAMERA_ROTATE_LEFT
  Key = KEY_KP4
  Transition = UP
  Modifiers = NONE
  UseableIn = GAME
End

CommandMap VIEW_LAST_RADAR_EVENT
  Key = KEY_SPACE
  Transition = DOWN
  Modifiers = NONE
  UseableIn = GAME
End

CommandMap TOGGLE_FAST_FORWARD_REPLAY
  Key = KEY_F
  Transition = DOWN
  Modifiers = NONE
  UseableIn = GAME
End

CommandMap OPTIONS
  Key = KEY_ESC
  Transition = DOWN
  Modifiers = NONE
  UseableIn = SHELL
End
";

    #[test]
    fn parses_key_mods_and_transition() {
        let b = parse_command_map("", TABLE).unwrap();
        assert!(b.iter().any(|x| x.meta == Meta::Stop && x.key == Some(KeyCode::KeyS) && x.mods == Mods::NONE && x.transition == Transition::Down));
        assert!(b.iter().any(|x| x.meta == Meta::CreateTeam(0) && x.key == Some(KeyCode::Digit0) && x.mods == Mods::CTRL));
        assert!(b.iter().any(|x| x.meta == Meta::SelectTeam(0) && x.mods == Mods::NONE));
        assert!(b.iter().any(|x| x.meta == Meta::BeginForceAttack && x.key.is_none() && x.mods == Mods::CTRL));
        assert!(b.iter().any(|x| x.meta == Meta::EndCameraRotateLeft && x.transition == Transition::Up && x.key == Some(KeyCode::Numpad4)));
        // `TOGGLE_FAST_FORWARD_REPLAY` is recognised (key consumed) but ignored.
        assert!(b.iter().any(|x| x.meta == Meta::Ignored && x.key == Some(KeyCode::KeyF)));
        // A `SHELL`-only row is dropped.
        assert!(!b.iter().any(|x| x.key == Some(KeyCode::Escape)));
    }

    #[test]
    fn second_file_overrides_but_keeps_order() {
        let lang = "CommandMap STOP\n Key = KEY_S\n Transition = DOWN\n Modifiers = NONE\n UseableIn = GAME\nEnd\n";
        let main = "CommandMap STOP\n Key = KEY_X\n Transition = DOWN\n Modifiers = NONE\n UseableIn = GAME\nEnd\n";
        let b = parse_command_map(lang, main).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].key, Some(KeyCode::KeyX));
    }

    #[test]
    fn resolve_matches_modifiers_and_first_wins() {
        let b = parse_command_map("", TABLE).unwrap();
        let get = |key, mods| resolve(&b, &[KeyEvent { key: Some(key), mods, transition: Transition::Down }]);
        assert_eq!(get(KeyCode::KeyS, Mods::NONE), vec![Meta::Stop]);
        assert_eq!(get(KeyCode::Digit0, Mods::CTRL), vec![Meta::CreateTeam(0)]);
        assert_eq!(get(KeyCode::Digit0, Mods::NONE), vec![Meta::SelectTeam(0)]);
        // Wrong modifier does not match.
        assert!(get(KeyCode::KeyX, Mods::CTRL).is_empty());
        // The shipped `english/commandmap.ini` row: Space alone, on key down (VIEW_LAST_RADAR_EVENT).
        assert_eq!(get(KeyCode::Space, Mods::NONE), vec![Meta::ViewLastRadarEvent]);
        assert!(get(KeyCode::Space, Mods::CTRL).is_empty());
        // Modifier-only row.
        assert_eq!(resolve(&b, &[KeyEvent { key: None, mods: Mods::CTRL, transition: Transition::Down }]), vec![Meta::BeginForceAttack]);
        // Up transition on the keypad key ends the rotate.
        assert_eq!(
            resolve(&b, &[KeyEvent { key: Some(KeyCode::Numpad4), mods: Mods::NONE, transition: Transition::Up }]),
            vec![Meta::EndCameraRotateLeft]
        );
    }

    #[test]
    fn mode_metas_toggle() {
        let mut m = MetaModes::default();
        assert!(apply_mode(Meta::BeginForceAttack, &mut m));
        assert!(m.force_attack);
        assert!(apply_mode(Meta::EndForceAttack, &mut m));
        assert!(!m.force_attack);
        assert!(apply_mode(Meta::BeginWaypoints, &mut m));
        assert!(m.waypoints);
        assert!(!apply_mode(Meta::Stop, &mut m));
    }
}
