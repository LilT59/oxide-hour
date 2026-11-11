//! Tracks whether the mouse is over a UI node, so clicks on the control bar, radar, menus and
//! overlays are not also handled as world clicks (retail: the window manager consumes the mouse
//! message before `CommandXlat` / `SelectionXlat` see it, `GameWindowManager::winProcessMouseEvent`).
//!
//! Uses Bevy's UI picking hover map: any UI node under the cursor blocks the world unless it is
//! marked [`bevy::picking::Pickable::IGNORE`] (full-screen containers, tooltips, the drag box).
//! The `bevy` `ui` feature enables `ui_picking`, and `DefaultPlugins` adds `DefaultPickingPlugins`
//! (which spawns the mouse pointer); mesh picking is opt-in and not added, so world meshes never
//! enter the hover map.
//!
//! `ZR_LOG_UI=1` logs transitions; `ZR_UI_PROBE=1` runs [`probe`], a synthetic-pointer sweep.

use bevy::camera::RenderTarget;
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::{Location, PointerAction, PointerId, PointerInput};
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowRef};

/// True while the mouse pointer is over a blocking UI node.
#[derive(Resource, Default)]
pub struct PointerOverUi(pub bool);

/// An overlay that owns all input is open: the pause menu, the options overlay or the science
/// panel. World clicks, control-group keys and command-bar hotkeys must not fire meanwhile.
#[derive(bevy::ecs::system::SystemParam)]
pub struct ModalOpen<'w> {
    pause: Res<'w, crate::PauseMenu>,
    options: Res<'w, crate::options::OptionsState>,
    promo: Res<'w, crate::promotions::PromotionsData>,
    over: Res<'w, PointerOverUi>,
}

impl ModalOpen<'_> {
    /// The mouse is over a blocking UI node.
    pub fn over_ui(&self) -> bool {
        self.over.0
    }

    pub fn get(&self) -> bool {
        self.pause.0 || self.options.open || self.promo.panel_open()
    }
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<PointerOverUi>();
    app.add_systems(PreUpdate, update.after(bevy::picking::PickingSystems::Hover));
    if std::env::var_os("ZR_UI_PROBE").is_some() {
        app.add_systems(Update, probe.run_if(in_state(crate::shell::ShellState::InGame)));
    }
}

#[allow(clippy::type_complexity)]
fn update(
    hover: Option<Res<HoverMap>>,
    mut over: ResMut<PointerOverUi>,
    mut last: Local<bool>,
    nodes: Query<(&ComputedNode, Option<&Name>, Has<Text>, Has<ImageNode>, Has<BackgroundColor>)>,
    mut seen: Local<Vec<Entity>>,
) {
    // Only UI nodes count: Bevy's window backend always reports the `Window` entity as hit
    // (behind everything), which would otherwise read as "over UI" on open terrain.
    over.0 = hover.as_ref().is_some_and(|h| h.values().any(|m| m.keys().any(|e| nodes.contains(*e))));
    // `ZR_LOG_UI=nodes`: log which node blocks whenever the hovered set changes.
    if std::env::var("ZR_LOG_UI").is_ok_and(|v| v == "nodes") {
        let mut now: Vec<Entity> = hover.iter().flat_map(|h| h.values().flat_map(|m| m.keys().copied())).collect();
        now.sort();
        if now != *seen {
            for e in &now {
                match nodes.get(*e) {
                    Ok((n, name, text, img, bg)) => info!("hover {e:?} {name:?} size {:?} text={text} image={img} bg={bg}", n.size()),
                    Err(_) => info!("hover {e:?} (no ComputedNode)"),
                }
            }
            *seen = now;
        }
    }
    // `ZR_LOG_UI=1`: log transitions (to check the HUD blocks and the open world does not).
    if over.0 != *last {
        *last = over.0;
        if std::env::var_os("ZR_LOG_UI").is_some() {
            info!("pointer over UI: {}", over.0);
        }
    }
}

const COLS: usize = 32;
const ROWS: usize = 18;

/// `ZR_UI_PROBE=1`: after the game has been running a while, moves the synthetic mouse pointer
/// over a COLS x ROWS grid of the window (via `PointerInput`, the same message `CursorMoved`
/// produces) and logs the resulting [`PointerOverUi`] as an ASCII map (`#` blocked, `.` open).
fn probe(
    mut writer: MessageWriter<PointerInput>,
    window: Query<(Entity, &Window), With<PrimaryWindow>>,
    over: Res<PointerOverUi>,
    mut frame: Local<u32>,
    mut cells: Local<Vec<bool>>,
) {
    *frame += 1;
    const START: u32 = 120;
    const PER_CELL: u32 = 3;
    let Ok((entity, win)) = window.single() else { return };
    if *frame < START {
        return;
    }
    let t = *frame - START;
    let (cell, phase) = ((t / PER_CELL) as usize, t % PER_CELL);
    if cell > COLS * ROWS {
        return;
    }
    // Read the result of the previous cell's move (two frames after it was written).
    if phase == 0 && cell > 0 && cells.len() < cell {
        cells.push(over.0);
    }
    if cell == COLS * ROWS {
        if cells.len() == COLS * ROWS {
            let mut s = format!("UI_PROBE {}x{} window {:.0}x{:.0} (# = pointer over UI, . = world):", COLS, ROWS, win.width(), win.height());
            for r in 0..ROWS {
                s.push('\n');
                s.extend(cells[r * COLS..(r + 1) * COLS].iter().map(|&b| if b { '#' } else { '.' }));
            }
            info!("{s}");
        }
        *frame += 1_000_000;
        return;
    }
    if phase == 0 {
        let (c, r) = (cell % COLS, cell / COLS);
        let pos = Vec2::new((c as f32 + 0.5) / COLS as f32 * win.width(), (r as f32 + 0.5) / ROWS as f32 * win.height());
        if let Some(target) = RenderTarget::Window(WindowRef::Entity(entity)).normalize(Some(entity)) {
            writer.write(PointerInput::new(PointerId::Mouse, Location { target, position: pos }, PointerAction::Move { delta: Vec2::ZERO }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::picking::backend::HitData;

    /// `PointerOverUi` follows the hover map: a hovered UI node blocks, an empty map does not, and
    /// a hovered non-UI entity (the `Window`, which Bevy's window backend always reports) does not.
    #[test]
    fn follows_hover_map() {
        let mut app = App::new();
        app.init_resource::<HoverMap>().init_resource::<PointerOverUi>();
        app.add_systems(Update, update);
        app.update();
        assert!(!app.world().resource::<PointerOverUi>().0);
        let hover = |app: &mut App, e: Entity| {
            let hit = HitData::new(e, 0.0, None, None);
            let mut map = app.world_mut().resource_mut::<HoverMap>();
            map.clear();
            map.entry(PointerId::Mouse).or_default().insert(e, hit);
        };
        let window = app.world_mut().spawn_empty().id();
        hover(&mut app, window);
        app.update();
        assert!(!app.world().resource::<PointerOverUi>().0, "window hit must not block");
        let node = app.world_mut().spawn(ComputedNode::default()).id();
        hover(&mut app, node);
        app.update();
        assert!(app.world().resource::<PointerOverUi>().0);
        app.world_mut().resource_mut::<HoverMap>().clear();
        app.update();
        assert!(!app.world().resource::<PointerOverUi>().0);
    }
}
