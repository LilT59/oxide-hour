//! RTS-style orbit camera and a free-fly camera (toggle with F). Bevy space (Y up).

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::MapRes;

#[derive(Resource)]
pub struct RtsCam {
    /// Ground point the camera orbits (Bevy space; y follows terrain).
    pub focus: Vec3,
    pub yaw: f32,
    /// Elevation above the horizon, radians.
    pub pitch: f32,
    pub dist: f32,
    pub min_dist: f32,
    pub max_dist: f32,
}

#[derive(Resource, Default)]
pub struct FreeCam {
    pub active: bool,
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Resource)]
pub struct Interactive(pub bool);

/// While true the RTS camera does nothing (set by possession, which drives the camera itself).
#[derive(Resource, Default)]
pub struct CamLock(pub bool);

/// Right-drag scroll and middle-drag rotate state (`LookAtXlat`'s `m_anchor` /
/// `m_middleButtonDownTimeMsec`). Client-only.
#[derive(Resource, Default)]
pub struct CameraDrag {
    /// Right-button down anchor in window pixels (while scrolling).
    pub(crate) rmb_anchor: Option<Vec2>,
    /// Middle-button down position and real time, for the middle-click reset test.
    mid_start: Option<(Vec2, f64)>,
}

/// `InGameUI::update`'s keyboard camera rotate speed: `GlobalData::m_keyboardCameraRotateSpeed`
/// (0.1 rad) normalised to the 30 Hz logic rate.
const KEYBOARD_CAMERA_ROTATE_SPEED: f32 = 0.1 * 30.0;
/// Right-drag scroll: world units per pixel of drag per second. Tuned to the viewer's focus/zoom
/// scale (the engine's `SCROLL_AMT` is in its own world units).
const RMB_SCROLL_RATE: f32 = 1.5;
/// `LookAtXlat`'s middle-click reset thresholds (`CLICK_DURATION_MSEC` / `PIXEL_OFFSET`).
const MIDDLE_CLICK_SECS: f64 = 0.167;
const MIDDLE_CLICK_PIXELS: f32 = 5.0;

/// The screen-space direction the RTS camera last panned in (x right, y down), set while
/// edge-panning or right-drag scrolling. The cursor module uses it to pick the `Scroll` cursor
/// direction (`W3DMouse::setCursorDirection`, `W3DMouse.cpp:671`). Retail sets `Mouse::SCROLL`
/// for right-drag scrolling (`InGameUI::setScrolling`, `InGameUI.cpp:3138`) and key/edge scrolling.
#[derive(Resource, Default)]
pub struct ScrollDir(pub Option<Vec2>);

/// The RTS camera's optional resources, bundled to stay within Bevy's 16-parameter system limit.
#[derive(bevy::ecs::system::SystemParam)]
pub struct CamDeps<'w> {
    pub scroll_dir: ResMut<'w, ScrollDir>,
    pub settings: Option<Res<'w, crate::options::GameSettings>>,
    pub hud: Option<Res<'w, crate::hud::HudData>>,
    pub mode: Option<Res<'w, crate::placement::OrderMode>>,
}

/// Zero Hour default view: about 37.5 degrees from vertical.
pub const RTS_PITCH: f32 = (90.0f32 - 37.5) * std::f32::consts::PI / 180.0;

pub fn rts_transform(c: &RtsCam) -> Transform {
    let off = Vec3::new(c.yaw.sin() * c.pitch.cos(), c.pitch.sin(), c.yaw.cos() * c.pitch.cos()) * c.dist;
    Transform::from_translation(c.focus + off).looking_at(c.focus, Vec3::Y)
}

#[allow(clippy::too_many_arguments)]
pub fn rts_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    window: Query<&Window, With<PrimaryWindow>>,
    free: Res<FreeCam>,
    map: Res<MapRes>,
    interactive: Res<Interactive>,
    lock: Res<CamLock>,
    sim: Option<NonSend<crate::sim_view::SimRes>>,
    modes: Res<crate::meta_keys::MetaModes>,
    mut deps: CamDeps,
    mut cam: ResMut<RtsCam>,
    mut drag: ResMut<CameraDrag>,
    mut q: Query<&mut Transform, With<Camera3d>>,
) {
    if free.active || lock.0 {
        deps.scroll_dir.0 = None;
        drag.rmb_anchor = None;
        drag.mid_start = None;
        return;
    }
    deps.scroll_dir.0 = None;
    // Letters that are command-card hotkeys right now (S = stop, A = attack move, ...) do not also
    // pan / rotate; Q / E also rotate the placement ghost while a building is being placed.
    let mut claimed = match (&deps.hud, &sim) {
        (Some(h), Some(s)) => crate::hud::command_hotkeys(h, s),
        _ => Vec::new(),
    };
    if deps.mode.is_some_and(|m| matches!(m.0, Some(crate::placement::Order::Place { .. }))) {
        claimed.extend([KeyCode::KeyQ, KeyCode::KeyE]);
    }
    let scroll_speed = deps.settings.as_ref().map_or(1.0, |s| s.scroll_speed());
    let move_anchor = deps.settings.as_ref().is_none_or(|s| s.move_scroll_anchor);
    let dt = time.delta_secs();
    let win = window.single().ok();
    let dev = crate::meta_keys::dev_mode();
    if interactive.0 {
        let (s, c) = (cam.yaw.sin(), cam.yaw.cos());
        let fwd = Vec3::new(-s, 0.0, -c);
        let right = Vec3::new(c, 0.0, -s);
        let mut dir = Vec3::ZERO;
        let key = |k| keys.pressed(k) && !claimed.contains(&k);
        // Retail scrolls with the arrow keys (`LookAtXlat` KEY_UP/DOWN/LEFT/RIGHT, SCROLL_KEY).
        if key(KeyCode::ArrowUp) {
            dir += fwd;
        }
        if key(KeyCode::ArrowDown) {
            dir -= fwd;
        }
        if key(KeyCode::ArrowRight) {
            dir += right;
        }
        if key(KeyCode::ArrowLeft) {
            dir -= right;
        }
        // The non-retail WASD pan is dev-only (`--dev`): retail binds W/Q/E/S/A/D to metas.
        if dev {
            if key(KeyCode::KeyW) {
                dir += fwd;
            }
            if key(KeyCode::KeyS) {
                dir -= fwd;
            }
            if key(KeyCode::KeyD) {
                dir += right;
            }
            if key(KeyCode::KeyA) {
                dir -= right;
            }
        }
        // Edge pan.
        let mut edge = Vec3::ZERO;
        if let Some(w) = win {
            if let (true, Some(p)) = (w.focused, w.cursor_position()) {
                let m = 4.0;
                if p.x < m {
                    dir -= right;
                    edge -= right;
                }
                if p.x > w.width() - m {
                    dir += right;
                    edge += right;
                }
                if p.y < m {
                    dir += fwd;
                    edge += fwd;
                }
                if p.y > w.height() - m {
                    dir -= fwd;
                    edge -= fwd;
                }
            }
        }
        if edge != Vec3::ZERO {
            // Screen space: +x right, +y down. A move into the screen (camera `fwd`) reads as up.
            let screen = Vec2::new(edge.dot(right), -edge.dot(fwd));
            deps.scroll_dir.0 = Some(screen.normalize_or_zero());
        }
        let boost = if key(KeyCode::ShiftLeft) { 2.5 } else { 1.0 };
        let step = dir.normalize_or_zero() * cam.dist * 1.2 * boost * scroll_speed * dt;
        cam.focus += step;

        // Keypad camera rotate/zoom (`MSG_META_BEGIN_CAMERA_ROTATE_*` / `_ZOOM_*`,
        // `CommandXlat.cpp:3463`, applied in `InGameUI::update`, `InGameUI.cpp:2120`).
        if modes.rotate_left && !modes.rotate_right {
            cam.yaw -= KEYBOARD_CAMERA_ROTATE_SPEED * dt;
        }
        if modes.rotate_right && !modes.rotate_left {
            cam.yaw += KEYBOARD_CAMERA_ROTATE_SPEED * dt;
        }
        let zoom = 2.0f32.powf(dt);
        if modes.zoom_in && !modes.zoom_out {
            cam.dist = (cam.dist / zoom).clamp(cam.min_dist, cam.max_dist);
        }
        if modes.zoom_out && !modes.zoom_in {
            cam.dist = (cam.dist * zoom).clamp(cam.min_dist, cam.max_dist);
        }
        // Dev-only Q/E rotate.
        if dev {
            if key(KeyCode::KeyQ) {
                cam.yaw -= 1.5 * dt;
            }
            if key(KeyCode::KeyE) {
                cam.yaw += 1.5 * dt;
            }
        }

        // Middle button: drag rotates the view (`LookAtXlat` `userSetAngle`); a click resets the
        // camera (`userResetPivotToGround` / angle / pitch / zoom to default, `CommandXlat.cpp:318`).
        if buttons.just_pressed(MouseButton::Middle) {
            if let Some(w) = win {
                drag.mid_start = Some((w.cursor_position().unwrap_or_default(), time.elapsed_secs_f64()));
            }
        }
        if buttons.pressed(MouseButton::Middle) {
            cam.yaw -= motion.delta.x * 0.005;
        }
        if buttons.just_released(MouseButton::Middle) {
            if let (Some((start, t0)), Some(w)) = (drag.mid_start.take(), win) {
                let moved = w.cursor_position().is_some_and(|p| p.distance(start) > MIDDLE_CLICK_PIXELS);
                if !moved && time.elapsed_secs_f64() - t0 < MIDDLE_CLICK_SECS {
                    cam.yaw = 0.0;
                    cam.pitch = RTS_PITCH;
                    cam.dist = crate::meta_keys::DEFAULT_DIST.clamp(cam.min_dist, cam.max_dist);
                }
            }
        }

        // Right-drag scrolls the ground plane (`LookAtXlat` SCROLL_RMB, `:457-484`); the drag
        // direction drives the 8-way `Scroll` cursor (`setCursorDirection`). A left-drag selection
        // in progress suppresses it (`!TheInGameUI->isSelecting()`).
        // `LookAtXlat.cpp:264`: the right button scrolls unless the alternate mouse is on and
        // `UseRightMouseScrollWithAlternateMouse` is off (`GameSettings::right_mouse_scroll_allowed`).
        if buttons.just_pressed(MouseButton::Right) {
            let rmb_scroll = deps.settings.as_ref().is_none_or(|s| s.right_mouse_scroll_allowed());
            if let (Some(w), true) = (win, rmb_scroll) {
                drag.rmb_anchor = w.cursor_position();
            }
        }
        if buttons.pressed(MouseButton::Right) && !buttons.pressed(MouseButton::Left) {
            if let (Some(anchor), Some(w)) = (drag.rmb_anchor, win) {
                if let Some(cur) = w.cursor_position() {
                    let delta = cur - anchor;
                    let len = delta.length();
                    if len > 0.0 {
                        let ndir = delta / len;
                        let world = right * ndir.x - fwd * ndir.y;
                        cam.focus += world * len * RMB_SCROLL_RATE * scroll_speed * dt;
                        deps.scroll_dir.0 = Some(ndir.normalize_or_zero());
                        // `InGameUI::shouldMoveRMBScrollAnchor`: keep the anchor within half a screen.
                        let max = Vec2::new(w.width() * 0.5, w.height() * 0.5);
                        let mut a = anchor;
                        if cur.x + max.x < a.x {
                            a.x = cur.x + max.x;
                        } else if cur.x - max.x > a.x {
                            a.x = cur.x - max.x;
                        }
                        if cur.y + max.y < a.y {
                            a.y = cur.y + max.y;
                        } else if cur.y - max.y > a.y {
                            a.y = cur.y - max.y;
                        }
                        // `MoveScrollAnchor` off: the anchor stays where the drag started.
                        drag.rmb_anchor = Some(if move_anchor { a } else { anchor });
                    }
                }
            }
        }
        if buttons.just_released(MouseButton::Right) {
            drag.rmb_anchor = None;
        }

        let ticks = match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y,
            MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
        };
        cam.dist = (cam.dist * 0.9f32.powf(ticks)).clamp(cam.min_dist, cam.max_dist);
    } else {
        drag.rmb_anchor = None;
    }
    let g = map.ground();
    // RTS mode keeps the focus inside the playable area (free-fly is unrestricted).
    let (pw, ph) = g.playable_size();
    cam.focus.x = cam.focus.x.clamp(0.0, pw);
    cam.focus.z = cam.focus.z.clamp(-ph, 0.0);
    let (fx, fz) = (cam.focus.x, cam.focus.z);
    cam.focus.y = g.height(fx, -fz);
    if let Ok(mut t) = q.single_mut() {
        *t = rts_transform(&cam);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn free_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    interactive: Res<Interactive>,
    rts: Res<RtsCam>,
    lock: Res<CamLock>,
    mut free: ResMut<FreeCam>,
    mut q: Query<&mut Transform, With<Camera3d>>,
) {
    let Ok(mut t) = q.single_mut() else { return };
    if lock.0 {
        return;
    }
    if interactive.0 && crate::meta_keys::dev_mode() && keys.just_pressed(KeyCode::KeyF) {
        free.active = !free.active;
        if free.active {
            let (yaw, pitch, _) = t.rotation.to_euler(EulerRot::YXZ);
            free.yaw = yaw;
            free.pitch = pitch;
        } else {
            *t = rts_transform(&rts);
        }
    }
    if !free.active || !interactive.0 {
        return;
    }
    if buttons.pressed(MouseButton::Right) {
        free.yaw -= motion.delta.x * 0.003;
        free.pitch = (free.pitch - motion.delta.y * 0.003).clamp(-1.55, 1.55);
    }
    t.rotation = Quat::from_euler(EulerRot::YXZ, free.yaw, free.pitch, 0.0);
    let mut d = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        d += *t.forward();
    }
    if keys.pressed(KeyCode::KeyS) {
        d -= *t.forward();
    }
    if keys.pressed(KeyCode::KeyD) {
        d += *t.right();
    }
    if keys.pressed(KeyCode::KeyA) {
        d -= *t.right();
    }
    if keys.pressed(KeyCode::Space) {
        d += Vec3::Y;
    }
    if keys.pressed(KeyCode::ControlLeft) {
        d -= Vec3::Y;
    }
    let speed = if keys.pressed(KeyCode::ShiftLeft) { 1200.0 } else { 300.0 };
    t.translation += d.normalize_or_zero() * speed * time.delta_secs();
}
