//! Lane 130, real data (skips when no Zero Hour install is found): the launch bones are resolved over every
//! `W3DModelDraw` module of the object (`Drawable::getProjectileLaunchOffset`, Drawable.cpp:662-671) from the state
//! model posed by the state's first animation frame (`ModelConditionInfo::validateCachedBones`, W3DModelDraw.cpp:590-713),
//! with the `AttachToBoneInAnotherModule` offset on the turret pivots (`CACHE_ATTACH_BONE`, :3363-3388).
//!
//! The GLA Technical is the case the port missed: its first draw module is the truck body (`W3DTruckDraw`, no weapon
//! bones) and its turret module (`ModuleTag_02`, a `W3DModelDraw`) carries the crate-upgrade `Muzzle02`/`Muzzle03` launch
//! bones, attached to the truck's `Dum_Turret` bone. The America Strategy Center keeps its turret in `ModuleTag_06`, the
//! sixth draw module.

use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::condition::bit_for_name;
use zr_sim::template::TemplateSource;
use zr_sim::weapon::{LaunchBones, LaunchSlot};

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

fn launch_bones(name: &str) -> Option<LaunchBones> {
    let vfs = vfs()?;
    let data = GameData::load(&vfs);
    data.store.find_template(name).map(|t| t.combat.launch_bones.clone())
}

fn bit(name: &str) -> u128 {
    1u128 << bit_for_name(name).expect("a model condition bit")
}

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3 && (a[2] - b[2]).abs() < 1e-3
}

fn positions(slot: LaunchSlot<'_>) -> Vec<[f32; 3]> {
    slot.barrels.iter().map(|b| b.pos).collect()
}

/// The GLA Technical's crate upgrades put the weapon in the turret module: `WEAPONSET_CRATEUPGRADE_ONE` uses
/// `WeaponLaunchBone = PRIMARY Muzzle02`, TWO uses `Muzzle03`, in states whose `Model` is `UITech_SKN` and whose
/// `TurretArtAngle = 180`. The barrel numbers are the skeleton posed by the state's first animation
/// (`UITech_SKL.UITech_STA` frame 0) with the 180-degree art pre-rotation; the bind pose would give Muzzle02 at
/// (-18.699972, -0.012912935, 12.120312) instead. The turret pivot is the frame-0 `Dum-TurManMVR` (the skeleton's origin,
/// i.e. (0,0,0)) plus the module's `AttachToBoneInAnotherModule` `Dum_Turret` offset from the chassis model (the three
/// chassis have different bodies). Fails without the multi-module walk (the first module, the truck, has no launch bones),
/// without the frame-0 pose, and without the attach bone.
#[test]
fn the_technicals_crate_upgrade_bones_come_from_its_turret_module() {
    for (name, attach) in [
        ("GLAVehicleTechnicalChassisOne", [-8.84082, -0.016450405, 2.9383779]),
        ("GLAVehicleTechnicalChassisTwo", [-6.501298, -0.019304842, 7.114524]),
        ("GLAVehicleTechnicalChassisThree", [-7.327842, -0.016450262, 3.535436]),
        ("Chem_GLAVehicleTechnicalChassisOne", [-8.84082, -0.016450405, 2.9383779]),
        ("Slth_GLAVehicleTechnicalChassisTwo", [-6.501298, -0.019304842, 7.114524]),
    ] {
        let Some(lb) = launch_bones(name) else { return };
        assert!(lb.select_slot(0, 0).barrels.is_empty(), "{name}: the default state has no launch bone");
        let one = lb.select_slot(0, bit("WEAPONSET_CRATEUPGRADE_ONE"));
        assert_eq!(one.barrels.len(), 1, "{name}: ONE has one barrel");
        assert!(close(one.barrels[0].pos, [10.34784, -0.006655191, 8.366588]), "{name}: ONE is Muzzle02 at frame 0 with the 180-degree art: {:?}", positions(one));
        let two = lb.select_slot(0, bit("WEAPONSET_CRATEUPGRADE_TWO"));
        assert!(close(two.barrels[0].pos, [10.440946, -0.006655164, 8.366588]), "{name}: TWO is Muzzle03: {:?}", positions(two));
        let turret = two.turret.expect("the turret module's turret");
        assert_eq!(turret.slots, 0b111, "{name}: the AIUpdate controls all three slots");
        assert!(close(turret.yaw_pivot, attach), "{name}: yaw pivot is the chassis's Dum_Turret attach bone: {:?}", turret.yaw_pivot);
        assert!(close(turret.pitch_pivot.unwrap(), attach), "{name}: no TurretPitch bone, so the pitch pivot is the attach point too");
    }
}

/// The America Strategy Center's only launch bone is in its sixth draw module (`ModuleTag_06`, model `ABStrategy_A8`);
/// modules 0..4 are the building and its construction parts. That model has no animation, so its bones are the bind pose.
/// Fails without the multi-module walk (the first module is `ABStrategy`, no weapon bones).
#[test]
fn the_strategy_center_reaches_its_sixth_draw_module() {
    for name in ["AmericaStrategyCenter", "AirF_AmericaStrategyCenter", "Lazr_AmericaStrategyCenter", "SupW_AmericaStrategyCenter"] {
        let Some(lb) = launch_bones(name) else { return };
        let def = lb.select_slot(0, 0);
        assert_eq!(def.barrels.len(), 1, "{name}: one turret muzzle");
        assert!(close(def.barrels[0].pos, [47.43672, -22.29571, 12.225883]), "{name}: ABStrategy_A8 Muzzle01: {:?}", positions(def));
        let turret = def.turret.expect("the strategy center turret");
        assert_eq!(turret.slots, 0b001, "{name}: the TurretAI controls the primary slot");
        assert!(close(turret.yaw_pivot, [20.448662, -22.454588, 3.9774756]), "{name}: Turret01's rest position: {:?}", turret.yaw_pivot);
    }
}

/// `validateCachedBones` poses the state's model with its first animation at frame 0 before caching the public bones
/// (`robj->Set_Animation(animToUse, whichFrame, MANUAL)`, W3DModelDraw.cpp:660-668), so a model whose bind pose differs
/// from its idle first frame gets different launch/turret bones. The Ranger's `Muzzle01` sits at
/// (0.9272638, -12.854595, 10.9587345) in the bind pose (`AIRngr_SKN`) and at (-5.980346, 0.11496925, 12.025444) in
/// `AIRngr_STA` frame 0, the value the engine uses. Reverting the pose to the bind pose fails this.
#[test]
fn the_pristine_bones_are_the_first_animation_frame() {
    let Some(lb) = launch_bones("AmericaInfantryRanger") else { return };
    // The Ranger's rifle is the SECONDARY weapon (`WeaponLaunchBone = SECONDARY Muzzle`).
    let secondary = lb.select_slot(1, 0);
    assert_eq!(secondary.barrels.len(), 1, "the rifle bone");
    assert!(close(secondary.barrels[0].pos, [-5.980346, 0.11496925, 12.025444]), "the frame-0 pose, not the bind pose: {:?}", positions(secondary));
    assert!(lb.select_slot(0, 0).barrels.is_empty(), "the primary slot has no launch bone");
}

/// Every shipped template that has a launch bone resolves it, including the 24 objects whose bones live only in a later
/// draw module (the 8 Technical variants, the 4 Strategy Center variants and the cinematic Technicals around them).
/// Prints the survey numbers; fails without the multi-module walk (those objects had no default bones at all).
#[test]
fn every_shipped_launch_bone_object_resolves() {
    let Some(vfs) = vfs() else { return };
    let data = GameData::load(&vfs);
    let ini = data.ini_db().expect("the parsed INI database");
    let mut with_bones = 0usize;
    let mut later_module = Vec::new();
    for t in data.templates().things() {
        let Some(o) = data.store.find_template(&t.name) else { continue };
        let lb = &o.combat.launch_bones;
        let any = lb.states.iter().any(|s| s.slots.iter().any(|b| !b.is_empty()))
            || lb.extra_modules.iter().any(|m| m.states.iter().any(|s| s.slots.iter().any(|b| !b.is_empty())));
        if !any {
            continue;
        }
        with_bones += 1;
        // Which `Draw` index holds the first state with a launch bone?
        let mut first_launch = None;
        for (i, states) in zr_w3d::draw_states_by_module(&ini, &t.name) {
            if states.states.iter().any(|s| s.weapon_launch_bone.iter().any(Option::is_some)) {
                first_launch = Some(i);
                break;
            }
        }
        if first_launch.is_some_and(|i| i != 0) {
            later_module.push(t.name.clone());
        }
    }
    println!("templates with launch bones: {with_bones}; first launch module is not Draw[0]: {} ({later_module:?})", later_module.len());
    assert!(with_bones >= 180, "the shipped launch-bone objects ({with_bones})");
    for name in [
        "GLAVehicleTechnicalChassisOne",
        "GLAVehicleTechnicalChassisTwo",
        "GLAVehicleTechnicalChassisThree",
        "AmericaStrategyCenter",
        "AirF_AmericaStrategyCenter",
        "Lazr_AmericaStrategyCenter",
        "SupW_AmericaStrategyCenter",
    ] {
        assert!(later_module.iter().any(|n| n == name), "{name} must reach a later draw module");
    }
    assert!(later_module.len() >= 12, "the Technical and Strategy Center variants: {} found", later_module.len());
}

/// The reference map's environment matches the engine default the sim uses (`GameData.ini` is `TimeOfDay = AFTERNOON`,
/// `Weather = NORMAL`; `GlobalData.cpp:725-730`), so the map hook the port does not call yet cannot change AN3: the
/// `Lighting` chunk stores 2 (afternoon) and `WorldInfo` `weather` 0 (normal). Fails if the map data or the default
/// differs.
#[test]
fn the_an3_map_is_afternoon_and_normal() {
    let Some(vfs) = vfs() else { return };
    let path = zr_game::game::map_path("Alpine Assault");
    let raw = vfs.read(&path).expect("the Alpine Assault map");
    let bytes = zr_assets::compression::decompress_if_needed(&raw).expect("decompress");
    let map = zr_map::MapFile::parse(&bytes).expect("parse the map");
    let lighting = map.lighting.as_ref().expect("the map's Lighting chunk");
    assert_eq!(lighting.time_of_day, 2, "TIME_OF_DAY_AFTERNOON");
    let weather = map.world_info.as_ref().and_then(|d| d.get("weather")).cloned();
    assert_eq!(weather, Some(zr_map::DictValue::Int(0)), "WEATHER_NORMAL");
}
