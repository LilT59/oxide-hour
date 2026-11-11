//! Screen-space overlays over the world: the construction percent of a structure being built and the
//! health bar of a selected object.
//!
//! `Drawable::drawConstructPercent` (`GameEngine/Source/GameClient/Drawable.cpp`): an object with
//! `OBJECT_STATUS_UNDER_CONSTRUCTION` shows `CONTROLBAR:UnderConstructionDesc` (the "Building: %d%%"
//! label) centred on the geometry's centre (`GeometryInfo::getCenterPosition`: the position, raised by half
//! the height except for spheres), white with a black drop shadow.
//!
//! `Drawable::drawHealthBar` / `computeHealthRegion` / `Object::getHealthBoxPosition` and
//! `getHealthBoxDimensions` (`Object.cpp:3370`): a bar 3 pixels high and `max(20, min(150, major +
//! minor)) * 2` wide, centred on the object's height plus 10, drawn for the selected objects. Its colour is
//! blue to cyan while under construction, and red to green otherwise, averaged with red when really
//! damaged or with green when undamaged. The bar is at the default zoom (1.0); retail also draws it under
//! the cursor (`TheInGameUI`'s moused-over drawable), which the viewer does not do yet.

use bevy::prelude::*;
use zr_sim::condition::{DAMAGED, REALLYDAMAGED};

use crate::cmd_avail;
use crate::convert;
use crate::sim_view::{SimActive, SimRes};

/// Font size of the construction label, in pixels.
const LABEL_FONT: f32 = 14.0;

/// Every overlay node; despawned and rebuilt each frame (a handful of objects at most).
#[derive(Component)]
struct Overlay;

pub fn add_systems(app: &mut App) {
    app.add_systems(Update, draw_overlays.after(crate::sim_view::sim_sync).run_if(resource_exists::<SimActive>));
}

/// `Drawable::drawHealthBar`'s colours for a health ratio: `(fill, outline)`.
fn health_colors(ratio: f32, under_construction: bool, really_damaged: bool, damaged: bool) -> (Color, Color) {
    if under_construction {
        // Blue to cyan (`GameMakeColor(0, ratio * 255, 255, 255)` and its outline at half green).
        return (Color::srgb_u8(0, (ratio * 255.0) as u8, 255), Color::srgb_u8(0, (ratio * 128.0) as u8, 128));
    }
    let (mut red, mut green) = if ratio >= 0.5 { (1.0 - ((ratio - 0.5) / 0.5), 1.0) } else { (1.0, 1.0 - ((0.5 - ratio) / 0.5)) };
    let (out_r, out_g) = (red * 0.5, green * 0.5);
    if really_damaged {
        // Averaged with red.
        red = (1.0 + red) * 0.5;
        green *= 0.5;
    } else if !damaged {
        // Averaged with green.
        green = (1.0 + green) * 0.5;
        red *= 0.5;
    }
    (Color::srgb(red, green, 0.0), Color::srgb(out_r, out_g, 0.0))
}

/// `TheTacticalView->worldToScreen` for a sim point, as a viewport pixel position.
fn to_screen(camera: &Camera, gt: &GlobalTransform, p: [f32; 3]) -> Option<Vec2> {
    camera.world_to_viewport(gt, convert::to_bevy(p)).ok()
}

fn draw_overlays(
    mut commands: Commands,
    sim: NonSend<SimRes>,
    cam: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    old: Query<Entity, With<Overlay>>,
) {
    for e in &old {
        commands.entity(e).despawn();
    }
    let Ok((camera, gt)) = cam.single() else { return };
    let label = sim.game.text().get("CONTROLBAR:UnderConstructionDesc").map(str::to_owned);
    let logic = sim.game.logic();
    for o in logic.objects() {
        if o.is_dying() || o.is_drawable_hidden() {
            continue;
        }
        let pos = [o.position.x, o.position.y, o.position.z];
        let geometry = &o.template.geometry;
        if o.under_construction {
            // drawConstructPercent: the label over the geometry centre.
            let delta = if matches!(geometry.shape, zr_sim::template::GeometryShape::Sphere) { 0.0 } else { geometry.height * 0.5 };
            if let (Some(fmt), Some(c)) = (label.as_deref(), to_screen(camera, gt, [pos[0], pos[1], pos[2] + delta])) {
                let text = cmd_avail::format_one(fmt, &format!("{}", o.construction_percent.round() as i32));
                spawn_label(&mut commands, c, &text);
            }
        }
        if !sim.selected.contains(&o.id) {
            continue;
        }
        // drawHealthBar (selected objects; `KINDOF_FORCEATTACKABLE` things and empty bodies draw nothing).
        let Some(body) = &o.body else { continue };
        let (health, max) = (body.health(), body.max_health());
        if max == 0.0 || health == 0.0 {
            continue;
        }
        let height = crate::construction_view::max_height_above_position(geometry);
        let Some(centre) = to_screen(camera, gt, [pos[0], pos[1], pos[2] + height + 10.0]) else { continue };
        let size = (geometry.major_radius + geometry.minor_radius).clamp(20.0, 150.0);
        let width = (size * 2.0).max(20.0);
        let left = centre.x - width * 0.45;
        let top = centre.y - 1.5;
        let ratio = health / max;
        let flags = o.model_conditions;
        let (fill, outline) = health_colors(ratio, o.under_construction, flags.test(REALLYDAMAGED), flags.test(DAMAGED));
        spawn_bar(&mut commands, left, top, width, ratio, fill, outline);
    }
}

fn spawn_label(commands: &mut Commands, at: Vec2, text: &str) {
    let width = text.chars().count() as f32 * LABEL_FONT * 0.55;
    let left = at.x - width * 0.5;
    // The drop shadow first (black, one pixel down and right), then the white text.
    commands.spawn((
        Overlay,
        Node { position_type: PositionType::Absolute, left: Val::Px(left + 1.0), top: Val::Px(at.y + 1.0), ..default() },
        Text::new(text),
        TextFont { font_size: FontSize::Px(LABEL_FONT), ..default() },
        TextColor(Color::BLACK),
    ));
    commands.spawn((
        Overlay,
        Node { position_type: PositionType::Absolute, left: Val::Px(left), top: Val::Px(at.y), ..default() },
        Text::new(text),
        TextFont { font_size: FontSize::Px(LABEL_FONT), ..default() },
        TextColor(Color::WHITE),
    ));
}

fn spawn_bar(commands: &mut Commands, left: f32, top: f32, width: f32, ratio: f32, fill: Color, outline: Color) {
    // `drawOpenRect` (a one pixel outline) around `drawFillRect` (the bar, inset by one pixel).
    commands
        .spawn((
            Overlay,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(left),
                top: Val::Px(top),
                width: Val::Px(width),
                height: Val::Px(3.0),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BorderColor::all(outline),
        ))
        .with_children(|bar| {
            bar.spawn((
                Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Px((width - 2.0) * ratio), height: Val::Px(1.0), ..default() },
                BackgroundColor(fill),
            ));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction_bars_are_blue_to_cyan() {
        let (fill, outline) = health_colors(1.0, true, false, false);
        assert_eq!(fill.to_srgba().to_u8_array()[..3], [0, 255, 255]);
        assert_eq!(outline.to_srgba().to_u8_array()[..3], [0, 128, 128]);
    }

    #[test]
    fn healthy_bars_are_green() {
        let (fill, _) = health_colors(1.0, false, false, false);
        let c = fill.to_srgba();
        assert!(c.green > 0.99 && c.red < 0.51, "{c:?}");
    }
}
