//! Client draw-module data the viewer needs beyond the condition states `zr-w3d` already parses:
//! tread and wheel/tire auxiliary fields of `W3DTankDraw` / `W3DTruckDraw` / `W3DTankTruckDraw`
//! (`Core/GameEngineDevice/.../Module/W3D*Draw.h`, `.../Draw/W3D*Draw.cpp`).
//!
//! The `ConditionState` / `DefaultConditionState` / `TransitionState` blocks (handled by
//! [`zr_w3d`]) are not stored here; they are ignored so the two parsers do not conflict. That data
//! also means `W3DModelDraw` itself stays with `zr-w3d`.

use zr_ini::{Block, Item};

use crate::diag::Diag;
use crate::value::Args;

/// A typed `W3D*Draw` module's non-condition-state data.
#[derive(Debug, Clone, PartialEq)]
pub enum DrawModule {
    /// `W3DTankDraw` / `W3DOverlordTankDraw`.
    TankDraw {
        class_name: String,
        tread_debris_left: String,
        tread_debris_right: String,
        /// `TreadAnimationRate` (`parseVelocityReal`: per second -> per frame).
        tread_animation_rate: f32,
        tread_pivot_speed_fraction: f32,
        tread_drive_speed_fraction: f32,
    },
    /// `W3DTruckDraw` / `W3DOverlordTruckDraw`.
    TruckDraw {
        class_name: String,
        dust: String,
        dirt_spray: String,
        powerslide_spray: String,
        front_left_tire_bone: String,
        front_right_tire_bone: String,
        rear_left_tire_bone: String,
        rear_right_tire_bone: String,
        mid_front_left_tire_bone: String,
        mid_front_right_tire_bone: String,
        mid_rear_left_tire_bone: String,
        mid_rear_right_tire_bone: String,
        mid_mid_left_tire_bone: String,
        mid_mid_right_tire_bone: String,
        cab_bone: String,
        trailer_bone: String,
        cab_rotation_multiplier: f32,
        trailer_rotation_multiplier: f32,
        rotation_damping: f32,
        tire_rotation_multiplier: f32,
        powerslide_rotation_addition: f32,
    },
    /// `W3DTankTruckDraw` (both treads and tires).
    TankTruckDraw {
        class_name: String,
        dust: String,
        dirt_spray: String,
        powerslide_spray: String,
        front_left_tire_bone: String,
        front_right_tire_bone: String,
        rear_left_tire_bone: String,
        rear_right_tire_bone: String,
        mid_front_left_tire_bone: String,
        mid_front_right_tire_bone: String,
        mid_rear_left_tire_bone: String,
        mid_rear_right_tire_bone: String,
        tire_rotation_multiplier: f32,
        powerslide_rotation_addition: f32,
        tread_debris_left: String,
        tread_debris_right: String,
        tread_animation_rate: f32,
        tread_pivot_speed_fraction: f32,
        tread_drive_speed_fraction: f32,
    },
    /// A draw class with no own data (`W3DPoliceCarDraw`).
    Marker { class_name: String },
}

impl DrawModule {
    /// `ModuleFactory` / `W3DModuleFactory` for the classes this module types.
    pub(crate) fn parse(class: &str, block: &Block, d: &mut Diag) -> Option<DrawModule> {
        match class {
            "W3DTankDraw" | "W3DOverlordTankDraw" => {
                let mut p = TankDrawData::new(class);
                each(block, d, class, |k, a| {
                    match k {
                        "TreadDebrisLeft" => p.tread_debris_left = a.next()?.to_string(),
                        "TreadDebrisRight" => p.tread_debris_right = a.next()?.to_string(),
                        "TreadAnimationRate" => p.tread_animation_rate = a.velocity()?,
                        "TreadPivotSpeedFraction" => p.tread_pivot_speed_fraction = a.real()?,
                        "TreadDriveSpeedFraction" => p.tread_drive_speed_fraction = a.real()?,
                        _ => {}
                    }
                    Ok(())
                });
                Some(DrawModule::TankDraw {
                    class_name: p.class_name,
                    tread_debris_left: p.tread_debris_left,
                    tread_debris_right: p.tread_debris_right,
                    tread_animation_rate: p.tread_animation_rate,
                    tread_pivot_speed_fraction: p.tread_pivot_speed_fraction,
                    tread_drive_speed_fraction: p.tread_drive_speed_fraction,
                })
            }
            "W3DTruckDraw" | "W3DOverlordTruckDraw" => {
                let mut p = TruckDrawData::new(class);
                each(block, d, class, |k, a| {
                    p.field(k, a)?;
                    Ok(())
                });
                Some(p.into_truck())
            }
            "W3DTankTruckDraw" => {
                let mut p = TankTruckData::new(class);
                each(block, d, class, |k, a| {
                    p.field(k, a)?;
                    Ok(())
                });
                Some(p.into_tank_truck())
            }
            "W3DPoliceCarDraw" => Some(DrawModule::Marker { class_name: class.to_string() }),
            _ => None,
        }
    }
}

/// Iterate a draw module's fields; nested condition-state blocks are left to `zr-w3d`.
fn each(block: &Block, _d: &mut Diag, _scope: &str, mut f: impl FnMut(&str, &mut Args) -> Result<(), String>) {
    for item in &block.items {
        let Item::Field(field) = item else {
            continue;
        };
        let mut a = Args::new(&field.value);
        let _ = f(&field.key, &mut a);
    }
}

struct TankDrawData {
    class_name: String,
    tread_debris_left: String,
    tread_debris_right: String,
    tread_animation_rate: f32,
    tread_pivot_speed_fraction: f32,
    tread_drive_speed_fraction: f32,
}

impl TankDrawData {
    fn new(class: &str) -> Self {
        TankDrawData {
            class_name: class.to_string(),
            tread_debris_left: "TrackDebrisDirtLeft".to_string(),
            tread_debris_right: "TrackDebrisDirtRight".to_string(),
            tread_animation_rate: 0.0,
            tread_pivot_speed_fraction: 0.6,
            tread_drive_speed_fraction: 0.3,
        }
    }
}

#[derive(Default)]
struct TruckDrawData {
    class_name: String,
    dust: String,
    dirt_spray: String,
    powerslide_spray: String,
    front_left_tire_bone: String,
    front_right_tire_bone: String,
    rear_left_tire_bone: String,
    rear_right_tire_bone: String,
    mid_front_left_tire_bone: String,
    mid_front_right_tire_bone: String,
    mid_rear_left_tire_bone: String,
    mid_rear_right_tire_bone: String,
    mid_mid_left_tire_bone: String,
    mid_mid_right_tire_bone: String,
    cab_bone: String,
    trailer_bone: String,
    cab_rotation_multiplier: f32,
    trailer_rotation_multiplier: f32,
    rotation_damping: f32,
    tire_rotation_multiplier: f32,
    powerslide_rotation_addition: f32,
}

impl TruckDrawData {
    fn new(class: &str) -> Self {
        TruckDrawData { class_name: class.to_string(), ..Default::default() }
    }
    fn field(&mut self, k: &str, a: &mut Args) -> Result<(), String> {
        match k {
            "Dust" => self.dust = a.next()?.to_string(),
            "DirtSpray" => self.dirt_spray = a.next()?.to_string(),
            "PowerslideSpray" => self.powerslide_spray = a.next()?.to_string(),
            "LeftFrontTireBone" => self.front_left_tire_bone = a.next()?.to_string(),
            "RightFrontTireBone" => self.front_right_tire_bone = a.next()?.to_string(),
            "LeftRearTireBone" => self.rear_left_tire_bone = a.next()?.to_string(),
            "RightRearTireBone" => self.rear_right_tire_bone = a.next()?.to_string(),
            "MidLeftFrontTireBone" => self.mid_front_left_tire_bone = a.next()?.to_string(),
            "MidRightFrontTireBone" => self.mid_front_right_tire_bone = a.next()?.to_string(),
            "MidLeftRearTireBone" => self.mid_rear_left_tire_bone = a.next()?.to_string(),
            "MidRightRearTireBone" => self.mid_rear_right_tire_bone = a.next()?.to_string(),
            "MidLeftMidTireBone" => self.mid_mid_left_tire_bone = a.next()?.to_string(),
            "MidRightMidTireBone" => self.mid_mid_right_tire_bone = a.next()?.to_string(),
            "CabBone" => self.cab_bone = a.next()?.to_string(),
            "TrailerBone" => self.trailer_bone = a.next()?.to_string(),
            "CabRotationMultiplier" => self.cab_rotation_multiplier = a.real()?,
            "TrailerRotationMultiplier" => self.trailer_rotation_multiplier = a.real()?,
            "RotationDamping" => self.rotation_damping = a.real()?,
            "TireRotationMultiplier" => self.tire_rotation_multiplier = a.real()?,
            "PowerslideRotationAddition" => self.powerslide_rotation_addition = a.real()?,
            _ => {
                let _ = a.rest();
            }
        }
        Ok(())
    }
    fn into_truck(self) -> DrawModule {
        DrawModule::TruckDraw {
            class_name: self.class_name,
            dust: self.dust,
            dirt_spray: self.dirt_spray,
            powerslide_spray: self.powerslide_spray,
            front_left_tire_bone: self.front_left_tire_bone,
            front_right_tire_bone: self.front_right_tire_bone,
            rear_left_tire_bone: self.rear_left_tire_bone,
            rear_right_tire_bone: self.rear_right_tire_bone,
            mid_front_left_tire_bone: self.mid_front_left_tire_bone,
            mid_front_right_tire_bone: self.mid_front_right_tire_bone,
            mid_rear_left_tire_bone: self.mid_rear_left_tire_bone,
            mid_rear_right_tire_bone: self.mid_rear_right_tire_bone,
            mid_mid_left_tire_bone: self.mid_mid_left_tire_bone,
            mid_mid_right_tire_bone: self.mid_mid_right_tire_bone,
            cab_bone: self.cab_bone,
            trailer_bone: self.trailer_bone,
            cab_rotation_multiplier: self.cab_rotation_multiplier,
            trailer_rotation_multiplier: self.trailer_rotation_multiplier,
            rotation_damping: self.rotation_damping,
            tire_rotation_multiplier: self.tire_rotation_multiplier,
            powerslide_rotation_addition: self.powerslide_rotation_addition,
        }
    }
}

struct TankTruckData {
    truck: TruckDrawData,
    tread_debris_left: String,
    tread_debris_right: String,
    tread_animation_rate: f32,
    tread_pivot_speed_fraction: f32,
    tread_drive_speed_fraction: f32,
}

impl TankTruckData {
    fn new(class: &str) -> Self {
        TankTruckData {
            truck: TruckDrawData::new(class),
            tread_debris_left: "TrackDebrisDirtLeft".to_string(),
            tread_debris_right: "TrackDebrisDirtRight".to_string(),
            tread_animation_rate: 0.0,
            tread_pivot_speed_fraction: 0.6,
            tread_drive_speed_fraction: 0.3,
        }
    }
    fn field(&mut self, k: &str, a: &mut Args) -> Result<(), String> {
        match k {
            "TreadDebrisLeft" => self.tread_debris_left = a.next()?.to_string(),
            "TreadDebrisRight" => self.tread_debris_right = a.next()?.to_string(),
            "TreadAnimationRate" => self.tread_animation_rate = a.velocity()?,
            "TreadPivotSpeedFraction" => self.tread_pivot_speed_fraction = a.real()?,
            "TreadDriveSpeedFraction" => self.tread_drive_speed_fraction = a.real()?,
            _ => self.truck.field(k, a)?,
        }
        Ok(())
    }
    fn into_tank_truck(self) -> DrawModule {
        DrawModule::TankTruckDraw {
            class_name: self.truck.class_name,
            dust: self.truck.dust,
            dirt_spray: self.truck.dirt_spray,
            powerslide_spray: self.truck.powerslide_spray,
            front_left_tire_bone: self.truck.front_left_tire_bone,
            front_right_tire_bone: self.truck.front_right_tire_bone,
            rear_left_tire_bone: self.truck.rear_left_tire_bone,
            rear_right_tire_bone: self.truck.rear_right_tire_bone,
            mid_front_left_tire_bone: self.truck.mid_front_left_tire_bone,
            mid_front_right_tire_bone: self.truck.mid_front_right_tire_bone,
            mid_rear_left_tire_bone: self.truck.mid_rear_left_tire_bone,
            mid_rear_right_tire_bone: self.truck.mid_rear_right_tire_bone,
            tire_rotation_multiplier: self.truck.tire_rotation_multiplier,
            powerslide_rotation_addition: self.truck.powerslide_rotation_addition,
            tread_debris_left: self.tread_debris_left,
            tread_debris_right: self.tread_debris_right,
            tread_animation_rate: self.tread_animation_rate,
            tread_pivot_speed_fraction: self.tread_pivot_speed_fraction,
            tread_drive_speed_fraction: self.tread_drive_speed_fraction,
        }
    }
}
