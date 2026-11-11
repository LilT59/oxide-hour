//! Vehicle tread scroll and tire spin, read from the simulation's per-object speed.
//!
//! Ported from the engine's client draw modules, which run once per drawn frame and read the object's
//! physics: `W3DTankDraw::doDrawModule` / `updateTreadPositions` (`W3DTankDraw.cpp`), `W3DTruckDraw::
//! doDrawModule` (`W3DTruckDraw.cpp`) and `W3DTankTruckDraw::doDrawModule` (`W3DTankTruckDraw.cpp`), all under
//! `GeneralsMD/Code/GameEngineDevice/Source/W3DDevice/GameClient/Drawable/Draw/`. The engine runs them at its
//! frame limit (30 fps), so this ticks at the logic rate (`sim_view::STEP`) and a tire turns one step per
//! logic frame of speed. The viewer only reads the sim.
//!
//! Tread scroll sets the `uv_transform` of each `TREADS*` sub-object's material. The material is a
//! per-instance copy, since the model's materials are shared by every object of that model. Tire spin is a
//! [`BoneControl`] on each tire bone (`HTreeClass::Control_Bone`), applied with the turret and recoil in
//! `anim::animate_rigs`. The front tires also take the steering angle and the four tires the suspension
//! height offsets from the drawable's `TWheelInfo` (`Drawable::calcPhysicsXformWheels`, ported in
//! `physics_xform`), exactly as the engine composes them: a front tire is
//! `Translate_Z(h) * Rotate_Z(steer) * Rotate_Y(spin)` and a rear tire `Rotate_Y(spin) * Translate_Z(h)`.
//!
//! Simplifications: the cab and trailer (`CabBone`) are not turned (that needs the AI path's cached
//! point). A `TREADS` mesh name selects a tread without checking its `LinearOffsetTextureMapper`. The
//! powerslide spray counts as present when the template names one. The airborne test is
//! `GameLogic::isSignificantlyAboveTerrain`'s rule.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::math::Affine2;
use bevy::prelude::*;
use zr_templates::module::{ModuleData, ModuleKind};
use zr_templates::{DrawModule, Templates};
use zr_w3d::{quat_about_axis, quat_mul, quat_rotate, BoneControl, Hierarchy};

use crate::anim::AnimRig;
use crate::physics_xform::{appearance_of, is_physics_appearance, wheel_info, VehiclePhysics, WheelInfo};
use crate::shroud::GhostSnapshot;
use crate::sim_view::{SimObject, SimRes, STEP};

/// `MAX_TREADS_PER_TANK` (`W3DTankDraw.h`): the engine tracks at most four tread sub-objects.
const MAX_TREADS: usize = 4;

/// `isSignificantlyAboveTerrain`: `9 * -gravity` (the sim's own rule, `collide.rs`).
const SIGNIFICANT_FALL: f32 = 9.0;

/// Which engine draw class the vehicle uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `W3DTankDraw`: treads only, with pivot and drive scrolling.
    Tank,
    /// `W3DTruckDraw`: tires only, reversed when moving backwards, with mid-tire pairs.
    Truck,
    /// `W3DTankTruckDraw`: tires and treads, drive scrolling only.
    TankTruck,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
    /// A `TREADS` sub-object with no `L` / `R` suffix (`TREAD_MIDDLE`).
    Middle,
}

/// Which axle a tire bone is on (`m_frontWheelRotation` and friends).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Front,
    Rear,
    MidFront,
    MidRear,
    MidMid,
}

const ROLES: [Role; 5] = [Role::Front, Role::Rear, Role::MidFront, Role::MidRear, Role::MidMid];

/// The tread and tire fields of a vehicle's draw module, from `zr_templates::DrawModule`.
#[derive(Debug)]
pub(crate) struct VehicleSpec {
    kind: Kind,
    /// `TreadAnimationRate` (UV units per logic frame).
    tread_rate: f32,
    pivot_fraction: f32,
    drive_fraction: f32,
    /// `TireRotationMultiplier`.
    tire_rotation: f32,
    /// `PowerslideRotationAddition`.
    powerslide_addition: f32,
    /// The template names a powerslide spray (`W3DTruckDraw` only starts its effect then).
    powerslide_spray: bool,
    /// Tire bone names, by role and side. Empty names are left out.
    tire_names: Vec<(Role, Side, String)>,
}

/// A resolved tire: the bone it rotates and the role that sets its angle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tire {
    bone: usize,
    role: Role,
    side: Side,
}

/// One tread sub-object's material. `offset` is the U offset (`customUVOffset.X`), kept in `[0, 1)`.
struct Tread {
    side: Side,
    material: Handle<StandardMaterial>,
    offset: f32,
}

impl Tread {
    /// `updateTreadPositions`: add `delta` and wrap into `[0, 1)`.
    fn scroll(&mut self, delta: f32) {
        let o = self.offset + delta;
        self.offset = o - o.floor();
    }
}

/// Per-object vehicle state: the tread offsets, the tire angles and the 30 Hz accumulator.
#[derive(Component)]
pub(crate) struct VehicleDraw {
    spec: Arc<VehicleSpec>,
    treads: Vec<Tread>,
    tires: Vec<Tire>,
    acc: f32,
    /// `m_frontWheelRotation` (also the mid-front tires).
    front: f32,
    /// `m_rearWheelRotation` (also the mid-rear and mid-mid tires).
    rear: f32,
    /// `m_isPowersliding` from the previous tick.
    powersliding: bool,
    /// `m_lastDirection`: the heading at the last pivot scroll.
    last_dir: Vec2,
}

/// What the sim says about one vehicle, read once per frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Motion {
    /// `getVelocityMagnitude()`, world units per logic frame.
    speed: f32,
    /// The locomotor's `Speed` (`getCurLocomotorSpeed`).
    max_speed: f32,
    /// `isMotive()`.
    motive: bool,
    /// `getTurning()`: `-1` right, `0` none, `1` left.
    turning: i8,
    /// `Locomotor::isMovingBackwards()`.
    backwards: bool,
    airborne: bool,
    /// The unit direction in 2D (`getUnitDirectionVector2D`).
    dir: Vec2,
    /// `Drawable::getWheelInfo() != nullptr`: `calcPhysicsXform` ran for this object's locomotor
    /// appearance, so it has a `DrawableLocoInfo` (and with it the wheel angle and height offsets).
    wheels: bool,
}

impl Motion {
    /// `velMag / maxSpeed`. A unit without a speed stays still rather than dividing by zero.
    fn ratio(&self) -> f32 {
        if self.max_speed > 0.0 {
            self.speed / self.max_speed
        } else {
            0.0
        }
    }
}

/// Whether the engine's `updateBones` drops a left tire whose right partner is missing.
fn is_paired(kind: Kind, role: Role) -> bool {
    match kind {
        Kind::Tank => false,
        Kind::Truck => matches!(role, Role::MidFront | Role::MidRear | Role::MidMid),
        Kind::TankTruck => !matches!(role, Role::MidMid),
    }
}

/// The left and right bones of one axle, with the engine's pairing rule: a left tire needs its right
/// partner. `Some(0)` counts as missing, as `Get_Bone_Index` returns 0 for an unknown name.
fn pair(paired: bool, left: Option<usize>, right: Option<usize>) -> (Option<usize>, Option<usize>) {
    let left = if paired && right.is_none() { None } else { left };
    (left, right)
}

/// `TREADS*` sub-objects: the mesh name after its container's `.`, starting `TREADS`, then `L` / `R` or
/// nothing (`W3DTankDraw::updateTreadObjects`).
fn tread_side(name: &str) -> Option<Side> {
    let rest = name.split_once('.')?.1;
    if !rest.get(..6)?.eq_ignore_ascii_case("treads") {
        return None;
    }
    Some(match rest.as_bytes().get(6) {
        Some(b'l' | b'L') => Side::Left,
        Some(b'r' | b'R') => Side::Right,
        _ => Side::Middle,
    })
}

/// Resolves the spec's tire names to bones of `h`, applying the engine's pairing rule. A bone at index 0
/// counts as missing, as in the engine.
fn resolve(spec: &VehicleSpec, h: &Hierarchy) -> Vec<Tire> {
    let find = |name: &str| h.bone_index(name).filter(|&i| i != 0);
    let mut out = Vec::new();
    for role in ROLES {
        let get = |side: Side| spec.tire_names.iter().find(|(r, s, _)| *r == role && *s == side).and_then(|(_, _, n)| find(n));
        let (left, right) = pair(is_paired(spec.kind, role), get(Side::Left), get(Side::Right));
        for (side, bone) in [(Side::Left, left), (Side::Right, right)] {
            if let Some(bone) = bone {
                out.push(Tire { bone, role, side });
            }
        }
    }
    out
}

/// The tire and tread fields of a draw module, or `None` for classes without them.
fn spec_from(d: &DrawModule) -> Option<VehicleSpec> {
    let names = |list: &[(Role, Side, &String)]| -> Vec<(Role, Side, String)> {
        list.iter().filter(|(_, _, n)| !n.is_empty()).map(|&(r, s, n)| (r, s, n.clone())).collect()
    };
    Some(match d {
        DrawModule::TankDraw { tread_animation_rate, tread_pivot_speed_fraction, tread_drive_speed_fraction, .. } => VehicleSpec {
            kind: Kind::Tank,
            tread_rate: *tread_animation_rate,
            pivot_fraction: *tread_pivot_speed_fraction,
            drive_fraction: *tread_drive_speed_fraction,
            tire_rotation: 0.0,
            powerslide_addition: 0.0,
            powerslide_spray: false,
            tire_names: Vec::new(),
        },
        DrawModule::TruckDraw {
            powerslide_spray,
            front_left_tire_bone,
            front_right_tire_bone,
            rear_left_tire_bone,
            rear_right_tire_bone,
            mid_front_left_tire_bone,
            mid_front_right_tire_bone,
            mid_rear_left_tire_bone,
            mid_rear_right_tire_bone,
            mid_mid_left_tire_bone,
            mid_mid_right_tire_bone,
            tire_rotation_multiplier,
            powerslide_rotation_addition,
            ..
        } => VehicleSpec {
            kind: Kind::Truck,
            tread_rate: 0.0,
            pivot_fraction: 0.0,
            drive_fraction: 0.0,
            tire_rotation: *tire_rotation_multiplier,
            powerslide_addition: *powerslide_rotation_addition,
            powerslide_spray: !powerslide_spray.is_empty(),
            tire_names: names(&[
                (Role::Front, Side::Left, front_left_tire_bone),
                (Role::Front, Side::Right, front_right_tire_bone),
                (Role::Rear, Side::Left, rear_left_tire_bone),
                (Role::Rear, Side::Right, rear_right_tire_bone),
                (Role::MidFront, Side::Left, mid_front_left_tire_bone),
                (Role::MidFront, Side::Right, mid_front_right_tire_bone),
                (Role::MidRear, Side::Left, mid_rear_left_tire_bone),
                (Role::MidRear, Side::Right, mid_rear_right_tire_bone),
                (Role::MidMid, Side::Left, mid_mid_left_tire_bone),
                (Role::MidMid, Side::Right, mid_mid_right_tire_bone),
            ]),
        },
        DrawModule::TankTruckDraw {
            powerslide_spray,
            front_left_tire_bone,
            front_right_tire_bone,
            rear_left_tire_bone,
            rear_right_tire_bone,
            mid_front_left_tire_bone,
            mid_front_right_tire_bone,
            mid_rear_left_tire_bone,
            mid_rear_right_tire_bone,
            tire_rotation_multiplier,
            powerslide_rotation_addition,
            tread_animation_rate,
            tread_pivot_speed_fraction,
            tread_drive_speed_fraction,
            ..
        } => VehicleSpec {
            kind: Kind::TankTruck,
            tread_rate: *tread_animation_rate,
            pivot_fraction: *tread_pivot_speed_fraction,
            drive_fraction: *tread_drive_speed_fraction,
            tire_rotation: *tire_rotation_multiplier,
            powerslide_addition: *powerslide_rotation_addition,
            powerslide_spray: !powerslide_spray.is_empty(),
            tire_names: names(&[
                (Role::Front, Side::Left, front_left_tire_bone),
                (Role::Front, Side::Right, front_right_tire_bone),
                (Role::Rear, Side::Left, rear_left_tire_bone),
                (Role::Rear, Side::Right, rear_right_tire_bone),
                (Role::MidFront, Side::Left, mid_front_left_tire_bone),
                (Role::MidFront, Side::Right, mid_front_right_tire_bone),
                (Role::MidRear, Side::Left, mid_rear_left_tire_bone),
                (Role::MidRear, Side::Right, mid_rear_right_tire_bone),
            ]),
        },
        _ => return None,
    })
}

/// The vehicle spec of a template: its first tank, truck or tank-truck draw module.
fn spec_of(templates: &Templates, template: &str) -> Option<VehicleSpec> {
    let thing = templates.thing(template)?;
    thing.modules_of(ModuleKind::Draw).find_map(|m| match &m.data {
        ModuleData::Draw(d) => spec_from(d),
        _ => None,
    })
}

/// Specs by template name, looked up once per template.
#[derive(Resource, Default)]
pub(crate) struct VehicleCache {
    specs: HashMap<String, Option<Arc<VehicleSpec>>>,
}

impl VehicleCache {
    fn spec(&mut self, templates: &Templates, template: &str) -> Option<Arc<VehicleSpec>> {
        if let Some(s) = self.specs.get(template) {
            return s.clone();
        }
        let s = spec_of(templates, template).map(Arc::new);
        self.specs.insert(template.to_owned(), s.clone());
        s
    }
}

impl VehicleDraw {
    /// A fresh state for a newly spawned rig: its tires resolved, and a private material per tread.
    fn new(spec: Arc<VehicleSpec>, rig: &AnimRig, parts: &Query<&MeshMaterial3d<StandardMaterial>>, materials: &mut Assets<StandardMaterial>) -> Self {
        let tires = resolve(&spec, &rig.asset.hierarchy);
        let mut treads = Vec::new();
        // `updateTreadObjects` only looks for treads when the template sets a `TreadAnimationRate`.
        if spec.tread_rate != 0.0 {
            for (e, full) in rig.full_names() {
                if treads.len() >= MAX_TREADS {
                    break;
                }
                let (Some(side), Ok(handle)) = (tread_side(full), parts.get(*e)) else { continue };
                let Some(base) = materials.get(&handle.0).cloned() else { continue };
                treads.push(Tread { side, material: materials.add(base), offset: 0.0 });
            }
        }
        VehicleDraw { spec, treads, tires, acc: 0.0, front: 0.0, rear: 0.0, powersliding: false, last_dir: Vec2::X }
    }

    /// One 30 Hz tick: the same order as the engine's `doDrawModule` (wheels, then the powerslide flag,
    /// then the treads). `Tank` only scrolls treads.
    fn tick(&mut self, m: &Motion) {
        let spec = Arc::clone(&self.spec);
        match spec.kind {
            Kind::Tank => self.tank_treads(&spec, m),
            Kind::Truck => {
                self.wheels(&spec, m);
                self.powerslide(&spec, m);
            }
            Kind::TankTruck => {
                self.wheels(&spec, m);
                self.powerslide(&spec, m);
                self.drive_treads(&spec, m);
            }
        }
    }

    /// The tire rotation step (`m_frontWheelRotation += factor * speed`). Trucks reverse their spin when
    /// backing up and add the powerslide addition to the rear tires while powersliding.
    fn wheels(&mut self, spec: &VehicleSpec, m: &Motion) {
        let left_axle = self.tires.iter().any(|t| t.side == Side::Left && matches!(t.role, Role::Front | Role::Rear));
        if !left_axle {
            return;
        }
        // `TankTruck` needs a `WheelInfo` for the whole block; `Truck` only for its bone controls.
        if spec.kind == Kind::TankTruck && !m.wheels {
            return;
        }
        let (mut speed, mut addition) = (m.speed, spec.powerslide_addition);
        if spec.kind == Kind::Truck && m.backwards {
            speed = -speed;
            addition = -addition;
        }
        self.front += spec.tire_rotation * speed;
        let rear_speed = if self.powersliding { speed + addition } else { speed };
        self.rear += spec.tire_rotation * rear_speed;
    }

    /// `W3DTruckDraw`'s powerslide flag: moving, on the ground, turning, with a spray to show.
    fn powerslide(&mut self, spec: &VehicleSpec, m: &Motion) {
        self.powersliding = m.motive && !m.airborne && spec.powerslide_spray && m.turning != 0;
    }

    /// `W3DTankDraw::doDrawModule`: turning on the spot scrolls the treads while the heading changes, and
    /// moving straight at drive speed scrolls them all forwards.
    fn tank_treads(&mut self, spec: &VehicleSpec, m: &Motion) {
        if self.treads.is_empty() {
            return;
        }
        let ratio = m.ratio();
        if m.turning != 0 && ratio < spec.pivot_fraction {
            if (1.0 - m.dir.dot(self.last_dir)).abs() > 0.00001 {
                let delta = if m.turning < 0 { -spec.tread_rate } else { spec.tread_rate };
                for t in &mut self.treads {
                    match t.side {
                        Side::Left => t.scroll(delta),
                        Side::Right => t.scroll(-delta),
                        // `updateTreadPositions` leaves a middle tread's offset unset (it reads an
                        // uninitialised value there); the middle tread is not scrolled here.
                        Side::Middle => {}
                    }
                }
            }
            self.last_dir = m.dir;
        } else if m.motive && ratio >= spec.drive_fraction {
            self.scroll_all(-spec.tread_rate);
        }
    }

    /// `W3DTankTruckDraw`: the pivot path is commented out in the engine, so treads only scroll at drive
    /// speed, all in one direction.
    fn drive_treads(&mut self, spec: &VehicleSpec, m: &Motion) {
        if self.treads.is_empty() {
            return;
        }
        if m.motive && m.ratio() >= spec.drive_fraction {
            self.scroll_all(-spec.tread_rate);
        }
    }

    fn scroll_all(&mut self, delta: f32) {
        for t in &mut self.treads {
            t.scroll(delta);
        }
    }

    /// Tire bone controls: the engine composes a front tire as `Translate_Z(h) * Rotate_Z(steer) *
    /// Rotate_Y(spin)` and a rear tire as `Rotate_Y(spin) * Translate_Z(h)` (`W3DTruckDraw.cpp:458-491`,
    /// `W3DTankTruckDraw.cpp:539-567`). `h` and the steer come from the drawable's `TWheelInfo`
    /// (`Drawable::calcPhysicsXformWheels`); the axle spin from the speed tick above. Controls need a
    /// `WheelInfo`, like the engine's `if (bone && wheelInfo)`.
    fn controls(&self, wheels: Option<WheelInfo>) -> Vec<BoneControl> {
        let Some(w) = wheels else {
            return Vec::new();
        };
        self.tires
            .iter()
            .map(|t| {
                let spin = match t.role {
                    Role::Front | Role::MidFront => self.front,
                    Role::Rear | Role::MidRear | Role::MidMid => self.rear,
                };
                let q_spin = quat_about_axis([0.0, 1.0, 0.0], spin);
                let (translation, rotation) = match t.role {
                    Role::Front | Role::MidFront => {
                        let h = match t.side {
                            Side::Left => w.front_left,
                            Side::Right => w.front_right,
                            Side::Middle => 0.0,
                        };
                        // Generals-space Z-up offset, then the steering about the model's up axis.
                        ([0.0, 0.0, h], quat_mul(&quat_about_axis([0.0, 0.0, 1.0], w.wheel_angle), &q_spin))
                    }
                    Role::Rear | Role::MidRear | Role::MidMid => {
                        let h = match t.side {
                            Side::Left => w.rear_left,
                            Side::Right => w.rear_right,
                            Side::Middle => 0.0,
                        };
                        // The engine rotates first, so the lift is along the spun-up axis.
                        (quat_rotate(&q_spin, [0.0, 0.0, h]), q_spin)
                    }
                };
                BoneControl { bone: t.bone, translation, rotation }
            })
            .collect()
    }
}

/// The motion this frame: velocity, locomotor speed, motive and turning flags, and the heading.
fn motion_of(sim: &SimRes, templates: &Templates, o: &zr_sim::Object, p: &zr_sim::physics::Physics) -> Motion {
    let logic = sim.game.logic();
    let ground = logic.terrain().surface_height(o.position.x, o.position.y, o.position.z);
    Motion {
        speed: p.velocity_magnitude(),
        max_speed: o.locomotor().map_or(0.0, |l| l.max_speed),
        motive: p.is_motive(sim.game.frame()),
        turning: p.turning,
        backwards: p.moving_backwards,
        airborne: o.position.z - ground > SIGNIFICANT_FALL * -logic.rules().gravity,
        dir: Vec2::new(o.angle.cos(), o.angle.sin()),
        // `getWheelInfo()` is non-null once `Drawable::calcPhysicsXform` has handled the appearance.
        wheels: appearance_of(templates, &o.template.name, o.locomotor_set).is_some_and(is_physics_appearance),
    }
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<VehicleCache>();
    app.add_systems(
        Update,
        update_vehicles
            .after(crate::sim_view::sim_sync)
            .before(crate::anim::animate_rigs)
            .run_if(resource_exists::<crate::sim_view::SimActive>),
    );
}

/// Ticks each vehicle at the logic rate and writes its tire controls for `animate_rigs`. A vehicle's
/// state is created the first time its rig is seen (a respawned object gets a fresh one).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn update_vehicles(
    time: Res<Time>,
    sim: NonSend<SimRes>,
    mut cache: ResMut<VehicleCache>,
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    parts: Query<&MeshMaterial3d<StandardMaterial>>,
    mut rigs: Query<(
        Entity,
        &SimObject,
        &mut AnimRig,
        Option<&mut VehicleDraw>,
        Option<&VehiclePhysics>,
        Option<&GhostSnapshot>,
    )>,
) {
    let dt = time.delta_secs();
    let templates = sim.game.data().templates();
    for (root, so, mut rig, draw, physics, ghost) in &mut rigs {
        // A ghost keeps its last-seen pose (`W3DRenderObjectSnapshot` stops updating the render object).
        if ghost.is_some() {
            continue;
        }
        let Some(o) = sim.game.logic().find_object(so.0) else { continue };
        let Some(spec) = cache.spec(templates, &o.template.name) else { continue };
        let Some(phys) = o.physics.as_ref() else { continue };
        let motion = motion_of(&sim, templates, o, phys);
        let Some(mut vehicle) = draw else {
            let vehicle = VehicleDraw::new(spec, &rig, &parts, &mut materials);
            if std::env::var_os("ZR_LOG_OBJECTS").is_some() {
                info!("vehicle {} {}: {:?}, {} tires, {} treads", so.0.0, o.template.name, vehicle.spec.kind, vehicle.tires.len(), vehicle.treads.len());
            }
            commands.entity(root).insert(vehicle);
            continue;
        };
        vehicle.acc += dt;
        while vehicle.acc >= STEP {
            vehicle.acc -= STEP;
            vehicle.tick(&motion);
        }
        rig.vehicle_controls = vehicle.controls(wheel_info(physics));
        if std::env::var_os("ZR_LOG_OBJECTS").is_some() && sim.game.frame() % 30 == 0 {
            let treads: Vec<String> = vehicle.treads.iter().map(|t| format!("{:.3}", t.offset)).collect();
            info!(
                "vehicle {} {}: speed {:.3}/{:.3} turn {} treads {:?} front {:.2} rear {:.2} wheel {:.2}",
                so.0.0,
                o.template.name,
                motion.speed,
                motion.max_speed,
                motion.turning,
                treads,
                vehicle.front,
                vehicle.rear,
                wheel_info(physics).map_or(0.0, |w| w.wheel_angle)
            );
        }
        for t in &vehicle.treads {
            let want = Vec2::new(t.offset, 0.0);
            // Write only on change, so the material is not re-uploaded every frame.
            if materials.get(&t.material).is_some_and(|m| m.uv_transform.translation != want) {
                if let Some(mut m) = materials.get_mut(&t.material) {
                    m.uv_transform = Affine2::from_translation(want);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tread(side: Side, offset: f32) -> Tread {
        Tread { side, material: Handle::default(), offset }
    }

    fn tank_spec(rate: f32) -> VehicleSpec {
        VehicleSpec {
            kind: Kind::Tank,
            tread_rate: rate,
            pivot_fraction: 0.6,
            drive_fraction: 0.3,
            tire_rotation: 0.0,
            powerslide_addition: 0.0,
            powerslide_spray: false,
            tire_names: Vec::new(),
        }
    }

    fn state(spec: VehicleSpec, treads: Vec<Tread>, tires: Vec<Tire>) -> VehicleDraw {
        VehicleDraw {
            spec: Arc::new(spec),
            treads,
            tires,
            acc: 0.0,
            front: 0.0,
            rear: 0.0,
            powersliding: false,
            last_dir: Vec2::X,
        }
    }

    fn moving(speed: f32, max: f32) -> Motion {
        Motion { speed, max_speed: max, motive: true, dir: Vec2::X, wheels: true, ..Motion::default() }
    }

    #[test]
    fn tread_names_follow_the_engine_suffix_rule() {
        assert_eq!(tread_side("tank.treadsl"), Some(Side::Left));
        assert_eq!(tread_side("tank.TREADSR01"), Some(Side::Right));
        assert_eq!(tread_side("tank.treads"), Some(Side::Middle));
        assert_eq!(tread_side("tank.body"), None);
        assert_eq!(tread_side("treadsl"), None, "the engine needs the container dot");
    }

    #[test]
    fn scrolling_wraps_into_the_unit_interval() {
        let mut t = tread(Side::Left, 0.9);
        t.scroll(0.2);
        assert!((t.offset - 0.1).abs() < 1e-6);
        t.scroll(-0.3);
        assert!((t.offset - 0.8).abs() < 1e-6);
    }

    #[test]
    fn a_stationary_unit_does_not_scroll() {
        let mut v = state(tank_spec(0.05), vec![tread(Side::Left, 0.0)], Vec::new());
        v.tick(&Motion { max_speed: 0.0, motive: true, ..Motion::default() });
        assert_eq!(v.treads[0].offset, 0.0);
    }

    #[test]
    fn driving_straight_scrolls_every_tread_the_same_way() {
        let mut v = state(tank_spec(0.1), vec![tread(Side::Left, 0.5), tread(Side::Right, 0.5)], Vec::new());
        v.tick(&moving(1.0, 1.0));
        assert!((v.treads[0].offset - 0.4).abs() < 1e-6);
        assert!((v.treads[1].offset - 0.4).abs() < 1e-6);
    }

    #[test]
    fn pivoting_scrolls_the_two_sides_in_opposite_directions() {
        let mut v = state(tank_spec(0.1), vec![tread(Side::Left, 0.5), tread(Side::Right, 0.5)], Vec::new());
        // Turning left (`turning = 1`) while the heading differs from the last one.
        v.last_dir = Vec2::Y;
        v.tick(&Motion { turning: 1, dir: Vec2::X, max_speed: 1.0, ..Motion::default() });
        assert!((v.treads[0].offset - 0.6).abs() < 1e-6, "left tread moves forwards");
        assert!((v.treads[1].offset - 0.4).abs() < 1e-6, "right tread moves backwards");
        assert_eq!(v.last_dir, Vec2::X);
    }

    #[test]
    fn tires_spin_with_speed_and_the_truck_reverses_when_backing_up() {
        let truck = VehicleSpec {
            kind: Kind::Truck,
            tread_rate: 0.0,
            pivot_fraction: 0.0,
            drive_fraction: 0.0,
            tire_rotation: 2.0,
            powerslide_addition: 0.0,
            powerslide_spray: false,
            tire_names: Vec::new(),
        };
        let tires = vec![Tire { bone: 3, role: Role::Front, side: Side::Left }];
        let mut v = state(truck, Vec::new(), tires);
        v.tick(&moving(0.5, 1.0));
        assert!((v.front - 1.0).abs() < 1e-6);
        v.tick(&Motion { backwards: true, ..moving(0.5, 1.0) });
        assert!((v.front - 0.0).abs() < 1e-6, "a backing truck spins its tires back");
    }

    #[test]
    fn tank_truck_tires_need_wheel_info() {
        let spec = VehicleSpec {
            kind: Kind::TankTruck,
            tread_rate: 0.0,
            pivot_fraction: 0.0,
            drive_fraction: 0.0,
            tire_rotation: 1.0,
            powerslide_addition: 0.0,
            powerslide_spray: false,
            tire_names: Vec::new(),
        };
        let tires = vec![Tire { bone: 2, role: Role::Rear, side: Side::Left }];
        let mut v = state(spec, Vec::new(), tires);
        v.tick(&Motion { wheels: false, ..moving(1.0, 1.0) });
        assert_eq!(v.rear, 0.0);
        v.tick(&moving(1.0, 1.0));
        assert!((v.rear - 1.0).abs() < 1e-6);
    }

    #[test]
    fn controls_turn_each_tire_about_y_by_its_axle_spin() {
        let tires = vec![Tire { bone: 4, role: Role::Front, side: Side::Left }, Tire { bone: 5, role: Role::MidMid, side: Side::Right }];
        let mut v = state(tank_spec(0.0), Vec::new(), tires);
        v.front = 0.3;
        v.rear = 0.7;
        let w = WheelInfo::default();
        let c = v.controls(Some(w));
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].bone, c[1].bone), (4, 5));
        assert_eq!(c[0].rotation, quat_about_axis([0.0, 1.0, 0.0], 0.3));
        assert_eq!(c[1].rotation, quat_about_axis([0.0, 1.0, 0.0], 0.7));
        assert!(v.controls(None).is_empty(), "no WheelInfo, no tire controls");
    }

    #[test]
    fn front_tires_steer_and_all_tires_take_their_height_offset() {
        let tires = vec![
            Tire { bone: 4, role: Role::Front, side: Side::Left },
            Tire { bone: 5, role: Role::Rear, side: Side::Right },
        ];
        let mut v = state(tank_spec(0.0), Vec::new(), tires);
        v.front = 0.5;
        v.rear = 0.5;
        let w = WheelInfo { front_left: -0.2, rear_right: 0.1, wheel_angle: 0.4, ..WheelInfo::default() };
        let c = v.controls(Some(w));
        // Front: `Translate_Z(h) * Rotate_Z(steer) * Rotate_Y(spin)`.
        assert_eq!(c[0].translation, [0.0, 0.0, -0.2]);
        assert_eq!(c[0].rotation, quat_mul(&quat_about_axis([0.0, 0.0, 1.0], 0.4), &quat_about_axis([0.0, 1.0, 0.0], 0.5)));
        // Rear: `Rotate_Y(spin) * Translate_Z(h)`, so the lift follows the spun axis and there is no steer.
        let spin = quat_about_axis([0.0, 1.0, 0.0], 0.5);
        assert_eq!(c[1].translation, quat_rotate(&spin, [0.0, 0.0, 0.1]));
        assert_eq!(c[1].rotation, spin);
        assert!((c[1].translation[0] - 0.1 * 0.5f32.sin()).abs() < 1e-6, "{:?}", c[1].translation);
    }

    #[test]
    fn a_left_tire_needs_its_right_partner_only_where_the_engine_pairs_them() {
        assert_eq!(pair(true, Some(1), None), (None, None));
        assert_eq!(pair(false, Some(1), None), (Some(1), None));
        assert_eq!(pair(true, Some(1), Some(2)), (Some(1), Some(2)));
        assert!(!is_paired(Kind::Truck, Role::Front));
        assert!(is_paired(Kind::TankTruck, Role::Rear));
    }
}
