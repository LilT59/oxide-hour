//! Possession radial command menu (docs/POSSESSION.md, "Client side": "the RTS UI collapses to a
//! minimap, credits/power, and a radial menu for the possessed unit's command buttons").
//!
//! This is a **possession-only addition**: the engine has no such menu. It reuses the exact command
//! data the RTS control bar uses (`CommandSet` -> `CommandButton`, `hud::slot_infos` /
//! `hud::dispatch_button`), so the icons and the issued `Command`s are identical to clicking the
//! control bar. Because the possession camera locks the cursor for mouse-look, the menu is driven
//! by holding a key (`Q`, or the middle mouse button) and aiming with mouse motion: the sector
//! nearest the accumulated aim direction is highlighted, and releasing activates it.
//!
//! Abilities that need a target (`SPECIAL_POWER` with `NEED_TARGET_POS`) are fired straight at the
//! possession crosshair's aim point (`PossessState::aim`) instead of entering the RTS placement
//! mode, since there is no free cursor in possession.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use zr_sim::Command;

use crate::hud::{self, HudData, SlotInfo};
use crate::sim_view::SimRes;

/// A slot's icon box, px.
const ICON: f32 = 44.0;
/// Ring radius, px (distance from screen centre to icon centre).
const RADIUS: f32 = 210.0;
/// Mouse-motion sensitivity for rotating the highlighted slot, radians per pixel.
const AIM_SENS: f32 = 0.006;

/// Key that holds the radial menu open.
const KEY_RADIAL: KeyCode = KeyCode::KeyQ;

/// `RadialState::open` plus the resolved slot list, so the UI can be (re)built and dispatched.
#[derive(Resource, Default)]
pub struct RadialState {
    pub open: bool,
    /// The highlighted slot index, or `None` when the ring is empty.
    pub selected: Option<usize>,
    /// Accumulated aim angle (sector picked by the nearest angle).
    angle: f32,
    /// The slot list captured when the menu opened (so the dispatch matches the shown icons).
    slots: Vec<Option<SlotInfo>>,
}

#[derive(Component)]
struct RadialRoot;
#[derive(Component)]
struct RadialSlot(usize);

impl RadialState {
    /// Whether the menu should be open this frame: a possessed unit plus the hold key.
    pub fn wants_open(sim: Option<&SimRes>, keys: &ButtonInput<KeyCode>, mouse: &ButtonInput<MouseButton>, demo: bool) -> bool {
        let possessing = sim.is_some_and(|s| s.possess.id.is_some());
        possessing && (demo || keys.pressed(KEY_RADIAL) || mouse.pressed(MouseButton::Middle))
    }
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<RadialState>();
    app.add_systems(
        Update,
        radial_control
            .after(crate::sim_view::sim_step)
            .run_if(in_state(crate::shell::ShellState::InGame)),
    );
}

/// Debug `--radial-demo`: open the menu (for screenshots).
fn demo_flag() -> bool {
    std::env::args().any(|a| a == "--radial-demo")
}

#[allow(clippy::too_many_arguments)]
fn radial_control(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut state: ResMut<RadialState>,
    sim: Option<NonSendMut<SimRes>>,
    hud: Option<ResMut<HudData>>,
    window: Query<&Window, With<PrimaryWindow>>,
    roots: Query<Entity, With<RadialRoot>>,
    mut assets: ResMut<Assets<Image>>,
    mut slots_q: Query<(&RadialSlot, &mut BackgroundColor, &mut ImageNode, &mut Node)>,
    mut audio: ResMut<crate::audio::AudioQueue>,
    mut mode: ResMut<crate::placement::OrderMode>,
) {
    let Some(mut sim_guard) = sim else { return };
    let Some(mut hud) = hud else { return };
    let sim: &mut SimRes = &mut sim_guard;
    let demo = demo_flag();

    let wanted = RadialState::wants_open(Some(&*sim), &keys, &mouse, demo);
    if !wanted {
        if state.open {
            // Release dispatch: activate the highlighted slot before tearing the UI down.
            if let Some(i) = state.selected {
                activate(sim, &hud, i, &mut mode, &mut audio);
            }
            state.open = false;
            state.selected = None;
            state.slots.clear();
            for e in &roots {
                commands.entity(e).despawn();
            }
        }
        return;
    }

    // (Re)build the ring's slot list when opening. The list is frozen until the menu closes so the
    // dispatch matches what the player saw.
    if !state.open {
        state.open = true;
        state.slots = hud::slot_infos(sim, &hud).to_vec();
        state.angle = -std::f32::consts::FRAC_PI_2; // top
        state.selected = if state.slots.iter().any(Option::is_some) { Some(0) } else { None };
        spawn_ring(&mut commands, &window, &state.slots);
    }

    // Mouse motion rotates the highlighted sector.
    if demo {
        state.selected = (0..state.slots.len()).find(|&i| state.slots[i].is_some());
    } else if motion.delta.length_squared() > 0.0 {
        state.angle += (motion.delta.x + motion.delta.y) * AIM_SENS;
        state.selected = nearest_slot(&state.slots, state.angle);
    }

    // Decode / refresh the icons (the same MappedImage path as the control bar).
    let HudData { vfs, language, images, textures, failed, .. } = &mut *hud;
    for (slot, mut bg, mut image, mut node) in &mut slots_q {
        let Some(info) = state.slots.get(slot.0).and_then(Option::as_ref) else { continue };
        let hot = state.selected == Some(slot.0);
        *bg = BackgroundColor(if hot { Color::srgba(0.95, 0.82, 0.2, 0.95) } else { Color::srgba(0.10, 0.11, 0.14, 0.9) });
        node.width = Val::Px(if hot { ICON + 6.0 } else { ICON });
        node.height = Val::Px(if hot { ICON + 6.0 } else { ICON });
        let Some(name) = info.image.as_deref() else { continue };
        if let Some(handle) = hud::decode_image(vfs, images, language, &mut assets, textures, failed, name) {
            image.image = handle;
            if let Some(img) = images.get(name) {
                image.rect = Some(Rect::new(img.left as f32, img.top as f32, img.right as f32, img.bottom as f32));
            }
        }
    }
}

/// The slot whose sector is nearest to the accumulated aim angle. Slots are evenly spaced starting
/// at the top; an empty slot is skipped in favour of the next populated one.
fn nearest_slot(slots: &[Option<SlotInfo>], angle: f32) -> Option<usize> {
    let n = slots.iter().filter(|s| s.is_some()).count();
    if n == 0 {
        return None;
    }
    let step = std::f32::consts::TAU / n as f32;
    // Slots start at the top (-y); the aim angle is measured from +x, so shift by pi/2.
    let wanted = (angle + std::f32::consts::FRAC_PI_2).rem_euclid(std::f32::consts::TAU);
    let index = (wanted / step).round() as usize % n;
    slots.iter().enumerate().filter(|(_, s)| s.is_some()).nth(index).map(|(i, _)| i)
}

/// Spawns the ring of icon buttons around the screen centre.
fn spawn_ring(commands: &mut Commands, window: &Query<&Window, With<PrimaryWindow>>, slots: &[Option<SlotInfo>]) {
    let (w, h) = window.single().map_or((1600.0, 900.0), |w| (w.width(), w.height()));
    let (cx, cy) = (w * 0.5, h * 0.5);
    let root = commands
        .spawn((
            RadialRoot,
            Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Px(w), height: Val::Px(h), ..default() },
            ZIndex(650),
        ))
        .id();
    let n = slots.iter().filter(|s| s.is_some()).count();
    if n == 0 {
        return;
    }
    let step = std::f32::consts::TAU / n as f32;
    let mut k = 0usize;
    for (i, info) in slots.iter().enumerate() {
        if info.is_none() {
            continue;
        }
        let a = -std::f32::consts::FRAC_PI_2 + k as f32 * step;
        k += 1;
        let (x, y) = (cx + a.cos() * RADIUS, cy + a.sin() * RADIUS);
        commands.spawn((
            RadialSlot(i),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(x - ICON * 0.5),
                top: Val::Px(y - ICON * 0.5),
                width: Val::Px(ICON),
                height: Val::Px(ICON),
                border: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.10, 0.11, 0.14, 0.9)),
            BorderColor::all(Color::srgb(0.6, 0.6, 0.65)),
            ImageNode::default(),
            ChildOf(root),
        ));
    }
}

/// Issues the slot's command. Mirrors [`hud::dispatch_button`], but target-needing abilities use the
/// possession crosshair's aim point instead of the RTS placement mode.
pub(crate) fn activate(sim: &mut SimRes, hud: &HudData, slot: usize, mode: &mut crate::placement::OrderMode, audio: &mut crate::audio::AudioQueue) {
    let Some(id) = sim.possess.id.or_else(|| sim.selected.first().copied()) else { return };
    let Some(set_name) = hud::selected_command_set(sim) else { return };
    let Some(btn) = hud.commands.set(&set_name).and_then(|s| s.slot(slot)).and_then(|n| hud.commands.button(n)) else { return };
    // A disabled command does nothing, like the control bar's greyscale buttons.
    let obj = sim.game.logic().find_object(id);
    if !crate::cmd_avail::command_availability(sim, btn, obj).enabled() {
        return;
    }
    let local = sim.local;
    let aim = sim.possess.aim().unwrap_or_else(|| sim.game.object(id).map_or(zr_sim::Coord3D::ZERO, |o| o.position));
    let click = || btn.unit_specific_sound.clone().unwrap_or_else(|| "GUIClick".to_owned());
    match &btn.command {
        zr_ui::command::CommandKind::SpecialPower
        | zr_ui::command::CommandKind::SpecialPowerFromShortcut
        | zr_ui::command::CommandKind::SpecialPowerConstruct => {
            if let Some(power) = &btn.special_power {
                audio.push(click(), None, None);
                sim.game.submit(local, Command::DoSpecialPower { source: id, power: power.clone(), target: zr_sim::special_power::SpecialPowerTarget::Position(aim) });
                info!("radial: {power} at ({:.0},{:.0})", aim.x, aim.y);
            }
        }
        zr_ui::command::CommandKind::AttackMove => {
            audio.push(click(), None, None);
            sim.game.submit(local, Command::AttackMove { ids: vec![id], pos: aim });
        }
        // A `FIRE_WEAPON` that takes a ground point fires at the aim point (`MSG_DO_WEAPON_AT_LOCATION`).
        zr_ui::command::CommandKind::FireWeapon if hud::target_flags(btn.options).0 => {
            audio.push(click(), None, None);
            let slot = hud::fire_slot(btn);
            if btn.options & zr_ui::command::command_option::USES_MINE_CLEARING_WEAPONSET != 0 {
                sim.game.submit(local, Command::SetMineClearingDetail { ids: vec![id] });
            }
            sim.game.submit(local, Command::DoWeaponAtLocation { ids: vec![id], slot, pos: aim, max_shots: hud::max_shots_arg(btn.max_shots_to_fire) });
        }
        zr_ui::command::CommandKind::Guard | zr_ui::command::CommandKind::GuardWithoutPursuit | zr_ui::command::CommandKind::GuardFlyingUnitsOnly => {
            audio.push(click(), None, None);
            let guard_mode = match btn.command {
                zr_ui::command::CommandKind::GuardWithoutPursuit => zr_sim::GuardMode::WithoutPursuit,
                zr_ui::command::CommandKind::GuardFlyingUnitsOnly => zr_sim::GuardMode::FlyingUnitsOnly,
                _ => zr_sim::GuardMode::Normal,
            };
            sim.game.submit(local, Command::Guard { ids: vec![id], target: zr_sim::orders::GuardTarget::Point(aim), mode: guard_mode });
        }
        _ => hud::dispatch_button(sim, mode, id, &[id], btn, None, audio),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> Option<SlotInfo> {
        Some(SlotInfo {
            image: None,
            label: None,
            tooltip: None,
            power: None,
            availability: crate::cmd_avail::CommandAvailability::Available,
            cost: None,
            requires: None,
            reason: None,
            contained: None,
        })
    }

    #[test]
    fn nearest_slot_picks_top_then_walks_clockwise() {
        let slots = vec![info(), info(), info(), info()];
        // angle = -pi/2 is straight up (slot 0).
        assert_eq!(nearest_slot(&slots, -std::f32::consts::FRAC_PI_2), Some(0));
        // 0 is to the right -> the second of four.
        assert_eq!(nearest_slot(&slots, 0.0), Some(1));
        // pi (left) -> the fourth.
        assert_eq!(nearest_slot(&slots, std::f32::consts::PI), Some(3));
    }

    #[test]
    fn nearest_slot_skips_empty_slots() {
        let slots = vec![None, info(), None, info()];
        assert_eq!(nearest_slot(&slots, -std::f32::consts::FRAC_PI_2), Some(1));
        assert!(nearest_slot(&[None, None], 0.0).is_none());
    }
}
