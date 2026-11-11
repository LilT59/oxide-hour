//! The control bar's power meter, `W3DPowerDraw` (`Core/GameEngineDevice/Source/W3DDevice/GameClient/
//! GUI/GUICallbacks/W3DControlBar.cpp:111`), drawn over the `.wnd` `ControlBar.wnd:PowerWindow` rect.
//!
//! Retail repeats a point image from the window's left edge up to the production level, on a log
//! scale (`logN(production, PowerBarBase) * size.x / PowerBarIntervals`). The points are green,
//! yellow when consumption is within `PowerBarYellowRange` below production, or red when consumption
//! exceeds production. A slider marks the consumption level on the same scale. With no production
//! and no consumption nothing is drawn, so the window's frame shows through. The last point is
//! clipped at the window's right edge (`reg.hi.x`).
//!
//! `GlobalData` keeps its ctor defaults here (`PowerBarBase` 7, `PowerBarIntervals` 3,
//! `PowerBarYellowRange` 5; `GlobalData.cpp:935-937`), since the retail INI does not override them.

use bevy::prelude::*;

use crate::sim_view::SimRes;

const POWER_BAR_BASE: f32 = 7.0;
const POWER_BAR_INTERVALS: f32 = 3.0;
const POWER_BAR_YELLOW_RANGE: i32 = 5;

/// Point images `[green, yellow, red]` (`centerBarGreen` / `centerBarYellow` / `centerBarRed`), the
/// retail `PowerPoint*` mapped images.
const POINT_NAMES: [&str; 3] = ["PowerPointG", "PowerPointY", "PowerPointR"];
const SLIDER_NAME: &str = "PowerBarSlider";

/// A mapped image as a node image: its texture, sub-rect and `getImageWidth` / `getImageHeight`.
#[derive(Clone)]
pub(crate) struct Art {
    pub image: Handle<Image>,
    pub rect: Option<Rect>,
    pub w: i32,
    pub h: i32,
}

/// Geometry and art of one power meter (`W3DPowerDraw` reads `winGetSize` and the mapped images).
#[derive(Component)]
pub(crate) struct PowerBar {
    size: (i32, i32),
    points: Vec<Art>,
    slider: Art,
}

/// The `i`-th repeating point tile under a [`PowerBar`].
#[derive(Component)]
pub(crate) struct PowerTile(usize);

/// The consumption slider under a [`PowerBar`].
#[derive(Component)]
pub(crate) struct PowerNeedle;

/// What `W3DPowerDraw` draws for one production / consumption pair, in window-local pixels.
#[derive(Debug, PartialEq)]
pub(crate) struct PowerPlan {
    /// Index into the point images: 0 green, 1 yellow, 2 red.
    pub color: usize,
    /// Point tiles `(x, w)`, in order. The last one may be clipped to the window.
    pub tiles: Vec<(i32, i32)>,
    /// The slider `(x, w)`, or `None` when nothing is drawn.
    pub needle: Option<(i32, i32)>,
}

/// `logN(value, PowerBarBase)`, `W3DControlBar.cpp:105`.
fn log_n(value: f32) -> f32 {
    value.log10() / POWER_BAR_BASE.log10()
}

/// `W3DPowerDraw`'s arithmetic for a window `size_x` pixels wide with `point_w`-pixel points and a
/// `slider_w`-pixel slider.
pub(crate) fn power_plan(production: i32, consumption: i32, size_x: i32, point_w: i32, slider_w: i32) -> PowerPlan {
    let per_interval = size_x as f32 / POWER_BAR_INTERVALS;
    let color = if consumption > production - POWER_BAR_YELLOW_RANGE && consumption <= production {
        1
    } else if consumption > production {
        2
    } else {
        0
    };
    // `logN(0)` is -inf, which the C++ `Int` assignment turns into INT_MIN: no points for zero
    // production. Zero consumption likewise gives no needle range.
    let mut range = if production > 0 { (log_n(production as f32) * per_interval) as i32 } else { 0 };
    if range >= size_x {
        range = size_x;
    }
    let mut tiles = Vec::new();
    let mut center = range;
    if center > 0 && point_w > 0 {
        // Whole repeats, then one more point clipped at the window's right edge.
        let pieces = center / point_w;
        for i in 0..pieces {
            tiles.push((i * point_w, point_w));
        }
        let start = pieces * point_w;
        center = size_x - start;
        if center > 0 {
            tiles.push((start, center.min(point_w)));
        }
    }
    // `logN(1)` is 0, so one unit of consumption is drawn at 1.5 (`W3DControlBar.cpp:250`).
    let needle_real = if consumption == 1 { 1.5 } else { consumption as f32 };
    let needle_range = if needle_real > 0.0 { (log_n(needle_real) * per_interval) as i32 } else { i32::MIN };
    if center <= 0 && needle_range <= 0 {
        return PowerPlan { color, tiles, needle: None };
    }
    let r = needle_range.max(0);
    let (start, end) = if r >= size_x { (size_x - slider_w, size_x) } else { (r - slider_w / 2, r + slider_w / 2) };
    // A slider at or left of the window's edge is pinned to it (`posXstart <= pos.x`).
    let (start, end) = if start <= 0 { (0, slider_w) } else { (start, end) };
    PowerPlan { color, tiles, needle: Some((start, end - start)) }
}

/// Builds the power meter under `parent` over `rect` (`x, y, w, h` in screen pixels, from the
/// `.wnd` / scheme). `load` decodes a mapped image; a missing image leaves no meter.
pub(crate) fn spawn(
    commands: &mut Commands,
    parent: Entity,
    rect: (f32, f32, f32, f32),
    z: i32,
    load: &mut dyn FnMut(&str) -> Option<Art>,
) {
    let mut points = Vec::with_capacity(POINT_NAMES.len());
    for name in POINT_NAMES {
        let Some(art) = load(name) else { return };
        points.push(art);
    }
    let Some(slider) = load(SLIDER_NAME) else { return };
    let (x, y, w, h) = rect;
    let size = (w.round() as i32, h.round() as i32);
    let point_w = points[0].w.max(1);
    let tile_count = (size.0 / point_w + 2).max(1) as usize;
    let root = commands
        .spawn((
            PowerBar { size, points, slider: slider.clone() },
            Node { position_type: PositionType::Absolute, left: Val::Px(x), top: Val::Px(y), width: Val::Px(w), height: Val::Px(h), ..default() },
            ZIndex(z),
            ChildOf(parent),
        ))
        .id();
    for i in 0..tile_count {
        commands.spawn((
            PowerTile(i),
            ImageNode::default(),
            Node { position_type: PositionType::Absolute, left: Val::Px((i as i32 * point_w) as f32), top: Val::Px(0.0), width: Val::Px(point_w as f32), height: Val::Px(h), ..default() },
            Visibility::Hidden,
            ChildOf(root),
        ));
    }
    commands.spawn((
        PowerNeedle,
        ImageNode::default(),
        Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Px(slider.w as f32), height: Val::Px(slider.h as f32), ..default() },
        Visibility::Hidden,
        ChildOf(root),
    ));
}

/// The child nodes of a power meter: its point tiles and its needle.
type PowerNodes<'w, 's> = Query<
    'w,
    's,
    (&'static mut Node, &'static mut Visibility, &'static mut ImageNode, Option<&'static PowerTile>, Option<&'static PowerNeedle>),
>;

/// Draws the local player's power meter (`W3DPowerDraw`, run each frame by the control bar).
pub(crate) fn update(sim: Option<NonSend<SimRes>>, bars: Query<(&PowerBar, &Children)>, mut nodes: PowerNodes<'_, '_>) {
    let Some(sim) = sim else { return };
    let (production, consumption) = sim.game.logic().energy_tally(sim.local);
    for (bar, children) in &bars {
        let point_w = bar.points[0].w.max(1);
        let plan = power_plan(production, consumption, bar.size.0, point_w, bar.slider.w);
        let point = &bar.points[plan.color];
        for child in children.iter() {
            let Ok((mut node, mut vis, mut img, tile, needle)) = nodes.get_mut(child) else { continue };
            if let Some(PowerTile(i)) = tile {
                match plan.tiles.get(*i) {
                    Some(&(x, w)) => {
                        set_if_ne(&mut node.left, Val::Px(x as f32));
                        set_if_ne(&mut node.width, Val::Px(w as f32));
                        set_if_ne(&mut node.height, Val::Px(bar.size.1 as f32));
                        set_if_ne(&mut img.image, point.image.clone());
                        set_if_ne(&mut img.rect, point.rect);
                        set_if_ne(&mut img.image_mode, NodeImageMode::Stretch);
                        set_if_ne(&mut *vis, Visibility::Inherited);
                    }
                    None => set_if_ne(&mut *vis, Visibility::Hidden),
                }
            } else if needle.is_some() {
                match plan.needle {
                    Some((x, w)) => {
                        let slider = &bar.slider;
                        set_if_ne(&mut node.left, Val::Px(x as f32));
                        set_if_ne(&mut node.width, Val::Px(w as f32));
                        set_if_ne(&mut node.top, Val::Px((bar.size.1 - slider.h) as f32));
                        set_if_ne(&mut node.height, Val::Px(slider.h as f32));
                        set_if_ne(&mut img.image, slider.image.clone());
                        set_if_ne(&mut img.rect, slider.rect);
                        set_if_ne(&mut img.image_mode, NodeImageMode::Stretch);
                        set_if_ne(&mut *vis, Visibility::Inherited);
                    }
                    None => set_if_ne(&mut *vis, Visibility::Hidden),
                }
            }
        }
    }
}

/// Writes `value` only when it differs, so unchanged components are not re-flagged as changed.
fn set_if_ne<T: PartialEq>(slot: &mut T, value: T) {
    if *slot != value {
        *slot = value;
    }
}

#[cfg(test)]
mod tests {
    use super::{power_plan, PowerPlan};

    // Retail point width (6 px) and slider width (11 px) on a window of 445 px.
    fn plan(production: i32, consumption: i32) -> PowerPlan {
        power_plan(production, consumption, 445, 6, 11)
    }

    /// No production and no consumption (the Alpine start frame): nothing is drawn.
    #[test]
    fn idle_bar_draws_nothing() {
        let p = plan(0, 0);
        assert!(p.tiles.is_empty());
        assert_eq!(p.needle, None);
    }

    /// Production with no draw: green points up to `logN(5) * size / 3`, clipped last point, slider
    /// pinned at the left edge.
    #[test]
    fn surplus_is_green_and_pins_the_needle_left() {
        let p = plan(5, 0);
        assert_eq!(p.color, 0);
        // range = trunc(log10(5)/log10(7) * 148.33) = 122 -> 20 whole points, then one clipped one.
        assert_eq!(p.tiles.len(), 21);
        assert_eq!(p.tiles[19], (114, 6));
        assert_eq!(p.tiles[20], (120, 6));
        assert_eq!(p.needle, Some((0, 11)));
    }

    /// Consumption within the yellow range below production is yellow; above it, red.
    #[test]
    fn consumption_colours_the_points() {
        assert_eq!(plan(10, 8).color, 1);
        assert_eq!(plan(10, 10).color, 1);
        assert_eq!(plan(3, 9).color, 2);
        assert_eq!(plan(10, 2).color, 0);
    }

    /// The needle sits at `logN(consumption)` on the same scale, centred on its tick.
    #[test]
    fn needle_follows_consumption_on_the_log_scale() {
        // consumption 1 is drawn at 1.5: trunc(log10(1.5)/log10(7) * 148.33) = 30, slider 25..35.
        assert_eq!(plan(0, 1).needle, Some((25, 10)));
        // consumption 7 is exactly one interval (148 px): slider 143..153.
        assert_eq!(plan(0, 7).needle, Some((143, 10)));
    }
}
