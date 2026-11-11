//! Options menu (`Menus/OptionsMenu.wnd`, `OptionsMenu.cpp`), drawn from the real `.wnd` with the
//! shell's gadget renderer ([`crate::gadget`]).
//!
//! The screen edits a draft ([`settings::Session`]). ACCEPT writes `options.ini` and applies the
//! draft; CANCEL, Esc, F10 and BACK drop it. As in `OptionsMenu.cpp`, nothing is applied while the
//! screen is open: BACK does not write the preferences, and the volumes, scroll factor, gamma,
//! detail, resolution and anti-aliasing take effect in `saveOptions` only.
//!
//! Applied on ACCEPT: the resolution and fullscreen (the window), the three volumes
//! ([`AudioBank::set_volumes`], with the 2D / 3D split), the scroll factor (the RTS camera pan),
//! the gamma ([`apply_display`]: a colour-grading gamma on the 3D camera, an approximation of the
//! display gamma ramp), the detail level's 2D shadow decals (`shadow.rs`), the RMB move-anchor
//! option (`camera.rs`, reachable only from a file: its check box is hidden in the `.wnd`), the mouse
//! scheme (`UseAlternateMouse`, `rts_input.rs`), the double-click attack-move (`UseDoubleClickAttackMove`,
//! a guard order, `rts_input.rs`), the alternate-mouse RMB scroll key (`camera.rs`, file only), the
//! detail level's particle cap and dynamic-LOD switch (`fx.rs` `ParticleManager`), and retaliation
//! (`Retaliation`): the in-game client posts `MSG_ENABLE_RETALIATION_MODE` once a second while the option
//! differs from the player's logical mode (`Player.cpp:704-714`, `sim_view.rs`'s `sim_step`).
//!
//! Stored in `options.ini` under the C++ key, with no effect in the viewer: the other detail fields
//! (3D and cloud shadows, light map, soft water, extra animations, heat effects, behind-building
//! markers, props). `TextureReduction` is applied at launch only ([`apply_texture_reduction`]).
//! IP addresses and firewall port are read back, not editable. Hidden in `OptionsMenu.wnd` itself (so not drawn, in
//! retail too): anti-aliasing, the language filter, the replay camera and scroll-anchor boxes, the
//! keyboard options button and the unlock-FPS box; their values are kept and written as read.
//! FULLSCREEN is a viewer addition (retail has no fullscreen toggle here), beside the detail combo.

mod settings;

use bevy::prelude::*;
use bevy::render::view::ColorGrading;
use bevy::window::{MonitorSelection, PrimaryWindow, WindowMode, WindowResolution};
use zr_text::GameText;
use zr_ui::wnd::{GadgetData, WndFile};

pub use settings::{CustomLod, Detail, GameSettings, InstallDefaults, LodPresets, Session};

use crate::audio::AudioBank;
use crate::gadget::{self, SliderBar};
use crate::shell::{self, ShellData, ShellState};

/// Resolutions offered by the combo (the engine reads the display mode list; a fixed list keeps
/// the viewer dependency-free).
const RESOLUTIONS: &[(u32, u32)] = &[(1280, 720), (1366, 768), (1600, 900), (1920, 1080), (2560, 1440)];

/// The options screen state.
#[derive(Resource, Default)]
pub struct OptionsState {
    pub open: bool,
    /// Rebuild the screen on the next frame (it opened, or a click changed what it shows).
    pub dirty: bool,
    built: bool,
    /// The ACCEPT / CANCEL draft (see [`Session`]).
    session: Session,
    /// The advanced display panel (`WinAdvancedDisplayOptions`), open at the `Custom` detail.
    advanced_open: bool,
}

impl OptionsState {
    /// Asks the builder to rebuild the screen (used when another screen opens it).
    pub fn request_rebuild(&mut self) {
        self.built = false;
        self.dirty = true;
    }
}

#[derive(Component)]
struct OptionsRoot;

/// What a clicked control does.
#[derive(Component, Clone, Copy)]
enum OptionsAction {
    Resolution,
    Detail,
    Fullscreen,
    Defaults,
    Accept,
    Cancel,
    Toggle(Toggle),
    FirewallRefresh,
    AdvancedAccept,
    AdvancedBack,
}

/// The check boxes, by their `.wnd` names (`OptionsMenu.cpp` `check*`).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Toggle {
    LanguageFilter,
    AlternateMouse,
    Retaliation,
    DoubleClickAttackMove,
    DrawScrollAnchor,
    MoveScrollAnchor,
    SaveCamera,
    UseCamera,
    Shadows3D,
    Shadows2D,
    CloudShadows,
    GroundLighting,
    SmoothWater,
    ExtraAnimations,
    NoDynamicLod,
    HeatEffects,
    BehindBuilding,
    ShowProps,
}

impl Toggle {
    /// The check box for a full `.wnd` window name. `CheckUnlockFPS` is not one: `OptionsMenu.cpp`
    /// never writes it, so it is drawn and does nothing.
    fn of(name: &str) -> Option<Toggle> {
        Some(match name.rsplit(':').next()? {
            "CheckLanguageFilter" => Toggle::LanguageFilter,
            "CheckAlternateMouse" => Toggle::AlternateMouse,
            "Retaliation" => Toggle::Retaliation,
            "CheckDoubleClickAttackMove" => Toggle::DoubleClickAttackMove,
            "CheckBoxDrawAnchor" => Toggle::DrawScrollAnchor,
            "CheckBoxMoveAnchor" => Toggle::MoveScrollAnchor,
            "CheckBoxSaveCamera" => Toggle::SaveCamera,
            "CheckBoxUseCamera" => Toggle::UseCamera,
            "Check3DShadows" => Toggle::Shadows3D,
            "Check2DShadows" => Toggle::Shadows2D,
            "CheckCloudShadows" => Toggle::CloudShadows,
            "CheckGroundLighting" => Toggle::GroundLighting,
            "CheckSmoothWater" => Toggle::SmoothWater,
            "CheckExtraAnimations" => Toggle::ExtraAnimations,
            "CheckNoDynamicLOD" => Toggle::NoDynamicLod,
            "CheckHeatEffects" => Toggle::HeatEffects,
            "CheckBehindBuilding" => Toggle::BehindBuilding,
            "CheckShowProps" => Toggle::ShowProps,
            _ => return None,
        })
    }

    /// The box's state. `CheckNoDynamicLOD` is inverted, as in `saveOptions`.
    fn get(self, s: &GameSettings) -> bool {
        match self {
            Toggle::LanguageFilter => s.language_filter,
            Toggle::AlternateMouse => s.alternate_mouse,
            Toggle::Retaliation => s.retaliation,
            Toggle::DoubleClickAttackMove => s.double_click_attack_move,
            Toggle::DrawScrollAnchor => s.draw_scroll_anchor,
            Toggle::MoveScrollAnchor => s.move_scroll_anchor,
            Toggle::SaveCamera => s.save_camera_in_replays,
            Toggle::UseCamera => s.use_camera_in_replays,
            Toggle::Shadows3D => s.custom.shadow_volumes,
            Toggle::Shadows2D => s.custom.shadow_decals,
            Toggle::CloudShadows => s.custom.cloud_map,
            Toggle::GroundLighting => s.custom.light_map,
            Toggle::SmoothWater => s.custom.soft_water,
            Toggle::ExtraAnimations => s.custom.extra_animations,
            Toggle::NoDynamicLod => !s.custom.dynamic_lod,
            Toggle::HeatEffects => s.custom.heat_effects,
            Toggle::BehindBuilding => s.custom.building_occlusion,
            Toggle::ShowProps => s.custom.show_trees,
        }
    }

    fn flip(self, s: &mut GameSettings) {
        let v = !self.get(s);
        match self {
            Toggle::LanguageFilter => s.language_filter = v,
            Toggle::AlternateMouse => s.alternate_mouse = v,
            Toggle::Retaliation => s.retaliation = v,
            Toggle::DoubleClickAttackMove => s.double_click_attack_move = v,
            Toggle::DrawScrollAnchor => s.draw_scroll_anchor = v,
            Toggle::MoveScrollAnchor => s.move_scroll_anchor = v,
            Toggle::SaveCamera => s.save_camera_in_replays = v,
            Toggle::UseCamera => s.use_camera_in_replays = v,
            Toggle::Shadows3D => s.custom.shadow_volumes = v,
            Toggle::Shadows2D => s.custom.shadow_decals = v,
            Toggle::CloudShadows => s.custom.cloud_map = v,
            Toggle::GroundLighting => s.custom.light_map = v,
            Toggle::SmoothWater => s.custom.soft_water = v,
            Toggle::ExtraAnimations => s.custom.extra_animations = v,
            Toggle::NoDynamicLod => s.custom.dynamic_lod = !v,
            Toggle::HeatEffects => s.custom.heat_effects = v,
            Toggle::BehindBuilding => s.custom.building_occlusion = v,
            Toggle::ShowProps => s.custom.show_trees = v,
        }
    }
}

/// The horizontal sliders, by their `.wnd` names. Positions are in the C++ units (see
/// [`GameSettings`]); each slider's range is its `.wnd` `MINVALUE` / `MAXVALUE`.
#[derive(Clone, Copy, PartialEq)]
enum Slider {
    Music,
    Sfx,
    Voice,
    Scroll,
    Gamma,
    TextureReduction,
    ParticleCap,
}

impl Slider {
    /// The slider for a full `.wnd` window name (`OptionsMenu.wnd:SliderMusicVolume`).
    fn of(name: &str) -> Option<Slider> {
        match name.rsplit(':').next()? {
            "SliderMusicVolume" => Some(Slider::Music),
            "SliderSFXVolume" => Some(Slider::Sfx),
            "SliderVoiceVolume" => Some(Slider::Voice),
            "SliderScrollSpeed" => Some(Slider::Scroll),
            "SliderGamma" => Some(Slider::Gamma),
            "LowResSlider" => Some(Slider::TextureReduction),
            "ParticleCapSlider" => Some(Slider::ParticleCap),
            _ => None,
        }
    }

    /// The slider's position (`GadgetSliderGetPosition`). The texture slider runs `2 - reduction`.
    fn position(self, s: &GameSettings) -> i32 {
        match self {
            Slider::Music => s.music,
            Slider::Sfx => s.sfx,
            Slider::Voice => s.voice,
            Slider::Scroll => s.scroll_factor,
            Slider::Gamma => s.gamma,
            Slider::TextureReduction => 2 - s.custom.texture_reduction,
            Slider::ParticleCap => s.custom.max_particles,
        }
    }

    fn set_position(self, s: &mut GameSettings, pos: i32) {
        match self {
            Slider::Music => {
                s.music = pos;
                s.volumes_pending = false;
            }
            Slider::Sfx => {
                s.sfx = pos;
                s.volumes_pending = false;
            }
            Slider::Voice => {
                s.voice = pos;
                s.volumes_pending = false;
            }
            Slider::Scroll => {
                s.scroll_factor = pos.max(1);
                s.scroll_pending = false;
            }
            Slider::Gamma => s.gamma = pos,
            Slider::TextureReduction => s.custom.texture_reduction = (2 - pos).clamp(0, 2),
            Slider::ParticleCap => s.custom.max_particles = pos.max(100),
        }
    }

    /// The position as a fraction of the range `min..=max` (the bar's fill).
    fn fraction(self, s: &GameSettings, min: i32, max: i32) -> f32 {
        if max <= min {
            return 0.0;
        }
        ((self.position(s) - min) as f32 / (max - min) as f32).clamp(0.0, 1.0)
    }
}

/// A live slider's root: its screen rect, so a press can be turned into a position, and its range.
#[derive(Component, Clone, Copy)]
struct OptionsSlider {
    which: Slider,
    rect: (f32, f32, f32, f32),
    min: i32,
    max: i32,
}

/// A volume or gamma as the C++ percent (`REAL_TO_INT(value * 100)`).
fn percent(fraction: f32) -> i32 {
    (fraction * 100.0).round() as i32
}

/// Applies the launch `TextureReduction`: `GameLODManager::setStaticLODLevel` passes the level to
/// `TheGameClient->setTextureLOD` (`GameLOD.cpp:565-580`, `:633`), which calls `WW3D::Set_Texture_Reduction(level, 32)`
/// (`W3DGameClient.cpp:199-206`). The level is the `Custom` branch's `TextureReduction`, or the detail level's
/// `TextureReductionFactor` preset (`LodPresets`), so it is applied where the install's INI becomes available (the
/// loading stage, and the shell map, which parses it itself). Like `TextureFilter`, a change needs a restart.
pub fn apply_texture_reduction(settings: &GameSettings, presets: &LodPresets) {
    crate::convert::set_texture_reduction(settings.lod(presets).texture_reduction.clamp(0, 2) as u8);
}

pub fn add_systems(app: &mut App) {
    let settings = GameSettings::load();
    // The engine applies `TextureFilter` and `AnisotropyLevel` at startup (`W3DDisplay.cpp:899`); textures made
    // after this use them. A change needs a restart (documented in LOG lane 61). `TextureReduction` needs the
    // install's detail presets, so it is applied where the INI is read ([`apply_texture_reduction`]).
    crate::convert::set_texture_filter(settings.texture_filter, settings.anisotropy);
    app.insert_resource(settings);
    app.init_resource::<LodPresets>();
    app.init_resource::<OptionsState>();
    app.add_systems(
        Update,
        (
            toggle_options,
            build_options,
            options_input,
            drag_sliders,
            refresh_sliders,
            seed_audio_defaults,
            seed_scroll_default,
            apply_settings,
            apply_display,
        )
            .chain(),
    );
}

/// F10 toggles the screen. Closing it without ACCEPT drops the draft (see [`build_options`]).
pub fn toggle_options(keys: Res<ButtonInput<KeyCode>>, mut state: ResMut<OptionsState>, shell: Option<Res<ShellData>>, mut auto_opened: Local<bool>) {
    if keys.just_pressed(KeyCode::F10) || (state.open && keys.just_pressed(KeyCode::Escape)) {
        state.open = !state.open;
        state.dirty = true;
        if !state.open {
            // Rebuild on the next open so the screen picks up the shell data again.
            state.built = false;
        }
    }
    // Debug: open once for a screenshot, once the shell data (the `.wnd`) is loaded.
    if !*auto_opened && !state.open && shell.is_some() && std::env::args().any(|a| a == "--open-options") {
        *auto_opened = true;
        state.open = true;
        state.dirty = true;
    }
}

/// Builds `Menus/OptionsMenu.wnd` when the screen opens or a click changed what it shows.
/// Dragging a slider does not rebuild (it only moves the bar), so the press is not lost.
#[allow(clippy::too_many_arguments)]
fn build_options(
    mut commands: Commands,
    mut state: ResMut<OptionsState>,
    shell: Option<Res<ShellData>>,
    settings: Res<GameSettings>,
    game: Option<Res<State<ShellState>>>,
    window: Query<&Window, With<PrimaryWindow>>,
    roots: Query<Entity, With<OptionsRoot>>,
) {
    // A closed screen must be despawned, not just left built: its full-screen root would keep
    // blocking world clicks through `ui_block` after Esc / F10 / ACCEPT closed it. Closing without
    // ACCEPT drops the draft (CANCEL).
    if !state.open {
        for e in &roots {
            commands.entity(e).despawn();
        }
        state.session.cancel();
        state.advanced_open = false;
        state.built = false;
        return;
    }
    if !state.session.is_open() {
        state.session.open(&settings);
        state.advanced_open = false;
    }
    if state.built && !state.dirty {
        return;
    }
    // The `.wnd` needs the shell's decoded images, so wait for the shell data.
    let Some(shell) = shell.as_deref() else { return };
    for e in &roots {
        commands.entity(e).despawn();
    }
    state.built = true;
    state.dirty = false;
    let win = window.single().ok();
    let in_game = game.is_some_and(|s| *s.get() == ShellState::InGame);
    let root = commands
        .spawn((
            OptionsRoot,
            Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
            ZIndex(500),
        ))
        .id();
    let Some(wnd) = &shell.options_wnd else {
        // No `OptionsMenu.wnd` in the install: keep the screen dismissable.
        spawn_button(&mut commands, root, (200.0, 430.0, 160.0, 30.0), "Accept", OptionsAction::Accept);
        return;
    };
    let Some(draft) = state.session.draft() else { return };
    let open = Open { in_game, advanced: state.advanced_open };
    build_wnd(&mut commands, root, shell, wnd, draft, win, open);
}

/// What the screen shows beyond the draft: the game is running (`OptionsMenu.cpp` disables the
/// detail, resolution and network controls in game) and the advanced panel is open.
#[derive(Clone, Copy)]
struct Open {
    in_game: bool,
    advanced: bool,
}

/// The detail combo's label (`GUI:Low` ... `GUI:Custom`, `GUI:VeryHigh` with its fallback).
fn detail_label(text: &GameText, d: Detail) -> String {
    let (label, fallback) = match d {
        Detail::Low => ("GUI:Low", "Low"),
        Detail::Medium => ("GUI:Medium", "Medium"),
        Detail::High => ("GUI:High", "High"),
        Detail::VeryHigh => ("GUI:VeryHigh", "Very High"),
        Detail::Custom => ("GUI:Custom", "Custom"),
    };
    text.get(label).unwrap_or(fallback).to_owned()
}

/// Draws `OptionsMenu.wnd` under `root` from the draft and wires its controls.
fn build_wnd(commands: &mut Commands, root: Entity, shell: &ShellData, wnd: &WndFile, draft: &GameSettings, win: Option<&Window>, open: Open) {
    let text = &shell.text;
    let raw = |key: &str| draft.raw.get(key).cloned().unwrap_or_default();
    let label = |name: &str| -> Option<String> {
        let short = name.rsplit(':').next().unwrap_or("");
        match short {
            "ComboBoxResolution" => Some(format!("{} x {}", draft.resolution.0, draft.resolution.1)),
            "ComboBoxDetail" => Some(detail_label(text, draft.detail)),
            // The LAN and GameSpy addresses are read from the file; this viewer has no address list.
            "ComboBoxIP" => Some(raw("IPAddress")),
            "ComboBoxOnlineIP" => Some(raw("GameSpyIPAddress")),
            "TextEntryFirewallPortOverride" => Some(raw("FirewallPortOverride")),
            // Other controls show no text (the `.wnd` text is a placeholder).
            s if s.starts_with("ComboBox") || s.starts_with("TextEntry") || s == "LabelVersion" => Some(String::new()),
            _ => None,
        }
    };
    let fraction = |name: &str| {
        let s = Slider::of(name)?;
        let (min, max) = wnd_range(wnd, name).unwrap_or((0, 100));
        Some(s.fraction(draft, min, max))
    };
    let checked = |name: &str| Toggle::of(name).map(|t| t.get(draft));
    // `CheckSendDelay` and the HTTP proxy are hidden in this fork (`ENABLE_GUI_HACKS`).
    let hide = |name: &str| name.ends_with("CheckSendDelay") || name.ends_with("TextEntryHTTPProxy") || name.ends_with("StaticTextHTTPProxy");
    let reveal = |name: &str| open.advanced && name.ends_with("WinAdvancedDisplayOptions");
    let scene = gadget::Scene {
        wnd,
        images: &shell.images,
        fonts: &shell.fonts,
        text,
        window: win,
        hide: &hide,
        reveal: &reveal,
        label: &label,
        checked: &checked,
        tint: &|_: &str| None::<Color>,
        fraction: &fraction,
    };
    for (name, entity) in gadget::render(commands, root, &scene) {
        let short = name.rsplit(':').next().unwrap_or("");
        match short {
            "ButtonAccept" => gadget::make_clickable(commands, entity, OptionsAction::Accept),
            "ButtonBack" => gadget::make_clickable(commands, entity, OptionsAction::Cancel),
            "ButtonDefaults" => gadget::make_clickable(commands, entity, OptionsAction::Defaults),
            "ButtonFirewallRefresh" if !open.in_game => gadget::make_clickable(commands, entity, OptionsAction::FirewallRefresh),
            "ButtonAdvanceAccept" => gadget::make_clickable(commands, entity, OptionsAction::AdvancedAccept),
            "ButtonAdvanceBack" => gadget::make_clickable(commands, entity, OptionsAction::AdvancedBack),
            // Disabled in game: the detail and resolution combos (`OptionsMenuInit`).
            "ComboBoxResolution" if !open.in_game => gadget::make_clickable(commands, entity, OptionsAction::Resolution),
            "ComboBoxDetail" if !open.in_game => gadget::make_clickable(commands, entity, OptionsAction::Detail),
            _ => {
                if let Some(t) = Toggle::of(&name) {
                    gadget::make_clickable(commands, entity, OptionsAction::Toggle(t));
                } else if let Some(which) = Slider::of(&name) {
                    let rect = wnd_rect(wnd, win, &name).unwrap_or((0.0, 0.0, 0.0, 0.0));
                    let (min, max) = wnd_range(wnd, &name).unwrap_or((0, 100));
                    gadget::make_clickable(commands, entity, OptionsSlider { which, rect, min, max });
                } else {
                    gadget::make_hoverable(commands, entity);
                }
            }
        }
    }
    // FULLSCREEN (a viewer addition): beside the detail combo, which has no retail setting here.
    // The Display panel ends about 70 units past the combo, so the button is no wider than that.
    // Not while the advanced panel covers the display options.
    if let Some(d) = wnd_rect(wnd, win, "ComboBoxDetail").filter(|_| !open.advanced) {
        let (sx, _) = shell::wnd_scale(wnd, win);
        let rect = (d.0 + d.2 + 8.0 * sx, d.1, 68.0 * sx, d.3);
        let text = if draft.fullscreen { "Fullscreen: On" } else { "Fullscreen: Off" };
        spawn_button(commands, root, rect, text, OptionsAction::Fullscreen);
    }
}

/// Handles the clicks. Edits go to the draft; ACCEPT applies and saves it, CANCEL drops it.
fn options_input(
    mut state: ResMut<OptionsState>,
    mut settings: ResMut<GameSettings>,
    actions: Query<(&Interaction, &OptionsAction), Changed<Interaction>>,
    audio: Option<Res<AudioBank>>,
    install: Option<Res<InstallDefaults>>,
) {
    if !state.open {
        return;
    }
    // Split the borrow: the session and the advanced panel flag are separate fields.
    let s = &mut *state;
    for (interaction, action) in &actions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match action {
            OptionsAction::Accept => {
                let relative = audio.as_deref().map_or(0.0, |a| a.relative_2d_volume());
                if s.session.accept(&mut settings) {
                    settings.save(relative);
                }
                s.open = false;
                continue;
            }
            OptionsAction::Cancel => {
                s.session.cancel();
                s.open = false;
                continue;
            }
            OptionsAction::AdvancedAccept => s.advanced_open = false,
            OptionsAction::AdvancedBack => {
                // `cancelAdvancedOptions`: the detail combo goes back to the applied level.
                s.advanced_open = false;
                if let Some(d) = s.session.draft_mut() {
                    d.detail = settings.detail;
                }
            }
            OptionsAction::Defaults => {
                // `setDefaults`: the audio, scroll, gamma and mouse / retaliation flags. Not the
                // display, the detail or the network.
                let fresh = GameSettings::default();
                if let Some(d) = s.session.draft_mut() {
                    if let Some(a) = audio.as_deref() {
                        let (music, sfx, voice) = a.volume_defaults();
                        d.music = percent(music);
                        d.sfx = percent(sfx);
                        d.voice = percent(voice);
                        d.volumes_pending = false;
                    }
                    match install.as_deref() {
                        Some(i) => {
                            d.scroll_factor = i.keyboard_scroll_factor;
                            d.scroll_pending = false;
                        }
                        None => d.scroll_factor = fresh.scroll_factor,
                    }
                    d.gamma = 50;
                    d.language_filter = true;
                    d.alternate_mouse = false;
                    d.retaliation = true;
                    d.double_click_attack_move = false;
                }
            }
            OptionsAction::Resolution => {
                if let Some(d) = s.session.draft_mut() {
                    let i = RESOLUTIONS.iter().position(|r| *r == d.resolution).unwrap_or(2);
                    d.resolution = RESOLUTIONS[(i + 1) % RESOLUTIONS.len()];
                }
            }
            OptionsAction::Detail => {
                if let Some(d) = s.session.draft_mut() {
                    d.detail = d.detail.next();
                    // `GCM_SELECTED` on the detail combo: Custom opens the advanced panel.
                    if d.detail == Detail::Custom {
                        s.advanced_open = true;
                    }
                }
            }
            OptionsAction::Fullscreen => {
                if let Some(d) = s.session.draft_mut() {
                    d.fullscreen = !d.fullscreen;
                }
            }
            OptionsAction::Toggle(t) => {
                if let Some(d) = s.session.draft_mut() {
                    t.flip(d);
                }
            }
            OptionsAction::FirewallRefresh => {
                // `ButtonFirewallRefresh`: the behaviour goes back to unknown (`FIREWALL_TYPE_UNKNOWN` = 0).
                if let Some(d) = s.session.draft_mut() {
                    d.raw.insert("FirewallBehavior".into(), "0".into());
                }
            }
        }
        s.dirty = true;
    }
}

/// Sets a slider from the pointer while it is held (`W3DHorizontalSlider`: the position follows the
/// mouse across the bar). Saves once the press is released.
fn drag_sliders(mut state: ResMut<OptionsState>, window: Query<&Window, With<PrimaryWindow>>, sliders: Query<(&Interaction, &OptionsSlider)>) {
    let cursor = window.single().ok().and_then(|w| w.cursor_position());
    let Some(draft) = state.session.draft_mut() else { return };
    for (interaction, slider) in &sliders {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let (x, _, w, _) = slider.rect;
        if let (Some(c), true) = (cursor, w > 0.0) {
            let f = ((c.x - x) / w).clamp(0.0, 1.0);
            let pos = slider.min + (f * (slider.max - slider.min) as f32).round() as i32;
            slider.which.set_position(draft, pos);
        }
    }
}

/// Keeps each slider's bar at the draft's setting.
fn refresh_sliders(state: Res<OptionsState>, mut bars: Query<(&OptionsSlider, &mut SliderBar)>) {
    let Some(draft) = state.session.draft() else { return };
    for (slider, mut bar) in &mut bars {
        let f = slider.which.fraction(draft, slider.min, slider.max);
        if bar.fraction != f {
            bar.fraction = f;
        }
    }
}

/// Applies the settings to the window and the audio bank.
fn apply_settings(settings: Res<GameSettings>, mut windows: Query<&mut Window, With<PrimaryWindow>>, mut audio: Option<ResMut<AudioBank>>) {
    let Some(mut window) = windows.iter_mut().next() else { return };
    let want = WindowResolution::new(settings.resolution.0, settings.resolution.1);
    let mode = if settings.fullscreen { WindowMode::BorderlessFullscreen(MonitorSelection::Current) } else { WindowMode::Windowed };
    if window.resolution.width() != settings.resolution.0 as f32 || window.resolution.height() != settings.resolution.1 as f32 || window.mode != mode {
        window.resolution = want;
        window.mode = mode;
    }
    if let Some(audio) = audio.as_deref_mut() {
        audio.set_volumes(settings.sfx as f32 / 100.0, settings.music as f32 / 100.0, settings.voice as f32 / 100.0);
    }
}

/// Reads the install's keyboard default scroll factor into a fresh `options.ini`, once the game has
/// loaded its `GameData` (`OptionPreferences::getScrollFactor` when the key is absent).
fn seed_scroll_default(install: Option<Res<InstallDefaults>>, mut settings: ResMut<GameSettings>) {
    if !settings.scroll_pending {
        return;
    }
    let Some(install) = install.as_deref() else { return };
    settings.seed_scroll(install);
}

/// Reads the install's audio defaults into a fresh `options.ini`'s volumes, once the game has loaded
/// them (`OptionPreferences::getMusicVolume` and friends when the key is absent).
fn seed_audio_defaults(audio: Option<Res<AudioBank>>, mut settings: ResMut<GameSettings>) {
    if !settings.volumes_pending {
        return;
    }
    let Some(audio) = audio.as_deref() else { return };
    let (music, sfx, voice) = audio.volume_defaults();
    settings.music = percent(music);
    settings.sfx = percent(sfx);
    settings.voice = percent(voice);
    settings.volumes_pending = false;
}

/// Applies the gamma slider to the 3D cameras. The original sets a display gamma ramp
/// (`TheDisplay->setGamma`, `OptionsMenu.cpp` saveOptions); here it is a colour-grading gamma
/// `1 / gamma` on the 3D camera, so it brightens the scene but not the interface. Gamma 50 leaves
/// the camera as it was.
fn apply_display(settings: Res<GameSettings>, mut commands: Commands, mut cams: Query<(Entity, Option<&mut ColorGrading>), With<Camera3d>>) {
    let exponent = if settings.gamma == 50 { 1.0 } else { 1.0 / settings::gamma_value(settings.gamma) };
    let set = |g: &mut ColorGrading| {
        g.shadows.gamma = exponent;
        g.midtones.gamma = exponent;
        g.highlights.gamma = exponent;
    };
    for (entity, grading) in &mut cams {
        match grading {
            Some(mut g) => {
                if g.midtones.gamma != exponent {
                    set(&mut g);
                }
            }
            None if exponent != 1.0 => {
                let mut g = ColorGrading::default();
                set(&mut g);
                commands.entity(entity).insert(g);
            }
            None => {}
        }
    }
}

/// A plain button for the viewer's own controls (not from the `.wnd`).
fn spawn_button(commands: &mut Commands, root: Entity, rect: (f32, f32, f32, f32), label: &str, action: OptionsAction) {
    let e = commands
        .spawn((
            action,
            Button,
            Interaction::default(),
            Node { position_type: PositionType::Absolute, left: Val::Px(rect.0), top: Val::Px(rect.1), width: Val::Px(rect.2), height: Val::Px(rect.3), border: UiRect::all(Val::Px(1.0)), ..default() },
            BackgroundColor(Color::srgba(0.14, 0.15, 0.18, 0.95)),
            BorderColor::all(Color::srgb(0.5, 0.5, 0.55)),
            ChildOf(root),
        ))
        .id();
    commands.spawn((
        Text::new(label.to_owned()),
        TextFont { font_size: FontSize::Px(13.0), ..default() },
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        TextLayout { justify: Justify::Center, ..default() },
        Node { width: Val::Percent(100.0), height: Val::Percent(100.0), align_items: AlignItems::Center, justify_content: JustifyContent::Center, ..default() },
        ChildOf(e),
    ));
}

/// A window's screen rect (scaled), by the end of its full `.wnd` name.
fn wnd_rect(wnd: &WndFile, win: Option<&Window>, name: &str) -> Option<(f32, f32, f32, f32)> {
    let (sx, sy) = shell::wnd_scale(wnd, win);
    wnd.windows
        .iter()
        .flat_map(|r| r.walk())
        .find(|w| w.name.as_deref().is_some_and(|n| n.ends_with(name)))
        .map(|w| shell::scaled(w.rect, sx, sy))
}

/// A horizontal slider's range (`MINVALUE` / `MAXVALUE` in its `.wnd` `SLIDERDATA`), by name.
fn wnd_range(wnd: &WndFile, name: &str) -> Option<(i32, i32)> {
    let w = wnd.windows.iter().flat_map(|r| r.walk()).find(|w| w.name.as_deref().is_some_and(|n| n.ends_with(name)))?;
    match &w.gadget {
        Some(GadgetData::Slider(s)) => Some((s.min_val, s.max_val)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_windows_map_to_their_settings() {
        assert!(matches!(Slider::of("OptionsMenu.wnd:SliderMusicVolume"), Some(Slider::Music)));
        assert!(matches!(Slider::of("OptionsMenu.wnd:SliderScrollSpeed"), Some(Slider::Scroll)));
        assert!(matches!(Slider::of("OptionsMenu.wnd:SliderGamma"), Some(Slider::Gamma)));
        assert!(matches!(Slider::of("OptionsMenu.wnd:LowResSlider"), Some(Slider::TextureReduction)));
        assert!(Slider::of("OptionsMenu.wnd:SliderBogus").is_none());
    }

    #[test]
    fn slider_positions_map_into_their_wnd_range() {
        let mut s = GameSettings::default();
        // Scroll 0..=300 (a `.wnd` range): position 150 is halfway.
        Slider::Scroll.set_position(&mut s, 150);
        assert_eq!(s.scroll_factor, 150);
        assert!((Slider::Scroll.fraction(&s, 0, 300) - 0.5).abs() < 1e-6);
        // The texture slider runs 2 - reduction: position 0 is reduction 2.
        Slider::TextureReduction.set_position(&mut s, 0);
        assert_eq!(s.custom.texture_reduction, 2);
        assert_eq!(Slider::TextureReduction.position(&s), 0);
        // A volume that is dragged stops being the fallback.
        assert!(s.volumes_pending);
        Slider::Music.set_position(&mut s, 37);
        assert_eq!((s.music, s.volumes_pending), (37, false));
        // A range with no width draws empty.
        assert_eq!(Slider::Gamma.fraction(&s, 5, 5), 0.0);
    }

    #[test]
    fn check_boxes_flip_their_setting_and_no_dynamic_lod_is_inverted() {
        let mut s = GameSettings::default();
        assert!(Toggle::MoveScrollAnchor.get(&s));
        Toggle::MoveScrollAnchor.flip(&mut s);
        assert!(!s.move_scroll_anchor);
        assert!(!Toggle::NoDynamicLod.get(&s), "dynamic LOD is on, so the inverted box is off");
        Toggle::NoDynamicLod.flip(&mut s);
        assert!(!s.custom.dynamic_lod, "the box is on: dynamic LOD off");
        assert!(Toggle::of("OptionsMenu.wnd:CheckUnlockFPS").is_none(), "never written by the original");
        assert!(Toggle::of("OptionsMenu.wnd:CheckBoxUseCamera").is_some());
    }

    #[test]
    fn percent_rounds_like_real_to_int() {
        assert_eq!(percent(0.55), 55);
        assert_eq!(percent(0.8 * 0.9), 72);
    }
}
