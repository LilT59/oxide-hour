//! In-game control bar: lays out `Window\ControlBar.wnd` with `bevy_ui`, scaled from the file's
//! `CREATIONRESOLUTION` to the window like `GameWindowManagerScript.cpp` does, draws the metallic
//! frame from the player's `ControlBarScheme` (`ControlBarScheme.cpp`), and populates the 14
//! `ControlBar.wnd:ButtonCommandNN` slots from the selected unit's `CommandSet`
//! (`ControlBar.cpp` / `CommandButton.ini` / `CommandSet.ini`).
//!
//! Z-order mirrors the engine's draw order: the scheme background (layers 5..3) sits under the
//! `.wnd` panels, the scheme foreground (layers 2..0) over them; command buttons are raised above
//! the static panels. Image buttons show only their cameo (the engine clears the text and uses the
//! `TextLabel`/`DescriptLabel` as a tooltip — `ControlBar::setControlCommand`), so labels are drawn
//! only on hover. Money/power use the sim's player money and energy tally.
//!
//! `--select-first-structure` (debug) selects the local player's starting building so a screenshot
//! shows its command set without interactive input. `UNIT_BUILD` clicks submit `Command::QueueUnit`.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use zr_assets::Vfs;
use zr_sim::{Command, Coord3D, ObjectId, PlayerIndex};

use crate::camera::Interactive;
use crate::cmd_avail::{self, CommandAvailability};
use crate::convert::TextureUse;
use zr_ui::command::{CommandDb, CommandKind};
use zr_ui::wnd::{status, style};
use zr_ui::{ImageDb, Rgba, SchemeDb, SchemeImage};

use crate::sim_view::SimRes;

mod power_bar;

/// Context windows the engine only shows via `ControlBar::switchToContext` (they are not
/// `HIDDEN` in the `.wnd` but start hidden). The command context is the only one we draw.
const RUNTIME_CONTEXT_WINDOWS: &[&str] = &[
    "ProductionQueueWindow",
    "ObserverPlayerListWindow",
    "ObserverPlayerInfoWindow",
    "OCLTimerWindow",
    "BeaconWindow",
    // Command-mode cursor overlay, positioned at the mouse at runtime.
    "WinUAttack",
];

/// Gadget styles whose `.wnd` draw color is an image tint, not a background fill.
const BUTTON_STYLES: u32 = style::PUSHBUTTON
    | style::RADIOBUTTON
    | style::CHECKBOX
    | style::VERTSLIDER
    | style::HORZSLIDER
    | style::SCROLLLISTBOX
    | style::ENTRYFIELD
    | style::COMBOBOX;

// UI stacking. The scheme background covers the `.wnd` parent placeholder; the scheme foreground
// sits above the panels, and the readouts/tooltip above everything.
const Z_BAR_BASE: i32 = 0;
const Z_SCHEME_BG: i32 = 10;
const Z_WND_PANEL: i32 = 20;
const Z_SLOT: i32 = 50;
const Z_SCHEME_FG: i32 = 100;
const Z_READOUT: i32 = 200;
const Z_TOOLTIP: i32 = 300;

/// `ControlBar.wnd:ButtonCommand01` .. `ButtonCommand14`.
const SLOT_COUNT: usize = 14;

#[derive(Resource)]
pub struct HudData {
    pub(crate) vfs: Vfs,
    /// Registry language (`Data\<language>`), for the localized art directory.
    pub(crate) language: String,
    pub(crate) ini: zr_ini::IniDb,
    pub(crate) images: ImageDb,
    pub(crate) commands: CommandDb,
    schemes: SchemeDb,
    /// Mapped-image name -> decoded Bevy texture, filled lazily.
    pub(crate) textures: HashMap<String, Handle<Image>>,
    pub(crate) failed: HashSet<String>,
    /// Window size in pixels, for scaling scheme images.
    pub(crate) win: (f32, f32),
    /// `ControlBar.wnd:LeftHUD` / `RightHUD` rects, scaled to the window (radar / portrait).
    pub(crate) left_hud_rect: Option<(f32, f32, f32, f32)>,
    /// `ControlBar.wnd:ExpBarForeground` (rank/experience bar) and `ButtonGeneral` (opens the
    /// purchase-science panel) rects, scaled to the window.
    pub(crate) exp_bar_rect: Option<(f32, f32, f32, f32)>,
    pub(crate) general_button_rect: Option<(f32, f32, f32, f32)>,
    /// `ControlBarScheme` `ExpBarForegroundImage` for the current side.
    pub(crate) exp_bar_image: Option<String>,
    /// `ControlBarScheme` command-button border colours for the local side.
    pub(crate) borders: BorderPalette,
    /// Player radar colours, by player index.
    pub(crate) colors: std::collections::BTreeMap<u8, [u8; 3]>,
    /// Side the current scheme layer entities were built for.
    scheme_side: Option<String>,
    scheme_entities: Vec<Entity>,
    debug_select_structure: bool,
    /// `--select-first-unit` (debug) selects the local player's first starting unit.
    debug_select_unit: bool,
    /// `--queue <template>`: queue one unit at the selected factory (screenshots).
    debug_queue: Option<String>,
    /// `--fire-power <power>`: fire a special power at the selected unit (screenshots).
    debug_power: Option<String>,
    /// `--cooldown-demo`: force the cooldown wipe to show at half (screenshots).
    debug_cooldown: bool,
    /// `--hover-button N`: force the tooltip for slot `N` (0-based) for screenshots.
    debug_hover: Option<usize>,
    applied_debug_select: bool,
}

#[derive(Component)]
pub(crate) struct HudRoot;
#[derive(Component)]
pub(crate) struct CommandSlot(usize);
#[derive(Component)]
struct CommandIcon(usize);
/// `ControlBar.wnd:CommandWindow` root (hidden while the under-construction card is shown).
#[derive(Component)]
pub(crate) struct CommandWindowRoot;
/// `ControlBar.wnd:UnderConstructionWindow` root (`ControlBar::populateUnderConstruction`).
#[derive(Component)]
pub(crate) struct UnderConstructionRoot;
/// `ControlBar.wnd:UnderConstructionDesc` static text (`updateConstructionTextDisplay`).
#[derive(Component)]
pub(crate) struct UnderConstructionText;
/// `ControlBar.wnd:ButtonCancelConstruction` (`Command_CancelConstruction`).
#[derive(Component)]
pub(crate) struct CancelConstructionButton;
/// The cameo image inside `ButtonCancelConstruction` (`Command_CancelConstruction`'s `ButtonImage`).
#[derive(Component)]
struct CancelIcon;
#[derive(Component)]
struct SchemeLayer;
#[derive(Component)]
pub(crate) struct MoneyText;
#[derive(Component)]
struct HudTooltip;
/// Markers for the optional HUD elements (kept in one component to stay within Bevy's query arity).
#[derive(Component)]
enum HudMarker {
    Portrait,
    /// `ControlBar.wnd:WinUnitSelected`: the blue cell grid behind the portrait, shown with it.
    UnitGrid,
    /// `ControlBar.wnd:RightHUD`'s scheme emblem: hidden while a unit's portrait is shown.
    Emblem,
    /// One row of the build queue (0..4); click cancels that entry.
    QueueSlot(usize),
    QueueFill(usize),
    /// Dark clock-wipe over a special-power button while it reloads.
    Cooldown(usize),
}

/// `MAX_BUILD_QUEUE_BUTTONS`: `ControlBar.wnd:ButtonQueue01` .. `ButtonQueue09`.
const QUEUE_ROWS: usize = 9;

/// `ControlBar.wnd` windows that `ControlBarScheme::init` moves and resizes from the scheme's
/// `*UL` / `*LR` corners (name, UL key, LR key).
const SCHEME_OVERRIDES: &[(&str, &str, &str)] = &[
    ("ControlBar.wnd:ButtonGeneral", "GeneralUL", "GeneralLR"),
    ("ControlBar.wnd:ButtonLarge", "MinMaxUL", "MinMaxLR"),
    ("ControlBar.wnd:WinUAttack", "UAttackUL", "UAttackLR"),
    ("ControlBar.wnd:ButtonOptions", "OptionsUL", "OptionsLR"),
    ("ControlBar.wnd:ButtonIdleWorker", "WorkerUL", "WorkerLR"),
    ("ControlBar.wnd:ButtonPlaceBeacon", "BeaconUL", "BeaconLR"),
    ("ControlBar.wnd:PopupCommunicator", "ChatUL", "ChatLR"),
    ("ControlBar.wnd:MoneyDisplay", "MoneyUL", "MoneyLR"),
    ("ControlBar.wnd:PowerWindow", "PowerBarUL", "PowerBarLR"),
];

/// The tab buttons and their scheme enabled images (`ControlBarScheme::init`). `ButtonPlaceBeacon`
/// is left out: `ControlBar::setControlBarSchemeByPlayer` hides it outside LAN/Internet games.
const SCHEME_TAB_IMAGES: &[(&str, &str)] = &[
    ("ControlBar.wnd:ButtonOptions", "OptionsButtonEnable"),
    ("ControlBar.wnd:ButtonIdleWorker", "IdleWorkerButtonEnable"),
    ("ControlBar.wnd:PopupCommunicator", "BuddyButtonEnable"),
    // The up/down toggle in its default (not low) stage: `ControlBar::setUpDownImages` puts the
    // `ToggleButtonDown*` images on `ButtonLarge` (the green arrow left of the star).
    ("ControlBar.wnd:ButtonLarge", "ToggleButtonDownOn"),
];

/// The general (star) button, drawn with its scheme images and the purchase flash
/// (`ControlBar::getStarImage`, `ControlBarScheme::init` sets `GeneralButtonEnable` / `Hightlited`).
#[derive(Component)]
struct GeneralStar {
    enable: (Handle<Image>, Option<Rect>),
    hilite: (Handle<Image>, Option<Rect>),
}

/// `ControlBar::getStarImage` state: `m_genStarFlash`, `m_lastFlashedAtPointValue` and the blink
/// accumulator. The flash starts armed (`ControlBar::init`, `m_genStarFlash = TRUE`).
struct StarFlash {
    points: Option<i32>,
    last_flashed_at: i32,
    flash: bool,
    acc: f32,
}

impl Default for StarFlash {
    fn default() -> Self {
        Self { points: None, last_flashed_at: -1, flash: true, acc: 0.0 }
    }
}

/// A `ControlBarScheme` image-name field (e.g. `OptionsButtonEnable`), if the scheme sets it.
fn scheme_image(ini: &zr_ini::IniDb, scheme_name: &str, key: &str) -> Option<String> {
    ini.resolved("ControlBarScheme", scheme_name)?.value(key).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// Scheme-placed windows: name -> `(x, y, w, h)` in window pixels.
type Overrides = HashMap<String, (f32, f32, f32, f32)>;

/// `ControlBarScheme` `ButtonBorder{Build,Action,Upgrade,System}Color`, by `ButtonBorderType`
/// (`ControlBar::setCommandBarBorder`).
#[derive(Clone, Copy, Default)]
pub(crate) struct BorderPalette {
    build: Option<Color>,
    action: Option<Color>,
    upgrade: Option<Color>,
    system: Option<Color>,
}

impl BorderPalette {
    fn get(&self, border: zr_ui::BorderType) -> Option<Color> {
        match border {
            zr_ui::BorderType::Build => self.build,
            zr_ui::BorderType::Action => self.action,
            zr_ui::BorderType::Upgrade => self.upgrade,
            zr_ui::BorderType::System => self.system,
            zr_ui::BorderType::None => None,
        }
    }
}

/// Parses `X:n Y:n` (`INI::parseICoord2D`).
fn scheme_xy(v: &str) -> Option<(i32, i32)> {
    let n = zr_ui::value::ints(v);
    Some((*n.first()?, *n.get(1)?))
}

/// Parses `R:n G:n B:n [A:n]` (`INI::parseColorInt`).
fn scheme_color(v: &str) -> Color {
    let n = zr_ui::value::ints(v);
    let c = |i: usize, d: i32| n.get(i).copied().unwrap_or(d).clamp(0, 255) as f32 / 255.0;
    Color::srgba(c(0, 0), c(1, 0), c(2, 0), c(3, 255))
}

/// `ControlBarScheme::init`: the `*UL` / `*LR` corners are scaled by the scheme's own
/// `ScreenCreationRes` multiplier (`resMultiplier`), and the window size is the corner span.
fn scheme_overrides(ini: &zr_ini::IniDb, scheme: &zr_ui::ControlBarScheme, win: (f32, f32)) -> Overrides {
    let mut out = Overrides::new();
    let (rw, rh) = scheme.screen_creation_res;
    let Some(block) = ini.resolved("ControlBarScheme", &scheme.name) else { return out };
    if rw <= 0 || rh <= 0 {
        return out;
    }
    let (mx, my) = (win.0 / rw as f32, win.1 / rh as f32);
    for (name, ul, lr) in SCHEME_OVERRIDES {
        let (Some(a), Some(b)) = (block.value(ul).and_then(scheme_xy), block.value(lr).and_then(scheme_xy)) else { continue };
        out.insert(
            (*name).to_owned(),
            (a.0 as f32 * mx, a.1 as f32 * my, (b.0 - a.0) as f32 * mx, (b.1 - a.1) as f32 * my),
        );
    }
    out
}

/// The scheme's border palette (`ControlBarSchemeManager` `ButtonBorder*Color` fields).
fn scheme_borders(ini: &zr_ini::IniDb, scheme: &zr_ui::ControlBarScheme) -> BorderPalette {
    let Some(block) = ini.resolved("ControlBarScheme", &scheme.name) else { return BorderPalette::default() };
    let c = |k: &str| block.value(k).map(scheme_color);
    BorderPalette {
        build: c("ButtonBorderBuildColor"),
        action: c("ButtonBorderActionColor"),
        upgrade: c("ButtonBorderUpgradeColor"),
        system: c("ButtonBorderSystemColor"),
    }
}

/// Rect of a `.wnd` window: the scheme's placement when it overrides it, else the scaled `.wnd` rect.
fn window_rect(overrides: &Overrides, w: &zr_ui::Window, sx: f32, sy: f32) -> (f32, f32, f32, f32) {
    w.name
        .as_deref()
        .and_then(|n| overrides.get(n).copied())
        .unwrap_or_else(|| scaled(w.rect, sx, sy))
}

pub fn add_systems(app: &mut App) {
    app.add_systems(OnEnter(crate::shell::ShellState::InGame), setup_hud.after(crate::sim_view::adopt_sim));
    app.add_systems(
        Update,
        (hud_scheme, hud_refresh, hud_under_construction, hud_clicks).chain().after(crate::sim_view::sim_step),
    );
    app.add_systems(Update, (power_bar::update, hud_general_star).after(crate::sim_view::sim_step));
}

/// `ControlBar::getStarImage` (`ControlBar.cpp:1670`): the general button shows its highlighted star
/// while the local player has science points to spend, blinking once a second (`m_genStarFlash`).
/// Any change in the points re-arms the flash (`onPlayerSciencePurchasePointsChanged`); it stops
/// once the points are spent (`points <= 0`, or below the last flashed value).
fn hud_general_star(sim: Option<NonSend<SimRes>>, time: Res<Time>, mut state: Local<StarFlash>, mut stars: Query<(&GeneralStar, &mut ImageNode)>) {
    let Some(sim) = sim else { return };
    let points = sim.game.logic().player_science_purchase_points(sim.local) as i32;
    if state.points != Some(points) {
        state.points = Some(points);
        state.flash = true;
    }
    if state.last_flashed_at > points || points <= 0 {
        state.flash = false;
    } else {
        state.last_flashed_at = points;
    }
    let hilite = if state.flash {
        state.acc += time.delta_secs();
        while state.acc >= 1.0 {
            state.acc -= 1.0;
        }
        state.acc >= 0.5
    } else {
        false
    };
    for (star, mut img) in &mut stars {
        let (image, rect) = if hilite { &star.hilite } else { &star.enable };
        if img.image != *image {
            img.image = image.clone();
        }
        if img.rect != *rect {
            img.rect = *rect;
        }
    }
}

/// The debug flag is read straight from the process arguments so `main.rs` needs no new field.
fn debug_flag() -> bool {
    std::env::args().any(|a| a == "--select-first-structure")
}

/// `--select-first-unit` (debug screenshot aid, the unit analogue of `--select-first-structure`).
fn debug_unit_flag() -> bool {
    std::env::args().any(|a| a == "--select-first-unit")
}

pub(crate) fn setup_hud(
    mut commands: Commands,
    mut assets: ResMut<Assets<Image>>,
    window: Query<&Window, With<PrimaryWindow>>,
    sim: Option<NonSend<SimRes>>,
) {
    let debug_select_structure = debug_flag();
    let debug_select_unit = debug_unit_flag();
    let Some(install) = zr_assets::Installs::detect().zero_hour else {
        warn!("hud: no Zero Hour install; control bar disabled");
        return;
    };
    let Ok(vfs) = Vfs::zero_hour(&install) else {
        warn!("hud: could not mount Zero Hour VFS; control bar disabled");
        return;
    };
    let ini = zr_ini::load_zero_hour(&vfs).db;
    let Some(bytes) = vfs.read(r"Window\ControlBar.wnd") else {
        warn!("hud: ControlBar.wnd not found; control bar disabled");
        return;
    };
    let Ok(controlbar) = zr_ui::parse(&bytes) else {
        warn!("hud: ControlBar.wnd did not parse; control bar disabled");
        return;
    };

    let language = zr_ini::detect_language(&vfs);
    let images = ImageDb::from_ini(&ini);
    let command_db = CommandDb::from_ini(&ini);
    let schemes = SchemeDb::from_ini(&ini);
    info!(
        "hud: ControlBar.wnd v{} ({} windows), {} mapped images, {} command buttons / {} sets, {} schemes",
        controlbar.file_version,
        controlbar.windows.iter().map(|w| w.walk().count()).sum::<usize>(),
        images.len(),
        command_db.buttons().len(),
        command_db.sets().len(),
        schemes.len()
    );

    let mut textures = HashMap::new();
    let mut failed = HashSet::new();

    // Scale from the file's creation resolution to the window (x and y independently, as the engine).
    let (win_w, win_h) = window.single().map(|w| (w.width(), w.height())).unwrap_or((1600.0, 900.0));
    let (cw, ch) = controlbar
        .windows
        .first()
        .map(|w| w.rect.creation_resolution)
        .filter(|&(x, y)| x > 0 && y > 0)
        .unwrap_or((800, 600));
    let (sx, sy) = (win_w / cw as f32, win_h / ch as f32);

    // The local side's scheme (`setControlBarSchemeByPlayer`): it places some windows and sets the
    // command-button border colours. Without a sim yet, the `Default` scheme is used.
    let side = sim.as_ref().and_then(|s| s.game.player(s.local)).map(|p| p.side.clone()).unwrap_or_default();
    let scheme = schemes.by_side(&side);
    let overrides = scheme.map_or_else(Overrides::new, |s| scheme_overrides(&ini, s, (win_w, win_h)));
    let borders = scheme.map_or_else(BorderPalette::default, |s| scheme_borders(&ini, s));
    let exp_bar_image = scheme.and_then(|s| s.exp_bar_foreground_image.clone());

    let root = commands
        .spawn((
            HudRoot,
            // The full-screen container itself must not block world clicks (`ui_block`).
            bevy::picking::Pickable::IGNORE,
            Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        ))
        .id();

    for w in &controlbar.windows {
        spawn_tree(&mut commands, &mut assets, root, w, (sx, sy), &overrides, &images, &vfs, &language, &mut textures, &mut failed, true);
    }

    // Money readout: the `.wnd` `MoneyDisplay` rect (or the scheme's), font size and enabled text color.
    if let Some(money) = controlbar.windows.iter().flat_map(|r| r.walk()).find(|w| w.name.as_deref() == Some("ControlBar.wnd:MoneyDisplay")) {
        let (x, y, w, h) = window_rect(&overrides, money, sx, sy);
        let size = money.font.as_ref().map_or(12.0, |f| f.size as f32 * sy).clamp(9.0, 22.0);
        let color = money
            .text_color
            .map_or(Color::srgb(0.95, 0.95, 0.85), |c| rgba(c.enabled.color));
        commands.spawn((
            MoneyText,
            Text::new(""),
            TextFont { font_size: FontSize::Px(size), ..default() },
            TextColor(color),
            TextLayout { justify: Justify::Center, ..default() },
            Node { position_type: PositionType::Absolute, left: Val::Px(x), top: Val::Px(y + h * 0.5 - size * 0.6), width: Val::Px(w), ..default() },
            ZIndex(Z_READOUT),
            ChildOf(root),
        ));
    }

    // Power meter over the `.wnd` `PowerWindow` rect (`W3DPowerDraw`, see `power_bar`).
    if let Some(power) = controlbar.windows.iter().flat_map(|r| r.walk()).find(|w| w.name.as_deref() == Some("ControlBar.wnd:PowerWindow")) {
        let rect = window_rect(&overrides, power, sx, sy);
        let mut load = |name: &str| {
            let handle = decode_image(&vfs, &images, &language, &mut assets, &mut textures, &mut failed, name)?;
            let m = images.get(name)?;
            Some(power_bar::Art {
                image: handle,
                rect: Some(Rect::new(m.left as f32, m.top as f32, m.right as f32, m.bottom as f32)),
                w: m.width,
                h: m.height,
            })
        };
        power_bar::spawn(&mut commands, root, rect, Z_READOUT, &mut load);
    }

    // Tooltip panel (hidden until a command button is hovered).
    commands.spawn((
        HudTooltip,
        bevy::picking::Pickable::IGNORE,
        Text::new(""),
        TextFont { font_size: FontSize::Px(14.0), ..default() },
        TextColor(Color::srgb(0.95, 0.95, 0.85)),
        TextLayout { justify: Justify::Center, ..default() },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(30.0),
            top: Val::Percent(58.0),
            width: Val::Percent(40.0),
            padding: UiRect::all(Val::Px(6.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.05, 0.06, 0.08, 0.9)),
        Visibility::Hidden,
        ZIndex(Z_TOOLTIP),
        ChildOf(root),
    ));

    let rect_of = |name: &str| {
        controlbar.windows.iter().flat_map(|r| r.walk()).find(|w| w.name.as_deref() == Some(name)).map(|w| window_rect(&overrides, w, sx, sy))
    };

    // Scheme art on windows the engine fills with images: the right HUD emblem
    // (`ControlBar::updateRightHUDImage`, visible when no unit portrait covers it) and the four
    // tab buttons (`ControlBarScheme::init`: `GadgetButtonSetEnabledImage`).
    if let Some(scheme) = scheme {
        let mut art: Vec<(&str, Option<String>)> = vec![("ControlBar.wnd:RightHUD", scheme.right_hud_image.clone())];
        for (win, key) in SCHEME_TAB_IMAGES {
            art.push((win, scheme_image(&ini, &scheme.name, key)));
        }
        for (win, image) in art {
            let (Some(name), Some((x, y, w, h))) = (image, rect_of(win)) else { continue };
            if let Some(handle) = decode_image(&vfs, &images, &language, &mut assets, &mut textures, &mut failed, &name) {
                let rect = images.get(&name).map(|i| Rect::new(i.left as f32, i.top as f32, i.right as f32, i.bottom as f32));
                let mut emblem = commands.spawn((
                    stretched_with(handle, rect),
                    Node { position_type: PositionType::Absolute, left: Val::Px(x), top: Val::Px(y), width: Val::Px(w), height: Val::Px(h), ..default() },
                    ZIndex(Z_WND_PANEL),
                    ChildOf(root),
                ));
                // `W3DRightHUDDraw` draws the emblem only while no unit is selected (`WIN_STATUS_IMAGE`).
                if win == "ControlBar.wnd:RightHUD" {
                    emblem.insert(HudMarker::Emblem);
                }
            }
        }
    }

    // The general (star) button: `GeneralButtonEnable`, with `GeneralButtonHightlited` for the purchase
    // flash (`ControlBar::getStarImage`, driven by `hud_general_star`).
    if let (Some(scheme), Some((x, y, w, h))) = (scheme, rect_of("ControlBar.wnd:ButtonGeneral")) {
        let mut load = |key: &str| {
            let name = scheme_image(&ini, &scheme.name, key)?;
            let handle = decode_image(&vfs, &images, &language, &mut assets, &mut textures, &mut failed, &name)?;
            let rect = images.get(&name).map(|i| Rect::new(i.left as f32, i.top as f32, i.right as f32, i.bottom as f32));
            Some((handle, rect))
        };
        if let Some(enable) = load("GeneralButtonEnable") {
            let hilite = load("GeneralButtonHightlited").unwrap_or_else(|| enable.clone());
            commands.spawn((
                GeneralStar { enable: enable.clone(), hilite },
                stretched_with(enable.0, enable.1),
                Node { position_type: PositionType::Absolute, left: Val::Px(x), top: Val::Px(y), width: Val::Px(w), height: Val::Px(h), ..default() },
                ZIndex(Z_WND_PANEL),
                ChildOf(root),
            ));
        }
    }

    // The selected unit's portrait (`ThingTemplate::SelectPortrait`) is the `CameoWindow` cameo
    // (`ControlBar::updateRightHUD...`: `m_rightHUDCameoWindow->winSetEnabledImage(0, portrait)`),
    // over the `WinUnitSelected` grid (`W3DCommandBarGridDraw`) that is shown with it.
    let grid_color = scheme.and_then(|s| s.command_bar_border_color).map_or(Color::srgb(0.0, 0.08, 0.49), rgba);
    if let Some((x, y, w, h)) = rect_of("ControlBar.wnd:WinUnitSelected") {
        let grid = commands
            .spawn((
                HudMarker::UnitGrid,
                Node { position_type: PositionType::Absolute, left: Val::Px(x), top: Val::Px(y), width: Val::Px(w), height: Val::Px(h), border: UiRect::all(Val::Px(1.0)), ..default() },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 128.0 / 255.0)),
                BorderColor::all(grid_color),
                Visibility::Hidden,
                ZIndex(Z_WND_PANEL + 1),
                ChildOf(root),
            ))
            .id();
        // `drawLine` at 1/3 and 2/3 of the window, both ways.
        for f in [1.0 / 3.0, 2.0 / 3.0] {
            commands.spawn((
                Node { position_type: PositionType::Absolute, left: Val::Percent(f * 100.0), top: Val::Px(0.0), width: Val::Px(1.0), height: Val::Percent(100.0), ..default() },
                BackgroundColor(grid_color),
                ChildOf(grid),
            ));
            commands.spawn((
                Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Percent(f * 100.0), width: Val::Percent(100.0), height: Val::Px(1.0), ..default() },
                BackgroundColor(grid_color),
                ChildOf(grid),
            ));
        }
    }
    if let Some((x, y, w, h)) = rect_of("ControlBar.wnd:CameoWindow") {
        commands.spawn((
            HudMarker::Portrait,
            stretched(),
            Node { position_type: PositionType::Absolute, left: Val::Px(x), top: Val::Px(y), width: Val::Px(w), height: Val::Px(h), ..default() },
            Visibility::Hidden,
            ZIndex(Z_WND_PANEL + 2),
            ChildOf(root),
        ));
    }

    // Build queue: the nine `ControlBar.wnd:ButtonQueueNN` buttons (`ControlBar::populateBuildQueue`
    // fills `m_queueData[i].control`), each drawn at its own `.wnd` rect.
    for i in 0..QUEUE_ROWS {
        if let Some((x, y, w, h)) = rect_of(&format!("ControlBar.wnd:ButtonQueue{:02}", i + 1)) {
            let slot = commands
                .spawn((
                    HudMarker::QueueSlot(i),
                    Button,
                    Interaction::default(),
                    stretched(),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(x),
                        top: Val::Px(y),
                        width: Val::Px(w),
                        height: Val::Px(h),
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.1, 0.11, 0.13, 0.95)),
                    BorderColor::all(Color::srgb(0.4, 0.4, 0.45)),
                    Visibility::Hidden,
                    ZIndex(Z_WND_PANEL + 2),
                    ChildOf(root),
                ))
                .id();
            commands.spawn((
                HudMarker::QueueFill(i),
                Node { position_type: PositionType::Absolute, left: Val::Px(0.0), bottom: Val::Px(0.0), width: Val::Percent(0.0), height: Val::Px(4.0), ..default() },
                BackgroundColor(Color::srgb(0.2, 0.9, 0.3)),
                ChildOf(slot),
            ));
        }
    }

    commands.insert_resource(HudData {
        vfs,
        language,
        ini,
        images,
        commands: command_db,
        schemes,
        textures,
        failed,
        win: (win_w, win_h),
        left_hud_rect: rect_of("ControlBar.wnd:LeftHUD"),
        exp_bar_rect: rect_of("ControlBar.wnd:ExpBarForeground"),
        general_button_rect: rect_of("ControlBar.wnd:ButtonGeneral"),
        exp_bar_image,
        borders,
        colors: std::collections::BTreeMap::new(),
        scheme_side: None,
        scheme_entities: Vec::new(),
        debug_select_structure,
        debug_select_unit,
        debug_queue: std::env::args().position(|a| a == "--queue").and_then(|i| std::env::args().nth(i + 1)),
        debug_power: std::env::args().position(|a| a == "--fire-power").and_then(|i| std::env::args().nth(i + 1)),
        debug_cooldown: std::env::args().any(|a| a == "--cooldown-demo"),
        debug_hover: std::env::args().position(|a| a == "--hover-button").and_then(|i| std::env::args().nth(i + 1)).and_then(|v| v.parse().ok()),
        applied_debug_select: false,
    });
}

fn scaled(rect: zr_ui::ScreenRect, sx: f32, sy: f32) -> (f32, f32, f32, f32) {
    (
        rect.upper_left.0 as f32 * sx,
        rect.upper_left.1 as f32 * sy,
        rect.width() as f32 * sx,
        rect.height() as f32 * sy,
    )
}

/// `[None]` and `W3DGameWinDefaultDraw` render the window's own draw data. Every other callback
/// (`W3DNoDraw`, the radar/portrait/power/grid draws) is a custom draw we don't implement, so the
/// window's `.wnd` fill is a placeholder the engine never shows.
fn draws_own_data(w: &zr_ui::Window) -> bool {
    match w.draw_callback.as_deref() {
        None => true,
        Some(c) => c.eq_ignore_ascii_case("W3DGameWinDefaultDraw"),
    }
}

/// Spawns a window and its descendants. `ancestor_shown` is false under a hidden or
/// runtime-context window, which mirrors the engine hiding the whole subtree.
#[allow(clippy::too_many_arguments)]
fn spawn_tree(
    commands: &mut Commands,
    assets: &mut Assets<Image>,
    root: Entity,
    w: &zr_ui::Window,
    (sx, sy): (f32, f32),
    overrides: &Overrides,
    images: &ImageDb,
    vfs: &Vfs,
    language: &str,
    textures: &mut HashMap<String, Handle<Image>>,
    failed: &mut HashSet<String>,
    ancestor_shown: bool,
) {
    let name = w.name.as_deref().unwrap_or("");
    let hidden = w.status & status::HIDDEN != 0;
    let runtime_ctx = RUNTIME_CONTEXT_WINDOWS.iter().any(|s| name.ends_with(s));
    // `CommandWindow` and `UnderConstructionWindow` are HIDDEN in the file but shown by
    // `switchToContext` for their respective contexts; both are toggled at runtime.
    let is_command_window = name == "ControlBar.wnd:CommandWindow";
    let is_uc_window = name == "ControlBar.wnd:UnderConstructionWindow";
    // `ButtonPlaceBeacon` is hidden outside LAN/Internet games (`ControlBar::setControlBarSchemeByPlayer`).
    let is_beacon_button = name == "ControlBar.wnd:ButtonPlaceBeacon";
    let shown = ancestor_shown && (!hidden || is_command_window || is_uc_window) && !runtime_ctx && !is_beacon_button;

    let slot = command_slot_index(w.name.as_deref());
    let (x, y, width, height) = window_rect(overrides, w, sx, sy);
    if shown && width > 0.0 && height > 0.0 && (slot.is_some() || is_uc_window || draws_own_data(w)) {
        let node = Node {
            position_type: PositionType::Absolute,
            left: Val::Px(x),
            top: Val::Px(y),
            width: Val::Px(width),
            height: Val::Px(height),
            ..default()
        };
        let button_like = w.style & BUTTON_STYLES != 0;
        // The `.wnd` parent placeholder is the base layer, under the scheme background; command
        // slots sit above the static panels.
        let z = if slot.is_some() {
            Z_SLOT
        } else if name == "ControlBar.wnd:ControlBarParent" {
            Z_BAR_BASE
        } else {
            Z_WND_PANEL
        };
        let mut entity = commands.spawn((node, ZIndex(z)));
        // Command buttons get a neutral background (their `.wnd` color is an image tint, not a fill)
        // and the `ButtonBorderType` outline drawn just outside the button (`drawOpenRect(-1, +2)`,
        // `W3DGadgetPushButtonImageDraw`); its colour is set per slot by `hud_refresh`.
        if slot.is_some() {
            entity.insert((BackgroundColor(Color::srgba(0.14, 0.15, 0.18, 0.94)), Outline::new(Val::Px(1.0), Val::Px(0.0), Color::NONE)));
        } else if let Some(d) = w.draw_data.enabled.first() {
            if d.color.a > 0 && !button_like {
                // The `.wnd` parent's translucent white is only visible through the frame's
                // radar/portrait cut-outs; the shipped bar is the scheme's dark metal. Use a dark
                // base so those cut-outs don't wash the bar out.
                let c = if name == "ControlBar.wnd:ControlBarParent" {
                    Color::srgba(0.05, 0.06, 0.08, 0.97)
                } else {
                    rgba(d.color)
                };
                entity.insert(BackgroundColor(c));
            }
            if let Some(name) = &d.image {
                if let Some(handle) = decode_image(vfs, images, language, assets, textures, failed, name) {
                    let rect = images.get(name).map(|i| Rect::new(i.left as f32, i.top as f32, i.right as f32, i.bottom as f32));
                    entity.insert(stretched_with(handle, rect));
                }
            }
        }
        entity.insert(ChildOf(root));

        // Context roots / the under-construction card's widgets are driven by
        // `hud_under_construction` and `hud_clicks`.
        if is_command_window {
            entity.insert(CommandWindowRoot);
        }
        if is_uc_window {
            entity.insert((UnderConstructionRoot, Visibility::Hidden));
        }
        if name.ends_with("ButtonCancelConstruction") {
            entity.insert((Button, Interaction::default(), CancelConstructionButton));
            // `setControlCommand`: the cancel button shows `Command_CancelConstruction`'s cameo.
            entity.with_children(|p| {
                p.spawn((CancelIcon, stretched(), full_node()));
            });
        }
        if name.ends_with("ControlBar.wnd:UnderConstructionDesc") {
            entity.insert((
                UnderConstructionText,
                Text::new(""),
                TextFont { font_size: FontSize::Px((w.font.as_ref().map_or(14, |f| f.size) as f32) * sy), ..default() },
                TextColor(Color::srgb(0.95, 0.95, 0.9)),
            ));
        }

        if let Some(i) = slot {
            // Keep the grid above the static bar planes.
            entity.insert((CommandSlot(i), Button, Interaction::default()));
            entity.with_children(|p| {
                // The cameo fills the whole button (`W3DGadgetPushButtonImageDraw` draws the enabled
                // image over the window rect).
                p.spawn((CommandIcon(i), stretched(), full_node()));
                p.spawn((
                    HudMarker::Cooldown(i),
                    Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(0.0), ..default() },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.65)),
                    Visibility::Hidden,
                ));
            });
        }
    }

    for c in &w.children {
        spawn_tree(commands, assets, root, c, (sx, sy), overrides, images, vfs, language, textures, failed, shown);
    }
}

/// An absolutely positioned node that fills its parent.
fn full_node() -> Node {
    Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() }
}

/// An `ImageNode` that fills its node like `W3DDisplay::drawImage(x1, y1, x2, y2)`. Bevy's default
/// `NodeImageMode::Auto` aspect-fits the source into the node and centres it, which shifted the
/// scheme frame and cameos off their `.wnd` rects (e.g. the dozer button hung off the centre panel).
fn stretched() -> ImageNode {
    ImageNode { image_mode: NodeImageMode::Stretch, ..default() }
}

fn stretched_with(image: Handle<Image>, rect: Option<Rect>) -> ImageNode {
    ImageNode { image, rect, image_mode: NodeImageMode::Stretch, ..default() }
}

fn rgba(c: Rgba) -> Color {
    Color::srgba(c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0, c.a as f32 / 255.0)
}

fn command_slot_index(name: Option<&str>) -> Option<usize> {
    let name = name?;
    let rest = name.strip_prefix("ControlBar.wnd:ButtonCommand")?;
    let n: usize = rest.parse().ok()?;
    (1..=SLOT_COUNT).contains(&n).then_some(n - 1)
}

/// Resolves a mapped image to a decoded Bevy texture, caching both hits and misses. The engine's
/// search order is used (`Data\<language>\Art\Textures\` then `Art\Textures\`), so localized art
/// loads even when the base folder only has the other page numbers.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_image(
    vfs: &Vfs,
    images: &ImageDb,
    language: &str,
    assets: &mut Assets<Image>,
    textures: &mut HashMap<String, Handle<Image>>,
    failed: &mut HashSet<String>,
    name: &str,
) -> Option<Handle<Image>> {
    if let Some(h) = textures.get(name) {
        return Some(h.clone());
    }
    if failed.contains(name) {
        return None;
    }
    let exists = |p: &str| vfs.read_archived(p).is_some();
    let path = match images.resolve(name, language, &exists) {
        zr_ui::ImageResolution::Found { path, .. } => path,
        _ => {
            failed.insert(name.to_owned());
            return None;
        }
    };
    match zr_w3d::load_texture(vfs, &path) {
        Ok(rgba) => {
            // A shell/UI image (`TextureUse::Ui`): `W3DDisplay::drawImage` loads it with
            // `Get_Texture(filename, MIP_LEVELS_1)` (`W3DDisplay.cpp:2798`) and no filter override.
            let h = assets.add(crate::convert::texture_image(&rgba, TextureUse::Ui));
            textures.insert(name.to_owned(), h.clone());
            // Precompute the disabled-button grayscale copy at load time (the CPU pixels are not
            // kept in the main world once the texture is uploaded, `RenderAssetUsages::RENDER_WORLD`).
            let gray = assets.add(crate::convert::gray_image(&rgba));
            textures.insert(gray_key(name), gray);
            Some(h)
        }
        Err(_) => {
            failed.insert(name.to_owned());
            None
        }
    }
}

/// Synthetic cache key for the grayscale copy of a mapped image (real names never contain `\u{1}`).
fn gray_key(name: &str) -> String {
    format!("{name}\u{1}gray")
}

/// The disabled-button grayscale texture for `name`, decoded on demand
/// (`W3DGadgetPushButtonImageDrawOne` `DRAW_IMAGE_GRAYSCALE`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_gray_image(
    vfs: &Vfs,
    images: &ImageDb,
    language: &str,
    assets: &mut Assets<Image>,
    textures: &mut HashMap<String, Handle<Image>>,
    failed: &mut HashSet<String>,
    name: &str,
) -> Option<Handle<Image>> {
    let key = gray_key(name);
    if let Some(h) = textures.get(&key) {
        return Some(h.clone());
    }
    decode_image(vfs, images, language, assets, textures, failed, name)?;
    textures.get(&key).cloned()
}

/// One command slot resolved for the current selection.
#[derive(Clone)]
pub(crate) struct SlotInfo {
    pub image: Option<String>,
    pub label: Option<String>,
    pub tooltip: Option<String>,
    /// `SpecialPower` name (for the cooldown wipe).
    pub power: Option<String>,
    /// `ControlBar::getCommandAvailability` result for the selected object.
    pub availability: CommandAvailability,
    /// `TOOLTIP:Cost` line (already formatted).
    pub cost: Option<String>,
    /// `CONTROLBAR:Requirements` line (already formatted).
    pub requires: Option<String>,
    /// A `TOOLTIP:TooltipNotEnoughMoneyToBuild`-style reason line.
    pub reason: Option<String>,
    /// For an `EXIT_CONTAINER` slot: the contained object this button exits
    /// (`ControlBar::m_containData[i].objectID`). `None` for every other command.
    pub contained: Option<ObjectId>,
}

/// Selected object's `CommandSet` name (`Object::getCommandSetString`): a rider-change override
/// (`RiderChangeContain`) wins over the template's `CommandSet` (`ThingTemplate::commandSet`).
pub(crate) fn selected_command_set(sim: &SimRes) -> Option<String> {
    let id = *sim.selected.first()?;
    if let Some(set) = sim.game.logic().find_object(id).and_then(|o| o.command_set_override()) {
        return Some(set.to_owned());
    }
    let template = sim.game.object(id)?.template.to_owned();
    let set = sim.game.data().templates().thing(&template)?.command_set.clone();
    (!set.is_empty()).then_some(set)
}

pub(crate) fn slot_infos(sim: &SimRes, hud: &HudData) -> [Option<SlotInfo>; SLOT_COUNT] {
    let mut out: [Option<SlotInfo>; SLOT_COUNT] = Default::default();
    let Some(set_name) = selected_command_set(sim) else { return out };
    let Some(set) = hud.commands.set(&set_name) else { return out };
    let local = sim.local;
    let obj = sim.selected.first().and_then(|&id| sim.game.logic().find_object(id));
    // `ControlBar::doTransportInventoryUI` / `populateStructureInventory`: the contiguous
    // `EXIT_CONTAINER` slots in the set are filled with the container's contents in order, and a
    // slot is only enabled while an object actually occupies it.
    let contained: Vec<ObjectId> = obj.map(|o| o.contained.clone()).unwrap_or_default();
    let mut exit_index = 0usize;
    for (i, info) in out.iter_mut().enumerate() {
        let Some(btn_name) = set.slot(i) else { continue };
        let Some(btn) = hud.commands.button(btn_name) else { continue };
        if btn.options & zr_ui::command_option::SCRIPT_ONLY != 0 {
            continue;
        }
        // `ControlBar::populateCommand`: a command that needs a special-power science the player
        // does not own is hidden, so locked superweapons never show as usable buttons (purchase
        // science / upgrade buttons keep showing; they are gated elsewhere).
        if btn.options & zr_ui::command_option::NEED_SPECIAL_POWER_SCIENCE != 0
            && !matches!(btn.command, CommandKind::PurchaseScience | CommandKind::PlayerUpgrade | CommandKind::ObjectUpgrade)
        {
            if let Some(power) = btn.special_power.as_deref() {
                if let Some(science) = sim.game.data().templates().special_power(power).and_then(|p| p.required_science.as_deref()) {
                    if !sim.game.logic().player_has_science(local, science) {
                        continue;
                    }
                }
            }
        }
        // `ControlBar::updateContextCommand`: a hidden command is not drawn at all.
        let base = cmd_avail::command_availability(sim, btn, obj);
        if base == CommandAvailability::Hidden {
            continue;
        }
        let mut image = btn.button_image.clone();
        let mut availability = base;
        let mut contained_id = None;
        if btn.command == CommandKind::ExitContainer {
            contained_id = contained.get(exit_index).copied();
            exit_index += 1;
            // The cameo is the contained object's (`populateInvDataCallback`).
            image = contained_id
                .and_then(|id| sim.game.object(id))
                .and_then(|o| sim.game.data().templates().thing(o.template))
                .and_then(|t| t.button_image.clone());
            // Empty slots stay as disabled buttons (`winEnable(FALSE)`); `base` is still
            // `Restricted` for a subdued container, never less than empty.
            availability = if contained_id.is_some() { CommandAvailability::Available } else { CommandAvailability::Restricted };
        }
        let cmd_avail::Tooltip { name, cost, description, requires, reason } = cmd_avail::build_tooltip(sim, btn, obj);
        *info = Some(SlotInfo {
            image,
            label: name,
            tooltip: description,
            power: btn.special_power.clone(),
            availability,
            cost,
            requires,
            reason,
            contained: contained_id,
        });
    }
    out
}

/// Selects the local player's scheme and (re)builds its layered images. The engine's
/// `setControlBarSchemeByPlayer` picks by side; `drawBackground` layers sit under the `.wnd`
/// panels and `drawForeground` layers over them.
#[allow(clippy::too_many_arguments)]
fn hud_scheme(
    mut commands: Commands,
    sim: Option<NonSend<SimRes>>,
    hud: Option<ResMut<HudData>>,
    mut assets: ResMut<Assets<Image>>,
    root: Query<Entity, With<HudRoot>>,
) {
    let (Some(sim), Some(mut hud)) = (sim, hud) else { return };
    // Player radar colours (faction colour), computed once.
    if hud.colors.is_empty() {
        for p in sim.game.players() {
            let c = match p.kind {
                zr_game::PlayerKind::Skirmish => crate::team_color::TeamColors::faction_color(&hud.ini, &p.faction),
                _ => crate::team_color::NEUTRAL,
            };
            hud.colors.insert(p.index, c);
        }
    }
    let side = sim.game.player(sim.local).map(|p| p.side.clone()).unwrap_or_default();
    if hud.scheme_side.as_deref() == Some(side.as_str()) {
        return;
    }
    for e in hud.scheme_entities.drain(..) {
        commands.entity(e).despawn();
    }
    hud.scheme_side = Some(side.clone());
    hud.exp_bar_image = hud.schemes.by_side(&side).and_then(|s| s.exp_bar_foreground_image.clone());
    let Some(scheme) = hud.schemes.by_side(&side) else { return };
    let Ok(root) = root.single() else { return };

    // Engine multiplier: display / scheme `ScreenCreationRes`.
    let (rw, rh) = scheme.screen_creation_res;
    let (sx, sy) = if rw > 0 && rh > 0 { (hud.win.0 / rw as f32, hud.win.1 / rh as f32) } else { (1.0, 1.0) };
    let layers: Vec<(SchemeImage, i32)> = scheme
        .background_images()
        .map(|i| (i.clone(), Z_SCHEME_BG))
        .chain(scheme.foreground_images().map(|i| (i.clone(), Z_SCHEME_FG)))
        .collect();

    let HudData { vfs, language, images, textures, failed, scheme_entities, .. } = &mut *hud;
    for (img, z) in layers {
        let Some(name) = img.image.as_deref() else { continue };
        let Some(handle) = decode_image(vfs, images, language, &mut assets, textures, failed, name) else { continue };
        let rect = images.get(name).map(|i| Rect::new(i.left as f32, i.top as f32, i.right as f32, i.bottom as f32));
        let (x, y) = (img.position.0 as f32 * sx, img.position.1 as f32 * sy);
        let (w, h) = (img.size.0 as f32 * sx, img.size.1 as f32 * sy);
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        let e = commands
            .spawn((
                SchemeLayer,
                Node { position_type: PositionType::Absolute, left: Val::Px(x), top: Val::Px(y), width: Val::Px(w), height: Val::Px(h), ..default() },
                stretched_with(handle, rect),
                ZIndex(z),
                ChildOf(root),
            ))
            .id();
        scheme_entities.push(e);
    }
}

/// Updates the bar for the current selection, possession state, money and power.
#[allow(clippy::type_complexity)]
fn hud_refresh(
    sim: Option<NonSendMut<SimRes>>,
    hud: Option<ResMut<HudData>>,
    mut assets: ResMut<Assets<Image>>,
    mut q: Query<(
        &mut Visibility,
        Option<&HudRoot>,
        Option<&CommandSlot>,
        Option<&CommandIcon>,
        Option<&Interaction>,
        Option<&MoneyText>,
        Option<&HudTooltip>,
        Option<&mut ImageNode>,
        Option<&mut Text>,
        Option<&mut Node>,
        Option<&mut BackgroundColor>,
        Option<&HudMarker>,
        Option<&mut Outline>,
    )>,
) {
    let Some(mut sim_guard) = sim else { return };
    let Some(mut hud) = hud else { return };
    let sim: &mut SimRes = &mut sim_guard;

    let card_demo = std::env::var("ZR_CARD_DEMO").ok();
    if (hud.debug_select_structure || hud.debug_select_unit || card_demo.is_some()) && !hud.applied_debug_select {
        hud.applied_debug_select = true;
        if let Some(case) = card_demo {
            // Debug `ZR_CARD_DEMO`: spawn the card's subject and select it (screenshots). No sim
            // command is faked; the shown card is the ordinary one for the selected object.
            match setup_card_demo(sim, &case) {
                Some(id) => {
                    sim.selected = vec![id];
                    info!("hud: ZR_CARD_DEMO={case} selected object {}", id.0);
                }
                None => warn!("hud: ZR_CARD_DEMO={case}: no matching template/objects"),
            }
        } else {
            let local = sim.local;
            let role = if hud.debug_select_unit { "StartingUnit" } else { "StartingBuilding" };
            let pick = sim
                .game
                .spawned()
                .iter()
                .find(|s| s.player == local && s.role.starts_with(role))
                .map(|s| s.id);
            if let Some(id) = pick {
                sim.selected = vec![id];
                info!("hud: --select-first-{role} selected {} for player {local}", id.0);
            }
            if let (Some(t), Some(factory)) = (hud.debug_queue.clone(), sim.selected.first().copied()) {
                sim.game.submit(local, Command::QueueUnit { factory, template: t.clone() });
                info!("hud: --queue {t} at {factory:?}");
            }
            if let (Some(power), Some(source)) = (hud.debug_power.clone(), sim.selected.first().copied()) {
                let pos = sim.game.object(source).map_or(Coord3D::new(0.0, 0.0, 0.0), |o| o.position);
                sim.game.submit(local, Command::DoSpecialPower { source, power: power.clone(), target: zr_sim::special_power::SpecialPowerTarget::Position(pos) });
                info!("hud: --fire-power {power}");
            }
        }
    }

    let possessing = sim.possessed().is_some();
    let slots = slot_infos(sim, &hud);
    let money = sim.game.logic().player(sim.local).map_or(0, |p| p.count_money());

    // Tooltip for the hovered slot: the engine's `ControlBarPopupDescription` content (name, cost,
    // description, `Requires: ...`). `--hover-button N` forces one slot for screenshots.
    let hover_index = hud.debug_hover.or_else(|| {
        q.iter().find_map(|(_, _, slot, _, interaction, ..)| match (slot, interaction) {
            (Some(s), Some(i)) if *i == Interaction::Hovered => Some(s.0),
            _ => None,
        })
    });
    let tooltip = hover_index
        .and_then(|i| slots.get(i).and_then(|s| s.as_ref()))
        .map(|s| cmd_avail::tooltip_text(sim, s))
        .filter(|t| !t.is_empty());

    // Unit portrait (`SelectPortrait`) and the selected factory's build queue.
    let selected_template = sim.selected.first().and_then(|&id| sim.game.object(id)).map(|o| o.template.to_owned());
    let portrait = selected_template
        .as_deref()
        .and_then(|t| sim.game.data().templates().thing(t))
        .and_then(|t| t.select_portrait.clone());
    let queue: Vec<Option<(String, f32)>> = sim
        .selected
        .first()
        .and_then(|&f| sim.game.logic().find_object(f))
        .and_then(|o| o.production.as_ref())
        .map(|p| {
            p.queue
                .iter()
                .map(|e| match &e.kind {
                    zr_sim::production::ProductionKind::Unit(t) => {
                        let prog = (e.frames as f32 / t.build_time_frames.max(1) as f32).clamp(0.0, 1.0);
                        sim.game.data().templates().thing(&t.name).and_then(|tt| tt.button_image.clone()).map(|i| (i, prog))
                    }
                    // `populateBuildQueue`: an upgrade entry shows the `UpgradeTemplate` cameo too
                    // (`GadgetButtonSetEnabledImage(control, ut->getButtonImage())`).
                    zr_sim::production::ProductionKind::Upgrade(name) => {
                        let total = sim.game.data().templates().upgrade(name).map_or(30.0, |u| (u.build_time * 30.0).max(1.0));
                        let prog = (e.frames as f32 / total).clamp(0.0, 1.0);
                        sim.game.data().templates().upgrade(name).and_then(|u| u.button_image.clone()).map(|i| (i, prog))
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let debug_cooldown = hud.debug_cooldown;
    // Each slot's `ButtonBorderType` (`ControlBar::setCommandBarBorder`), from its command.
    let borders = hud.borders;
    let mut border_types = [zr_ui::BorderType::None; SLOT_COUNT];
    if let Some(set) = selected_command_set(sim).and_then(|n| hud.commands.set(&n)) {
        for (i, t) in border_types.iter_mut().enumerate() {
            if let Some(btn) = set.slot(i).and_then(|n| hud.commands.button(n)) {
                *t = btn.border_type;
            }
        }
    }
    let HudData { vfs, language, images, textures, failed, .. } = &mut *hud;
    for (mut vis, root, slot, icon, interaction, money_marker, tooltip_marker, mut image, mut text, mut node, mut bg, marker, mut outline) in &mut q {
        if root.is_some() {
            set_changed!(*vis, if possessing { Visibility::Hidden } else { Visibility::Inherited });
        }
        if let Some(slot) = slot {
            set_changed!(*vis, if slots[slot.0].is_some() { Visibility::Inherited } else { Visibility::Hidden });
            // Hover feedback: brighten the button border.
            if let Some(bg) = bg.as_deref_mut() {
                let hot = matches!(interaction, Some(i) if *i == Interaction::Hovered);
                set_changed!(*bg, BackgroundColor(if hot { Color::srgba(0.24, 0.26, 0.3, 0.96) } else { Color::srgba(0.14, 0.15, 0.18, 0.94) }));
            }
            if let Some(o) = outline.as_deref_mut() {
                set_changed!(o.color, borders.get(border_types[slot.0]).unwrap_or(Color::NONE));
            }
        }
        if let Some(icon) = icon {
            let info = slots[icon.0].as_ref();
            match info.and_then(|s| s.image.as_deref()) {
                Some(name) => {
                    // `COMMAND_RESTRICTED` draws the grayscale disabled image; `COMMAND_CANT_AFFORD`
                    // keeps the colour but dims it (`W3DGadgetPushButtonImageDrawOne`).
                    let restricted = info.is_some_and(|s| s.availability == CommandAvailability::Restricted);
                    let handle = if restricted {
                        decode_gray_image(vfs, images, language, &mut assets, textures, failed, name)
                    } else {
                        decode_image(vfs, images, language, &mut assets, textures, failed, name)
                    };
                    match handle {
                        Some(handle) => {
                            if let Some(img) = images.get(name) {
                                if let Some(node) = image.as_deref_mut() {
                                    set_changed!(node.image, handle);
                                    set_changed!(node.rect, Some(Rect::new(img.left as f32, img.top as f32, img.right as f32, img.bottom as f32)));
                                    let tint = if info.is_some_and(|s| s.availability == CommandAvailability::CantAfford) {
                                        Color::srgb(0.565, 0.565, 0.565)
                                    } else {
                                        Color::WHITE
                                    };
                                    set_changed!(node.color, tint);
                                }
                            }
                            set_changed!(*vis, Visibility::Inherited);
                        }
                        None => set_changed!(*vis, Visibility::Hidden),
                    }
                }
                None => set_changed!(*vis, Visibility::Hidden),
            }
        }
        if money_marker.is_some() {
            if let Some(txt) = text.as_deref_mut() {
                // `InGameUI::update`: `GUI:ControlBarMoneyDisplay` (`$ %d`) with the player's cash.
                let want = sim.game.text().get("GUI:ControlBarMoneyDisplay").unwrap_or("$ %d").replace("%d", &money.to_string());
                set_changed!(**txt, want);
            }
        }
        if tooltip_marker.is_some() {
            if let Some(txt) = text.as_deref_mut() {
                // Clone only when the text differs (`Text` writes re-run text layout).
                if txt.as_str() != tooltip.as_deref().unwrap_or("") {
                    **txt = tooltip.clone().unwrap_or_default();
                }
            }
            set_changed!(*vis, if tooltip.is_some() { Visibility::Inherited } else { Visibility::Hidden });
        }
        match marker {
            Some(HudMarker::Portrait) => {
                match portrait.as_deref().and_then(|n| decode_image(vfs, images, language, &mut assets, textures, failed, n).map(|h| (n, h))) {
                    Some((name, handle)) => {
                        if let (Some(img), Some(node)) = (images.get(name), image.as_deref_mut()) {
                            set_changed!(node.image, handle);
                            set_changed!(node.rect, Some(Rect::new(img.left as f32, img.top as f32, img.right as f32, img.bottom as f32)));
                        }
                        set_changed!(*vis, Visibility::Inherited);
                    }
                    None => set_changed!(*vis, Visibility::Hidden),
                }
            }
            Some(HudMarker::UnitGrid) => {
                set_changed!(*vis, if portrait.is_some() { Visibility::Inherited } else { Visibility::Hidden });
            }
            Some(HudMarker::Emblem) => {
                set_changed!(*vis, if portrait.is_some() { Visibility::Hidden } else { Visibility::Inherited });
            }
            Some(HudMarker::QueueSlot(i)) => match queue.get(*i).and_then(|x| x.as_ref()) {
                Some((name, _)) => {
                    if let Some(handle) = decode_image(vfs, images, language, &mut assets, textures, failed, name) {
                        if let (Some(img), Some(node)) = (images.get(name), image.as_deref_mut()) {
                            set_changed!(node.image, handle);
                            set_changed!(node.rect, Some(Rect::new(img.left as f32, img.top as f32, img.right as f32, img.bottom as f32)));
                        }
                    }
                    set_changed!(*vis, Visibility::Inherited);
                }
                None => set_changed!(*vis, Visibility::Hidden),
            },
            Some(HudMarker::QueueFill(i)) => {
                if let (Some((_, prog)), Some(node)) = (queue.get(*i).and_then(|x| x.as_ref()), node.as_deref_mut()) {
                    set_changed!(node.width, Val::Percent(prog * 100.0));
                }
            }
            Some(HudMarker::Cooldown(i)) => {
                let source = sim.selected.first().copied();
                let power = slots[*i].as_ref().and_then(|s| s.power.as_deref());
                // `--cooldown-demo`: show a half wipe on any special-power button for screenshots.
                if debug_cooldown && power.is_some() {
                    if let Some(node) = node.as_deref_mut() {
                        set_changed!(node.height, Val::Percent(50.0));
                    }
                    set_changed!(*vis, Visibility::Inherited);
                    continue;
                }
                match (source, power) {
                    (Some(source), Some(power)) => {
                        let ready = sim.game.logic().special_power_ready_at(source, power);
                        let total = sim.game.logic().special_power_reload(power).max(1);
                        let remaining = ready.saturating_sub(sim.game.frame());
                        if remaining > 0 && total > 0 {
                            if let Some(node) = node.as_deref_mut() {
                                set_changed!(node.height, Val::Percent((remaining as f32 / total as f32) * 100.0));
                            }
                            set_changed!(*vis, Visibility::Inherited);
                        } else {
                            set_changed!(*vis, Visibility::Hidden);
                        }
                    }
                    _ => set_changed!(*vis, Visibility::Hidden),
                }
            }
            None => {}
        }
    }
}

/// Switches between the command context and the under-construction card
/// (`ControlBar::evaluateContextUI` -> `CB_CONTEXT_UNDER_CONSTRUCTION`,
/// `ControlBar::populateUnderConstruction`/`updateConstructionTextDisplay`).
#[allow(clippy::type_complexity)]
fn hud_under_construction(
    sim: Option<NonSend<SimRes>>,
    hud: Option<ResMut<HudData>>,
    mut assets: ResMut<Assets<Image>>,
    mut icons: Query<&mut ImageNode, With<CancelIcon>>,
    mut q: Query<(
        &mut Visibility,
        Option<&CommandWindowRoot>,
        Option<&CommandSlot>,
        Option<&UnderConstructionRoot>,
        Option<&UnderConstructionText>,
        Option<&CancelConstructionButton>,
        Option<&mut Text>,
    )>,
) {
    let Some(sim) = sim else { return };
    let under = sim
        .selected
        .first()
        .and_then(|&id| sim.game.logic().find_object(id))
        .filter(|o| o.under_construction);
    let active = under.is_some();
    // The cancel button's cameo (`populateUnderConstruction` -> `setControlCommand`).
    if let (true, Some(mut hud)) = (active, hud) {
        let HudData { vfs, language, images, textures, failed, commands, .. } = &mut *hud;
        let cameo = commands.button("Command_CancelConstruction").and_then(|b| b.button_image.clone());
        if let Some(name) = cameo {
            if let Some(handle) = decode_image(vfs, images, language, &mut assets, textures, failed, &name) {
                let rect = images.get(&name).map(|i| Rect::new(i.left as f32, i.top as f32, i.right as f32, i.bottom as f32));
                for mut node in &mut icons {
                    set_changed!(node.image, handle.clone());
                    set_changed!(node.rect, rect);
                }
            }
        }
    }
    let desc = under.map(|o| {
        let fmt = sim.game.text().get("CONTROLBAR:UnderConstructionDesc").unwrap_or("Under construction").to_owned();
        cmd_avail::format_one(&fmt, &format!("{}", o.construction_percent.round() as i32))
    });
    for (mut vis, cmd, slot, uc, txt_marker, cancel, mut text) in &mut q {
        // The `.wnd` tree is drawn flat under `HudRoot`, so hiding `CommandWindow` alone would not
        // hide its slot buttons: hide them directly. `hud_refresh` (which ran first) restores the
        // per-slot visibility whenever the under-construction context is not active.
        if active && (cmd.is_some() || slot.is_some()) {
            set_changed!(*vis, Visibility::Hidden);
        }
        // The `.wnd` tree is flat under `HudRoot`, so the under-construction window's own
        // `Visibility::Hidden` does not hide its children (`ButtonCancelConstruction`,
        // `UnderConstructionDesc`); both are toggled directly here. The desc text has a near-black
        // `.wnd` fill, so leaving it drawn while another context is active showed a black card.
        if uc.is_some() || cancel.is_some() || txt_marker.is_some() {
            set_changed!(*vis, if active { Visibility::Inherited } else { Visibility::Hidden });
        }
        if txt_marker.is_some() {
            if let Some(t) = text.as_deref_mut() {
                **t = desc.clone().unwrap_or_default();
            }
        }
    }
}

/// Dispatches the clicked command button to the sim (`ControlBar::setControlCommand` semantics).
#[allow(clippy::too_many_arguments)]
fn hud_clicks(
    hud: Option<Res<HudData>>,
    sim: Option<NonSendMut<SimRes>>,
    keys: Res<ButtonInput<KeyCode>>,
    interactive: Res<Interactive>,
    modal: crate::ui_block::ModalOpen,
    mut mode: ResMut<crate::placement::OrderMode>,
    mut audio: ResMut<crate::audio::AudioQueue>,
    mut eva_req: ResMut<crate::eva::EvaRequests>,
    mut msg_req: ResMut<crate::messages::MessageRequests>,
    q: Query<(&Interaction, &CommandSlot), Changed<Interaction>>,
    queue_q: Query<(&Interaction, &HudMarker), Changed<Interaction>>,
    cancel_q: Query<&Interaction, (With<CancelConstructionButton>, Changed<Interaction>)>,
) {
    // The pause menu / options / science panel own all input (no clicks or `&` hotkeys).
    if modal.get() {
        return;
    }
    let Some(hud) = hud else { return };
    let Some(mut sim_guard) = sim else { return };
    let sim: &mut SimRes = &mut sim_guard;
    // The control bar is hidden while possessing (the radial menu replaces it); its command
    // buttons are not clickable and its `&` hotkeys must not fire (Q opens the radial menu).
    if sim.possessed().is_some() {
        return;
    }
    // Under-construction card cancel (`Command_CancelConstruction`): retail's
    // `onDozerCancelConstruct` refunds and destroys the selected unfinished building; the sim
    // models it as `CancelConstruct` on that building's `builder` dozer.
    if cancel_q.iter().any(|i| *i == Interaction::Pressed) {
        if let Some(&building) = sim.selected.first() {
            let dozer = sim.game.logic().find_object(building).and_then(|o| o.builder);
            if let Some(dozer) = dozer {
                let local = sim.local;
                sim.game.submit(local, Command::CancelConstruct { dozer });
                info!("hud: cancel construction of {building:?} (dozer {dozer:?})");
            }
        }
    }
    // Cancel a queued production item. `populateBuildQueue` puts `Command_CancelUnitCreate` on a
    // unit entry and `Command_CancelUpgradeCreate` on an upgrade entry, so the click must send the
    // matching message.
    for (interaction, marker) in &queue_q {
        if let (Interaction::Pressed, HudMarker::QueueSlot(index)) = (interaction, marker) {
            if let Some(factory) = sim.selected.first().copied() {
                let local = sim.local;
                let entry = sim
                    .game
                    .logic()
                    .find_object(factory)
                    .and_then(|o| o.production.as_ref())
                    .and_then(|p| p.queue.get(*index))
                    .map(|e| match &e.kind {
                        zr_sim::production::ProductionKind::Unit(_) => None,
                        zr_sim::production::ProductionKind::Upgrade(name) => Some(name.clone()),
                    });
                match entry {
                    Some(Some(upgrade)) => {
                        sim.game.submit(local, Command::CancelUpgrade { building: factory, upgrade });
                        info!("hud: cancel upgrade queue entry {index} at {factory:?}");
                    }
                    Some(None) => {
                        sim.game.submit(local, Command::CancelUnit { factory, index: *index });
                        info!("hud: cancel queue entry {index} at {factory:?}");
                    }
                    None => {}
                }
            }
        }
    }
    let Some(factory) = sim.selected.first().copied() else { return };
    let Some(set_name) = selected_command_set(sim) else { return };
    let ids = sim.selected.clone();
    // Resolved per slot once: availability and the exit-container object both come from here, so
    // the click uses the same state the icon was drawn from.
    let slots = slot_infos(sim, &hud);

    // Clicks. A disabled button does nothing (retail `winEnable(FALSE)`: the gadget never sends
    // the click).
    for (interaction, slot) in &q {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some(info) = slots.get(slot.0).and_then(|s| s.as_ref()) else { continue };
        let Some(btn) = hud.commands.set(&set_name).and_then(|s| s.slot(slot.0)).and_then(|n| hud.commands.button(n)) else { continue };
        if !info.availability.enabled() {
            // Retail's `processCommandUI` reports why a command was refused; a `CANT_AFFORD`
            // button (or any build the player cannot pay for) plays the insufficient-funds EVA and
            // the `GUI:NotEnoughMoneyToBuild` line (`ControlBarCommandProcessing.cpp:244/289/331/402`).
            let obj = sim.game.logic().find_object(factory);
            if info.availability == CommandAvailability::CantAfford || cmd_avail::refuses_for_money(sim, btn, obj) {
                eva_req.push(crate::eva::EvaMessage::InsufficientFunds);
                msg_req.push_label(sim, "GUI:NotEnoughMoneyToBuild");
            }
            continue;
        }
        dispatch_button(sim, &mut mode, factory, &ids, btn, info.contained, &mut audio);
    }

    // Hotkeys from the `&` letter in the resolved `TextLabel` (`ControlBar::setControlCommand`).
    if interactive.0 {
        if let Some(set) = hud.commands.set(&set_name) {
            for (i, info) in slots.iter().enumerate() {
                let Some(info) = info.as_ref() else { continue };
                let Some(btn) = set.slot(i).and_then(|n| hud.commands.button(n)) else { continue };
                let Some(c) = btn.text_label.as_deref().and_then(|l| hotkey_letter(sim.game.text().get(l).unwrap_or(l))) else { continue };
                if let Some(k) = keycode_for(c) {
                    if keys.just_pressed(k) && info.availability.enabled() {
                        dispatch_button(sim, &mut mode, factory, &ids, btn, info.contained, &mut audio);
                    }
                }
            }
        }
    }
}

/// Keys that are `&` hotkeys of the selected object's command card. The viewer's WASD / Q / E
/// camera keys yield to these (retail scrolls with the arrow keys only, so its letter hotkeys
/// never double as camera keys).
pub(crate) fn command_hotkeys(hud: &HudData, sim: &SimRes) -> Vec<KeyCode> {
    let mut out = Vec::new();
    if sim.possessed().is_some() {
        return out;
    }
    let Some(set) = selected_command_set(sim).and_then(|n| hud.commands.set(&n)) else { return out };
    for i in 0..SLOT_COUNT {
        let Some(btn) = set.slot(i).and_then(|n| hud.commands.button(n)) else { continue };
        let Some(c) = btn.text_label.as_deref().and_then(|l| hotkey_letter(sim.game.text().get(l).unwrap_or(l))) else { continue };
        out.extend(keycode_for(c));
    }
    out
}

/// The letter marked by `&` in a button label (`&Dozer` -> `D`).
fn hotkey_letter(label: &str) -> Option<char> {
    let mut it = label.chars();
    while let Some(c) = it.next() {
        if c == '&' {
            return it.next().filter(|c| !c.is_whitespace());
        }
    }
    None
}

/// `&`-letter -> `KeyCode` (letters and digits).
fn keycode_for(c: char) -> Option<KeyCode> {
    Some(match c.to_ascii_uppercase() {
        'A' => KeyCode::KeyA,
        'B' => KeyCode::KeyB,
        'C' => KeyCode::KeyC,
        'D' => KeyCode::KeyD,
        'E' => KeyCode::KeyE,
        'F' => KeyCode::KeyF,
        'G' => KeyCode::KeyG,
        'H' => KeyCode::KeyH,
        'I' => KeyCode::KeyI,
        'J' => KeyCode::KeyJ,
        'K' => KeyCode::KeyK,
        'L' => KeyCode::KeyL,
        'M' => KeyCode::KeyM,
        'N' => KeyCode::KeyN,
        'O' => KeyCode::KeyO,
        'P' => KeyCode::KeyP,
        'Q' => KeyCode::KeyQ,
        'R' => KeyCode::KeyR,
        'S' => KeyCode::KeyS,
        'T' => KeyCode::KeyT,
        'U' => KeyCode::KeyU,
        'V' => KeyCode::KeyV,
        'W' => KeyCode::KeyW,
        'X' => KeyCode::KeyX,
        'Y' => KeyCode::KeyY,
        'Z' => KeyCode::KeyZ,
        '0' => KeyCode::Digit0,
        '1' => KeyCode::Digit1,
        '2' => KeyCode::Digit2,
        '3' => KeyCode::Digit3,
        '4' => KeyCode::Digit4,
        '5' => KeyCode::Digit5,
        '6' => KeyCode::Digit6,
        '7' => KeyCode::Digit7,
        '8' => KeyCode::Digit8,
        '9' => KeyCode::Digit9,
        _ => return None,
    })
}

/// One `CommandButton` action (`ControlBar::processCommandUI` semantics). `contained` is the
/// `EXIT_CONTAINER` slot's contained object (`slot_infos`), used by passenger buttons.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dispatch_button(
    sim: &mut SimRes,
    mode: &mut crate::placement::OrderMode,
    factory: ObjectId,
    ids: &[ObjectId],
    btn: &zr_ui::command::CommandButton,
    contained: Option<ObjectId>,
    audio: &mut crate::audio::AudioQueue,
) {
    let local = sim.local;
    let ids = ids.to_vec();
    // `ControlBarCommandProcessing.cpp`: play the button's `UnitSpecificSound` (else the UI click).
    audio.push(btn.unit_specific_sound.clone().unwrap_or_else(|| "GUIClick".to_owned()), None, None);
    // `ControlBarCommandProcessing.cpp:214-220`: a target-needing button with `USES_MINE_CLEARING_WEAPONSET`
    // sends `MSG_SET_MINE_CLEARING_DETAIL` to the selection before it enters the target mode.
    if needs_target(btn.options) && btn.options & zr_ui::command::command_option::USES_MINE_CLEARING_WEAPONSET != 0 {
        sim.game.submit(local, Command::SetMineClearingDetail { ids: ids.clone() });
    }
    match &btn.command {
        CommandKind::UnitBuild => match &btn.object {
            Some(t) => {
                sim.game.submit(local, Command::QueueUnit { factory, template: t.clone() });
                info!("hud: queued {t} at {factory:?}");
            }
            None => warn!("hud: UNIT_BUILD `{}` has no Object", btn.name),
        },
        CommandKind::Stop => {
            sim.game.submit(local, Command::Stop { ids });
        }
        CommandKind::AttackMove => {
            mode.0 = Some(crate::placement::Order::AttackMove { ids, forced: None, spec: crate::pending_cursor::PendingSpec::from_button(btn) });
            info!("hud: click a ground point to attack-move");
        }
        CommandKind::Guard | CommandKind::GuardWithoutPursuit | CommandKind::GuardFlyingUnitsOnly => {
            let guard_mode = match btn.command {
                CommandKind::GuardWithoutPursuit => zr_sim::GuardMode::WithoutPursuit,
                CommandKind::GuardFlyingUnitsOnly => zr_sim::GuardMode::FlyingUnitsOnly,
                _ => zr_sim::GuardMode::Normal,
            };
            mode.0 = Some(crate::placement::Order::Guard { ids, mode: guard_mode, forced: None, spec: crate::pending_cursor::PendingSpec::from_button(btn) });
            info!("hud: click a ground point to guard");
        }
        CommandKind::Sell => {
            sim.game.submit(local, Command::Sell { building: factory });
            info!("hud: sell {factory:?}");
        }
        CommandKind::SetRallyPoint => {
            mode.0 = Some(crate::placement::Order::Rally { factory, spec: crate::pending_cursor::PendingSpec::from_button(btn) });
            info!("hud: click a ground point to set the rally point");
        }
        CommandKind::PlayerUpgrade | CommandKind::ObjectUpgrade => match &btn.upgrade {
            Some(u) => {
                sim.game.submit(local, Command::QueueUpgrade { building: factory, upgrade: u.clone() });
                info!("hud: queued upgrade {u} at {factory:?}");
            }
            None => warn!("hud: upgrade `{}` has no Upgrade", btn.name),
        },
        CommandKind::Evacuate => {
            sim.game.submit(local, Command::Evacuate { container: factory });
        }
        CommandKind::DozerConstruct => match &btn.object {
            Some(t) => {
                mode.0 = Some(crate::placement::Order::Place { dozer: factory, template: t.clone(), forced: None });
                info!("hud: place {t}");
            }
            None => warn!("hud: DOZER_CONSTRUCT `{}` has no Object", btn.name),
        },
        CommandKind::DozerConstructCancel => {
            let dozer = sim.game.logic().find_object(factory).and_then(|o| o.builder);
            match dozer {
                Some(dozer) => {
                    sim.game.submit(local, Command::CancelConstruct { dozer });
                    info!("hud: cancel construction of {factory:?}");
                }
                None => warn!("hud: `{}` has no builder to cancel", btn.name),
            }
        }
        // Empty slot: the button is disabled in retail (`winEnable(FALSE)`); nothing to do.
        CommandKind::ExitContainer => {
            if let Some(id) = contained {
                sim.game.submit(local, Command::Exit { container: factory, ids: vec![id] });
                info!("hud: exit {id:?} from {factory:?}");
            }
        }
        CommandKind::ToggleOvercharge => {
            sim.game.submit(local, Command::ToggleOvercharge { ids });
            info!("hud: toggle overcharge");
        }
        CommandKind::SwitchWeapon => {
            // `commandButton->getWeaponSlot()`; `WeaponSlotType` defaults to PRIMARY.
            let slot = cmd_avail::weapon_slot_index(btn.weapon_slot.as_deref()).unwrap_or(0);
            sim.game.submit(local, Command::SwitchWeapons { ids, slot });
            info!("hud: switch weapons to slot {slot}");
        }
        CommandKind::CombatDrop => {
            let (need_pos, _) = target_flags(btn.options);
            mode.0 = Some(crate::placement::Order::CombatDrop { ids, need_pos, options: btn.options, forced: None, spec: crate::pending_cursor::PendingSpec::from_button(btn) });
            info!("hud: click an object or ground point to combat-drop");
        }
        CommandKind::HijackVehicle | CommandKind::ConvertToCarBomb | CommandKind::SabotageBuilding => {
            let kind = match btn.command {
                CommandKind::HijackVehicle => crate::placement::EnterKind::HijackVehicle,
                CommandKind::ConvertToCarBomb => crate::placement::EnterKind::ConvertToCarBomb,
                _ => crate::placement::EnterKind::SabotageBuilding,
            };
            mode.0 = Some(crate::placement::Order::EnterTarget { ids, kind, options: btn.options, spec: crate::pending_cursor::PendingSpec::from_button(btn) });
            info!("hud: click a target ({kind:?})");
        }
        // `SPECIAL_POWER_CONSTRUCT` enters the structure-placement interface with the power's
        // `Object` (`ControlBarCommandProcessing.cpp:320/352` via `InGameUI::placeBuildAvailable`);
        // the click then fires the power at the spot (`PlaceEventTranslator.cpp:252`).
        CommandKind::SpecialPowerConstruct | CommandKind::SpecialPowerConstructFromShortcut => {
            match (btn.special_power.as_deref(), btn.object.as_deref()) {
                (Some(power), Some(template)) => {
                    mode.0 = Some(crate::placement::Order::SpecialPowerConstruct { source: factory, power: power.to_owned(), template: template.to_owned(), forced: None });
                    info!("hud: place {template} for {power}");
                }
                _ => warn!("hud: SPECIAL_POWER_CONSTRUCT `{}` needs a SpecialPower and Object", btn.name),
            }
        }
        CommandKind::SpecialPower | CommandKind::SpecialPowerFromShortcut => match &btn.special_power {
            Some(power) => {
                let (need_pos, need_obj) = target_flags(btn.options);
                let radius = special_power_radius(sim, power);
                mode.0 = Some(crate::placement::Order::SpecialPower {
                    source: factory,
                    power: power.clone(),
                    radius,
                    need_pos,
                    need_obj,
                    options: btn.options,
                    forced: None,
                    spec: crate::pending_cursor::PendingSpec::from_button(btn),
                });
                info!("hud: target {power}");
            }
            None => warn!("hud: SPECIAL_POWER `{}` has no SpecialPower", btn.name),
        },
        CommandKind::SelectAllUnitsOfType => match &btn.object {
            Some(t) => {
                // `ControlBarCommandProcessing.cpp`: deselect all, then select every player object
                // whose template matches (the sim's own selection is the viewer's client state).
                let sel: Vec<ObjectId> = sim
                    .game
                    .logic()
                    .objects()
                    .filter(|o| !o.is_destroyed() && o.player == local && o.template.name == *t)
                    .map(|o| o.id)
                    .collect();
                if !sel.is_empty() {
                    info!("hud: select all {} ({})", t, sel.len());
                    sim.selected = sel;
                }
            }
            None => warn!("hud: SELECT_ALL_UNITS_OF_TYPE `{}` has no Object", btn.name),
        },
        // `GUI_COMMAND_WAYPOINTS` is an empty case in `processCommandUI`; no shipped button has it.
        CommandKind::Waypoints => info!("hud: WAYPOINTS does nothing in retail"),
        // `GUI_COMMAND_HACK_INTERNET` (`ControlBarCommandProcessing.cpp:744`): `MSG_INTERNET_HACK` for the selection.
        CommandKind::HackInternet => {
            sim.game.submit(local, Command::HackInternet { ids });
            info!("hud: hack internet");
        }
        // `GUI_COMMAND_FIRE_WEAPON` (`ControlBarCommandProcessing.cpp:823`): `MSG_DO_WEAPON` with the
        // button's slot and shot count. A button with a target bit enters the target mode instead
        // (`ControlBarCommandProcessing.cpp:214`), and its click sends the weapon order.
        CommandKind::FireWeapon => {
            let slot = fire_slot(btn);
            let max_shots = max_shots_arg(btn.max_shots_to_fire);
            if needs_target(btn.options) {
                let (need_pos, need_obj) = target_flags(btn.options);
                mode.0 = Some(crate::placement::Order::FireWeapon {
                    ids,
                    slot,
                    max_shots,
                    need_pos,
                    need_obj,
                    options: btn.options,
                    spec: crate::pending_cursor::PendingSpec::from_button(btn),
                });
                info!("hud: click a target for weapon slot {slot}");
            } else {
                sim.game.submit(local, Command::DoWeapon { ids, slot, max_shots });
                info!("hud: fire weapon slot {slot}");
            }
        }
        other => info!("hud: click `{}` ({other:?}) not dispatched yet", btn.name),
    }
}

/// `(NEED_TARGET_POS, NEED_TARGET_*_OBJECT)` from a `CommandButton`'s `Options`.
pub(crate) fn target_flags(options: u32) -> (bool, bool) {
    use zr_ui::command::command_option as opt;
    let need_pos = options & opt::NEED_TARGET_POS != 0;
    let need_obj = options & (opt::NEED_TARGET_ENEMY_OBJECT | opt::NEED_TARGET_NEUTRAL_OBJECT | opt::NEED_TARGET_ALLY_OBJECT) != 0;
    (need_pos, need_obj)
}

/// `COMMAND_OPTION_NEED_TARGET` (`ControlBar.h:146`): the button enters a target mode on click
/// (`ControlBarCommandProcessing.cpp:214`). `CONTEXTMODE_COMMAND` is part of the mask.
pub(crate) fn needs_target(options: u32) -> bool {
    use zr_ui::command::command_option as opt;
    options & (opt::NEED_TARGET_ENEMY_OBJECT | opt::NEED_TARGET_NEUTRAL_OBJECT | opt::NEED_TARGET_ALLY_OBJECT | opt::NEED_TARGET_POS | opt::CONTEXTMODE_COMMAND) != 0
}

/// The `WeaponSlot` of a `FIRE_WEAPON` button (`WeaponSlotType` defaults to PRIMARY).
pub(crate) fn fire_slot(btn: &zr_ui::command::CommandButton) -> usize {
    slot_or_primary(btn.weapon_slot.as_deref())
}

/// `WeaponSlotTypeNames` -> index, with the engine's PRIMARY default for a missing or unknown name.
pub(crate) fn slot_or_primary(slot: Option<&str>) -> usize {
    cmd_avail::weapon_slot_index(slot).unwrap_or(0)
}

/// The `MaxShotsToFire` argument for the sim: `NO_MAX_SHOTS_LIMIT` (`Weapon.h:53`, `0x7fffffff`) is
/// the sim's unlimited form, a negative count (`weapon_orders::shot_limit`). Other values pass through.
pub(crate) fn max_shots_arg(max_shots_to_fire: i32) -> i32 {
    if max_shots_to_fire == 0x7fff_ffff {
        -1
    } else {
        max_shots_to_fire
    }
}

/// `SpecialPowerTemplate::RadiusCursorRadius` (the radius cursor ring). A missing template has no
/// radius, so no ring (`InGameUI::setRadiusCursor` returns on a zero radius, `InGameUI.cpp:1513`).
fn special_power_radius(sim: &SimRes, power: &str) -> f32 {
    sim.game.data().templates().special_power(power).map_or(0.0, |p| p.radius_cursor_radius)
}

/// Debug `ZR_CARD_DEMO=<case>` (screenshots): spawn and select an object whose command card shows
/// the case's buttons. The template is found by `CommandSet` so no template name is hard-coded.
fn setup_card_demo(sim: &mut SimRes, case: &str) -> Option<ObjectId> {
    let local = sim.local;
    let start = sim.game.spawned().iter().find(|s| s.player == local).map(|s| s.pos)?;
    match case {
        "overcharge" => {
            let t = thing_with_command_set(sim, "ChinaPowerPlantCommandSet")?;
            sim.game.spawn_object(&t, Coord3D::new(start.x + 120.0, start.y + 90.0, 0.0), 0.0, local)
        }
        "combatdrop" => {
            let t = thing_with_command_set(sim, "AmericaVehicleChinookCommandSet")?;
            let pos = Coord3D::new(start.x + 150.0, start.y + 60.0, 0.0);
            let ch = sim.game.spawn_unit(local, &t, pos, 0.0)?;
            board_riders(sim, local, ch, "AmericaInfantryRanger", pos);
            Some(ch)
        }
        "hacker" => {
            // The hacker's card shows `HACK_INTERNET` (`Command_ChinaInfantryHackerInternetHack`).
            let t = thing_with_command_set(sim, "ChinaInfantryHackerCommandSet")?;
            sim.game.spawn_unit(local, &t, Coord3D::new(start.x + 120.0, start.y + 90.0, 0.0), 0.0)
        }
        "garrison" => {
            let t = thing_with_command_set(sim, "ChinaBunkerCommandSet")?;
            let pos = Coord3D::new(start.x + 120.0, start.y + 90.0, 0.0);
            let bunker = sim.game.spawn_object(&t, pos, 0.0, local)?;
            board_riders(sim, local, bunker, "ChinaInfantryRedguard", pos);
            Some(bunker)
        }
        "underconstruction" => {
            let t = thing_with_command_set(sim, "ChinaPowerPlantCommandSet")?;
            let dozer = sim.game.spawned().iter().find(|s| s.player == local && s.template.contains("Dozer")).map(|s| s.id)?;
            let pos = Coord3D::new(start.x + 150.0, start.y + 40.0, 0.0);
            sim.game.submit(local, Command::Construct { dozer, template: t, pos, angle: 0.0 });
            // Let the dozer start: the building exists under construction and is selectable.
            sim.game.run_frames(20);
            sim.game.logic().objects().filter(|o| o.player == local && o.under_construction).map(|o| o.id).max()
        }
        _ => None,
    }
}

/// The first `ThingTemplate` with this `CommandSet` (`ThingTemplate::getCommandSetString`).
fn thing_with_command_set(sim: &SimRes, set: &str) -> Option<String> {
    sim.game
        .data()
        .templates()
        .things()
        .iter()
        .find(|t| t.command_set.eq_ignore_ascii_case(set))
        .map(|t| t.name.clone())
}

/// Spawns four infantry next to `container` and orders them in (`Command::Enter`), then runs frames
/// so the passenger buttons are populated (`ControlBar::populateStructureInventory`).
fn board_riders(sim: &mut SimRes, local: PlayerIndex, container: ObjectId, rider: &str, near: Coord3D) {
    let mut ids = Vec::new();
    for i in 0..4 {
        let p = Coord3D::new(near.x + 10.0 + i as f32 * 4.0, near.y + 10.0, 0.0);
        if let Some(id) = sim.game.spawn_unit(local, rider, p, 0.0) {
            ids.push(id);
        }
    }
    sim.game.submit(local, Command::Enter { ids, container });
    sim.game.run_frames(200);
}

#[cfg(test)]
mod tests {
    use super::*;
    use zr_ui::command::command_option as opt;

    #[test]
    fn target_flags_reads_pos_and_object_bits() {
        assert_eq!(target_flags(0), (false, false));
        assert_eq!(target_flags(opt::NEED_TARGET_POS), (true, false));
        assert_eq!(target_flags(opt::NEED_TARGET_ENEMY_OBJECT), (false, true));
        assert_eq!(target_flags(opt::NEED_TARGET_NEUTRAL_OBJECT | opt::NEED_TARGET_ALLY_OBJECT), (false, true));
        assert_eq!(target_flags(opt::NEED_TARGET_POS | opt::NEED_TARGET_ENEMY_OBJECT), (true, true));
    }

    #[test]
    fn needs_target_includes_context_mode() {
        assert!(!needs_target(0));
        assert!(!needs_target(opt::OK_FOR_MULTI_SELECT | opt::USES_MINE_CLEARING_WEAPONSET));
        assert!(needs_target(opt::NEED_TARGET_POS));
        assert!(needs_target(opt::NEED_TARGET_ENEMY_OBJECT | opt::OK_FOR_MULTI_SELECT));
        assert!(needs_target(opt::CONTEXTMODE_COMMAND));
    }

    #[test]
    fn max_shots_maps_the_retail_unlimited_value_to_the_sim_form() {
        // `NO_MAX_SHOTS_LIMIT` (`Weapon.h:53`) is the default `MaxShotsToFire`; the sim takes a negative.
        assert_eq!(max_shots_arg(0x7fff_ffff), -1);
        assert_eq!(max_shots_arg(1), 1);
        assert_eq!(max_shots_arg(0), 0);
    }

    #[test]
    fn weapon_slot_reads_the_slot_and_defaults_to_primary() {
        assert_eq!(slot_or_primary(None), 0);
        assert_eq!(slot_or_primary(Some("TERTIARY")), 2);
        assert_eq!(slot_or_primary(Some("SECONDARY")), 1);
    }

    #[test]
    fn hotkey_letter_reads_the_ampersand() {
        assert_eq!(hotkey_letter("&Dozer"), Some('D'));
        assert_eq!(hotkey_letter("Attack &Move"), Some('M'));
        assert_eq!(hotkey_letter("NoHotkey"), None);
    }
}
