//! Lane 134, real data (skips when no Zero Hour install is found): the fire-FX / recoil / muzzle-flash bones of the
//! GLA Technical's crate upgrades and the Strategy Center's cannon come from later `W3DModelDraw` modules.
//!
//! `Drawable::handleWeaponFireFX` asks every draw module in `Draw` order (Drawable.cpp:4191-4214); a module whose
//! current state has no barrel for the slot skips (W3DModelDraw.cpp:3700-3709). The Technical's first module is the
//! truck body (`W3DTruckDraw`, no weapon bones) and its turret attach module (`ModuleTag_02`, `UITech_SKN`) carries
//! `WeaponFireFXBone` / `WeaponMuzzleFlash`; the Strategy Center's cannon is `ModuleTag_06` (`ABStrategy_A8`).

use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::condition::bit_for_name;
use zr_w3d::{ConditionFlags, ConditionState, Hierarchy};

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

fn bit(name: &str) -> ConditionFlags {
    assert!(bit_for_name(name).is_some(), "{name} is a model condition bit");
    ConditionFlags::from_names([name])
}

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3 && (a[2] - b[2]).abs() < 1e-3
}

/// The state's bone translation at the pristine pose `validateCachedBones` caches: the state's first animation at
/// frame 0 (W3DModelDraw.cpp:590-713), else the bind pose.
fn pristine_bone(vfs: &Vfs, h: &Hierarchy, state: &ConditionState, name: &str) -> Option<[f32; 3]> {
    let pose = match state.animations.first().and_then(|a| zr_w3d::load_animation(vfs, &a.name).ok()) {
        Some(anim) => h.pose_trs(&anim, 0.0),
        None => h.bind_pose_trs(),
    };
    h.bone_index(name).map(|i| pose[i].0)
}

/// `ModelConditionInfo::validateWeaponBarrelInfo`'s `%02d` scan for one slot against a model's skeleton: a barrel
/// exists when any of the four bones resolves, and the scan stops at the first index where none does (an empty list
/// retries the unadorned names). Returns the resolved bone indices `(fx, recoil, muzzle)`.
fn barrel_bones(h: &Hierarchy, state: &ConditionState, slot: usize) -> Vec<(Option<usize>, Option<usize>, Option<usize>)> {
    let at = |name: &Option<String>, suffix: Option<u32>| {
        let n = name.as_deref()?;
        match suffix {
            Some(i) => h.bone_index(&format!("{n}{i:02}")),
            None => h.bone_index(n),
        }
    };
    let mut out = Vec::new();
    for i in 1..=99u32 {
        let b = (at(&state.weapon_fire_fx_bone[slot], Some(i)), at(&state.weapon_recoil_bone[slot], Some(i)), at(&state.weapon_muzzle_flash[slot], Some(i)));
        if b.0.is_none() && b.1.is_none() && b.2.is_none() {
            break;
        }
        out.push(b);
    }
    if out.is_empty() {
        let b = (at(&state.weapon_fire_fx_bone[slot], None), at(&state.weapon_recoil_bone[slot], None), at(&state.weapon_muzzle_flash[slot], None));
        if b.0.is_some() || b.1.is_some() || b.2.is_some() {
            out.push(b);
        }
    }
    out
}

/// The turret module of the GLA Technical carries the crate upgrades' fire FX and muzzle flash:
/// `WEAPONSET_CRATEUPGRADE_ONE` -> `Muzzle02`/`MuzzleFX02`, TWO -> `Muzzle03`/`MuzzleFX01` (the INI keeps the
/// first flash mesh for the RPG). The fire bones exist in the turret skeleton `UITech_SKN` at its `UITech_STA`
/// frame-0 pose, and not in the body skeleton `UVTechTrck`; the module is attached at the chassis's `Dum_Turret`
/// bone. Fails without the multi-module walk (the body module has no weapon bones) and without the per-state walk.
#[test]
fn the_technicals_crate_upgrades_resolve_in_its_turret_module() {
    let Some(vfs) = vfs() else { return };
    let data = GameData::load(&vfs);
    let ini = data.ini_db().expect("ini");
    for (name, attach) in [
        ("GLAVehicleTechnicalChassisOne", [-8.84082, -0.016450405, 2.9383779]),
        ("GLAVehicleTechnicalChassisTwo", [-6.501298, -0.019304842, 7.114524]),
        ("GLAVehicleTechnicalChassisThree", [-7.327842, -0.016450262, 3.535436]),
    ] {
        let modules = zr_game::launch_bones::weapon_draw_modules(&ini, &vfs, name);
        assert_eq!(modules.len(), 2, "{name}: the truck body and the turret attach module");
        let body = &modules[0];
        assert_eq!(body.index, 0, "{name}: the body is Draw[0]");
        assert!(body.attach.is_none(), "{name}: the body is not attached");
        assert!(
            !body.states.states.iter().any(|s| s.weapon_fire_fx_bone.iter().any(Option::is_some)
                || s.weapon_recoil_bone.iter().any(Option::is_some)
                || s.weapon_muzzle_flash.iter().any(Option::is_some)),
            "{name}: the truck body has no weapon FX bones"
        );
        let turret = &modules[1];
        assert_eq!(turret.index, 1, "{name}: the turret module is Draw[1]");
        assert_eq!(turret.attach_bone.as_deref(), Some("Dum_Turret"));
        assert!(close(turret.attach.expect("the cached attach translation"), attach), "{name}: Dum_Turret offset {:?}", turret.attach);
        let body_state = body.states.find_best_state(&ConditionFlags::new()).expect("the body's default state");
        let body_model = body_state.model.clone().expect("the truck body model");
        let Some(body_h) = zr_w3d::model_hierarchy(&vfs, &body_model) else { return };
        let Some(turret_h) = zr_w3d::model_hierarchy(&vfs, "UITech_SKN") else { return };
        for (bone, flags) in [("Muzzle02", bit("WEAPONSET_CRATEUPGRADE_ONE")), ("Muzzle03", bit("WEAPONSET_CRATEUPGRADE_TWO"))] {
            let state = turret.states.find_best_state(&flags).expect("the crate-upgrade state");
            assert_eq!(state.weapon_fire_fx_bone[0].as_deref(), Some(bone), "{name}: the fire FX bone");
            assert!(turret_h.bone_index(bone).is_some(), "{name}: {bone} is in UITech_SKN");
            assert!(body_h.bone_index(bone).is_none(), "{name}: {bone} is not in the body skeleton");
            let pos = pristine_bone(&vfs, &turret_h, state, bone).expect("the frame-0 bone");
            assert!(!close(pos, [0.0; 3]), "{name}: {bone} is posed away from the origin");
        }
        let one = turret.states.find_best_state(&bit("WEAPONSET_CRATEUPGRADE_ONE")).expect("ONE");
        assert_eq!(one.weapon_muzzle_flash[0].as_deref(), Some("MuzzleFX02"), "{name}: ONE's flash sub-object");
        let two = turret.states.find_best_state(&bit("WEAPONSET_CRATEUPGRADE_TWO")).expect("TWO");
        assert_eq!(two.weapon_muzzle_flash[0].as_deref(), Some("MuzzleFX01"), "{name}: TWO reuses MuzzleFX01");
        assert!(one.weapon_recoil_bone[0].is_none() && two.weapon_recoil_bone[0].is_none(), "{name}: no recoil bone");
        // The flash names are sub-objects of the turret model (`findSingleSubObj`, W3DModelDraw.cpp:457-493).
        let model = zr_w3d::load_model(&vfs, "UITech_SKN").expect("the turret model");
        for flash in ["MuzzleFX01", "MuzzleFX02", "MuzzleFX03"] {
            assert!(model.meshes.iter().any(|m| m.name.eq_ignore_ascii_case(flash)), "{name}: {flash} sub-object");
        }
        // The fire-FX bones are frame-0 positions in the turret skeleton (lane 130's table).
        let one_pos = pristine_bone(&vfs, &turret_h, one, "Muzzle02").expect("Muzzle02");
        assert!(close(one_pos, [-10.34784, 0.006655, 8.366588]), "{name}: ONE Muzzle02 frame 0: {one_pos:?}");
        let two_pos = pristine_bone(&vfs, &turret_h, two, "Muzzle03").expect("Muzzle03");
        assert!(close(two_pos, [-10.440946, 0.006655, 8.366588]), "{name}: TWO Muzzle03 frame 0: {two_pos:?}");
    }
}

/// The Technical's turret art is modelled 180 degrees backward (`TurretArtAngle = 180`). The engine's client turns the
/// turret bone by the TurretAI's body-relative angle **plus** the art angle (`handleClientTurretPositioning`,
/// W3DModelDraw.cpp:2440-2451), so at relative 0 the 20Cal's rest muzzle (-X) points +X at the Crusader. The turret
/// ring sits at the chassis's `Dum_Turret` (chassis one: -8.84082 x), so the muzzle lands at body x -0.41 (the middle
/// of the bed) and the `WeaponFireFXBone` `Muzzle01` at +0.11: the flash is at the muzzle, not at the truck centre.
/// Fails if the art angle is dropped (the gun then points backwards) or the attach offset is wrong.
#[test]
fn the_technicals_turret_art_points_the_20cal_forward() {
    let Some(vfs) = vfs() else { return };
    let ini = zr_ini::load_zero_hour(&vfs).db;
    let modules = zr_game::launch_bones::weapon_draw_modules(&ini, &vfs, "GLAVehicleTechnicalChassisOne");
    let turret = &modules[1];
    let state = turret.states.find_best_state(&ConditionFlags::new()).expect("the default state");
    assert_eq!(state.turret.as_deref(), Some("Dum-TurManMVR"), "the turret bone");
    assert!((state.turret_art_angle - std::f32::consts::PI).abs() < 1e-4, "TurretArtAngle = 180 degrees");
    let h = zr_w3d::model_hierarchy(&vfs, "UITech_SKN").expect("the turret hierarchy");
    let clip = state.animations.first().and_then(|a| zr_w3d::load_animation(&vfs, &a.name).ok());
    let bind = h.bind_pose_trs();
    let skin = |bone: usize, v: [f32; 3], pose: &[([f32; 3], [f32; 4])]| -> [f32; 3] {
        let inv = [-bind[bone].1[0], -bind[bone].1[1], -bind[bone].1[2], bind[bone].1[3]];
        let local = zr_w3d::quat_rotate(&inv, [v[0] - bind[bone].0[0], v[1] - bind[bone].0[1], v[2] - bind[bone].0[2]]);
        let w = zr_w3d::quat_rotate(&pose[bone].1, local);
        [pose[bone].0[0] + w[0], pose[bone].0[1] + w[1], pose[bone].0[2] + w[2]]
    };
    let model = zr_w3d::load_model(&vfs, "UITech_SKN").expect("the turret model");
    let gun = model.meshes.iter().find(|m| m.name.eq_ignore_ascii_case("20CAL")).expect("the 20Cal mesh");
    let x_range = |pose: &[([f32; 3], [f32; 4])]| {
        gun.positions.iter().map(|p| skin(gun.bone.expect("rigid"), *p, pose)[0]).fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), x| (lo.min(x), hi.max(x)))
    };
    let attach = turret.attach.expect("the Dum_Turret offset")[0];
    // Relative turret angle 0 (the target is straight ahead) + the art angle: the viewer's `pose`.
    let posed = h.pose_trs_controlled(
        clip.as_ref(),
        0.0,
        &[zr_w3d::BoneControl { bone: 1, translation: [0.0; 3], rotation: zr_w3d::quat_about_axis([0.0, 0.0, 1.0], state.turret_art_angle) }],
    );
    let (lo, hi) = x_range(&posed);
    assert!((lo - -2.41).abs() < 1e-2 && (hi - 8.43).abs() < 1e-2, "the 20Cal under rel 0 + art 180: x [{lo}, {hi}]");
    assert!((hi + attach - -0.41).abs() < 1e-2, "the muzzle is at body x -0.41 (the bed centre), pointing +X: {}", hi + attach);
    let fx = posed[h.bone_index("Muzzle01").expect("Muzzle01")].0[0] + attach;
    assert!((fx - 0.1139).abs() < 1e-2, "the WeaponFireFXBone is at body x {fx}, just beyond the muzzle");
    assert!(fx > hi + attach, "the flash bone is at/just beyond the mesh tip ({fx} vs {})", hi + attach);
    // Without the art the muzzle is the -X end: the art angle is what turns the gun toward the target.
    let rest = h.pose_trs_controlled(clip.as_ref(), 0.0, &[]);
    let (rest_lo, rest_hi) = x_range(&rest);
    assert!((rest_lo - -8.43).abs() < 1e-2 && (rest_hi - 2.41).abs() < 1e-2, "the rest art points backward: x [{rest_lo}, {rest_hi}]");
    // The gunner turns with the same bone: `Dum-MainMan`'s forward is -X without the art and +X with it.
    let main = h.bone_index("DUM-MAINMAN").expect("the gunner root");
    assert!(zr_w3d::quat_rotate(&rest[main].1, [1.0, 0.0, 0.0])[0] < -0.99, "the gunner's rest art faces backward");
    assert!(zr_w3d::quat_rotate(&posed[main].1, [1.0, 0.0, 0.0])[0] > 0.99, "the art turns the gunner toward the target");
}

/// The duel used for `duel_follow_120.png`: at frame 120 the Technical's body and turret are both at world angle 0
/// (the Crusader is straight +X, 36 units away), so the viewer's turret control is `relative 0 + TurretArtAngle
/// 180` — the only rotation in the render. The turret then holds world +X while the truck turns away (f150:
/// body -2.189, turret 0, relative +2.189). Fails if the sim's turret stops tracking the target.
///
/// The turn after f120 is the **wreck's**: the Crusader's shells kill the Technical at f125, and the death OCL's
/// `ApplyRandomForce` sets a yaw/roll/pitch rate (`ApplyRandomForceNugget::create`, ObjectCreationList.cpp:622), so
/// from f125 on the body heading is the transform matrix's `Get_Z_Rotation()` = `atan2(Row[1][0], Row[0][0])`
/// (`matrix3d.cpp:285`, `Thing.cpp:263-270`) of a matrix post-multiplied by `Rotate_X(roll)`, `Rotate_Y(pitch)`,
/// `Rotate_Z(yaw)` every frame (`PhysicsUpdate.cpp:747-749`). Lane 175 measured the rates 0.038173258/-0.049876511/
/// 0.030494733 (scaled by `PitchRollYawFactor` 2) and replayed the three C++ rotations on the f124 identity matrix:
/// the heading at f150 is -2.189011574, the port's value to 2e-10. The old expectation 1.985 was the pre-wave-20
/// port integrating *only* the yaw rate into the matrix (26 x 0.076346516 = 1.985009432), which the C++ never does.
/// Dropping the `Rotate_X`/`Rotate_Y` calls makes this test report +1.985 again.
#[test]
fn the_duel_technical_turret_faces_the_crusader() {
    let Some(vfs) = vfs() else { return };
    let data = GameData::load(&vfs);
    let mut g = zr_game::Game::new_skirmish_with(
        std::sync::Arc::new(data),
        &vfs,
        "Alpine Assault",
        &[zr_game::PlayerSetup::ai("FactionGLA", 0), zr_game::PlayerSetup::ai("FactionAmerica", 1)],
        1,
    )
    .expect("the AN3 skirmish");
    let (w, h) = g.logic().terrain().world_size();
    let (cx, cy) = (w * 0.45, h * 0.33);
    let gla = g.skirmish_player(0).expect("the GLA slot").index;
    let usa = g.skirmish_player(1).expect("the America slot").index;
    let z = g.logic().terrain().ground_height(cx - 18.0, cy);
    let tech = g.spawn_object("GLAVehicleTechnicalChassisOne", zr_sim::Coord3D::new(cx - 18.0, cy, z), 0.0, gla).expect("the Technical");
    let crusader = g.spawn_object("AmericaTankCrusader", zr_sim::Coord3D::new(cx + 18.0, cy, z), std::f32::consts::PI, usa).expect("the Crusader");
    g.submit(gla, zr_sim::Command::Attack { ids: vec![tech], target: crusader });
    g.submit(usa, zr_sim::Command::Attack { ids: vec![crusader], target: tech });
    while g.frame() < 120 {
        g.step();
    }
    let o = g.logic().find_object(tech).expect("the Technical is alive");
    let rel = zr_sim::math::angle_diff(o.combat.turret_angle, o.angle);
    assert!(o.angle.abs() < 0.01, "the truck still faces +X at f120: body {}", o.angle);
    assert!(o.combat.turret_angle.abs() < 0.01, "the turret faces the Crusader at +X: {}", o.combat.turret_angle);
    assert!(rel.abs() < 0.01, "relative turret angle 0, so only TurretArtAngle 180 rotates the art: {rel}");
    while g.frame() < 150 {
        g.step();
    }
    let o = g.logic().find_object(tech).expect("the Technical is alive");
    let rel = zr_sim::math::angle_diff(o.combat.turret_angle, o.angle);
    assert!((o.angle + 2.189).abs() < 0.02, "the wreck's body has turned away: {}", o.angle);
    assert!(o.combat.turret_angle.abs() < 0.02, "the turret still holds +X: {}", o.combat.turret_angle);
    assert!((rel - 2.189).abs() < 0.03, "relative +2.189 rad while the body turns: {rel}");
}

/// The Strategy Center's cannon is `ModuleTag_06` (`Draw[5]`, model `ABStrategy_A8`): `WeaponRecoilBone = Barrel`,
/// `WeaponMuzzleFlash = MuzzleFX`, `WeaponLaunchBone = Muzzle`, and `Turret`/`TurretPitch` bones. The module is not
/// attached (it sits at the object's origin, the model carries its own placement). Fails without the multi-module
/// walk.
#[test]
fn the_strategy_centers_cannon_bones_are_in_its_sixth_module() {
    let Some(vfs) = vfs() else { return };
    let data = GameData::load(&vfs);
    let ini = data.ini_db().expect("ini");
    for name in ["AmericaStrategyCenter", "AirF_AmericaStrategyCenter", "Lazr_AmericaStrategyCenter", "SupW_AmericaStrategyCenter"] {
        let modules = zr_game::launch_bones::weapon_draw_modules(&ini, &vfs, name);
        let cannon = modules.iter().find(|m| m.index == 5).expect("the sixth draw module");
        assert!(cannon.attach.is_none(), "{name}: the cannon module is not attached");
        let state = cannon.states.find_best_state(&ConditionFlags::new()).expect("the default state");
        assert_eq!(state.weapon_recoil_bone[0].as_deref(), Some("Barrel"), "{name}: the recoil bone");
        assert_eq!(state.weapon_muzzle_flash[0].as_deref(), Some("MuzzleFX"), "{name}: the flash bone");
        assert_eq!(state.turret.as_deref(), Some("Turret01"), "{name}: the turret bone");
        assert_eq!(state.turret_pitch.as_deref(), Some("TurretEL"), "{name}: the pitch bone");
        let Some(h) = zr_w3d::model_hierarchy(&vfs, "ABStrategy_A8") else { return };
        for bone in ["Muzzle01", "MuzzleFX01", "Turret01", "TurretEL"] {
            assert!(h.bone_index(bone).is_some(), "{name}: {bone} is in ABStrategy_A8");
        }
        // `validateWeaponBarrelInfo`'s %02d scan resolves one barrel (Muzzle01/MuzzleFX01, and `Barrel01` if the
        // model has it; the recoil bone is optional in the scan).
        let barrels = barrel_bones(&h, state, 0);
        assert_eq!(barrels.len(), 1, "{name}: one cannon barrel: {barrels:?}");
        assert_eq!(barrels[0].2, h.bone_index("MuzzleFX01"), "{name}: the muzzle flash sub-object's bone is MuzzleFX01");
    }
}

/// Every template whose weapon bones are only in a later module (25: the Technical variants, the Strategy Center
/// variants, a cinematic car, the PT boat) and the CombatBike's rider module must be reachable by the client's walk.
/// Fails without the multi-module listing (those templates would resolve nothing).
#[test]
fn every_shipped_late_weapon_module_is_listed() {
    let Some(vfs) = vfs() else { return };
    let data = GameData::load(&vfs);
    let ini = data.ini_db().expect("ini");
    let mut later = Vec::new();
    let mut multi = Vec::new();
    for t in data.templates().things() {
        let modules = zr_game::launch_bones::weapon_draw_modules(&ini, &vfs, &t.name);
        let weapon: Vec<usize> = modules
            .iter()
            .filter(|m| {
                m.states.states.iter().any(|s| {
                    s.weapon_fire_fx_bone.iter().any(Option::is_some)
                        || s.weapon_recoil_bone.iter().any(Option::is_some)
                        || s.weapon_muzzle_flash.iter().any(Option::is_some)
                        || s.weapon_launch_bone.iter().any(Option::is_some)
                })
            })
            .map(|m| m.index)
            .collect();
        if weapon.len() > 1 {
            multi.push(t.name.clone());
        }
        if weapon.first().is_some_and(|i| *i != 0) {
            later.push(t.name.clone());
        }
    }
    for name in [
        "GLAVehicleTechnicalChassisOne",
        "GLAVehicleTechnicalChassisTwo",
        "GLAVehicleTechnicalChassisThree",
        "AmericaStrategyCenter",
        "AirF_AmericaStrategyCenter",
        "Lazr_AmericaStrategyCenter",
        "SupW_AmericaStrategyCenter",
        "CivilianVehiclePTBoat",
    ] {
        assert!(later.iter().any(|n| n == name), "{name} has its weapon bones in a later module");
    }
    assert!(later.len() >= 25, "the shipped late-module objects: {}", later.len());
    for name in ["GLAVehicleCombatBike", "Chem_GLAVehicleCombatBike", "GLAVehicleCombatBikeTerrorist"] {
        assert!(multi.iter().any(|n| n == name), "{name} has weapon bones in two modules (body + rider)");
    }
}
