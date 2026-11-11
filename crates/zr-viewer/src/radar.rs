//! Minimap radar over the `.wnd` `LeftHUD` window.
//!
//! The terrain layer is built like `W3DRadar::buildTerrainTexture`
//! (`Core/GameEngineDevice/Source/W3DDevice/Common/System/W3DRadar.cpp`): sample the world at each
//! radar pixel, use the water colour underwater (the `WaterTransparency` radar colour) and a
//! height-shaded terrain colour otherwise, darkening cliffs. Object dots follow `Radar::addObject`
//! (structures bigger than units), tinted with the owner's player colour. A camera view trapezoid
//! (`W3DRadar` `m_viewBox`) is drawn from the four screen corners projected onto the ground.
//!
//! The map is drawn aspect-fitted into the `.wnd` window the way `W3DLeftHUDDraw` calls
//! `W3DRadar::draw`: the radar window inset by 1 px, the map `findDrawPositions`-letterboxed inside
//! it, black bars outside. `GameLogic::cell_shroud_status` overlays the shroud: clear is transparent,
//! fogged is black at alpha 127/255, shrouded is opaque black (`W3DRadar::setShroudLevel`).
//!
//! Left-click moves the RTS camera; right-click orders the selection to move there. A click on the
//! black letterbox has no world position (`Radar::localPixelToRadar`).

use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui::RelativeCursorPosition;
use bevy::window::PrimaryWindow;

use zr_sim::pathfind::CellType;
use zr_sim::{CellShroudStatus, Coord3D, ObjectShroudStatus};

use crate::camera::RtsCam;
use crate::convert;
use crate::hud::HudRoot;
use crate::sim_view::{ray_ground, SimRes};
use crate::MapRes;

/// Retail default when `WaterTransparency` has no `RadarWaterColor`.
const WATER_DEFAULT: [u8; 3] = [38, 90, 165];
const CLIFF_SHADE: f32 = 0.62;
/// `W3DRadar::setShroudLevel`: fogged is black at alpha 127/255 (≈ half).
const FOG_KEEP: f32 = 1.0 - 127.0 / 255.0;

#[derive(Resource)]
pub struct Radar {
    handle: Handle<Image>,
    base: Option<Vec<u8>>,
    work: Vec<u8>,
    w: usize,
    h: usize,
    world: (f32, f32),
    /// `Radar::findDrawPositions` result: the map texture's letterboxed rect inside the radar
    /// window `(ul_x, ul_y, lr_x, lr_y)`, in local radar pixels (`Radar.cpp:771`). The area outside
    /// is black (`W3DRadar::draw`'s fill rects).
    fit: (i32, i32, i32, i32),
    /// `WaterTransparency` `RadarWaterColor`.
    water: [u8; 3],
}

#[derive(Component)]
struct RadarNode;

/// `Radar::findDrawPositions` (`Core/GameEngine/Source/Common/System/Radar.cpp:771`): fit the map
/// extent into the radar region preserving its aspect ratio, returning the local
/// `(ul_x, ul_y, lr_x, lr_y)` of the map inside `[0, region_w] x [0, region_h]`. The leftover strip
/// is the black letterbox the engine fills in `W3DRadar::draw` (`W3DRadar.cpp:1458`).
fn find_draw_positions(region_w: i32, region_h: i32, map_w: f32, map_h: f32) -> (i32, i32, i32, i32) {
    if map_w <= 0.0 || map_h <= 0.0 || region_w <= 0 || region_h <= 0 {
        return (0, 0, region_w, region_h);
    }
    let ratio_width = map_w / region_w as f32;
    let ratio_height = map_h / region_h as f32;
    if ratio_width >= ratio_height {
        // The map is wider than the region: bars at top and bottom (aspect fit by width).
        let radar_x = map_w / ratio_width; // == region_w
        let radar_y = map_h / ratio_width;
        let ul_y = (region_h as f32 - radar_y) / 2.0;
        let ul_x = 0.0f32;
        // Match the original's float -> `ICoord2D` truncation.
        (ul_x as i32, ul_y as i32, radar_x as i32, region_h - ul_y as i32)
    } else {
        // Bars at left and right (aspect fit by height).
        let radar_x = map_w / ratio_height;
        let radar_y = map_h / ratio_height; // == region_h
        let ul_x = (region_w as f32 - radar_x) / 2.0;
        let ul_y = 0.0f32;
        (ul_x as i32, ul_y as i32, region_w - ul_x as i32, radar_y as i32)
    }
}

/// `RadarEventType` (`Core/GameEngine/Include/Common/Radar.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum RadarEventType {
    Construction,
    Upgrade,
    UnderAttack,
    Information,
    BeaconPulse,
    Infiltration,
    BattlePlan,
    StealthDiscovered,
    StealthNeutralized,
    Fake,
}

impl RadarEventType {
    /// `radarColorLookupTable` (`Radar.cpp:870`).
    fn colors(self) -> ([u8; 4], [u8; 4]) {
        match self {
            RadarEventType::Construction => ([128, 128, 255, 255], [128, 255, 255, 255]),
            RadarEventType::Upgrade => ([128, 0, 64, 255], [255, 185, 220, 255]),
            RadarEventType::UnderAttack => ([255, 0, 0, 255], [255, 128, 128, 255]),
            RadarEventType::Information | RadarEventType::BeaconPulse => ([255, 255, 0, 255], [255, 255, 128, 255]),
            RadarEventType::Infiltration => ([0, 255, 255, 255], [128, 255, 255, 255]),
            RadarEventType::BattlePlan => ([255, 255, 255, 255], [255, 255, 255, 255]),
            RadarEventType::StealthDiscovered | RadarEventType::StealthNeutralized => ([0, 255, 0, 255], [0, 128, 0, 255]),
            RadarEventType::Fake => ([0, 0, 0, 0], [0, 0, 0, 0]),
        }
    }
}

/// One live `Radar::m_event[]` entry.
#[derive(Clone, Copy)]
struct RadarEvent {
    event_type: RadarEventType,
    create_frame: u32,
    die_frame: u32,
    fade_frame: u32,
    world: Coord3D,
    color1: [u8; 4],
    color2: [u8; 4],
}

/// `Radar::m_event[ MAX_RADAR_EVENTS ]` plus the "last event" index.
#[derive(Resource, Default)]
pub struct RadarEvents {
    events: Vec<RadarEvent>,
    /// `Radar::m_lastRadarEvent` (world position of the newest non-beacon event).
    last: Option<Coord3D>,
}

impl RadarEvents {
    /// `Radar::createEvent`: push a timed event with the type's colours (4 s life, 0.5 s fade).
    pub fn create_event(&mut self, world: Coord3D, event_type: RadarEventType, frame: u32) {
        const SECONDS_TO_LIVE: f32 = 4.0;
        const SECONDS_BEFORE_FADE: f32 = 0.5;
        let (color1, color2) = event_type.colors();
        let die = frame + (zr_sim::LOGICFRAMES_PER_SECOND as f32 * SECONDS_TO_LIVE) as u32;
        let fade = die.saturating_sub((zr_sim::LOGICFRAMES_PER_SECOND as f32 * SECONDS_BEFORE_FADE) as u32);
        if self.events.len() >= MAX_RADAR_EVENTS {
            self.events.remove(0);
        }
        self.events.push(RadarEvent { event_type, create_frame: frame, die_frame: die, fade_frame: fade, world, color1, color2 });
        if event_type != RadarEventType::BeaconPulse {
            self.last = Some(world);
        }
    }

    /// `Radar::tryEvent`: suppress a same-type event within 250 units created in the last 10 s
    /// (`PRESERVE_RADAR_WARNING_SUPPRESSION` keeps map-wide suppression for under-attack).
    pub fn try_event(&mut self, event_type: RadarEventType, world: Coord3D, frame: u32) -> bool {
        const CLOSE_SQ: f32 = 250.0 * 250.0;
        const FRAMES_BETWEEN: u32 = zr_sim::LOGICFRAMES_PER_SECOND * 10;
        for e in &self.events {
            if e.event_type != event_type {
                continue;
            }
            let dx = e.world.x - world.x;
            let dy = e.world.y - world.y;
            let is_close = dx * dx + dy * dy <= CLOSE_SQ || event_type == RadarEventType::UnderAttack;
            if is_close && frame.wrapping_sub(e.create_frame) < FRAMES_BETWEEN {
                return false;
            }
        }
        self.create_event(world, event_type, frame);
        true
    }

    /// `Radar::getLastEventLoc` (`Radar.cpp:1019`): the newest non-beacon event's position. Like
    /// retail it is kept after the event expires, and there is no cycling through older events.
    /// `VIEW_LAST_RADAR_EVENT` (Space) centres the camera on it (`CommandXlat.cpp:3073`).
    pub fn last_pos(&self) -> Option<Coord3D> {
        self.last
    }

    fn prune(&mut self, frame: u32) {
        self.events.retain(|e| frame < e.die_frame);
    }
}

/// `Radar::MAX_RADAR_EVENTS`.
const MAX_RADAR_EVENTS: usize = 64;

pub fn add_systems(app: &mut App) {
    app.init_resource::<RadarEvents>();
    app.add_systems(OnEnter(crate::shell::ShellState::InGame), setup_radar.after(crate::hud::setup_hud));
    app.add_systems(Update, radar_update.after(crate::sim_view::sim_step).run_if(in_state(crate::shell::ShellState::InGame)));
}

fn setup_radar(
    mut commands: Commands,
    hud: Option<Res<crate::hud::HudData>>,
    mut assets: ResMut<Assets<Image>>,
    root: Query<Entity, With<HudRoot>>,
    mut events: ResMut<RadarEvents>,
) {
    // A fresh game starts with no radar events (`Radar::init` sets `m_lastRadarEvent = -1`,
    // `Radar.cpp:225`). Cleared before the HUD checks so a game without a radar starts clean too.
    events.events.clear();
    events.last = None;
    let Some(hud) = hud else { return };
    let Some(rect) = hud.left_hud_rect else { return };
    let Ok(root) = root.single() else { return };
    // `W3DLeftHUDDraw` draws the radar into `winGetScreenPosition() + (1,1)` with
    // `winGetSize() - (2,2)` (`W3DControlBar.cpp:76`); the 1 px border is the `.wnd` LeftHUD fill.
    let (nx, ny) = (rect.0 + 1.0, rect.1 + 1.0);
    let (w, h) = ((rect.2 - 2.0).round().max(32.0) as usize, (rect.3 - 2.0).round().max(32.0) as usize);
    // Keep the CPU copy (`MAIN_WORLD | RENDER_WORLD`) so `radar_update` can rewrite the texture each
    // frame; the family textures of `convert::texture_image` are `RENDER_WORLD`-only, which drops the
    // CPU copy after extraction and the radar would never update.
    let mut image = Image::new(
        Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        vec![0u8; w * h * 4],
        TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    );
    // `W3DRadar::init` (`W3DRadar.cpp:910-928`): every radar texture is `MIP_LEVELS_1`; the shroud sets
    // `FILTER_TYPE_DEFAULT` min/mag, so the engine's global mode applies.
    image.sampler = crate::convert::texture_sampler(convert::TextureUse::Radar);
    let handle = assets.add(image);
    commands.spawn((
        RadarNode,
        ImageNode { image: handle.clone(), image_mode: NodeImageMode::Stretch, ..default() },
        RelativeCursorPosition::default(),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(nx),
            top: Val::Px(ny),
            width: Val::Px(w as f32),
            height: Val::Px(h as f32),
            ..default()
        },
        ZIndex(30),
        ChildOf(root),
    ));
    let water = radar_water_color(&hud.ini);
    commands.insert_resource(Radar { handle, base: None, work: vec![0u8; w * h * 4], w, h, world: (0.0, 0.0), fit: (0, 0, w as i32, h as i32), water });
}

/// `WaterTransparency` `RadarWaterColor` (`Water.cpp` `parseWaterTransparencyDefinition`).
fn radar_water_color(ini: &zr_ini::IniDb) -> [u8; 3] {
    ini.get("WaterTransparency", "")
        .and_then(|b| b.value("RadarWaterColor"))
        .map(|v| {
            let n = zr_ui::value::ints(v);
            let g = |i: usize| n.get(i).copied().unwrap_or(WATER_DEFAULT[i] as i32).clamp(0, 255) as u8;
            [g(0), g(1), g(2)]
        })
        .unwrap_or(WATER_DEFAULT)
}

fn put(buf: &mut [u8], w: usize, h: usize, x: i32, y: i32, c: [u8; 4]) {
    if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
        return;
    }
    let i = (y as usize * w + x as usize) * 4;
    buf[i..i + 4].copy_from_slice(&c);
}

fn fill(buf: &mut [u8], w: usize, h: usize, cx: i32, cy: i32, r: i32, c: [u8; 4]) {
    for y in (cy - r)..=(cy + r) {
        for x in (cx - r)..=(cx + r) {
            if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r {
                put(buf, w, h, x, y, c);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn line(buf: &mut [u8], w: usize, h: usize, mut x0: i32, mut y0: i32, x1: i32, y1: i32, c: [u8; 4]) {
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let mut err = dx + dy;
    loop {
        put(buf, w, h, x0, y0, c);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn height_color(t: f32) -> [u8; 4] {
    // Low: green, mid: tan, high: grey/white.
    let (a, b) = if t < 0.5 { ([46.0, 92.0, 40.0], [128.0, 118.0, 72.0]) } else { ([128.0, 118.0, 72.0], [190.0, 190.0, 185.0]) };
    let f = if t < 0.5 { t / 0.5 } else { (t - 0.5) / 0.5 };
    let l = |i: usize| (a[i] + (b[i] - a[i]) * f) as u8;
    [l(0), l(1), l(2), 255]
}

fn build_base(radar: &mut Radar, sim: &SimRes, map: &MapRes) {
    let terrain = sim.game.logic().terrain();
    let pathfinder = sim.game.logic().pathfinder();
    let (world_w, world_h) = terrain.world_size();
    radar.world = (world_w, world_h);
    let (w, h) = (radar.w, radar.h);
    // `W3DRadar::draw` fits the terrain texture into the window with `findDrawPositions`; the
    // `m_terrainImage`/`m_overlayImage`/`m_shroudImage` are only drawn inside that rect, and the
    // rest is black letterbox (`W3DRadar.cpp:1458`).
    radar.fit = find_draw_positions(w as i32, h as i32, world_w, world_h);
    let (ul_x, ul_y, lr_x, lr_y) = radar.fit;
    let (fit_w, fit_h) = ((lr_x - ul_x).max(1), (lr_y - ul_y).max(1));
    let to_world = |px: i32, py: i32| {
        (
            (px - ul_x) as f32 / fit_w as f32 * world_w,
            (1.0 - (py - ul_y) as f32 / fit_h as f32) * world_h,
        )
    };
    let mut zmin = f32::MAX;
    let mut zmax = f32::MIN;
    for py in ul_y..lr_y {
        for px in ul_x..lr_x {
            let (wx, wy) = to_world(px, py);
            let z = terrain.ground_height(wx, wy);
            zmin = zmin.min(z);
            zmax = zmax.max(z);
        }
    }
    let span = (zmax - zmin).max(1.0);
    // The letterbox is opaque black (the engine's `GameMakeColor(0,0,0,255)` fill rects).
    let mut base = vec![0u8; w * h * 4];
    for p in base.chunks_exact_mut(4) {
        p[3] = 255;
    }
    for py in ul_y..lr_y {
        for px in ul_x..lr_x {
            let (wx, wy) = to_world(px, py);
            let c = if terrain.is_underwater(wx, wy) {
                [radar.water[0], radar.water[1], radar.water[2], 255]
            } else {
                let mut c = height_color((terrain.ground_height(wx, wy) - zmin) / span);
                if matches!(pathfinder.cell_type_at(&Coord3D::new(wx, wy, 0.0)), Some(CellType::Cliff)) {
                    for ch in c.iter_mut().take(3) {
                        *ch = (*ch as f32 * CLIFF_SHADE) as u8;
                    }
                }
                c
            };
            put(&mut base, w, h, px, py, c);
        }
    }
    let _ = map;
    radar.base = Some(base);
}

#[allow(clippy::too_many_arguments)]
fn radar_update(
    mut radar: Option<ResMut<Radar>>,
    sim: Option<NonSendMut<SimRes>>,
    hud: Option<Res<crate::hud::HudData>>,
    map: Option<Res<MapRes>>,
    mut assets: ResMut<Assets<Image>>,
    window: Query<&Window, With<PrimaryWindow>>,
    cam: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    rcp: Query<&RelativeCursorPosition, With<RadarNode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    modal: crate::ui_block::ModalOpen,
    mut rts: ResMut<RtsCam>,
    mut events: ResMut<RadarEvents>,
    settings: Option<Res<crate::options::GameSettings>>,
) {
    let (Some(radar), Some(mut sim_guard), Some(hud)) = (radar.as_mut(), sim, hud) else { return };
    let sim: &mut SimRes = &mut sim_guard;
    if radar.base.is_none() {
        if let Some(map) = map.as_deref() {
            build_base(radar, sim, map);
        }
        if radar.base.is_none() {
            return;
        }
    }
    {
        let Radar { base, work, .. } = &mut **radar;
        if let Some(base) = base.as_ref() {
            work.copy_from_slice(base);
        }
    }
    let (w, h) = (radar.w, radar.h);
    let (world_w, world_h) = radar.world;
    let (ul_x, ul_y, lr_x, lr_y) = radar.fit;
    let fit_w = (lr_x - ul_x).max(1) as f32;
    let fit_h = (lr_y - ul_y).max(1) as f32;
    let to_px = |wx: f32, wy: f32| {
        (
            ul_x + (wx / world_w * fit_w) as i32,
            ul_y + ((1.0 - wy / world_h) * fit_h) as i32,
        )
    };
    let to_world = |px: i32, py: i32| {
        ((px - ul_x) as f32 / fit_w * world_w, (1.0 - (py - ul_y) as f32 / fit_h) * world_h)
    };

    // Shroud overlay (`GameLogic::cell_shroud_status`): fogged dimmed, shrouded black. Only the
    // aspect-fitted map area has world coordinates; the letterbox stays black.
    let local = sim.local;
    for py in ul_y..lr_y {
        for px in ul_x..lr_x {
            let (wx, wy) = to_world(px, py);
            match sim.game.logic().cell_shroud_status(local, &Coord3D::new(wx, wy, 0.0)) {
                CellShroudStatus::Clear => {}
                CellShroudStatus::Fogged => {
                    // `W3DRadar::setShroudLevel`: black at alpha 127/255 over the terrain.
                    let i = (py as usize * w + px as usize) * 4;
                    for k in 0..3 {
                        radar.work[i + k] = (radar.work[i + k] as f32 * FOG_KEEP) as u8;
                    }
                }
                CellShroudStatus::Shrouded => put(&mut radar.work, w, h, px, py, [0, 0, 0, 255]),
            }
        }
    }

    // Object dots (own objects always; enemy objects only when visible).
    for o in sim.game.objects() {
        if sim.possessed() == Some(o.id) || o.is_drawable_hidden() {
            continue;
        }
        let status = sim.game.logic().object_shroud_status(local, o.id);
        if o.owner != local && status >= ObjectShroudStatus::Fogged {
            continue;
        }
        let (px, py) = to_px(o.position.x, o.position.y);
        let color = hud.colors.get(&o.owner).copied().unwrap_or(crate::team_color::NEUTRAL);
        // `Radar::addObject`: a partially-clear enemy dot is drawn dimmed.
        let dim = if o.owner != local && status == ObjectShroudStatus::PartialClear { 0.5 } else { 1.0 };
        let c = [(color[0] as f32 * dim) as u8, (color[1] as f32 * dim) as u8, (color[2] as f32 * dim) as u8, 255];
        let structure = sim.game.logic().find_object(o.id).is_some_and(|obj| obj.is_kind_of(zr_sim::template::kind_of::STRUCTURE));
        fill(&mut radar.work, w, h, px, py, if structure { 3 } else { 1 }, c);
    }

    // Radar events (`Radar::m_event[]`): a pulsing marker that expands with age and fades in the
    // last 0.5 s (`internalCreateEvent` `fadeFrame`). The engine's W3DRadar draws a ring; a filled
    // disc is a faithful-enough read of the colour table here.
    let frame = sim.game.frame();
    events.prune(frame);
    for e in &events.events {
        let (px, py) = to_px(e.world.x, e.world.y);
        let age = frame.saturating_sub(e.create_frame) as f32;
        let life = (e.die_frame - e.create_frame).max(1) as f32;
        let t = (age / life).clamp(0.0, 1.0);
        let radius = 2.0 + t * 9.0;
        let mut c = e.color1;
        if frame >= e.fade_frame {
            let fade = (e.die_frame - frame) as f32 / (e.die_frame - e.fade_frame).max(1) as f32;
            c[3] = (c[3] as f32 * fade.clamp(0.0, 1.0)) as u8;
        }
        fill(&mut radar.work, w, h, px, py, radius as i32, c);
        // A brighter inner dot in `color2` makes the event readable on the terrain colours.
        let mut c2 = e.color2;
        c2[3] = c[3];
        fill(&mut radar.work, w, h, px, py, 1, c2);
    }
    // Camera view trapezoid (the four screen corners on the ground).
    if let (Ok((camera, gt)), Ok(window)) = (cam.single(), window.single()) {        let (vw, vh) = (window.width(), window.height());
        let ground = map.as_ref().map(|m| m.ground());
        let mut pts = Vec::new();
        for (sx, sy) in [(0.0, 0.0), (vw, 0.0), (vw, vh), (0.0, vh)] {
            if let Ok(ray) = camera.viewport_to_world(gt, Vec2::new(sx, sy)) {
                let hit = ground.as_ref().and_then(|g| ray_ground(ray.origin, *ray.direction, g));
                if let Some(p) = hit {
                    pts.push(to_px(p.x, -p.z));
                }
            }
        }
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            line(&mut radar.work, w, h, a.0, a.1, b.0, b.1, [255, 255, 255, 220]);
        }
    }

    // The radar image has one level (it is drawn 1:1), so only the base level needs updating. Write only when
    // it changed: `get_mut` marks the image changed and re-uploads it, which is wasted on a frame where the
    // radar did not move.
    let len = radar.work.len();
    if assets.get(&radar.handle).and_then(|i| i.data.as_deref()).is_some_and(|d| d.len() >= len && d[..len] != radar.work[..]) {
        if let Some(mut image) = assets.get_mut(&radar.handle) {
            if let Some(data) = image.data.as_mut() {
                data[..len].copy_from_slice(&radar.work);
            }
        }
    }

    // Clicks. A radar click is world input: an open pause menu / options / science panel owns the
    // mouse (the texture above still updates while the sim runs under a non-pausing overlay).
    if modal.get() {
        return;
    }
    let Ok(rcp) = rcp.single() else { return };
    let Some(n) = rcp.normalized else { return };
    if !rcp.cursor_over {
        return;
    }
    // `Radar::localPixelToRadar` (`Radar.cpp:586`): clicks outside the aspect-fitted map area (the
    // black letterbox) have no world position.
    let lx = (n.x.clamp(0.0, 1.0) * w as f32) as i32;
    let ly = (n.y.clamp(0.0, 1.0) * h as f32) as i32;
    if lx < ul_x || lx > lr_x || ly < ul_y || ly > lr_y {
        return;
    }
    let wx = (lx - ul_x) as f32 / fit_w * world_w;
    let wy = (1.0 - (ly - ul_y) as f32 / fit_h) * world_h;
    // `ControlBarCallback.cpp` (the radar's `GWM_*_DOWN`): the scheme's button orders the selection
    // to the point, the other one moves the view. Regular mouse: left orders, right looks.
    let alt = settings.as_ref().is_some_and(|s| s.alternate_mouse);
    let (left, right) = (buttons.just_pressed(MouseButton::Left), buttons.just_pressed(MouseButton::Right));
    if left || right {
        let has_selection = !sim.selected.is_empty();
        let moves = crate::rts_input::radar_click_moves_units(alt, left, has_selection);
        if moves {
            let z = sim.game.logic().terrain().ground_height(wx, wy);
            let ids = sim.selected.clone();
            sim.game.submit(sim.local, zr_sim::Command::MoveTo { ids, pos: Coord3D::new(wx, wy, z) });
        } else {
            let z = sim.game.logic().terrain().ground_height(wx, wy);
            rts.focus = convert::to_bevy([wx, wy, z]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{find_draw_positions, RadarEventType, RadarEvents};
    use zr_sim::Coord3D;

    /// `Radar::getLastEventLoc`: the newest non-beacon event wins, beacon pulses never become the
    /// "last" event, and the position survives the event's expiry (`internalCreateEvent`).
    #[test]
    fn last_event_is_newest_non_beacon_and_survives_expiry() {
        let mut ev = RadarEvents::default();
        assert_eq!(ev.last_pos(), None);
        ev.create_event(Coord3D::new(10.0, 20.0, 0.0), RadarEventType::Construction, 0);
        ev.create_event(Coord3D::new(30.0, 40.0, 0.0), RadarEventType::BeaconPulse, 5);
        assert_eq!(ev.last_pos(), Some(Coord3D::new(10.0, 20.0, 0.0)));
        ev.create_event(Coord3D::new(50.0, 60.0, 0.0), RadarEventType::UnderAttack, 10);
        assert_eq!(ev.last_pos(), Some(Coord3D::new(50.0, 60.0, 0.0)));
        // Far past every event's die frame: the events are pruned but the last position stays.
        ev.prune(100_000);
        assert!(ev.events.is_empty());
        assert_eq!(ev.last_pos(), Some(Coord3D::new(50.0, 60.0, 0.0)));
    }

    /// A square map in a square region fills it (`findDrawPositions`).
    #[test]
    fn square_map_fills_the_region() {
        assert_eq!(find_draw_positions(226, 226, 2400.0, 2400.0), (0, 0, 226, 226));
    }

    /// A map wider than the region is inset at the top and bottom (`W3DRadar.cpp:1461`).
    #[test]
    fn wide_map_is_letterboxed_top_and_bottom() {
        // 2400x1300 into a square 226x226 region: fit by width, bars above and below.
        let (ul_x, ul_y, lr_x, lr_y) = find_draw_positions(226, 226, 2400.0, 1300.0);
        assert_eq!((ul_x, lr_x), (0, 226));
        assert!(ul_y > 0 && lr_y < 226);
        assert_eq!(lr_y, 226 - ul_y);
    }

    /// A map taller than the region is inset at the left and right.
    #[test]
    fn tall_map_is_letterboxed_left_and_right() {
        let (ul_x, ul_y, lr_x, lr_y) = find_draw_positions(226, 226, 1300.0, 2400.0);
        assert_eq!((ul_y, lr_y), (0, 226));
        assert!(ul_x > 0 && lr_x < 226);
        assert_eq!(lr_x, 226 - ul_x);
    }
}
