//! Structure placement and pending target-order modes.
//!
//! `DOZER_CONSTRUCT` enters a placement mode (`InGameUI::placeBuildAvailable` /
//! `handleBuildPlacements`): a ghost of the structure follows the cursor, snapped to the ground,
//! rotates with `Q`/`E`, and is tinted green/red by `GameLogic::is_location_legal_to_build_by`. Left
//! click issues `Command::Construct`; right-click / `Esc` cancels.
//!
//! `SPECIAL_POWER_CONSTRUCT` reuses the same placement interface with the power's `Object` as the
//! ghost (`ControlBarCommandProcessing.cpp:320`); a legal click fires the power at the spot instead
//! of building with a dozer.
//!
//! The same pending mode covers the other click-target orders: `SET_RALLY_POINT`, `ATTACK_MOVE`,
//! `GUARD`, `COMBATDROP`, `SPECIAL_POWER`, `FIRE_WEAPON` (with a target) and the context-mode button
//! commands (`HIJACK_VEHICLE` / `CONVERT_TO_CARBOMB` / `SABOTAGE_BUILDING`). A power with `NEED_TARGET_POS`
//! gets a ground ring of the power's radius (the `RadiusCursorType` cursor);
//! `NEED_TARGET_*_OBJECT` powers and the enter-type commands resolve an object under the cursor on
//! the next left-click, and `COMBATDROP` accepts either (object first, then position), matching
//! `CommandTranslator::handleGuiCommand` / `issueCombatDropCommand` (`CommandXlat.cpp`).

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use zr_sim::math::Coord3D;
use zr_sim::{CombatDropTarget, Command, GuardMode, GuardTarget, ObjectId};

use crate::camera::Interactive;
use crate::hud::HudData;
use crate::pending_cursor::{self, PendingSpec};
use crate::sim_view::{ray_ground, ray_sphere, SimRes};
use crate::MapRes;

/// The enter-type GUI commands that need an object target (`CommandKind` names; retail reaches them
/// through `CommandXlat::createEnterMessage`, which sends `MSG_DO_ENTER`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterKind {
    HijackVehicle,
    ConvertToCarBomb,
    SabotageBuilding,
}

/// A pending click order. `spec` is the command button's cursor and radius fields
/// ([`PendingSpec`]); an order with no button (the `--place` debug flags) carries the default.
#[derive(Debug, Clone, PartialEq)]
pub enum Order {
    /// Set the factory's rally point on the next left-click.
    Rally { factory: ObjectId, spec: PendingSpec },
    /// Place `template` for `dozer` on the next left-click.
    Place { dozer: ObjectId, template: String, forced: Option<Coord3D> },
    /// `ATTACK_MOVE` to the clicked ground point.
    AttackMove { ids: Vec<ObjectId>, forced: Option<Coord3D>, spec: PendingSpec },
    /// `GUARD` the clicked ground point.
    Guard { ids: Vec<ObjectId>, mode: GuardMode, forced: Option<Coord3D>, spec: PendingSpec },
    /// `SPECIAL_POWER` at the clicked point (`need_pos`) and/or object (`need_obj`). The command
    /// button's `Options` decide which the click resolves (`options`). `radius` is the power's
    /// `RadiusCursorRadius`, drawn when `spec.radius_cursor` is set.
    SpecialPower { source: ObjectId, power: String, radius: f32, need_pos: bool, need_obj: bool, options: u32, forced: Option<Coord3D>, spec: PendingSpec },
    /// `SPECIAL_POWER_CONSTRUCT` / `SPECIAL_POWER_CONSTRUCT_FROM_SHORTCUT`: place the power's
    /// `template` with the structure-placement interface (`InGameUI::placeBuildAvailable`,
    /// `ControlBarCommandProcessing.cpp:320`). A legal click fires the power at the spot with the
    /// ghost's angle (`MSG_DO_SPECIAL_POWER_AT_LOCATION`, `PlaceEventTranslator.cpp:252`) rather
    /// than building it with a dozer.
    SpecialPowerConstruct { source: ObjectId, power: String, template: String, forced: Option<Coord3D> },
    /// `COMBATDROP` (Chinook): an object (`MSG_COMBATDROP_AT_OBJECT`) or a ground position
    /// (`MSG_COMBATDROP_AT_LOCATION`).
    CombatDrop { ids: Vec<ObjectId>, need_pos: bool, options: u32, forced: Option<Coord3D>, spec: PendingSpec },
    /// `HIJACK_VEHICLE` / `CONVERT_TO_CARBOMB` / `SABOTAGE_BUILDING`: pick an object and issue
    /// `MSG_DO_ENTER` for the whole selection (retail `createEnterMessage`).
    EnterTarget { ids: Vec<ObjectId>, kind: EnterKind, options: u32, spec: PendingSpec },
    /// A `FIRE_WEAPON` button with a target (`ControlBarCommandProcessing.cpp:214`): the next click
    /// resolves to `MSG_DO_WEAPON_AT_LOCATION` or `MSG_DO_WEAPON_AT_OBJECT`
    /// (`GUICommandTranslator::doFireWeaponCommand`, `GUICommandTranslator.cpp:112`, or the context path
    /// `CommandXlat::handleGuiCommand` for a `CONTEXTMODE` button).
    FireWeapon { ids: Vec<ObjectId>, slot: usize, max_shots: i32, need_pos: bool, need_obj: bool, options: u32, spec: PendingSpec },
}

#[derive(Resource, Default)]
pub struct OrderMode(pub Option<Order>);

/// The ground point the active order currently targets (cursor or forced).
#[derive(Resource, Default)]
pub struct TargetPos(Option<Coord3D>);

/// The object under the cursor for the active order (`NEED_TARGET_*_OBJECT` / `COMBATDROP`), if any.
#[derive(Resource, Default)]
pub struct TargetObj(Option<ObjectId>);

#[derive(Resource, Default)]
pub struct Ghost {
    root: Option<Entity>,
    materials: Vec<Handle<StandardMaterial>>,
    template: String,
    pos: Coord3D,
    angle: f32,
    legal: bool,
}

#[derive(Resource, Default)]
struct Ring {
    entity: Option<Entity>,
    material: Option<Handle<StandardMaterial>>,
    radius: f32,
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<OrderMode>();
    app.init_resource::<TargetPos>();
    app.init_resource::<TargetObj>();
    app.init_resource::<Ghost>();
    app.init_resource::<Ring>();
    app.add_systems(OnEnter(crate::shell::ShellState::InGame), setup_debug_place);
    app.add_systems(Update, pending_demo.before(placement_update));
    app.add_systems(Update, (placement_update, order_click).chain().after(crate::sim_view::sim_step));
}

/// `ZR_PENDING_DEMO=<CommandButton name>` (screenshots): once a unit is selected, clicks that button, so
/// its pending cursor and radius ring show. `--place-pos x,y` sets the ground point of the ring.
fn pending_demo(
    mut done: Local<bool>,
    mut mode: ResMut<OrderMode>,
    mut audio: ResMut<crate::audio::AudioQueue>,
    hud: Option<Res<HudData>>,
    sim: Option<NonSendMut<SimRes>>,
) {
    if *done {
        return;
    }
    let Ok(name) = std::env::var("ZR_PENDING_DEMO") else {
        *done = true;
        return;
    };
    let (Some(hud), Some(mut sim)) = (hud, sim) else { return };
    let Some(factory) = sim.selected.first().copied() else { return };
    *done = true;
    let Some(btn) = hud.commands.button(&name) else {
        warn!("ZR_PENDING_DEMO: no CommandButton `{name}`");
        return;
    };
    let ids = sim.selected.clone();
    // The ring sits on `--place-pos`, else on the selected unit (a screenshot has no pointer on the ground).
    let at = forced_pos().or_else(|| sim.cur.get(&factory).map(|s| Coord3D::new(s.pos[0], s.pos[1], s.pos[2])));
    crate::hud::dispatch_button(&mut sim, &mut mode, factory, &ids, btn, None, &mut audio);
    if let (Some(p), Some(Order::SpecialPower { forced, .. } | Order::AttackMove { forced, .. } | Order::Guard { forced, .. } | Order::CombatDrop { forced, .. })) =
        (at, mode.0.as_mut())
    {
        *forced = Some(p);
    }
    info!("ZR_PENDING_DEMO: {name} -> {:?}", mode.0.is_some());
}

pub(crate) fn forced_pos() -> Option<Coord3D> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == "--place-pos")?;
    let (x, y) = args.get(i + 1)?.split_once(',')?;
    Some(Coord3D::new(x.trim().parse().ok()?, y.trim().parse().ok()?, 0.0))
}

/// Debug flags enter a pending mode for screenshots: `--place <template>`, `--special <power>`,
/// `--attack-move`, `--guard`, each with optional `--place-pos x,y`.
fn setup_debug_place(mut mode: ResMut<OrderMode>) {
    let args: Vec<String> = std::env::args().collect();
    let forced = forced_pos();
    if let Some(i) = args.iter().position(|a| a == "--place") {
        if let Some(template) = args.get(i + 1).cloned() {
            mode.0 = Some(Order::Place { dozer: ObjectId::INVALID, template, forced });
            return;
        }
    }
    if let Some(i) = args.iter().position(|a| a == "--special") {
        if let Some(power) = args.get(i + 1).cloned() {
            // No button: a debug ring at 80 (the `DEBUG` radius type is only a marker that a ring is wanted).
            let spec = PendingSpec { radius_cursor: "DEBUG".into(), hint_split: true, ..PendingSpec::default() };
            mode.0 = Some(Order::SpecialPower {
                source: ObjectId::INVALID,
                power,
                radius: 80.0,
                need_pos: true,
                need_obj: false,
                options: zr_ui::command::command_option::NEED_TARGET_POS,
                forced,
                spec,
            });
            return;
        }
    }
    // `Command_AttackMove` (`CursorName = AttackMove`, no radius cursor) and `Command_Guard` (no CursorName).
    if args.iter().any(|a| a == "--attack-move") {
        let spec = PendingSpec { cursor_name: "AttackMove".into(), invalid_cursor_name: "GenericInvalid".into(), ..PendingSpec::default() };
        mode.0 = Some(Order::AttackMove { ids: Vec::new(), forced, spec });
    } else if args.iter().any(|a| a == "--guard") {
        mode.0 = Some(Order::Guard { ids: Vec::new(), mode: GuardMode::Normal, forced, spec: PendingSpec::default() });
    }
}

/// Local player's dozer (fallback: first mobile local object).
fn local_dozer(sim: &SimRes) -> Option<ObjectId> {
    sim.game
        .spawned()
        .iter()
        .find(|s| s.player == sim.local && s.template.contains("Dozer"))
        .or_else(|| sim.game.spawned().iter().find(|s| s.player == sim.local && s.role.starts_with("StartingUnit")))
        .map(|s| s.id)
        .or_else(|| sim.game.objects().find(|o| o.owner == sim.local).map(|o| o.id))
}

/// `BuildAssistant::isLocationLegalToBuild` for the local player's builder, the same test
/// `Command::Construct` runs, so the ghost tint matches what the order will do.
fn is_legal(sim: &SimRes, template: &str, pos: &Coord3D, angle: f32) -> bool {
    use zr_sim::TemplateSource;
    let Some(t) = sim.game.data().store.find_template(template) else { return false };
    sim.game.logic().is_location_legal_to_build_by(&t, pos, angle, Some(sim.local))
}

fn ground_point(sim: &SimRes, pos: Coord3D) -> Coord3D {
    let z = sim.game.logic().terrain().ground_height(pos.x, pos.y);
    Coord3D::new(pos.x, pos.y, z)
}

/// The cursor on the ground, or `forced` (screenshots).
fn target_point(forced: Option<Coord3D>, sim: &SimRes, cam: &Query<(&Camera, &GlobalTransform), With<Camera3d>>, window: &Query<&Window, With<PrimaryWindow>>, map: &Option<Res<MapRes>>) -> Coord3D {
    if let Some(p) = forced {
        return ground_point(sim, p);
    }
    if let (Ok((camera, gt)), Ok(window), Some(map)) = (cam.single(), window.single(), map.as_deref()) {
        if let Some(cursor) = window.cursor_position() {
            if let Ok(ray) = camera.viewport_to_world(gt, cursor) {
                if let Some(p) = ray_ground(ray.origin, *ray.direction, &map.ground()) {
                    return ground_point(sim, Coord3D::new(p.x, -p.z, 0.0));
                }
            }
        }
    }
    ground_point(sim, Coord3D::new(0.0, 0.0, 0.0))
}

/// The nearest object under the cursor ray (the same sphere pick as `context_cmd::pick_any`).
fn pick_object(sim: &SimRes, cam: &Query<(&Camera, &GlobalTransform), With<Camera3d>>, window: &Query<&Window, With<PrimaryWindow>>) -> Option<ObjectId> {
    let (Ok((camera, gt)), Ok(window)) = (cam.single(), window.single()) else { return None };
    let cursor = window.cursor_position()?;
    let ray = camera.viewport_to_world(gt, cursor).ok()?;
    let (o, d) = (ray.origin, *ray.direction);
    let mut best: Option<(f32, ObjectId)> = None;
    for (id, snap) in &sim.cur {
        if sim.game.object(*id).is_none() {
            continue;
        }
        let r = sim.game.object_radius(*id).unwrap_or(5.0).max(8.0);
        let c = crate::convert::to_bevy(snap.pos) + Vec3::Y * r * 0.5;
        if let Some(t) = ray_sphere(o, d, c, r) {
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, *id));
            }
        }
    }
    best.map(|(_, id)| id)
}

/// `resolveGuiCommandTarget`: the object is only a valid target while its relationship matches one
/// of the button's `NEED_TARGET_*_OBJECT` bits (allies / enemies / neutral).
pub(crate) fn object_target_allowed(sim: &SimRes, local: u8, target: ObjectId, options: u32) -> bool {
    use zr_sim::damage::Relationship;
    use zr_ui::command::command_option as opt;
    let Some(o) = sim.game.logic().find_object(target) else { return false };
    match sim.game.logic().relationship(local, o.player) {
        Relationship::Enemies => options & opt::NEED_TARGET_ENEMY_OBJECT != 0,
        Relationship::Neutral => options & opt::NEED_TARGET_NEUTRAL_OBJECT != 0,
        Relationship::Allies => options & opt::NEED_TARGET_ALLY_OBJECT != 0,
    }
}

/// The target a needing-a-target GUI command resolves to (`handleGuiCommand` / `createEnterMessage`:
/// a valid object wins over the ground point; a position-only command uses the point).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GuiTarget {
    Object(ObjectId),
    Position(Coord3D),
    /// Neither accepted a target; the pending mode stays active (invalid cursor in retail).
    None,
}

/// `resolveGuiCommandTarget`'s outcome for one click. `object` is already relationship-filtered.
pub(crate) fn resolve_gui_target(need_obj: bool, need_pos: bool, object: Option<ObjectId>, point: Coord3D) -> GuiTarget {
    if need_obj {
        if let Some(o) = object {
            return GuiTarget::Object(o);
        }
    }
    if need_pos {
        return GuiTarget::Position(point);
    }
    GuiTarget::None
}

/// Build / move the placement ghost or the radius ring, and record the target point.
#[allow(clippy::too_many_arguments)]
fn placement_update(
    mut commands: Commands,
    mode: Res<OrderMode>,
    mut target: ResMut<TargetPos>,
    mut target_obj: ResMut<TargetObj>,
    mut ghost: ResMut<Ghost>,
    mut ring: ResMut<Ring>,
    sim: Option<NonSend<SimRes>>,
    hud: Option<Res<HudData>>,
    map: Option<Res<MapRes>>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Query<&Window, With<PrimaryWindow>>,
    cam: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    over_ui: Res<crate::ui_block::PointerOverUi>,
) {
    let (Some(sim), Some(hud)) = (sim, hud) else { return };
    // The point the next click resolves at (`TargetPos`), for every pending order. Placement sets it
    // from the ghost below; the other orders need it here (a `FIRE_WEAPON` has no ring to set it).
    if let Some(order) = mode.0.as_ref() {
        if !matches!(order, Order::Place { .. } | Order::SpecialPowerConstruct { .. }) {
            target.0 = Some(target_point(pending_cursor::order_forced(order), &sim, &cam, &window, &map));
        }
    }
    let ring_radius = mode.0.as_ref().and_then(|o| pending_cursor::pending_radius(o, &sim));

    // Object under the cursor for the orders that can target one (`NEED_TARGET_*_OBJECT`).
    let wants_obj = matches!(
        mode.0,
        Some(Order::SpecialPower { need_obj: true, .. })
            | Some(Order::CombatDrop { .. })
            | Some(Order::EnterTarget { .. })
            | Some(Order::FireWeapon { need_obj: true, .. })
    );
    target_obj.0 = if wants_obj { pick_object(&sim, &cam, &window) } else { None };

    // Placement ghost. Both the dozer build and the special-power construct use
    // `InGameUI::placeBuildAvailable`'s structure ghost (`ControlBarCommandProcessing.cpp:265/352`).
    let placement = match mode.0.as_ref() {
        Some(Order::Place { template, forced, .. }) => Some((template.clone(), *forced)),
        Some(Order::SpecialPowerConstruct { template, forced, .. }) => Some((template.clone(), *forced)),
        _ => None,
    };
    if let Some((template, forced)) = placement {
        let pos = target_point(forced, &sim, &cam, &window, &map);
        let mut angle = ghost.angle;
        if keys.pressed(KeyCode::KeyQ) {
            angle += 0.04;
        }
        if keys.pressed(KeyCode::KeyE) {
            angle -= 0.04;
        }
        let legal = is_legal(&sim, &template, &pos, angle);
        if ghost.template != template || ghost.root.is_none() {
            if let Some(e) = ghost.root.take() {
                commands.entity(e).despawn();
            }
            ghost.materials.clear();
            let parts = crate::source::object_model(&hud.ini, &hud.vfs, &template).unwrap_or_default();
            let root = commands.spawn((Transform::default(), Visibility::default())).id();
            if parts.is_empty() {
                let m = meshes.add(Cuboid::new(20.0, 12.0, 20.0));
                let mat = materials.add(StandardMaterial { base_color: Color::srgba(0.2, 1.0, 0.2, 0.5), alpha_mode: AlphaMode::Blend, ..default() });
                ghost.materials.push(mat.clone());
                commands.entity(root).with_children(|p| {
                    p.spawn((Mesh3d(m), MeshMaterial3d(mat), Transform::IDENTITY));
                });
            } else {
                for part in &parts {
                    let (m, mut mat) = crate::convert::model_part(part, &mut images, None);
                    mat.base_color = Color::srgba(0.2, 1.0, 0.2, 0.55);
                    mat.alpha_mode = AlphaMode::Blend;
                    let handle = materials.add(mat);
                    ghost.materials.push(handle.clone());
                    let mesh = meshes.add(m);
                    commands.entity(root).with_children(|p| {
                        p.spawn((Mesh3d(mesh), MeshMaterial3d(handle), Transform::IDENTITY));
                    });
                }
            }
            ghost.root = Some(root);
            ghost.template = template.clone();
        }
        ghost.pos = pos;
        ghost.angle = angle;
        ghost.legal = legal;
        let color = if legal { Color::srgba(0.15, 1.0, 0.2, 0.7) } else { Color::srgba(1.0, 0.12, 0.1, 0.7) };
        for h in &ghost.materials {
            if let Some(mut mat) = materials.get_mut(h) {
                mat.base_color = color;
            }
        }
        if let Some(root) = ghost.root {
            commands.entity(root).insert(crate::convert::object_transform([pos.x, pos.y, pos.z], angle));
        }
        target.0 = Some(pos);
    } else if let Some(e) = ghost.root.take() {
        commands.entity(e).despawn();
    }

    // Target cursor: a ground ring of the radius from the button's `RadiusCursorType` (`setRadiusCursor`,
    // `InGameUI.cpp:1423`). The mesh is sized once, so a ring of another radius is rebuilt.
    if let (Some(radius), Some(pos)) = (ring_radius, target.0) {
        if ring.entity.is_some() && (ring.radius - radius).abs() > f32::EPSILON {
            if let Some(e) = ring.entity.take() {
                commands.entity(e).despawn();
            }
            ring.material = None;
        }
        if ring.entity.is_none() {
            let mat = materials.add(StandardMaterial { base_color: Color::srgba(0.3, 1.0, 0.4, 0.5), unlit: true, alpha_mode: AlphaMode::Blend, ..default() });
            let mesh = meshes.add(Torus::new(radius * 0.96, radius));
            let e = commands.spawn((Mesh3d(mesh), MeshMaterial3d(mat.clone()), Transform::default())).id();
            ring.entity = Some(e);
            ring.material = Some(mat);
            ring.radius = radius;
        }
        if let Some(e) = ring.entity {
            // `createCommandHint` returns before the radius decal is set while a window is under the pointer.
            let visibility = if over_ui.0 { Visibility::Hidden } else { Visibility::Inherited };
            commands.entity(e).insert((crate::convert::object_transform([pos.x, pos.y, pos.z + 1.0], 0.0), visibility));
        }
    } else if let Some(e) = ring.entity.take() {
        commands.entity(e).despawn();
        ring.material = None;
    }
}

/// Left-click commits the pending order; right-click / `Esc` cancels.
#[allow(clippy::too_many_arguments)]
pub fn order_click(
    mut mode: ResMut<OrderMode>,
    target: Res<TargetPos>,
    target_obj: Res<TargetObj>,
    ghost: Res<Ghost>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    sim: Option<NonSendMut<SimRes>>,
    interactive: Res<Interactive>,
    over_ui: Res<crate::ui_block::PointerOverUi>,
    modal: crate::ui_block::ModalOpen,
    mut msg_req: ResMut<crate::messages::MessageRequests>,
) {
    if mode.0.is_none() {
        return;
    }
    // Esc / clicks while an overlay is open belong to the overlay (Esc closing options must not
    // also cancel the order; main.rs orders those two).
    if modal.get() {
        return;
    }
    // Screenshots run non-interactively: keep the pending mode so the cursor stays visible.
    if !interactive.0 {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) || buttons.just_pressed(MouseButton::Right) {
        mode.0 = None;
        return;
    }
    // A click on the control bar / radar belongs to that UI (a command button changes the mode
    // itself), so it does not commit the order.
    if !buttons.just_pressed(MouseButton::Left) || over_ui.0 {
        return;
    }
    let Some(mut sim_guard) = sim else { return };
    let sim = &mut *sim_guard;
    let local = sim.local;
    let point = target.0.unwrap_or(ghost.pos);
    let obj = target_obj.0;
    let sel = sim.selected.clone();
    // Clone so the arm can clear the pending mode while still reading it.
    let order = mode.0.clone().expect("checked above");
    match order {
        Order::Rally { factory, .. } => {
            mode.0 = None;
            sim.game.submit(local, Command::SetRallyPoint { factory, pos: point });
            info!("hud: rally point for {factory:?}");
        }
        Order::Place { dozer, template, .. } => {
            // `InGameUI::handleBuildPlacements`: an illegal spot shows `displayCantBuildMessage`
            // (`GUI:CantBuildThere`, `LBC_GENERIC_FAILURE`) and does not place.
            if !ghost.legal {
                msg_req.push_label(sim, "GUI:CantBuildThere");
                return;
            }
            mode.0 = None;
            let dozer = if dozer == ObjectId::INVALID { local_dozer(sim).unwrap_or(ObjectId::INVALID) } else { dozer };
            if dozer != ObjectId::INVALID {
                sim.game.submit(local, Command::Construct { dozer, template: template.clone(), pos: ghost.pos, angle: ghost.angle });
                info!("hud: construct {template}");
            }
        }
        Order::AttackMove { ids, .. } => {
            mode.0 = None;
            let ids = if ids.is_empty() { sel } else { ids };
            sim.game.submit(local, Command::AttackMove { ids, pos: point });
        }
        Order::Guard { ids, mode: guard_mode, .. } => {
            mode.0 = None;
            let ids = if ids.is_empty() { sel } else { ids };
            sim.game.submit(local, Command::Guard { ids, target: GuardTarget::Point(point), mode: guard_mode });
        }
        Order::SpecialPower { source, power, need_pos, need_obj, options, .. } => {
            let source = if source == ObjectId::INVALID { sel.first().copied().unwrap_or(ObjectId::INVALID) } else { source };
            if source == ObjectId::INVALID {
                mode.0 = None;
                return;
            }
            let obj_allowed = if need_obj { obj.filter(|&t| object_target_allowed(sim, local, t, options)) } else { None };
            // `handleGuiCommand`'s DO path issues only a valid command (`CommandXlat.cpp:1708`); an invalid
            // click keeps the mode, as the invalid cursor says it will.
            if !pending_cursor::special_power_valid(sim.game.logic(), &sel, sim.game.frame(), &power, need_obj, obj_allowed) {
                return;
            }
            match resolve_gui_target(need_obj, need_pos, obj_allowed, point) {
                GuiTarget::Object(t) => {
                    mode.0 = None;
                    info!("hud: {power} at object {}", t.0);
                    sim.game.submit(local, Command::DoSpecialPower { source, power, target: zr_sim::special_power::SpecialPowerTarget::Object(t) });
                }
                GuiTarget::Position(p) => {
                    mode.0 = None;
                    info!("hud: {power} at ({:.0},{:.0})", p.x, p.y);
                    sim.game.submit(local, Command::DoSpecialPower { source, power, target: zr_sim::special_power::SpecialPowerTarget::Position(p) });
                }
                // Invalid object target and no position fallback: keep the mode (retail shows the
                // invalid cursor and leaves the GUI command active).
                GuiTarget::None => {}
            }
        }
        Order::SpecialPowerConstruct { source, power, template, .. } => {
            // `PlaceEventTranslator.cpp:238/252`: the placement must be legal; a legal spot fires
            // `MSG_DO_SPECIAL_POWER_AT_LOCATION` at the ghost position with the ghost's angle (the
            // sim runs the power's OCL to create the structure; `LIKE_EXISTING` takes the angle).
            if !ghost.legal {
                msg_req.push_label(sim, "GUI:CantBuildThere");
                return;
            }
            let source = if source == ObjectId::INVALID { sel.first().copied().unwrap_or(ObjectId::INVALID) } else { source };
            if source == ObjectId::INVALID {
                mode.0 = None;
                return;
            }
            mode.0 = None;
            info!("hud: {power} construct {template} at ({:.0},{:.0})", ghost.pos.x, ghost.pos.y);
            sim.game.submit(local, Command::DoSpecialPowerAtLocation { source, power: power.clone(), pos: ghost.pos, angle: Some(ghost.angle) });
        }
        Order::CombatDrop { ids, need_pos, options, .. } => {
            let units = if ids.is_empty() { sel } else { ids };
            let obj_ok = obj.filter(|&t| object_target_allowed(sim, local, t, options));
            // `handleGuiCommand`'s COMBATDROP test (`CommandXlat.cpp:1701`): an invalid hint keeps the mode.
            if !pending_cursor::combat_drop_valid(sim.game.logic(), &units, obj_ok) {
                return;
            }
            // A valid context command clears the mode even when it sends nothing (`setGUICommand(nullptr)`
            // after the DO path, `CommandXlat.cpp:1743`).
            mode.0 = None;
            if let Some(target) = match resolve_gui_target(true, need_pos, obj_ok, point) {
                GuiTarget::Object(t) => Some(CombatDropTarget::Object(t)),
                GuiTarget::Position(p) => Some(CombatDropTarget::Position(p)),
                GuiTarget::None => None,
            } {
                sim.game.submit(local, Command::CombatDrop { ids: units, target });
            }
        }
        Order::EnterTarget { ids, kind, options, .. } => {
            let ids = if ids.is_empty() { sel } else { ids };
            let obj_ok = obj.filter(|&t| object_target_allowed(sim, local, t, options));
            // `createEnterMessage` through `handleGuiCommand` (`CommandXlat.cpp:1672`): an invalid hint keeps the mode.
            if !pending_cursor::enter_valid(sim.game.logic(), &ids, kind, obj_ok) {
                return;
            }
            let Some(t) = obj_ok else { return };
            mode.0 = None;
            sim.game.submit(local, Command::Enter { ids, container: t });
            info!("hud: enter {t:?} ({kind:?})");
        }
        Order::FireWeapon { ids, slot, max_shots, need_pos, need_obj, options, spec } => {
            let ids = if ids.is_empty() { sel } else { ids };
            let object = if need_obj { obj.filter(|&t| object_target_allowed(sim, local, t, options)) } else { None };
            // A plain command completes on any click; a context command only when its hint is valid
            // (`pending_cursor::fire_weapon_click_clears`).
            let valid = pending_cursor::fire_weapon_valid(sim.game.logic(), &ids, slot, need_obj, object);
            if !pending_cursor::fire_weapon_click_clears(spec.hint_split, valid) {
                return;
            }
            mode.0 = None;
            if let Some(cmd) = fire_weapon_click(ids, slot, max_shots, need_pos, need_obj, point, object) {
                sim.game.submit(local, cmd);
            }
        }
    }
}

/// The weapon order a `FIRE_WEAPON` click sends (`GUICommandTranslator::doFireWeaponCommand`,
/// `GUICommandTranslator.cpp:112-200`). A position-taking button fires at the click point first;
/// otherwise an object-taking button needs `object` (already relationship-filtered) and returns `None`
/// without one, which keeps the mode. A button with neither bit fires `MSG_DO_WEAPON`.
pub(crate) fn fire_weapon_click(ids: Vec<ObjectId>, slot: usize, max_shots: i32, need_pos: bool, need_obj: bool, point: Coord3D, object: Option<ObjectId>) -> Option<Command> {
    if need_pos {
        return Some(Command::DoWeaponAtLocation { ids, slot, pos: point, max_shots });
    }
    if !need_obj {
        return Some(Command::DoWeapon { ids, slot, max_shots });
    }
    object.map(|target| Command::DoWeaponAtObject { ids, slot, target, max_shots })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_target_prefers_an_allowed_object() {
        let p = Coord3D::new(3.0, 4.0, 0.0);
        let o = ObjectId(7);
        assert_eq!(resolve_gui_target(true, true, Some(o), p), GuiTarget::Object(o));
        assert_eq!(resolve_gui_target(true, false, Some(o), p), GuiTarget::Object(o));
    }

    #[test]
    fn gui_target_falls_back_to_the_position() {
        let p = Coord3D::new(3.0, 4.0, 0.0);
        assert_eq!(resolve_gui_target(true, true, None, p), GuiTarget::Position(p));
        assert_eq!(resolve_gui_target(false, true, None, p), GuiTarget::Position(p));
    }

    #[test]
    fn gui_target_is_none_when_only_an_object_target_is_accepted() {
        let p = Coord3D::new(3.0, 4.0, 0.0);
        assert_eq!(resolve_gui_target(true, false, None, p), GuiTarget::None);
        assert_eq!(resolve_gui_target(false, false, None, p), GuiTarget::None);
    }

    #[test]
    fn fire_weapon_no_target_bits_sends_do_weapon() {
        let ids = vec![ObjectId(3)];
        let p = Coord3D::new(3.0, 4.0, 0.0);
        assert_eq!(
            fire_weapon_click(ids.clone(), 1, -1, false, false, p, None),
            Some(Command::DoWeapon { ids, slot: 1, max_shots: -1 })
        );
    }

    #[test]
    fn fire_weapon_position_fires_at_the_point_even_with_an_object_under_it() {
        // `Command_DisarmMinesAtPosition` (`NEED_TARGET_POS`, mine clearing) and `Command_BattleshipFire`.
        let ids = vec![ObjectId(3)];
        let p = Coord3D::new(3.0, 4.0, 0.0);
        assert_eq!(
            fire_weapon_click(ids.clone(), 0, 2, true, false, p, None),
            Some(Command::DoWeaponAtLocation { ids: ids.clone(), slot: 0, pos: p, max_shots: 2 })
        );
        assert_eq!(
            fire_weapon_click(ids.clone(), 0, 2, true, true, p, Some(ObjectId(9))),
            Some(Command::DoWeaponAtLocation { ids, slot: 0, pos: p, max_shots: 2 })
        );
    }

    #[test]
    fn fire_weapon_object_only_waits_for_a_valid_object() {
        // `Command_ChinaTankECMDisableVehicle` (`NEED_TARGET_ENEMY_OBJECT`): empty ground keeps the mode.
        let ids = vec![ObjectId(3)];
        let p = Coord3D::new(3.0, 4.0, 0.0);
        assert_eq!(fire_weapon_click(ids.clone(), 0, -1, false, true, p, None), None);
        assert_eq!(
            fire_weapon_click(ids.clone(), 0, -1, false, true, p, Some(ObjectId(9))),
            Some(Command::DoWeaponAtObject { ids, slot: 0, target: ObjectId(9), max_shots: -1 })
        );
    }
}
