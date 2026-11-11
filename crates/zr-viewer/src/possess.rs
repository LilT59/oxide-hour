//! Possession client (docs/POSSESSION.md, "Client side"): P possesses the selected unit, E
//! releases it. While possessed the camera is a third-person spring arm, WASD moves the unit
//! relative to the camera, the mouse looks, LMB / RMB are fire / alt-fire.
//!
//! The client only reads sim state. Input reaches the sim as `Command::PossessInput`, one per
//! logic frame, submitted by [`PossessState::frame_command`] right before each `Game::step`
//! (see `sim_view::sim_step`). No client-side prediction: offline the sim is local, and the
//! possessed unit is drawn interpolated between logic frames like every other object.
//!
//! Camera feel is taken from Renegade's `CCameraClass` (`Combat/ccamera.cpp`): default profile
//! height 1.95 and distance 3.1 for a 1.8 m character (ratio 1.72 to the body height), the
//! camera is pulled in immediately when the arm hits geometry and unwinds back out at
//! `CAMERA_UNWIND_SPEED = 1.0` (fraction per second). Geometry here is terrain only for now.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use zr_game::Game;
use zr_sim::command::buttons;
use zr_sim::template::{kind_of, LocomotorKind};
use zr_sim::{Command, Coord3D, ObjectId, PlayerIndex, PossessInput};
use zr_w3d::Hierarchy;

use crate::anim::AnimRig;
use crate::camera::{CamLock, Interactive, RtsCam};
use crate::convert;
use crate::sim_view::{interpolated, ray_ground, ray_sphere, SimObject, SimRes, STEP};
use crate::MapRes;

pub const KEY_W: u8 = 1;
pub const KEY_A: u8 = 2;
pub const KEY_S: u8 = 4;
pub const KEY_D: u8 = 8;
const KEY_FIRE: u8 = 16;
const KEY_ALT: u8 = 32;
const KEY_UP: u8 = 64;
const KEY_DOWN: u8 = 128;

/// `CAMERA_UNWIND_SPEED` in ccamera.cpp: arm fraction regained per second.
const UNWIND_SPEED: f32 = 1.0;
/// Camera stays this far above the terrain.
const GROUND_CLEARANCE: f32 = 3.0;
const LOOK_SENS: f32 = 0.0025;
/// Renegade-like over-the-shoulder offset, as a fraction of the unit's body height. The arm is
/// shifted sideways (screen right) and the view direction is unchanged, so the unit sits left of
/// centre while the crosshair still points down the screen-centre ray.
const SHOULDER_FRAC: f32 = 0.32;
/// Toggle auto-follow (camera yaw eases to the unit's heading after the mouse is idle).
const KEY_FOLLOW: KeyCode = KeyCode::KeyT;
/// Seconds without mouse motion before auto-follow starts easing the camera.
const AUTO_FOLLOW_IDLE: f32 = 1.5;
/// Auto-follow yaw easing rate, radians per second.
const AUTO_FOLLOW_RATE: f32 = 2.5;
/// Toggle first / third person.
const KEY_VIEW: KeyCode = KeyCode::KeyV;
/// Toggle `T` follow is [`KEY_FOLLOW`].
const KEY_ASCEND: KeyCode = KeyCode::Space;
const KEY_DESCEND: KeyCode = KeyCode::ControlLeft;
/// Minimum pitch (looking slightly up) and maximum (steeply down). Keeps the camera from ever
/// looking straight up, so a collapsed arm cannot point the view at the sky.
const PITCH_MIN: f32 = -0.15;
const PITCH_MAX: f32 = 1.45;

/// Vehicle bone-camera anchor names, highest priority first (`Hierarchy::bone_index`, then a
/// substring match). An explicit camera bone wins; otherwise the gunner turret, the driver's
/// cockpit, the barrel, then the muzzle / fire points.
const VEHICLE_BONES: &[&str] = &["CAMERA", "TURRET", "DRIVER", "BARREL", "MUZZLEFX", "FIREPOINT"];
/// Infantry head bones, highest priority first.
const INFANTRY_BONES: &[&str] = &["BIP01 HEAD", "HEAD"];

/// The full arm direction (unit -> camera) for a yaw/pitch. `pitch > 0` places the camera above and
/// tilts the view down; the camera looks along `-dir`.
pub fn arm_direction(yaw: f32, pitch: f32) -> Vec3 {
    Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos())
}

/// Camera eye and look point for a (possibly collapsed) arm.
///
/// Renegade (`ccamera.cpp`) builds the transform from the anchor's heading and tilt and only
/// slides the camera along the arm, so the orientation is fixed by yaw/pitch and pulling the arm in
/// never rotates the view. `shoulder` shifts eye and look by the same vector, so the view direction
/// is unchanged (the crosshair ray is still the camera forward ray).
pub fn camera_pose(pivot: Vec3, yaw: f32, pitch: f32, arm: f32, frac: f32, shoulder: Vec3) -> (Vec3, Vec3) {
    let dir = arm_direction(yaw, pitch);
    let eye = pivot + dir * arm * frac;
    let forward = -dir;
    (eye + shoulder, eye + forward + shoulder)
}

/// Largest arm fraction in `[min_frac, 1]` whose point stays above `ground` and outside every
/// vertical cylinder `(base, radius, height)`. Pure, so the collision math is unit-testable.
pub fn arm_sweep(pivot: Vec3, dir: Vec3, arm: f32, min_frac: f32, ground: impl Fn(Vec3) -> f32, cylinders: &[(Vec3, f32, f32)]) -> f32 {
    let mut target = 1.0f32;
    const SAMPLES: u32 = 24;
    for i in 1..=SAMPLES {
        let f = i as f32 / SAMPLES as f32;
        let q = pivot + dir * arm * f;
        if q.y < ground(q) {
            target = ((i - 1) as f32 / SAMPLES as f32).max(min_frac);
            break;
        }
    }
    for (base, r, height) in cylinders {
        if let Some(t) = ray_cylinder_entry(pivot, dir, arm, *base, *r, *height) {
            target = target.min(t.max(min_frac));
        }
    }
    target
}

/// Index of the first bone in `names` (`Hierarchy::find_bone`), or `None`.
pub fn find_bone(h: &Hierarchy, names: &[&str]) -> Option<usize> {
    h.find_bone(names)
}

/// Held keys for `frames` logic frames (`--script-input`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScriptStep {
    pub keys: u8,
    pub frames: u32,
}

/// Parse `"W:60,D:30"`: letters among W A S D (move), F (fire), R (alt-fire); `-` = idle.
pub fn parse_script(s: &str) -> Vec<ScriptStep> {
    s.split(',')
        .filter_map(|part| {
            let (k, n) = part.split_once(':')?;
            let mut keys = 0;
            for c in k.chars() {
                keys |= match c.to_ascii_uppercase() {
                    'W' => KEY_W,
                    'A' => KEY_A,
                    'S' => KEY_S,
                    'D' => KEY_D,
                    'F' => KEY_FIRE,
                    'R' => KEY_ALT,
                    _ => 0,
                };
            }
            Some(ScriptStep { keys, frames: n.trim().parse().ok()? })
        })
        .collect()
}

/// Camera-relative WASD to a world-XY move vector (Generals: x east, y north), quantized.
///
/// Camera yaw `y` in Bevy space looks along `(-sin y, 0, -cos y)`; Bevy `(x, z)` maps to
/// Generals `(x, -z)`, so forward is `(-sin y, cos y)` and right is `(cos y, sin y)`.
pub fn move_vector(keys: u8, yaw: f32) -> (i16, i16) {
    let axis = |pos: u8, neg: u8| (keys & pos != 0) as i32 as f32 - (keys & neg != 0) as i32 as f32;
    let (f, r) = (axis(KEY_W, KEY_S), axis(KEY_D, KEY_A));
    let (s, c) = (yaw.sin(), yaw.cos());
    let (mut x, mut y) = (-s * f + c * r, c * f + s * r);
    let len = (x * x + y * y).sqrt();
    if len > 1.0 {
        x /= len;
        y /= len;
    }
    let q = |v: f32| (v * 32767.0).round().clamp(-32767.0, 32767.0) as i16;
    (q(x), q(y))
}

/// The move vector for a unit. Fixed-wing aircraft (`Wings`) get the raw throttle/turn axes
/// (`move_y` = W/S, `move_x` = D/A) because they cannot strafe; everything else gets the
/// camera-relative world vector ([`move_vector`]).
pub fn move_vector_for(kind: Option<LocomotorKind>, keys: u8, yaw: f32) -> (i16, i16) {
    if kind == Some(LocomotorKind::Wings) {
        let axis = |pos: u8, neg: u8| (keys & pos != 0) as i32 as f32 - (keys & neg != 0) as i32 as f32;
        let q = |v: f32| (v * 32767.0).round().clamp(-32767.0, 32767.0) as i16;
        (q(axis(KEY_D, KEY_A)), q(axis(KEY_W, KEY_S)))
    } else {
        move_vector(keys, yaw)
    }
}

fn button_bits(keys: u8) -> u8 {
    (if keys & KEY_FIRE != 0 { buttons::FIRE } else { 0 })
        | (if keys & KEY_ALT != 0 { buttons::ALT_FIRE } else { 0 })
        | (if keys & KEY_UP != 0 { buttons::ASCEND } else { 0 })
        | (if keys & KEY_DOWN != 0 { buttons::DESCEND } else { 0 })
}

/// Camera yaw that puts the camera behind a unit facing Generals angle `a`.
pub fn yaw_behind(a: f32) -> f32 {
    (-a.cos()).atan2(a.sin())
}

/// Vertical cylinders for every structure except `skip`: `(base, radius, height)` in Bevy space.
/// Uses the bounding-circle radius and the geometry height already on the sim template.
fn structure_bounds(sim: &SimRes, skip: ObjectId) -> Vec<(Vec3, f32, f32)> {
    let mut out = Vec::new();
    for (id, s) in &sim.cur {
        if *id == skip {
            continue;
        }
        let Some(o) = sim.game.logic().find_object(*id) else { continue };
        if !o.template.is_kind_of(kind_of::STRUCTURE) {
            continue;
        }
        let radius = sim.game.object_radius(*id).unwrap_or(5.0).max(2.0);
        let height = o.template.geometry.height.max(radius);
        out.push((convert::to_bevy(s.pos), radius, height));
    }
    out
}

/// First fraction in `[0, 1]` where the segment `origin + dir * length * t` enters the vertical
/// cylinder `(base, radius, height)`, or `None` if it misses / passes over the top.
fn ray_cylinder_entry(origin: Vec3, dir: Vec3, length: f32, base: Vec3, radius: f32, height: f32) -> Option<f32> {
    let (a, b, c) = (dir.x, dir.z, base.y);
    let (px, pz) = (origin.x - base.x, origin.z - base.z);
    let quad = a * a + b * b;
    if quad <= 1e-6 || length <= 0.0 {
        return None;
    }
    let half_b = px * a + pz * b;
    let disc = half_b * half_b - quad * (px * px + pz * pz - radius * radius);
    if disc < 0.0 {
        return None;
    }
    let s = (-half_b - disc.sqrt()) / quad; // distance along the ray to the near wall
    let t = (s / length).clamp(0.0, 1.0);
    let y = origin.y + dir.y * length * t;
    (y >= c && y <= c + height).then_some(t)
}

/// Shortest signed difference `b - a`, wrapped to `[-pi, pi]`.
fn shortest_angle(d: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut d = d % TAU;
    if d > PI {
        d -= TAU;
    } else if d < -PI {
        d += TAU;
    }
    d
}

pub struct PossessStart {
    pub id: ObjectId,
    pub yaw: f32,
    /// Logic frame when `Possess` was submitted.
    pub frame: u32,
    /// Debug `--first-person`: start in the first-person bone camera.
    pub first_person: bool,
}

/// `--possess`: possess the local dozer (else the first starting unit), then run the scripted
/// keys for their frames. Returns the state the viewer starts with.
pub fn auto_possess(game: &mut Game, local: PlayerIndex, script: &[ScriptStep], prefer: Option<ObjectId>, first_person: bool) -> Option<PossessStart> {
    let pick = prefer
        .filter(|id| game.object(*id).is_some_and(|o| o.owner == local))
        .or_else(|| game.spawned().iter().find(|s| s.player == local && s.template.contains("Dozer")).map(|s| s.id))
        .or_else(|| game.spawned().iter().find(|s| s.player == local && s.role.starts_with("StartingUnit")).map(|s| s.id))?;
    let o = game.object(pick)?;
    let yaw = yaw_behind(o.angle);
    let (pos, angle) = (o.position, o.angle);
    let frame = game.frame();
    game.submit(local, Command::Possess { id: pick });
    let aim = Coord3D::new(pos.x + angle.cos() * 200.0, pos.y + angle.sin() * 200.0, pos.z);
    for step in script {
        let (mx, my) = move_vector(step.keys, yaw);
        for _ in 0..step.frames {
            game.submit(local, Command::PossessInput(PossessInput { move_x: mx, move_y: my, aim, buttons: button_bits(step.keys) }));
            game.step();
        }
    }
    if script.is_empty() {
        game.step(); // the Possess message is applied at the start of a frame
    }
    if let Some(o) = game.object(pick) {
        info!("possess: unit {} ({}) at ({:.1}, {:.1}) after {} scripted frames", o.id.0, o.template, o.position.x, o.position.y, script.iter().map(|s| s.frames).sum::<u32>());
    }
    Some(PossessStart { id: pick, yaw, frame, first_person })
}

/// Client-side possession state, owned by `SimRes`.
pub struct PossessState {
    pub id: Option<ObjectId>,
    /// Logic frame when `Possess` was submitted (the sim applies it on the next step).
    begin_frame: u32,
    pub yaw: f32,
    pub pitch: f32,
    /// Wheel zoom multiplier on the default arm length.
    pub zoom: f32,
    /// Arm fraction after collision (1 = full length).
    frac: f32,
    held: u8,
    latched: u8,
    aim: Option<Coord3D>,
    /// Camera yaw eases toward the unit's heading after the mouse is idle (default off).
    pub auto_follow: bool,
    /// Seconds since the mouse last moved.
    idle: f32,
    /// First-person (bone) camera instead of the third-person spring arm.
    pub first_person: bool,
    /// The possessed unit's locomotor kind (for the fixed-wing throttle/turn mapping).
    loco_kind: Option<LocomotorKind>,
}

impl Default for PossessState {
    fn default() -> Self {
        PossessState { id: None, begin_frame: 0, yaw: 0.0, pitch: 0.35, zoom: 1.0, frac: 1.0, held: 0, latched: 0, aim: None, auto_follow: false, idle: 0.0, first_person: false, loco_kind: None }
    }
}

impl PossessState {
    pub fn begin(&mut self, id: ObjectId, yaw: f32, frame: u32) {
        *self = PossessState { id: Some(id), yaw, begin_frame: frame, ..Default::default() };
    }

    pub fn end(&mut self) {
        self.id = None;
    }

    /// The current crosshair aim point (world space), for the radial menu's targeted abilities.
    pub fn aim(&self) -> Option<Coord3D> {
        self.aim
    }

    /// The input message for the next logic frame (held keys plus anything pressed since the
    /// last one, so a quick tap is not lost between frames).
    pub fn frame_command(&mut self) -> Option<Command> {
        self.id?;
        let aim = self.aim?;
        let keys = self.held | self.latched;
        self.latched = 0;
        let (move_x, move_y) = move_vector_for(self.loco_kind, keys, self.yaw);
        Some(Command::PossessInput(PossessInput { move_x, move_y, aim, buttons: button_bits(keys) }))
    }
}

#[derive(Component)]
struct PossessUi;

#[derive(Component)]
struct PossessHud;

pub fn add_systems(app: &mut App) {
    app.add_systems(Update, possess_control.before(crate::sim_view::sim_step).run_if(in_state(crate::shell::ShellState::InGame)));
    app.add_systems(
        Update,
        (possess_camera, hud_update)
            .chain()
            .after(crate::sim_view::sim_step)
            .after(crate::camera::rts_camera)
            .after(crate::camera::free_camera)
            .run_if(in_state(crate::shell::ShellState::InGame)),
    );
    app.add_systems(Update, possess_fog.run_if(in_state(crate::shell::ShellState::InGame)));
}

/// A subtle distance fog while possessing, so the far map edge / black void reads as haze. The
/// engine has no distance fog (`GameData` has none); this is a documented possession-only addition
/// (POSSESSION.md "Visual fidelity"). The skybox sets `fog_enabled = false`, so the sky stays clean.
fn possess_fog(sim: Option<NonSend<SimRes>>, mut commands: Commands, cam: Query<(Entity, Option<&DistanceFog>), With<Camera3d>>) {
    let active = sim.is_some_and(|s| s.possess.id.is_some());
    let Ok((e, fog)) = cam.single() else { return };
    if active && fog.is_none() {
        commands.entity(e).insert(DistanceFog {
            color: Color::srgb(0.52, 0.60, 0.70),
            directional_light_color: Color::NONE,
            directional_light_exponent: 0.0,
            falloff: FogFalloff::Linear { start: 600.0, end: 3200.0 },
        });
    } else if !active && fog.is_some() {
        commands.entity(e).remove::<DistanceFog>();
    }
}

fn spawn_ui(commands: &mut Commands) {
    let bar = |w: f32, h: f32, l: f32, t: f32| {
        (Node { position_type: PositionType::Absolute, width: Val::Px(w), height: Val::Px(h), left: Val::Px(l), top: Val::Px(t), ..default() }, BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.85)))
    };
    commands
        .spawn((
            PossessUi,
            Node { position_type: PositionType::Absolute, left: Val::Percent(50.0), top: Val::Percent(50.0), width: Val::Px(0.0), height: Val::Px(0.0), ..default() },
        ))
        .with_children(|c| {
            c.spawn(bar(2.0, 8.0, -1.0, -14.0));
            c.spawn(bar(2.0, 8.0, -1.0, 6.0));
            c.spawn(bar(8.0, 2.0, -14.0, -1.0));
            c.spawn(bar(8.0, 2.0, 6.0, -1.0));
        });
    commands.spawn((
        PossessUi,
        PossessHud,
        Text::new(""),
        TextFont { font_size: FontSize::Px(20.0), ..default() },
        TextColor(Color::srgb(0.95, 0.95, 0.8)),
        Node { position_type: PositionType::Absolute, left: Val::Px(16.0), bottom: Val::Px(16.0), ..default() },
    ));
}

fn set_cursor(c: &mut CursorOptions, locked: bool) {
    c.grab_mode = if locked { CursorGrabMode::Locked } else { CursorGrabMode::None };
    c.visible = !locked;
}

/// Start / stop possession, then sample mouse look and keys and update the aim point.
#[allow(clippy::too_many_arguments)]
fn possess_control(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    interactive: Res<Interactive>,
    map: Res<MapRes>,
    mut lock: ResMut<CamLock>,
    mut rts: ResMut<RtsCam>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    cam: Query<&Transform, With<Camera3d>>,
    ui: Query<Entity, With<PossessUi>>,
    sim: Option<NonSendMut<SimRes>>,
) {
    let Some(mut sim) = sim else { return };
    let local = sim.local;
    // Release when the sim no longer has this unit possessed by us (death, capture, ...).
    // The Possess message is applied on the first step after submission, so wait for one.
    if let Some(id) = sim.possess.id {
        let applied = sim.game.frame() > sim.possess.begin_frame;
        if sim.game.object(id).is_none() || (applied && sim.game.logic().possessed_unit(local) != Some(id)) {
            sim.possess.end();
        }
    }
    if interactive.0 {
        if sim.possess.id.is_none() && keys.just_pressed(KeyCode::KeyP) {
            let start = sim.selected.first().and_then(|&id| sim.game.object(id)).filter(|o| o.owner == local).map(|o| (o.id, o.angle));
            if let Some((id, angle)) = start {
                let frame = sim.game.frame();
                sim.game.submit(local, Command::Possess { id });
                sim.possess.begin(id, yaw_behind(angle), frame);
                sim.selected = vec![id];
            }
        } else if sim.possess.id.is_some() && keys.just_pressed(KeyCode::KeyE) {
            sim.game.submit(local, Command::Release);
            sim.possess.end();
        }
    }
    let active = sim.possess.id;
    // Mode switch effects (UI, cursor, RTS camera hand-back).
    if active.is_some() != lock.0 {
        lock.0 = active.is_some();
        if let Ok(mut c) = cursor.single_mut() {
            set_cursor(&mut c, active.is_some() && interactive.0);
        }
        if active.is_some() {
            spawn_ui(&mut commands);
        } else {
            for e in &ui {
                commands.entity(e).despawn();
            }
            if let Some(o) = sim.selected.first().and_then(|&id| sim.game.object(id)) {
                rts.focus = convert::to_bevy([o.position.x, o.position.y, o.position.z]);
            }
        }
    }
    let Some(id) = active else { return };
    let loco_kind = sim.game.logic().find_object(id).and_then(|o| o.locomotor()).map(|l| l.kind);
    let p = &mut sim.possess;
    p.loco_kind = loco_kind;
    // While the radial menu is held open it consumes mouse motion to pick a sector, so the camera
    // must not also look around (`radial.rs`).
    let radial_open = keys.pressed(KeyCode::KeyQ) || mouse.pressed(MouseButton::Middle);
    if interactive.0 {
        if keys.just_pressed(KEY_FOLLOW) {
            p.auto_follow = !p.auto_follow;
        }
        if keys.just_pressed(KEY_VIEW) {
            p.first_person = !p.first_person;
        }
        if !radial_open {
            p.yaw -= motion.delta.x * LOOK_SENS;
            p.pitch = (p.pitch + motion.delta.y * LOOK_SENS).clamp(PITCH_MIN, PITCH_MAX);
        }
        let ticks = match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y,
            MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
        };
        p.zoom = (p.zoom * 0.9f32.powf(ticks)).clamp(0.3, 4.0);
        let mut held = 0;
        let mut latch = 0;
        for (k, bit) in [(KeyCode::KeyW, KEY_W), (KeyCode::KeyA, KEY_A), (KeyCode::KeyS, KEY_S), (KeyCode::KeyD, KEY_D)] {
            if keys.pressed(k) {
                held |= bit;
            }
            if keys.just_pressed(k) {
                latch |= bit;
            }
        }
        for (k, bit) in [(KEY_ASCEND, KEY_UP), (KEY_DESCEND, KEY_DOWN)] {
            if keys.pressed(k) {
                held |= bit;
            }
            if keys.just_pressed(k) {
                latch |= bit;
            }
        }
        for (b, bit) in [(MouseButton::Left, KEY_FIRE), (MouseButton::Right, KEY_ALT)] {
            if mouse.pressed(b) {
                held |= bit;
            }
            if mouse.just_pressed(b) {
                latch |= bit;
            }
        }
        p.held = held;
        p.latched |= latch;
    }

    // Aim: where the screen-centre ray hits terrain or another object (previous camera pose).
    let Ok(t) = cam.single() else { return };
    let (o, d) = (t.translation, *t.forward());
    let mut best = ray_ground(o, d, &map.ground()).map(|g| (g.distance(o), g));
    for (oid, s) in &sim.cur {
        if *oid == id {
            continue;
        }
        let r = sim.game.object_radius(*oid).unwrap_or(5.0).max(6.0);
        let c = convert::to_bevy(s.pos) + Vec3::Y * r * 0.5;
        if let Some(dist) = ray_sphere(o, d, c, r) {
            if best.is_none_or(|(bd, _)| dist < bd) {
                best = Some((dist, o + d * dist));
            }
        }
    }
    let far = o + d * 2000.0;
    let hit = best.map_or(far, |(_, g)| g);
    sim.possess.aim = Some(Coord3D::new(hit.x, -hit.z, hit.y));
}

/// Camera for the possessed unit: a third-person spring arm (`ccamera.cpp`) or, in first person, a
/// camera on the unit's turret / cockpit / head bone. The orientation always comes from yaw/pitch,
/// so pulling the arm in (near a structure) never rotates the view.
#[allow(clippy::too_many_arguments)]
fn possess_camera(
    time: Res<Time>,
    map: Res<MapRes>,
    motion: Res<AccumulatedMouseMotion>,
    sim: Option<NonSendMut<SimRes>>,
    mut cam: Query<&mut Transform, With<Camera3d>>,
    mut roots: Query<(&SimObject, &Transform, &mut Visibility, Option<&AnimRig>), Without<Camera3d>>,
    joints: Query<&Transform, (Without<SimObject>, Without<Camera3d>)>,
) {
    let Some(mut sim) = sim else { return };
    let Some(id) = sim.possess.id else { return };
    let dt = time.delta_secs();
    let alpha = (sim.acc / STEP).clamp(0.0, 1.0);
    let Some((pos, angle)) = interpolated(&sim, id, alpha) else { return };
    let radius = sim.game.object_radius(id).unwrap_or(5.0);
    // Renegade profile ratios scaled to the unit: body height ~ 1.2 * radius (9..30 units),
    // pivot 1.95/1.8 of it, arm 3.1/1.8 of it, tripled for the larger-than-life Generals scale.
    let body = (radius * 1.2).clamp(9.0, 30.0);
    let pivot = convert::to_bevy(pos) + Vec3::Y * body * (1.95 / 1.8);
    let loco = sim.game.logic().find_object(id).and_then(|o| o.locomotor());
    let air = loco.is_some_and(|l| l.is_airborne() || l.kind == LocomotorKind::Hover);
    let infantry = sim.game.logic().find_object(id).is_some_and(|o| o.is_kind_of(kind_of::INFANTRY));
    let structures = structure_bounds(&sim, id);

    // Auto-follow (off by default, T toggles): after the mouse is idle, ease the yaw behind the
    // unit's own heading. Turreted units keep aiming through the turret, so this only frames them.
    let p = &mut sim.possess;
    if motion.delta.length_squared() > 0.0 {
        p.idle = 0.0;
    } else {
        p.idle += dt;
    }
    if p.auto_follow && p.idle > AUTO_FOLLOW_IDLE {
        p.yaw += shortest_angle(yaw_behind(angle) - p.yaw).clamp(-AUTO_FOLLOW_RATE * dt, AUTO_FOLLOW_RATE * dt);
    }

    // First person: eye on a bone, and hide the unit's own model (no viewmodel assets exist).
    let first = p.first_person;
    let mut first_eye = None;
    for (so, t, mut vis, rig) in &mut roots {
        if so.0 != id {
            continue;
        }
        *vis = if first { Visibility::Hidden } else { Visibility::Inherited };
        if first {
            first_eye = first_person_eye(rig, t, &joints, infantry);
        }
    }
    if first {
        let eye = first_eye.unwrap_or(pivot);
        let forward = -arm_direction(p.yaw, p.pitch);
        if let Ok(mut t) = cam.single_mut() {
            *t = Transform::from_translation(eye).looking_to(forward, Vec3::Y);
        }
        return;
    }

    // Third person: aircraft get a longer arm and are not clamped to the ground.
    let arm = body * (3.1 / 1.8) * 3.0 * p.zoom * if air { 1.8 } else { 1.0 };
    let dir = arm_direction(p.yaw, p.pitch);
    let ground = map.ground();
    let clearance = if air { 0.0 } else { GROUND_CLEARANCE };
    let h = |q: Vec3| ground.height(q.x, -q.z) + clearance;
    // Keep the camera outside the unit's own bounding circle so a fully pulled-in arm never ends
    // inside the model.
    let min_frac = ((radius + 1.5) / arm).clamp(0.1, 1.0);
    let target = arm_sweep(pivot, dir, arm, min_frac, h, &structures);
    let frac = &mut p.frac;
    if target < *frac {
        *frac = target;
    } else {
        *frac += (target - *frac).min(UNWIND_SPEED * dt);
    }
    let forward = -dir;
    let right = forward.cross(Vec3::Y).normalize_or_zero();
    let shoulder = right * body * SHOULDER_FRAC;
    let (mut eye, _look) = camera_pose(pivot, p.yaw, p.pitch, arm, *frac, shoulder);
    if !air {
        eye.y = eye.y.max(h(eye));
    }
    if let Ok(mut t) = cam.single_mut() {
        *t = Transform::from_translation(eye).looking_to(forward, Vec3::Y);
    }
}

/// World position of the possessed unit's first-person bone (the first [`VEHICLE_BONES`] /
/// [`INFANTRY_BONES`] name in the hierarchy), or `None` to fall back to the body-height pivot.
fn first_person_eye(rig: Option<&AnimRig>, root: &Transform, joints: &Query<&Transform, (Without<SimObject>, Without<Camera3d>)>, infantry: bool) -> Option<Vec3> {
    let rig = rig?;
    let names = if infantry { INFANTRY_BONES } else { VEHICLE_BONES };
    let index = find_bone(&rig.asset.hierarchy, names)?;
    let joint = rig.joint(index)?;
    let local = joints.get(joint).ok()?;
    Some(root.mul_transform(*local).translation)
}

fn hud_update(sim: Option<NonSend<SimRes>>, mut q: Query<&mut Text, With<PossessHud>>) {
    let Some(sim) = sim else { return };
    let Ok(mut text) = q.single_mut() else { return };
    let Some(o) = sim.possess.id.and_then(|id| sim.game.object(id)) else { return };
    let follow = if sim.possess.auto_follow { "follow ON" } else { "follow off" };
    let view = if sim.possess.first_person { "1st" } else { "3rd" };
    **text = format!(
        "{}   HP {:.0}   {view} person  {follow}\nWASD move  mouse look  LMB fire  RMB alt  Space/Ctrl altitude  V view  wheel zoom  T follow  E release",
        o.template, o.health
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_parses() {
        assert_eq!(parse_script("W:60,D:30"), vec![ScriptStep { keys: KEY_W, frames: 60 }, ScriptStep { keys: KEY_D, frames: 30 }]);
        assert_eq!(parse_script("WD:5,-:3,F:2")[2].keys, KEY_FIRE);
    }

    #[test]
    fn move_is_camera_relative_in_world_xy() {
        // yaw 0: forward is north (+y), right is east (+x).
        assert_eq!(move_vector(KEY_W, 0.0), (0, 32767));
        assert_eq!(move_vector(KEY_D, 0.0), (32767, 0));
        // Camera turned left by 90 degrees (yaw +pi/2): forward is west.
        let (x, y) = move_vector(KEY_W, std::f32::consts::FRAC_PI_2);
        assert_eq!((x, y.abs()), (-32767, 0));
        // Diagonals are normalized.
        let (x, y) = move_vector(KEY_W | KEY_D, 0.0);
        assert!((x as i32 - y as i32).abs() <= 1 && x < 24000);
    }

    #[test]
    fn arm_cylinder_test_hits_the_near_wall_and_ignores_passes_over() {
        // Cylinder at x=5, radius 2, 10 tall; ray along +x from (0, 5, 0) enters at x=3.
        assert!((ray_cylinder_entry(Vec3::new(0.0, 5.0, 0.0), Vec3::X, 10.0, Vec3::new(5.0, 0.0, 0.0), 2.0, 10.0).unwrap() - 0.3).abs() < 1e-5);
        // Passing above the top misses.
        assert!(ray_cylinder_entry(Vec3::new(0.0, 20.0, 0.0), Vec3::X, 10.0, Vec3::new(5.0, 0.0, 0.0), 2.0, 10.0).is_none());
        // Passing beside it misses.
        assert!(ray_cylinder_entry(Vec3::new(0.0, 5.0, 3.0), Vec3::X, 10.0, Vec3::new(5.0, 0.0, 0.0), 2.0, 10.0).is_none());
    }

    #[test]
    fn camera_sits_behind_a_facing_unit() {
        // Unit facing east (angle 0): camera west of it, i.e. offset (sin, cos) = (-1, 0) in Bevy.
        let y = yaw_behind(0.0);
        assert!((y.sin() + 1.0).abs() < 1e-5 && y.cos().abs() < 1e-5);
        // Moving "forward" from that camera heads east.
        let (x, yy) = move_vector(KEY_W, y);
        assert!(x > 32000 && yy.abs() < 5);
    }

    #[test]
    fn collapsing_the_arm_does_not_rotate_the_view() {
        // Orientation is fixed by yaw/pitch; only the eye moves along the arm (the regression was
        // `looking_at(look)` flipping the view up when a structure collapsed the arm).
        let pivot = Vec3::new(0.0, 10.0, 0.0);
        let full = camera_pose(pivot, 0.0, 0.35, 30.0, 1.0, Vec3::ZERO);
        let pulled = camera_pose(pivot, 0.0, 0.35, 30.0, 0.1, Vec3::ZERO);
        let dir_full = (full.1 - full.0).normalize();
        let dir_pulled = (pulled.1 - pulled.0).normalize();
        assert!(dir_full.abs_diff_eq(dir_pulled, 1e-5), "view direction is arm-independent");
        assert!(dir_full.y < 0.0, "positive pitch looks down, never at the sky");
        assert!((pulled.0 - pivot).length() < (full.0 - pivot).length(), "pulled in");
    }

    #[test]
    fn arm_sweep_pulls_in_at_ground_and_structures_and_respects_the_minimum() {
        let pivot = Vec3::new(0.0, 10.0, 0.0);
        let dir = Vec3::new(0.0, 0.3, 1.0).normalize();
        // Flat ground far below: full arm.
        assert!((arm_sweep(pivot, dir, 30.0, 0.1, |_| -100.0, &[]) - 1.0).abs() < 1e-5);
        // A wall cylinder behind the pivot pulls the arm in.
        let wall = [(Vec3::new(0.0, 0.0, 5.0), 2.0, 20.0)];
        let f = arm_sweep(pivot, dir, 30.0, 0.1, |_| -100.0, &wall);
        assert!(f < 0.5, "pulled in by the wall: {f}");
        // Ground above the arm path clamps to the minimum fraction.
        let f = arm_sweep(pivot, dir, 30.0, 0.25, |_| 100.0, &[]);
        assert!((f - 0.25).abs() < 1e-5);
    }

    #[test]
    fn wings_use_raw_throttle_and_turn_axes() {
        // W is full throttle (move_y), D is full right turn (move_x), regardless of camera yaw.
        let (x, y) = move_vector_for(Some(LocomotorKind::Wings), KEY_W | KEY_D, 1.234);
        assert_eq!((x, y), (32767, 32767));
        // Ground units still use the camera-relative world vector.
        assert_eq!(move_vector_for(Some(LocomotorKind::Treads), KEY_W, 0.0), move_vector(KEY_W, 0.0));
    }
}
