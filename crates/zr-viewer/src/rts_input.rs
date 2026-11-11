//! RTS input for the live sim: click / box select, move, attack and attack-ground orders,
//! selection rings and short-lived order markers. Debug UI, not the final one.
//!
//! Two mouse schemes, chosen by `UseAlternateMouse` (`GlobalData::m_useAlternateMouse`); the
//! routing is the pure [`left_click_action`] / [`right_click_action`] below.
//!
//! * Regular (the default): a left click on a unit selects it (Shift toggles), a left click
//!   anywhere else is the context order (`CommandXlat.cpp:4067`, `LEFT_CLICK`), and a right click
//!   deselects everyone (`SelectionXlat.cpp`, `onRawMouseRightButtonUp`). Right drag scrolls.
//! * Alternate: the left click only selects, and a click on nothing deselects everyone
//!   (`SelectionXlat.cpp`, `onRawMouseLeftButtonUp`); the right click is the context order
//!   (`CommandXlat.cpp:3992`). Right drag scrolls only with `UseRightMouseScrollWithAlternateMouse`.
//!
//! Double-click attack-move (`UseDoubleClickAttackMove`, `CommandXlat.cpp:3973` / `:4048`) is a
//! guard order at the point, `MSG_DO_GUARD_POSITION` with `GUARDMODE_NORMAL`, not an attack-move.
//!
//! The context evaluation is `crate::context_cmd` (`CommandXlat::evaluateContextCommand`): enter /
//! repair / resume / attack / capture / move, with the matching cursor; Ctrl is force-attack
//! (`MSG_META_BEGIN_FORCEATTACK`). The client only submits `zr_sim::Command`s; the selection is
//! client state.

use std::collections::HashSet;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use zr_sim::{Command, GuardMode, GuardTarget, ObjectId};

use crate::camera::{FreeCam, Interactive};
use crate::context_cmd::{self, ContextModes};
use crate::convert;
use crate::sim_view::is_structure;
use crate::sim_view::{ray_sphere, SimRes};
use crate::MapRes;

/// Drag distance (pixels) below which a left press/release is a click.
const DRAG_THRESHOLD: f32 = 5.0;
const MARKER_SECS: f32 = 0.8;

#[derive(Component)]
struct SelectionRing(ObjectId);

#[derive(Component)]
struct OrderMarker {
    ttl: f32,
}

#[derive(Component)]
struct DragBox;

#[derive(Resource)]
pub(crate) struct RtsAssets {
    pub(crate) mesh: Handle<Mesh>,
    ring: Handle<StandardMaterial>,
    mv: Handle<StandardMaterial>,
    attack: Handle<StandardMaterial>,
}

#[derive(Resource, Default)]
struct DragState {
    start: Option<Vec2>,
    /// Time (real seconds) and position of the last left click, for double-click detection.
    last_click: Option<(f64, Vec2)>,
    /// The same for right clicks (`MSG_MOUSE_RIGHT_DOUBLE_CLICK`).
    last_rclick: Option<(f64, Vec2)>,
    /// Right-button down anchor (`MetaEventTranslator::m_mouseDownPosition`), for the click/drag
    /// test on release (retail `Mouse.ini` `DragTolerance` = 25 px).
    rmb_anchor: Option<Vec2>,
    /// The right button press cancelled a pending placement/special-power order, so its release
    /// must not also issue a context command (`CommandXlat`'s `placeBuildAvailable` consumes it).
    rmb_consumed: bool,
    /// The left press was consumed by a pending order (placement or GUI command), so its release is
    /// not a selection click (`GUICommandTranslator` runs before `SelectionTranslator`).
    lmb_order: bool,
    /// `SelectionTranslator::m_preventLeftClickDeselectionInAlternateMouseModeForOneClick`: set when an
    /// order completes, so the next alternate-mouse click on nothing keeps the selection once.
    prevent_deselect: bool,
}

/// What a left click landed on (`SelectionTranslator` picks the selectable drawables in the region,
/// `CommandTranslator` the nearest drawable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hit {
    /// One of the local player's objects; `structure` is `is_structure` (not mass-selectable).
    Own { structure: bool },
    /// Any other object (enemy, neutral, a non-selectable one).
    Other,
    /// No object under the cursor.
    Ground,
}

/// What a point left click does, after the selection and command translators run in retail order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LeftAction {
    None,
    /// Select the object under the cursor (Shift toggles).
    Select,
    /// Double click on an own mass-selectable object: select every matching unit on screen.
    SelectMatching,
    /// Regular mode on an own object: the context order if the object gives one, else select it.
    OrderOrSelect,
    /// The context order at the point (`evaluateContextCommand`).
    Order,
    /// Double-click attack-move: a guard order at the point.
    GuardAt,
}

/// What a point right click does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RightAction {
    /// Deselect every object (`deselectAllDrawables`).
    Deselect,
    /// The context order at the point.
    Order,
    /// Double-click attack-move (alternate mouse): a guard order at the point.
    GuardAt,
}

/// The left click routing. Regular mouse: an own object selects (and gives a command only through
/// [`LeftAction::OrderOrSelect`]); anything else is the order (`CommandXlat.cpp:4067`, LEFT_CLICK, which
/// is actioned unless `UseAlternateMouse` is set). Alternate mouse: the left click only selects
/// (`CommandXlat.cpp:4092`, `contextCommandForNewSelection` is off, `SelectionInfo.cpp:189`).
pub(crate) fn left_click_action(alt: bool, hit: Hit, double: bool, double_attack: bool) -> LeftAction {
    match hit {
        Hit::Own { structure } => {
            if double && !structure {
                LeftAction::SelectMatching
            } else if double && structure && !alt && double_attack {
                LeftAction::GuardAt
            } else if alt {
                LeftAction::Select
            } else {
                LeftAction::OrderOrSelect
            }
        }
        Hit::Other | Hit::Ground => {
            if alt {
                LeftAction::None
            } else if double && double_attack {
                LeftAction::GuardAt
            } else {
                LeftAction::Order
            }
        }
    }
}

/// The right click routing: regular mouse deselects (`SelectionXlat.cpp`, `!m_useAlternateMouse`), and
/// the order is the alternate-mouse right click (`CommandXlat.cpp:3992`). A right double click is the
/// guard order when `UseDoubleClickAttackMove` is on (`CommandXlat.cpp:3971`), else the single click.
pub(crate) fn right_click_action(alt: bool, double: bool, double_attack: bool) -> RightAction {
    if !alt {
        RightAction::Deselect
    } else if double && double_attack {
        RightAction::GuardAt
    } else {
        RightAction::Order
    }
}

/// Radar click (`ControlBarCallback.cpp`, `GWM_LEFT_DOWN` / `GWM_RIGHT_DOWN`): the view moves to the
/// point unless the selection is empty, in which case, or for the other button, the click moves the
/// units there. Regular mouse: the left button orders, the right looks; alternate mouse: the reverse.
pub(crate) fn radar_click_moves_units(alt: bool, left: bool, has_selection: bool) -> bool {
    has_selection && alt != left
}

/// Double-click window (`GlobalData::m_doubleClickTimeMS` is the OS double-click time, 500 ms by
/// default) and pixel tolerance.
const DOUBLE_CLICK_SECS: f64 = 0.5;
const DOUBLE_CLICK_PIXELS: f32 = 5.0;

/// Right-button click/drag tolerance (`Data\INI\Mouse.ini` `DragTolerance` = 25 px): a press that
/// moves less than this between down and up is a click and issues the context command; further and
/// it is a camera drag (`MetaEventTranslator::onMouseEvent` collapses the region to a point).
const RMB_DRAG_PIXELS: f32 = 25.0;

pub fn add_systems(app: &mut App) {
    app.init_resource::<DragState>();
    app.add_systems(OnEnter(crate::shell::ShellState::InGame), setup.after(crate::sim_view::adopt_sim));
    app.add_systems(
        Update,
        (rts_input, sync_rings)
            .chain()
            .after(crate::sim_view::sim_step)
            .before(crate::placement::order_click)
            .run_if(in_state(crate::shell::ShellState::InGame)),
    );
    app.add_systems(Update, fade_markers.run_if(in_state(crate::shell::ShellState::InGame)));
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.insert_resource(DragState::default());
    let unlit = |c: Color| StandardMaterial { base_color: c, unlit: true, ..default() };
    commands.insert_resource(RtsAssets {
        mesh: meshes.add(Torus::new(0.93, 1.0)),
        ring: materials.add(unlit(Color::srgb(1.0, 0.9, 0.1))),
        mv: materials.add(unlit(Color::srgb(0.2, 1.0, 0.3))),
        attack: materials.add(unlit(Color::srgb(1.0, 0.15, 0.1))),
    });
    commands.spawn((
        DragBox,
        bevy::picking::Pickable::IGNORE,
        Node { position_type: PositionType::Absolute, border: UiRect::all(Val::Px(1.0)), display: Display::None, ..default() },
        BorderColor::all(Color::srgb(0.3, 1.0, 0.4)),
        BackgroundColor(Color::srgba(0.3, 1.0, 0.4, 0.08)),
    ));
}

/// Nearest object under the ray among those passing `filter`.
fn pick(sim: &SimRes, o: Vec3, d: Vec3, filter: impl Fn(&SimRes, u8) -> bool) -> Option<ObjectId> {
    let mut best: Option<(f32, ObjectId)> = None;
    for (id, s) in &sim.cur {
        if sim.game.object(*id).is_none_or(|v| !filter(sim, v.owner)) {
            continue;
        }
        let r = sim.game.object_radius(*id).unwrap_or(5.0).max(8.0);
        let c = convert::to_bevy(s.pos) + Vec3::Y * r * 0.5;
        if let Some(t) = ray_sphere(o, d, c, r) {
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, *id));
            }
        }
    }
    best.map(|(_, id)| id)
}

pub(crate) fn spawn_marker(commands: &mut Commands, assets: &RtsAssets, gen: [f32; 3], attack: bool) {
    let mat = if attack { assets.attack.clone() } else { assets.mv.clone() };
    commands.spawn((OrderMarker { ttl: MARKER_SECS }, Mesh3d(assets.mesh.clone()), MeshMaterial3d(mat), convert::object_transform([gen[0], gen[1], gen[2] + 0.8], 0.0).with_scale(Vec3::new(10.0, 1.0, 10.0))));
}

/// The read-only resources the input system needs, bundled to stay within Bevy's 16-parameter
/// system limit.
#[derive(bevy::ecs::system::SystemParam)]
struct InputDeps<'w> {
    map: Res<'w, MapRes>,
    assets: Res<'w, RtsAssets>,
    mode: Res<'w, crate::placement::OrderMode>,
    modal: crate::ui_block::ModalOpen<'w>,
    modes: Res<'w, crate::meta_keys::MetaModes>,
    /// The applied options (the mouse scheme and the double-click attack-move).
    settings: Option<Res<'w, crate::options::GameSettings>>,
}

#[allow(clippy::too_many_arguments)]
fn rts_input(
    mut commands: Commands,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    interactive: Res<Interactive>,
    free: Res<FreeCam>,
    mut drag: ResMut<DragState>,
    window: Query<&Window, With<PrimaryWindow>>,
    cam: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut boxq: Query<&mut Node, With<DragBox>>,
    time: Res<Time<Real>>,
    mut audio: ResMut<crate::audio::AudioQueue>,
    deps: InputDeps,
    sim: Option<NonSendMut<SimRes>>,
) {
    let Some(mut sim) = sim else { return };
    let hide = |boxq: &mut Query<&mut Node, With<DragBox>>| {
        if let Ok(mut n) = boxq.single_mut() {
            n.display = Display::None;
        }
    };
    if !interactive.0 || free.active || sim.possessed().is_some() || deps.modal.get() {
        drag.start = None;
        drag.rmb_anchor = None;
        drag.rmb_consumed = false;
        drag.lmb_order = false;
        hide(&mut boxq);
        return;
    }
    let (Ok(w), Ok((camera, gt))) = (window.single(), cam.single()) else { return };
    let Some(cursor) = w.cursor_position() else { return };
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    let alt_key = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    let local = sim.local;
    let ray = camera.viewport_to_world(gt, cursor).ok();
    // The mouse scheme and the double-click attack-move option (`UseAlternateMouse`,
    // `UseDoubleClickAttackMove`); absent settings are the retail defaults.
    let alt = deps.settings.as_ref().is_some_and(|s| s.alternate_mouse);
    let double_attack = deps.settings.as_ref().is_some_and(|s| s.double_click_attack_move);
    let now = time.elapsed_secs_f64();

    // Right button. Retail starts `SCROLL_RMB` on down (the camera system) and issues the context
    // command only if the press was a click, i.e. moved less than `DragTolerance` between down and
    // up (`MetaEventTranslator::onMouseEvent` collapses the region to a point). A press that
    // cancels a pending placement / special-power order is consumed instead (`CommandXlat.cpp:3950`).
    if buttons.just_pressed(MouseButton::Right) {
        drag.rmb_anchor = Some(cursor);
        drag.rmb_consumed = deps.mode.0.is_some();
    }
    if buttons.just_released(MouseButton::Right) {
        let anchor = drag.rmb_anchor.take();
        let consumed = std::mem::take(&mut drag.rmb_consumed);
        let click = anchor.is_some_and(|a| a.distance(cursor) <= RMB_DRAG_PIXELS);
        if click && !consumed && !deps.modal.over_ui() {
            let double = drag.last_rclick.is_some_and(|(t, p)| now - t < DOUBLE_CLICK_SECS && p.distance(cursor) <= DOUBLE_CLICK_PIXELS);
            drag.last_rclick = if double { None } else { Some((now, cursor)) };
            match right_click_action(alt, double, double_attack) {
                // `SelectionTranslator::onRawMouseRightButtonUp`: regular mouse deselects everyone.
                RightAction::Deselect => sim.selected.clear(),
                // `CommandTranslator::onMouseRightClick` (`CommandXlat.cpp:3992`): alternate mouse only.
                RightAction::Order => {
                    if let (Ok(ray), false) = (camera.viewport_to_world(gt, cursor), sim.selected.is_empty()) {
                        context_cmd::order_context(&mut commands, &mut sim, &deps.assets, &deps.map, &mut audio, ray.origin, *ray.direction, order_modes(&deps.modes));
                    }
                }
                RightAction::GuardAt => {
                    if let Ok(ray) = camera.viewport_to_world(gt, cursor) {
                        guard_at(&mut sim, &deps.map, ray.origin, *ray.direction);
                    }
                }
            }
        }
    }

    // A left press that a pending placement / rally / GUI order consumes is not a selection click
    // (`GUICommandTranslator` runs before `SelectionTranslator`). When the press completed the order,
    // the next alternate-mouse click on nothing keeps the selection once (`GUICommandTranslator`'s
    // `setPreventLeftClickDeselectionInAlternateMouseModeForOneClick` on COMMAND_COMPLETE).
    let order_pending = deps.mode.0.is_some();
    if buttons.just_pressed(MouseButton::Left) {
        drag.lmb_order = order_pending;
    }
    if buttons.just_released(MouseButton::Left) && std::mem::take(&mut drag.lmb_order) && !order_pending {
        drag.prevent_deselect = true;
    }

    // A pending placement / rally order owns the next left click.
    if order_pending {
        drag.start = None;
        hide(&mut boxq);
        return;
    }

    // Selection rectangle. A press on the control bar / radar / a menu belongs to that UI.
    if buttons.just_pressed(MouseButton::Left) && !deps.modal.over_ui() {
        drag.start = Some(cursor);
    }
    if let (Some(s), true) = (drag.start, buttons.pressed(MouseButton::Left)) {
        if let Ok(mut n) = boxq.single_mut() {
            if s.distance(cursor) > DRAG_THRESHOLD {
                let (lo, hi) = (s.min(cursor), s.max(cursor));
                n.display = Display::Flex;
                n.left = Val::Px(lo.x);
                n.top = Val::Px(lo.y);
                n.width = Val::Px(hi.x - lo.x);
                n.height = Val::Px(hi.y - lo.y);
            }
        }
    }
    if buttons.just_released(MouseButton::Left) {
        hide(&mut boxq);
        if let Some(s) = drag.start.take() {
            if s.distance(cursor) > DRAG_THRESHOLD {
                let (lo, hi) = (s.min(cursor), s.max(cursor));
                let mut units = Vec::new();
                let mut structures = Vec::new();
                for (id, snap) in &sim.cur {
                    if sim.game.object(*id).is_none_or(|v| v.owner != local) {
                        continue;
                    }
                    let p = convert::to_bevy(snap.pos);
                    if let Ok(sp) = camera.world_to_viewport(gt, p) {
                        if sp.x >= lo.x && sp.x <= hi.x && sp.y >= lo.y && sp.y <= hi.y {
                            if is_structure(&sim.game, *id) {
                                structures.push(*id);
                            } else {
                                units.push(*id);
                            }
                        }
                    }
                }
                // `Object::isMassSelectable` = selectable && !KINDOF_STRUCTURE (`Object.cpp:3095`);
                // `GameLogic::selectObject` refuses a non-mass-selectable object unless it starts a
                // new selection (`GameLogic.cpp:2719`), so a box with units never grabs structures.
                let hits = if units.is_empty() { structures } else { units };
                if !shift {
                    sim.selected.clear();
                }
                for id in hits {
                    if !sim.selected.contains(&id) {
                        sim.selected.push(id);
                    }
                }
            } else if let Some(ray) = ray {
                let (o, d) = (ray.origin, *ray.direction);
                let double = drag.last_click.is_some_and(|(t, p)| now - t < DOUBLE_CLICK_SECS && p.distance(cursor) <= DOUBLE_CLICK_PIXELS);
                drag.last_click = if double { None } else { Some((now, cursor)) };
                // `SelectionTranslator::onRawMouseLeftButtonUp`: alternate mouse, a plain click with no
                // pending order deselects everyone (retail's `!getGUICommand()` and no modifier keys),
                // unless the one-click prevention is set.
                let plain = !shift && !ctrl && !alt_key;
                if alt && plain && !std::mem::take(&mut drag.prevent_deselect) {
                    sim.selected.clear();
                }
                if deps.modes.force_attack {
                    // Ctrl is force-attack (`MSG_META_BEGIN_FORCEATTACK`): the force evaluation at the point.
                    // The alternate mouse's left button is not an order (`CommandXlat.cpp:4092`).
                    if !alt {
                        context_cmd::order_context(&mut commands, &mut sim, &deps.assets, &deps.map, &mut audio, o, d, order_modes(&deps.modes));
                    }
                } else {
                    let own = pick(&sim, o, d, |sim, owner| owner == sim.local);
                    let hit = match own {
                        Some(id) => Hit::Own { structure: is_structure(&sim.game, id) },
                        None if context_cmd::pick_any(&sim, o, d).is_some() => Hit::Other,
                        None => Hit::Ground,
                    };
                    match left_click_action(alt, hit, double, double_attack) {
                        LeftAction::None => {}
                        LeftAction::Order => {
                            context_cmd::order_context(&mut commands, &mut sim, &deps.assets, &deps.map, &mut audio, o, d, order_modes(&deps.modes));
                        }
                        LeftAction::GuardAt => guard_at(&mut sim, &deps.map, o, d),
                        LeftAction::Select => {
                            if let Some(id) = own {
                                select_click(&mut sim, id, shift);
                            }
                        }
                        // Regular mouse on an own object: the context command when it gives one
                        // (`contextCommandForNewSelection`), otherwise the selection.
                        LeftAction::OrderOrSelect => {
                            if let Some(id) = own {
                                let eval = context_cmd::eval_at(&sim, Some(id), context_cmd::ground_point(&deps.map, o, d), order_modes(&deps.modes));
                                if !context_cmd::submit_eval(&mut commands, &mut sim, &deps.assets, &mut audio, &eval) {
                                    select_click(&mut sim, id, shift);
                                }
                            }
                        }
                        // `SelectionTranslator::onMouseLeftDoubleClick`: select every matching unit on
                        // screen (Alt: across the map).
                        LeftAction::SelectMatching => {
                            if let Some(id) = own {
                                sim.selected = vec![id];
                                select_matching(&mut sim, camera, gt, w.size(), id, alt_key);
                            }
                        }
                    }
                }
            }
        }
    }

}

/// The meta-key modes the context evaluation reads (`InGameUI`'s force-attack, force-move, waypoint
/// and prefer-selection flags).
fn order_modes(m: &crate::meta_keys::MetaModes) -> ContextModes {
    ContextModes { force_attack: m.force_attack, force_move: m.force_move, waypoints: m.waypoints, prefer_selection: m.prefer_selection }
}

/// Guard order at a ground point for the selection (`MSG_DO_GUARD_POSITION`, `GUARDMODE_NORMAL`,
/// `CommandXlat.cpp:3973` for the alternate right button, `:4048` for the regular left one). It is the
/// double-click attack-move: no marker or voice, as in retail.
fn guard_at(sim: &mut SimRes, map: &MapRes, o: Vec3, d: Vec3) {
    if sim.selected.is_empty() {
        return;
    }
    let Some(pos) = context_cmd::ground_point(map, o, d) else { return };
    let (local, ids) = (sim.local, sim.selected.clone());
    sim.game.submit(local, Command::Guard { ids, target: GuardTarget::Point(pos), mode: GuardMode::Normal });
}

/// A selection click on `id`: Shift toggles it in the selection, otherwise it replaces it
/// (`SelectionTranslator::onMouseLeftClick`'s selection branch).
fn select_click(sim: &mut SimRes, id: ObjectId, shift: bool) {
    if shift {
        match sim.selected.iter().position(|&s| s == id) {
            Some(i) => {
                sim.selected.remove(i);
            }
            None => sim.selected.push(id),
        }
    } else {
        sim.selected = vec![id];
    }
}

/// `InGameUI::selectMatchingAcrossScreen` / `selectMatchingAcrossMap` (`similarUnitSelection`): adds
/// every local, uncontained, mass-selectable object of the picked object's template.
fn select_matching(sim: &mut SimRes, camera: &Camera, gt: &GlobalTransform, view: Vec2, picked: ObjectId, across_map: bool) {
    let local = sim.local;
    let Some(template) = sim.game.object(picked).map(|o| o.template.to_owned()) else { return };
    if is_structure(&sim.game, picked) {
        return;
    }
    let mut add = Vec::new();
    for (id, snap) in &sim.cur {
        let Some(o) = sim.game.object(*id) else { continue };
        if o.owner != local || o.template != template || sim.selected.contains(id) || sim.game.logic().is_contained(*id) || is_structure(&sim.game, *id) {
            continue;
        }
        if !across_map {
            let on_screen = camera.world_to_viewport(gt, convert::to_bevy(snap.pos)).is_ok_and(|p| p.x >= 0.0 && p.y >= 0.0 && p.x <= view.x && p.y <= view.y);
            if !on_screen {
                continue;
            }
        }
        add.push(*id);
    }
    sim.selected.extend(add);
}

/// Queues the order-feedback voice (`VoiceMove` / `VoiceAttack`) for each ordered object.
pub(crate) fn order_voice(sim: &SimRes, audio: &mut crate::audio::AudioQueue, ids: &[ObjectId], key: &str) {
    let templates = sim.game.data().templates();
    for &id in ids {
        if let Some(o) = sim.game.object(id) {
            if let Some(ev) = templates.thing(o.template).and_then(|t| t.sounds.get(key)) {
                audio.push(ev.clone(), Some(id), Some([o.position.x, o.position.y, o.position.z]));
            }
        }
    }
}

/// One ring per selected live object, following its interpolated position.
fn sync_rings(
    mut commands: Commands,
    assets: Res<RtsAssets>,
    sim: Option<NonSend<SimRes>>,
    mut rings: Query<(Entity, &SelectionRing, &mut Transform)>,
) {
    let Some(sim) = sim else { return };
    let alpha = (sim.acc / crate::sim_view::STEP).clamp(0.0, 1.0);
    let sel: HashSet<ObjectId> = sim.selected.iter().copied().collect();
    let mut have = HashSet::new();
    let transform = |id: ObjectId, pos: [f32; 3]| {
        let r = (sim.game.object_radius(id).unwrap_or(5.0) * 1.15).max(6.0);
        convert::object_transform([pos[0], pos[1], pos[2] + 0.6], 0.0).with_scale(Vec3::new(r, 1.0, r))
    };
    for (e, ring, mut t) in &mut rings {
        match crate::sim_view::interpolated(&sim, ring.0, alpha) {
            Some((pos, _)) if sel.contains(&ring.0) => {
                *t = transform(ring.0, pos);
                have.insert(ring.0);
            }
            _ => commands.entity(e).despawn(),
        }
    }
    for &id in &sim.selected {
        if have.contains(&id) {
            continue;
        }
        if let Some((pos, _)) = crate::sim_view::interpolated(&sim, id, alpha) {
            commands.spawn((SelectionRing(id), Mesh3d(assets.mesh.clone()), MeshMaterial3d(assets.ring.clone()), transform(id, pos)));
        }
    }
}

fn fade_markers(mut commands: Commands, time: Res<Time>, mut q: Query<(Entity, &mut OrderMarker, &mut Transform)>) {
    for (e, mut m, mut t) in &mut q {
        m.ttl -= time.delta_secs();
        if m.ttl <= 0.0 {
            commands.entity(e).despawn();
        } else {
            let s = 4.0 + 8.0 * m.ttl / MARKER_SECS;
            t.scale = Vec3::new(s, 1.0, s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: Hit = Hit::Own { structure: false };

    /// Regular mouse (the default): the left click orders, the right click deselects. This is the
    /// `CommandXlat.cpp:4067` LEFT_CLICK branch with `m_useAlternateMouse` false.
    #[test]
    fn regular_left_click_orders_and_right_click_deselects() {
        assert_eq!(left_click_action(false, Hit::Ground, false, false), LeftAction::Order);
        assert_eq!(left_click_action(false, Hit::Other, false, false), LeftAction::Order);
        assert_eq!(left_click_action(false, OWN, false, false), LeftAction::OrderOrSelect, "own object: order if it gives one");
        assert_eq!(right_click_action(false, false, false), RightAction::Deselect);
        assert_eq!(right_click_action(false, true, false), RightAction::Deselect, "a double right click deselects too");
    }

    /// Alternate mouse: the left click only selects, the right click is the order (`CommandXlat.cpp:3992`,
    /// `:4092`), and the click on nothing does not order.
    #[test]
    fn alternate_left_click_selects_and_right_click_orders() {
        assert_eq!(left_click_action(true, Hit::Ground, false, false), LeftAction::None, "no order from the left button");
        assert_eq!(left_click_action(true, Hit::Other, false, false), LeftAction::None);
        assert_eq!(left_click_action(true, OWN, false, false), LeftAction::Select);
        assert_eq!(right_click_action(true, false, false), RightAction::Order);
        assert_eq!(right_click_action(true, false, true), RightAction::Order, "no double click: a single order");
    }

    /// `UseDoubleClickAttackMove`: the regular left double click on the ground (or an enemy) and the
    /// alternate right double click are a guard at the point, not the single click's order.
    #[test]
    fn double_click_attack_move_is_a_guard_at_the_point() {
        assert_eq!(left_click_action(false, Hit::Ground, true, true), LeftAction::GuardAt);
        assert_eq!(left_click_action(false, Hit::Other, true, true), LeftAction::GuardAt);
        assert_eq!(left_click_action(false, Hit::Ground, true, false), LeftAction::Order, "option off: the click's order");
        assert_eq!(right_click_action(true, true, true), RightAction::GuardAt);
        assert_eq!(right_click_action(true, true, false), RightAction::Order, "option off");
        assert_eq!(right_click_action(false, true, true), RightAction::Deselect, "the option is an alternate-mouse right click");
        assert_eq!(left_click_action(true, Hit::Ground, true, true), LeftAction::None, "alternate mouse: the left button does not order");
    }

    /// A double click on an own mass-selectable object selects every matching unit, in both schemes.
    /// On an own structure it is not a select-matching (`isMassSelectable`), so the option guards.
    #[test]
    fn double_click_on_own_units_selects_matching() {
        assert_eq!(left_click_action(false, OWN, true, false), LeftAction::SelectMatching);
        assert_eq!(left_click_action(true, OWN, true, true), LeftAction::SelectMatching);
        let structure = Hit::Own { structure: true };
        assert_eq!(left_click_action(false, structure, true, true), LeftAction::GuardAt);
        assert_eq!(left_click_action(true, structure, true, true), LeftAction::Select);
    }

    /// The radar (`ControlBarCallback.cpp`): an empty selection always moves the view; with units the
    /// regular left button and the alternate right button order them, the other button looks.
    #[test]
    fn radar_click_orders_the_units_with_the_scheme_button() {
        assert!(radar_click_moves_units(false, true, true), "regular: left click moves the units");
        assert!(!radar_click_moves_units(false, false, true), "regular: right click looks");
        assert!(!radar_click_moves_units(true, true, true), "alternate: left click looks");
        assert!(radar_click_moves_units(true, false, true), "alternate: right click moves the units");
        for alt in [false, true] {
            for left in [false, true] {
                assert!(!radar_click_moves_units(alt, left, false), "an empty selection never moves units");
            }
        }
    }
}
