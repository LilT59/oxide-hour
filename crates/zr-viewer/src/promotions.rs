//! General's promotions UI (`ControlBar` purchase-science context, `GeneralsExpPoints.cpp`):
//! the rank/experience bar in the control bar (`ControlBar.wnd:ExpBarForeground` +
//! `ControlBarScheme` `ExpBarForegroundImage`), the `GeneralsExpPoints.wnd` purchase-science panel
//! opened from `ControlBar.wnd:ButtonGeneral`, and the left-side special-power shortcut bar
//! (`PlayerTemplate` `SpecialPowerShortcutWinName` / `SpecialPowerShortcutCommandSet`).
//!
//! Sciences are bought with `Command::PurchaseScience`; the panel enables a button when the player
//! lacks the science, meets its prerequisites and can afford it (`TheScienceStore` /
//! `Player::hasScience`). Shortcut buttons dispatch their `CommandButton` (usually a
//! `SPECIAL_POWER_FROM_SHORTCUT`) through the shared `hud::dispatch_button`.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use zr_assets::Vfs;
use zr_sim::{Command, ObjectId};
use zr_ui::command::CommandKind;
use zr_ui::wnd::WndFile;
use zr_ui::{ImageDb, ImageResolution};

use crate::hud::HudData;
use crate::shell::{self, ImageEntry};
use crate::sim_view::SimRes;

/// A decoded `.wnd` image + the dynamic UI state.
#[derive(Resource, Default)]
pub struct PromotionsData {
    exp_wnd: Option<WndFile>,
    shortcut_wnd: Option<WndFile>,
    images: HashMap<String, ImageEntry>,
    textures: HashMap<String, Handle<Image>>,
    failed: HashSet<String>,
    panel_open: bool,
    done: bool,
}

#[derive(Component)]
struct ExpBarImage;
#[derive(Component)]
struct ExpBarFill;
#[derive(Component)]
struct GeneralButton;
#[derive(Component)]
pub struct PromotionsRoot;
#[derive(Component)]
struct RankText;
#[derive(Component)]
struct PointsText;
#[derive(Component)]
struct TitleText;
#[derive(Component)]
struct ExpPanelFill;
#[derive(Component)]
struct ScienceButton {
    science: String,
}
#[derive(Component)]
struct ExitButton;
/// The `GenPowersShortcutBarParent` container.
#[derive(Component)]
struct ShortcutBar;
/// The `ButtonParentN` frame behind shortcut slot `N - 1` (tagged by `shell::spawn_wnd`).
#[derive(Component)]
pub struct ShortcutFrame(pub usize);
#[derive(Component)]
struct ShortcutButton {
    index: usize,
    command: String,
}
#[derive(Component)]
struct ShortcutCooldown(usize);

pub fn add_systems(app: &mut App) {
    app.init_resource::<PromotionsData>();
    app.add_systems(
        Update,
        close_on_escape
            .before(crate::placement::order_click)
            .before(crate::options::toggle_options)
            .run_if(in_state(crate::shell::ShellState::InGame)),
    );
    app.add_systems(
        Update,
        (setup_promotions, promotions_input, refresh_promotions, update_shortcut_bar)
            .chain()
            .after(crate::sim_view::sim_step)
            .run_if(in_state(crate::shell::ShellState::InGame)),
    );
}

impl PromotionsData {
    /// The `GeneralsExpPoints.wnd` purchase panel is showing.
    pub fn panel_open(&self) -> bool {
        self.panel_open
    }
}

/// Esc closes the science panel (retail: `GeneralsExpPoints.wnd` is closed by Esc through
/// `GameWindowManager`'s window input). Ordered after the pause menu's check (which skips Esc while
/// the panel is open) and before placement-cancel / options-close, so those win when active.
pub fn close_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    mut data: ResMut<PromotionsData>,
    mode: Res<crate::placement::OrderMode>,
    options: Res<crate::options::OptionsState>,
    mut panel: Query<&mut Visibility, With<PromotionsRoot>>,
) {
    if keys.just_pressed(KeyCode::Escape) && data.panel_open && mode.0.is_none() && !options.open {
        data.panel_open = false;
        if let Ok(mut vis) = panel.single_mut() {
            *vis = Visibility::Hidden;
        }
    }
}

/// Loads the `.wnd`s and builds the bar, panel and shortcut bar once.
#[allow(clippy::too_many_arguments)]
fn setup_promotions(
    mut commands: Commands,
    mut data: ResMut<PromotionsData>,
    hud: Option<Res<HudData>>,
    sim: Option<NonSend<SimRes>>,
    mut assets: ResMut<Assets<Image>>,
    window: Query<&Window, With<PrimaryWindow>>,
    hud_root: Query<Entity, With<crate::hud::HudRoot>>,
) {
    if data.done {
        return;
    }
    let (Some(hud), Some(sim)) = (hud, sim) else { return };
    data.done = true;
    let win = window.single().ok();
    let text = sim.game.text().clone();

    // The purchase-science layout; the shortcut layout is per `PlayerTemplate`.
    let exp_wnd = load_wnd(&hud.vfs, r"Window\GeneralsExpPoints.wnd");
    let shortcut_name = local_player_template(&sim).map(|t| t.special_power_shortcut_win.clone()).unwrap_or_default();
    let shortcut_wnd = if shortcut_name.is_empty() { None } else { load_wnd(&hud.vfs, &wnd_path(&shortcut_name)) };
    info!(
        "promotions: GeneralsExpPoints.wnd {}, shortcut {shortcut_name:?} {}",
        if exp_wnd.is_some() { "loaded" } else { "missing" },
        if shortcut_wnd.is_some() { "loaded" } else { "missing" }
    );
    if let Some(t) = local_player_template(&sim) {
        info!(
            "promotions: science sets {:?}/{:?}/{:?}, shortcut set {:?} buttons {}",
            t.purchase_science_rank1, t.purchase_science_rank3, t.purchase_science_rank8, t.special_power_shortcut_set, t.special_power_shortcut_buttons
        );
    }

    // Decode every mapped image both layouts reference.
    let mut to_decode: Vec<String> = Vec::new();
    for w in [&exp_wnd, &shortcut_wnd].into_iter().flatten() {
        for win in w.windows.iter().flat_map(|r| r.walk()) {
            for d in &win.draw_data.enabled {
                if let Some(n) = &d.image {
                    to_decode.push(n.clone());
                }
            }
        }
    }
    for name in to_decode {
        if data.images.contains_key(&name) {
            continue;
        }
        if let Some(entry) = decode_mapped(&hud.vfs, &hud.images, &hud.language, &mut assets, &mut data, &name) {
            data.images.insert(name, entry);
        }
    }
    data.exp_wnd = exp_wnd;
    data.shortcut_wnd = shortcut_wnd;

    // Experience bar + general button live in the control bar.
    let Some(root) = hud_root.iter().next() else { return };
    if let Some(rect) = hud.exp_bar_rect {
        let bar = commands
            .spawn((
                ExpBarImage,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(rect.0),
                    top: Val::Px(rect.1),
                    width: Val::Px(rect.2),
                    height: Val::Px(rect.3),
                    overflow: Overflow::clip(),
                    ..default()
                },
                ImageNode::default(),
                ZIndex(210),
                ChildOf(root),
            ))
            .id();
        commands.spawn((
            ExpBarFill,
            Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(0.0), height: Val::Percent(100.0), ..default() },
            BackgroundColor(Color::srgba(0.2, 0.75, 1.0, 0.55)),
            ChildOf(bar),
        ));
    }
    if let Some(rect) = hud.general_button_rect {
        commands.spawn((
            GeneralButton,
            Button,
            Interaction::default(),
            Node { position_type: PositionType::Absolute, left: Val::Px(rect.0), top: Val::Px(rect.1), width: Val::Px(rect.2), height: Val::Px(rect.3), ..default() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.0)),
            ZIndex(220),
            ChildOf(root),
        ));
    }

    // The purchase-science panel (hidden until the general button is clicked).
    let open_science = std::env::args().any(|a| a == "--open-science");
    data.panel_open = open_science;
    let exp_wnd = data.exp_wnd.clone();
    if let Some(wnd) = &exp_wnd {
        let panel = commands
            .spawn((
                PromotionsRoot,
                Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
                if open_science { Visibility::Inherited } else { Visibility::Hidden },
                ZIndex(400),
            ))
            .id();
        shell::render_wnd_background(&mut commands, panel, wnd, &data.images, &text, win, true);
        spawn_panel_text(&mut commands, panel, wnd, win, "StaticTextLevel", RankText);
        spawn_panel_text(&mut commands, panel, wnd, win, "StaticTextRankPointsAvailable", PointsText);
        spawn_panel_text(&mut commands, panel, wnd, win, "StaticTextTitle", TitleText);
        if let Some(rect) = wnd_rect(wnd, win, "ProgressBarExperience") {
            let bar = commands
                .spawn((
                    Node { position_type: PositionType::Absolute, left: Val::Px(rect.0), top: Val::Px(rect.1), width: Val::Px(rect.2), height: Val::Px(rect.3), overflow: Overflow::clip(), ..default() },
                    ZIndex(410),
                    ChildOf(panel),
                ))
                .id();
            commands.spawn((
                ExpPanelFill,
                Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(0.0), height: Val::Percent(100.0), ..default() },
                BackgroundColor(Color::srgba(0.2, 0.75, 1.0, 0.7)),
                ChildOf(bar),
            ));
        }
        // The purchase-science buttons use their `CommandButton`'s `ButtonImage` MappedImage, like
        // the control bar (`ControlBar::drawButton` / `GeneralsExpPoints.cpp`); decode lazily and
        // cache in `data.images` as the rest of the panel does.
        let data = &mut *data;
        spawn_panel_buttons(
            &mut commands,
            panel,
            wnd,
            win,
            &hud,
            &sim,
            &mut data.images,
            &mut data.textures,
            &mut data.failed,
            &mut assets,
        );
        if let Some(rect) = wnd_rect(wnd, win, "ButtonExit") {
            let e = commands
                .spawn((
                    ExitButton,
                    Button,
                    Interaction::default(),
                    Node { position_type: PositionType::Absolute, left: Val::Px(rect.0), top: Val::Px(rect.1), width: Val::Px(rect.2), height: Val::Px(rect.3), border: UiRect::all(Val::Px(1.0)), ..default() },
                    BackgroundColor(Color::srgba(0.14, 0.15, 0.18, 0.95)),
                    BorderColor::all(Color::srgb(0.5, 0.5, 0.55)),
                    ZIndex(420),
                    ChildOf(panel),
                ))
                .id();
            commands.spawn((
                Text::new(sim.game.text().get("GUI:Done").unwrap_or("Done").to_owned()),
                TextFont { font_size: FontSize::Px(13.0), ..default() },
                TextColor(Color::srgb(0.95, 0.95, 0.9)),
                TextLayout { justify: Justify::Center, ..default() },
                Node { width: Val::Percent(100.0), height: Val::Percent(100.0), align_items: AlignItems::Center, justify_content: JustifyContent::Center, ..default() },
                ChildOf(e),
            ));
        }
    }

    // The special-power shortcut bar.
    let shortcut_wnd = data.shortcut_wnd.clone();
    if let Some(wnd) = &shortcut_wnd {
        // Hidden until a shortcut power is available (`ControlBar::updateSpecialPowerShortcut`);
        // the full-screen container never blocks the world (`ui_block`).
        let bar = commands
            .spawn((
                ShortcutBar,
                bevy::picking::Pickable::IGNORE,
                Visibility::Hidden,
                Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
                ZIndex(150),
            ))
            .id();
        shell::render_wnd_background(&mut commands, bar, wnd, &data.images, &text, win, true);
        spawn_shortcut_buttons(&mut commands, bar, wnd, win, &hud, &sim, &mut data, &mut assets);
    }
}

/// Rebuilds the science + shortcut buttons from the local player's `PlayerTemplate`.
#[allow(clippy::too_many_arguments)]
fn spawn_panel_buttons(
    commands: &mut Commands,
    panel: Entity,
    wnd: &WndFile,
    win: Option<&Window>,
    hud: &HudData,
    sim: &SimRes,
    images: &mut HashMap<String, ImageEntry>,
    textures: &mut HashMap<String, Handle<Image>>,
    failed: &mut HashSet<String>,
    assets: &mut Assets<Image>,
) {
    let Some(template) = local_player_template(sim) else { return };
    let sets = [
        (template.purchase_science_rank1.as_str(), "ButtonRank1Number"),
        (template.purchase_science_rank3.as_str(), "ButtonRank3Number"),
        (template.purchase_science_rank8.as_str(), "ButtonRank8Number"),
    ];
    for (set_name, prefix) in sets {
        let Some(set) = hud.commands.set(set_name) else { continue };
        for (i, slot) in set.slots.iter().enumerate() {
            let Some(btn) = slot.as_deref().and_then(|n| hud.commands.button(n)) else { continue };
            if btn.command != CommandKind::PurchaseScience || btn.science.is_empty() {
                continue;
            }
            let name = format!("{prefix}{i}");
            let Some(rect) = wnd_rect(wnd, win, &name) else { continue };
            // `setControlCommand` always draws the command button's `ButtonImage`; a button with no
            // resolvable image contributes nothing (retail never draws its own dark placeholder).
            let Some(image) = btn.button_image.as_deref().filter(|n| !n.is_empty()) else { continue };
            if !images.contains_key(image) {
                if let Some(entry) = decode_mapped_cached(&hud.vfs, &hud.images, &hud.language, assets, textures, failed, image) {
                    images.insert(image.to_owned(), entry);
                }
            }
            let Some(entry) = images.get(image) else { continue };
            commands.spawn((
                ScienceButton { science: btn.science[0].clone() },
                Button,
                Interaction::default(),
                Node { position_type: PositionType::Absolute, left: Val::Px(rect.0), top: Val::Px(rect.1), width: Val::Px(rect.2), height: Val::Px(rect.3), border: UiRect::all(Val::Px(1.0)), ..default() },
                ImageNode { image: entry.handle.clone(), rect: entry.rect, image_mode: NodeImageMode::Stretch, ..default() },
                BackgroundColor(Color::srgba(0.14, 0.15, 0.18, 0.9)),
                BorderColor::all(Color::srgb(0.5, 0.5, 0.55)),
                ZIndex(415),
                ChildOf(panel),
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_shortcut_buttons(
    commands: &mut Commands,
    bar: Entity,
    wnd: &WndFile,
    win: Option<&Window>,
    hud: &HudData,
    sim: &SimRes,
    data: &mut PromotionsData,
    assets: &mut Assets<Image>,
) {
    let Some(template) = local_player_template(sim) else { return };
    let Some(set) = hud.commands.set(&template.special_power_shortcut_set) else { return };
    let count = template.special_power_shortcut_buttons as usize;
    for i in 0..count {
        let Some(btn) = set.slot(i).and_then(|n| hud.commands.button(n)) else { continue };
        let Some(rect) = wnd_rect(wnd, win, &format!("ButtonCommand{}", i + 1)) else { continue };
        // `setControlCommand` draws the command button's `ButtonImage` (`GadgetButtonSetEnabledImage`).
        let image = btn
            .button_image
            .as_deref()
            .filter(|n| !n.is_empty())
            .and_then(|n| decode_mapped_cached(&hud.vfs, &hud.images, &hud.language, assets, &mut data.textures, &mut data.failed, n))
            .map(|entry| ImageNode { image: entry.handle, rect: entry.rect, image_mode: NodeImageMode::Stretch, ..default() });
        let e = commands
            .spawn((
                ShortcutButton { index: i, command: btn.name.clone() },
                Visibility::Hidden,
                Button,
                Interaction::default(),
                Node { position_type: PositionType::Absolute, left: Val::Px(rect.0), top: Val::Px(rect.1), width: Val::Px(rect.2), height: Val::Px(rect.3), border: UiRect::all(Val::Px(1.0)), ..default() },
                BackgroundColor(Color::srgba(0.14, 0.15, 0.18, 0.9)),
                BorderColor::all(Color::srgb(0.5, 0.5, 0.55)),
                ZIndex(160),
                ChildOf(bar),
            ))
            .id();
        if let Some(image) = image {
            commands.entity(e).insert(image);
        }
        commands.spawn((
            ShortcutCooldown(i),
            Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(0.0), ..default() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.65)),
            Visibility::Hidden,
            ChildOf(e),
        ));
    }
}

/// Handles the general button, science purchases, exit and shortcut activation.
#[allow(clippy::too_many_arguments)]
fn promotions_input(
    mut data: ResMut<PromotionsData>,
    hud: Option<Res<HudData>>,
    sim: Option<NonSendMut<SimRes>>,
    mut mode: ResMut<crate::placement::OrderMode>,
    mut audio: ResMut<crate::audio::AudioQueue>,
    general: Query<&Interaction, (With<GeneralButton>, Changed<Interaction>)>,
    exit: Query<&Interaction, (With<ExitButton>, Changed<Interaction>)>,
    science: Query<(&Interaction, &ScienceButton), Changed<Interaction>>,
    shortcut: Query<(&Interaction, &ShortcutButton), Changed<Interaction>>,
    mut panel: Query<&mut Visibility, With<PromotionsRoot>>,
) {
    let (Some(hud), Some(mut sim_guard)) = (hud, sim) else { return };
    let sim: &mut SimRes = &mut sim_guard;

    if general.iter().any(|i| *i == Interaction::Pressed) {
        data.panel_open = !data.panel_open;
        if let Ok(mut vis) = panel.single_mut() {
            *vis = if data.panel_open { Visibility::Inherited } else { Visibility::Hidden };
        }
        return;
    }
    if exit.iter().any(|i| *i == Interaction::Pressed) {
        data.panel_open = false;
        if let Ok(mut vis) = panel.single_mut() {
            *vis = Visibility::Hidden;
        }
        return;
    }
    let local = sim.local;
    for (interaction, btn) in &science {
        if *interaction != Interaction::Pressed {
            continue;
        }
        sim.game.submit(local, Command::PurchaseScience { science: btn.science.clone() });
        audio.push("GUIClick", None, None);
        info!("promotions: purchase science {}", btn.science);
    }
    for (interaction, btn) in &shortcut {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some(button) = hud.commands.button(&btn.command) else { continue };
        let source = shortcut_source(sim, button.special_power.as_deref());
        if let Some(source) = source {
            let ids = vec![source];
            crate::hud::dispatch_button(sim, &mut mode, source, &ids, button, None, &mut audio);
        }
        let _ = btn.index;
    }
}

/// Updates the exp bar, panel readouts and shortcut cooldown wipes.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn refresh_promotions(
    mut data: ResMut<PromotionsData>,
    hud: Option<Res<HudData>>,
    sim: Option<NonSend<SimRes>>,
    mut assets: ResMut<Assets<Image>>,
    mut fills: Query<(&mut Node, Option<&ExpBarFill>, Option<&ExpPanelFill>, Option<&ShortcutCooldown>, Option<&mut Visibility>)>,
    buttons: Query<&ShortcutButton>,
    mut bar_images: Query<&mut ImageNode, With<ExpBarImage>>,
    mut texts: Query<(&mut Text, Option<&RankText>, Option<&PointsText>, Option<&TitleText>)>,
) {
    let Some(sim) = sim else { return };
    let Some(hud) = hud else { return };
    let local = sim.local;
    let (down, up) = sim.game.logic().player_rank_band(local);
    let skill = sim.game.logic().player_skill_points(local);
    let progress = if up > down && up != u32::MAX {
        ((skill.saturating_sub(down)) as f32 / (up - down) as f32).clamp(0.0, 1.0)
    } else {
        1.0
    };

    // The scheme's `ExpBarForegroundImage` is the bar's background (decoded lazily).
    if let Some(name) = &hud.exp_bar_image {
        if let Ok(mut img) = bar_images.single_mut() {
            if img.image == Handle::default() {
                if let Some(entry) = decode_mapped(&hud.vfs, &hud.images, &hud.language, &mut assets, &mut data, name) {
                    img.image = entry.handle;
                    img.rect = entry.rect;
                }
            }
        }
    }

    for (mut node, exp, panel, cooldown, vis) in &mut fills {
        if exp.is_some() || panel.is_some() {
            node.width = Val::Percent(progress * 100.0);
        }
        if let Some(ShortcutCooldown(i)) = cooldown {
            // The slot's command is the compacted one (`update_shortcut_bar`), not the raw command
            // set index.
            let power = buttons
                .iter()
                .find(|b| b.index == *i)
                .and_then(|b| hud.commands.button(&b.command))
                .and_then(|btn| btn.special_power.clone());
            let show = match power.and_then(|p| shortcut_source(&sim, Some(&p)).map(|s| (s, p))) {
                Some((source, power)) => {
                    let ready = sim.game.logic().special_power_ready_at(source, &power);
                    let total = sim.game.logic().special_power_reload(&power).max(1);
                    let remaining = ready.saturating_sub(sim.game.frame());
                    if remaining > 0 {
                        node.height = Val::Percent((remaining as f32 / total as f32) * 100.0);
                    }
                    remaining > 0
                }
                None => false,
            };
            if let Some(mut v) = vis {
                *v = if show { Visibility::Inherited } else { Visibility::Hidden };
            }
        }
    }

    let rank = sim.game.logic().player_rank_level(local);
    let points = sim.game.logic().player_science_purchase_points(local);
    for (mut text, rank_m, points_m, title_m) in &mut texts {
        if rank_m.is_some() {
            **text = sim.game.text().get("SCIENCE:Rank").map_or_else(|| format!("Rank {rank}"), |f| f.replace("%d", &rank.to_string()));
        } else if points_m.is_some() {
            **text = points.to_string();
        } else if title_m.is_some() {
            let key = format!("SCIENCE:Rank{rank}");
            **text = sim.game.text().get(&key).map(str::to_owned).unwrap_or_default();
        }
    }
}

/// Whether shortcut button `btn` is shown for the local player
/// (`ControlBar::populateSpecialPowerShortcut` / `updateSpecialPowerShortcut`): a button that needs
/// an upgrade or the power's science, or whose power no live object has, stays hidden, and a
/// select-all-units-of-type button needs a live unit of that type.
fn shortcut_button_shown(sim: &SimRes, btn: &zr_ui::command::CommandButton) -> bool {
    use zr_ui::command::CommandKind;
    use zr_ui::command_option;
    let local = sim.local;
    let logic = sim.game.logic();
    if btn.options & command_option::NEED_UPGRADE != 0 {
        if let Some(up) = &btn.upgrade {
            if !logic.player_upgrades(local).any(|u| u.eq_ignore_ascii_case(up)) {
                return false;
            }
        }
    }
    let live = |o: &zr_sim::Object| o.player == local && !o.is_dead() && !o.under_construction;
    if btn.options & command_option::NEED_SPECIAL_POWER_SCIENCE != 0 {
        let Some(power) = btn.special_power.as_deref() else { return false };
        if !logic.objects().any(|o| live(o) && o.template.special_powers.iter().any(|p| p.power == power)) {
            return false;
        }
        if let Some(science) = sim.game.data().templates().special_power(power).and_then(|p| p.required_science.as_deref()) {
            if !logic.player_has_science(local, science) {
                return false;
            }
        }
    }
    if btn.command == CommandKind::SelectAllUnitsOfType {
        let Some(thing) = btn.object.as_deref() else { return false };
        return logic.objects().any(|o| live(o) && o.template.name.eq_ignore_ascii_case(thing));
    }
    true
}

/// Shows the shortcut bar only while at least one shortcut button is available, and hides the
/// buttons / frames of the unavailable slots (retail hides both, `populateSpecialPowerShortcut`).
///
/// Retail packs the available commands into the first slots in command-set order (`currentButton`
/// is a running counter, not the set index) and calls `setControlCommand` on that window, so the
/// command and its `ButtonImage` move with the slot. We mirror that: button slot `k` gets the
/// `k`-th available command.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_shortcut_bar(
    mut commands: Commands,
    hud: Option<Res<HudData>>,
    sim: Option<NonSend<SimRes>>,
    mut data: ResMut<PromotionsData>,
    mut assets: ResMut<Assets<Image>>,
    mut bar: Query<&mut Visibility, (With<ShortcutBar>, Without<ShortcutButton>, Without<ShortcutFrame>)>,
    mut buttons: Query<(Entity, &mut ShortcutButton, &mut Visibility), (Without<ShortcutBar>, Without<ShortcutFrame>)>,
    mut frames: Query<(&ShortcutFrame, &mut Visibility), (Without<ShortcutBar>, Without<ShortcutButton>)>,
) {
    let (Some(hud), Some(sim)) = (hud, sim) else { return };
    let Ok(mut bar_vis) = bar.single_mut() else { return };
    // The available commands, in command-set order.
    let available: Vec<&zr_ui::command::CommandButton> = local_player_template(&sim)
        .and_then(|t| hud.commands.set(&t.special_power_shortcut_set))
        .map(|set| {
            set.slots
                .iter()
                .flatten()
                .filter_map(|n| hud.commands.button(n))
                .filter(|c| shortcut_button_shown(&sim, c))
                .collect()
        })
        .unwrap_or_default();

    // Reborrow so the texture / failure caches can be split (disjoint field borrows).
    let data = &mut *data;
    let mut shown = [false; 16];
    for (e, mut b, mut vis) in &mut buttons {
        let target = available.get(b.index).copied();
        if let Some(btn) = target {
            if b.command != btn.name {
                b.command = btn.name.clone();
                // `setControlCommand` -> `GadgetButtonSetEnabledImage`.
                let node = btn
                    .button_image
                    .as_deref()
                    .filter(|n| !n.is_empty())
                    .and_then(|n| decode_mapped_cached(&hud.vfs, &hud.images, &hud.language, &mut assets, &mut data.textures, &mut data.failed, n))
                    .map(|entry| ImageNode { image: entry.handle, rect: entry.rect, image_mode: NodeImageMode::Stretch, ..default() });
                match node {
                    Some(node) => {
                        commands.entity(e).insert(node);
                    }
                    None => {
                        commands.entity(e).remove::<ImageNode>();
                    }
                }
            }
        }
        let on = target.is_some();
        if let Some(s) = shown.get_mut(b.index) {
            *s = on;
        }
        vis.set_if_neq(if on { Visibility::Inherited } else { Visibility::Hidden });
    }
    for (f, mut vis) in &mut frames {
        let on = shown.get(f.0).copied().unwrap_or(false);
        vis.set_if_neq(if on { Visibility::Inherited } else { Visibility::Hidden });
    }
    bar_vis.set_if_neq(if available.is_empty() { Visibility::Hidden } else { Visibility::Inherited });
}

/// The local player's `PlayerTemplate`.
fn local_player_template(sim: &SimRes) -> Option<&zr_game::PlayerTemplate> {
    let faction = sim.game.player(sim.local)?.faction.clone();
    sim.game.data().player_template(&faction)
}

/// An owned object that can fire `power` (selected first, else the local starting building).
fn shortcut_source(sim: &SimRes, power: Option<&str>) -> Option<ObjectId> {
    let _ = power;
    if let Some(&id) = sim.selected.first() {
        return Some(id);
    }
    sim.game.spawned().iter().find(|s| s.player == sim.local).map(|s| s.id)
}

fn spawn_panel_text(commands: &mut Commands, panel: Entity, wnd: &WndFile, win: Option<&Window>, name: &str, marker: impl Component) {
    let Some(rect) = wnd_rect(wnd, win, name) else { return };
    commands.spawn((
        marker,
        Text::new(""),
        TextFont { font_size: FontSize::Px((rect.3 * 0.6).clamp(10.0, 22.0)), ..default() },
        TextColor(Color::srgb(0.95, 0.95, 0.85)),
        TextLayout { justify: Justify::Center, ..default() },
        Node { position_type: PositionType::Absolute, left: Val::Px(rect.0), top: Val::Px(rect.1), width: Val::Px(rect.2), height: Val::Px(rect.3), align_items: AlignItems::Center, justify_content: JustifyContent::Center, ..default() },
        ZIndex(415),
        ChildOf(panel),
    ));
}

/// A `.wnd` window's scaled rect.
fn wnd_rect(wnd: &WndFile, win: Option<&Window>, name: &str) -> Option<(f32, f32, f32, f32)> {
    let (sx, sy) = shell::wnd_scale(wnd, win);
    wnd.windows
        .iter()
        .flat_map(|r| r.walk())
        .find(|w| w.name.as_deref().is_some_and(|n| n.ends_with(name)))
        .map(|w| shell::scaled(w.rect, sx, sy))
}

fn load_wnd(vfs: &Vfs, path: &str) -> Option<WndFile> {
    vfs.read(path).and_then(|b| zr_ui::parse(&b).ok())
}

/// `SpecialPowerShortcutWinName` is a bare `.wnd` name or a `Window\...` path.
fn wnd_path(name: &str) -> String {
    if name.contains('\\') {
        name.to_owned()
    } else {
        format!(r"Window\{name}")
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_mapped(
    vfs: &Vfs,
    images: &ImageDb,
    language: &str,
    assets: &mut Assets<Image>,
    data: &mut PromotionsData,
    name: &str,
) -> Option<ImageEntry> {
    decode_mapped_cached(vfs, images, language, assets, &mut data.textures, &mut data.failed, name)
}

/// [`decode_mapped`] over the cache maps directly, so call sites that must not borrow all of
/// `PromotionsData` (the science panel borrows `exp_wnd`) can still decode `CommandButton`
/// `ButtonImage`s.
#[allow(clippy::too_many_arguments)]
fn decode_mapped_cached(
    vfs: &Vfs,
    images: &ImageDb,
    language: &str,
    assets: &mut Assets<Image>,
    textures: &mut HashMap<String, Handle<Image>>,
    failed: &mut HashSet<String>,
    name: &str,
) -> Option<ImageEntry> {
    // The `MappedImage` sub-rect is a property of the image definition, not the decoded texture, so
    // recompute it on a cache hit too: a cached handle alone would draw the whole atlas page
    // (this is what made the Spy Satellite shortcut button render the full image atlas).
    let rect = images.get(name).map(|i| Rect::new(i.left as f32, i.top as f32, i.right as f32, i.bottom as f32));
    if let Some(h) = textures.get(name) {
        return Some(ImageEntry { handle: h.clone(), rect });
    }
    if failed.contains(name) {
        return None;
    }
    let exists = |p: &str| vfs.read_archived(p).is_some();
    let path = match images.resolve(name, language, &exists) {
        ImageResolution::Found { path, .. } => path,
        _ => {
            failed.insert(name.to_owned());
            return None;
        }
    };
    match zr_w3d::load_texture(vfs, &path) {
        Ok(rgba) => {
            let handle = assets.add(crate::convert::texture_image(&rgba, crate::convert::TextureUse::Ui));
            textures.insert(name.to_owned(), handle.clone());
            Some(ImageEntry { handle, rect })
        }
        Err(_) => {
            failed.insert(name.to_owned());
            None
        }
    }
}
