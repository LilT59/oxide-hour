//! Start screen: the real `Window\Menus\MainMenu.wnd` (`MainMenu.cpp`, `MainMenuInit` /
//! `MainMenuSystem`), with the default menu (`MainMenu.wnd:MapBorder2`) and the single-player
//! submenu (`MainMenu.wnd:MapBorder`) shown one at a time.
//!
//! The engine hides every dropdown panel except the active one (`initialHide` + the transition
//! handler's `MainMenuDefaultMenu`) and hides the faction / recent-save windows
//! (`showSelectiveButtons(SHOW_NONE)`); [`hidden_for`] reproduces that. The `.wnd` is drawn by
//! [`crate::gadget`] (the push buttons with their Left/Middle/Right art, hover and pressed states).
//! The buttons that lead somewhere in this port are clickable: `ButtonSinglePlayer` and
//! `ButtonSkirmish` (into the setup screen), `ButtonOptions`, `ButtonExit`, `ButtonSingleBack`;
//! the rest only highlight.
//!
//! The 3D shell map (`TheShell->showShellMap(TRUE)`) runs behind the menu as the shell `GAME_SHELL`
//! sim ([`crate::shellmap`]). The `.wnd`'s
//! `MainMenuBackdrop` fills the screen instead.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::gadget;
use crate::options::OptionsState;
use crate::shell::{ShellData, ShellState};

/// Which dropdown of the start screen is open (`dropDown` in `MainMenu.cpp`).
#[derive(Resource, Clone, Copy, PartialEq, Eq, Default)]
pub enum MainMenuPage {
    /// `DROPDOWN_MAIN` (`MainMenu.wnd:MapBorder2`): Single Player, Multiplayer, Options, Exit.
    #[default]
    Main,
    /// `DROPDOWN_SINGLE` (`MainMenu.wnd:MapBorder`): the single-player actions, incl. Skirmish.
    Single,
}

/// Set when the start screen must be (re)built.
#[derive(Resource, Default)]
pub struct MainMenuDirty(pub bool);

#[derive(Component)]
struct MainMenuRoot;

#[derive(Component, Clone, Copy)]
enum MainMenuAction {
    SinglePlayer,
    Skirmish,
    Options,
    Exit,
    Back,
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<MainMenuPage>();
    app.init_resource::<MainMenuDirty>();
    app.add_systems(OnEnter(ShellState::MainMenu), mark_dirty);
    app.add_systems(OnExit(ShellState::MainMenu), despawn_main_menu);
    app.add_systems(Update, (main_menu_build, main_menu_input).run_if(in_state(ShellState::MainMenu)));
}

/// `OnExit(MainMenu)`: drop the start screen even if the state changed before it was built.
fn despawn_main_menu(mut commands: Commands, existing: Query<Entity, With<MainMenuRoot>>) {
    for e in &existing {
        commands.entity(e).despawn();
    }
}

fn mark_dirty(mut page: ResMut<MainMenuPage>, mut dirty: ResMut<MainMenuDirty>) {
    // `--menu-single`: open the single-player submenu directly (screenshot harness).
    *page = if std::env::args().any(|a| a == "--menu-single") { MainMenuPage::Single } else { MainMenuPage::default() };
    dirty.0 = true;
}

/// Whether a `.wnd` window is hidden for `page` (`initialHide` + `showSelectiveButtons` + the
/// dropdown panels the transition handler leaves parked). Names are suffix-matched.
/// The five faction emblems of `MainMenu.wnd` (`WinFactionUS`, `WinFactionGLA`, `WinFactionChina`,
/// `WinFactionTraining`, `WinFactionSkirmish`). Retail's `MainMenuFaction*` transitions
/// (`PatchINI.big` WindowTransitions.ini, MAINMENUMEDIUMSCALEUP) scale them in on the single-player
/// page and leave them there; the scale-in animation itself is not ported.
fn is_faction_emblem(name: &str) -> bool {
    const FACTION_EMBLEMS: &[&str] = &["WinFactionUS", "WinFactionGLA", "WinFactionChina", "WinFactionTraining", "WinFactionSkirmish"];
    FACTION_EMBLEMS.iter().any(|s| name.ends_with(s))
}

fn hidden_for(name: &str, page: MainMenuPage) -> bool {
    // The five faction emblems: `initialHide` hides them, and the single-player page shows them
    // (see [`is_faction_emblem`]).
    if is_faction_emblem(name) {
        return page != MainMenuPage::Single;
    }
    const ALWAYS_HIDDEN: &[&str] = &[
        "WinGrowMarker",
        "WinFactionTrainingSmall",
        "WinFactionTrainingMedium",
        "WinFactionSkirmishSmall",
        "WinFactionSkirmishMedium",
        "WinFactionUSSmall",
        "WinFactionUSMedium",
        "WinFactionGLASmall",
        "WinFactionGLAMedium",
        "WinFactionChinaSmall",
        "WinFactionChinaMedium",
        "ShellMenuScheme",
        "GreenDot",
        "Clock",
        "ButtonGetMapPack",
        "ButtonGetUpdate",
        "ButtonUSARecentSave",
        "ButtonUSALoadGame",
        "ButtonGLARecentSave",
        "ButtonGLALoadGame",
        "ButtonChinaRecentSave",
        "ButtonChinaLoadGame",
    ];
    if ALWAYS_HIDDEN.iter().any(|s| name.ends_with(s)) {
        return true;
    }
    // The dropdown panels: the active page's panel stays, the others are parked.
    let parked: &[&str] = match page {
        MainMenuPage::Main => &["MapBorder", "MapBorder1", "MapBorder3", "MapBorder4"],
        MainMenuPage::Single => &["MapBorder2", "MapBorder1", "MapBorder3", "MapBorder4"],
    };
    // `MapBorder` must be matched exactly (it is a prefix of `MapBorder1..4`): `ends_with` of the
    // exact names is safe because the shorter names do not end with the longer ones.
    parked.iter().any(|s| name.ends_with(s))
}

#[allow(clippy::too_many_arguments)]
fn main_menu_build(
    mut commands: Commands,
    mut dirty: ResMut<MainMenuDirty>,
    page: Res<MainMenuPage>,
    shell: Option<Res<ShellData>>,
    window: Query<&Window, With<PrimaryWindow>>,
    existing: Query<Entity, With<MainMenuRoot>>,
) {
    if !dirty.0 {
        return;
    }
    dirty.0 = false;
    for e in &existing {
        commands.entity(e).despawn();
    }
    let win = window.single().ok();
    let root = commands
        .spawn((
            MainMenuRoot,
            Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
        ))
        .id();
    let Some(shell) = shell else {
        // No install: a bare prompt so the screen is not empty.
        commands.spawn((Text::new("Zero Hour install not found (set ZR_ZEROHOUR_DIR)"), TextFont { font_size: FontSize::Px(20.0), ..default() }, ChildOf(root)));
        return;
    };
    if let Some(wnd) = &shell.main_menu_wnd {
        let page = *page;
        // The `.wnd` gadgets (`MainMenu.wnd`'s push buttons with their Left/Middle/Right art).
        let scene = gadget::Scene {
            wnd,
            images: &shell.images,
            fonts: &shell.fonts,
            text: &shell.text,
            window: win,
            hide: &|n| hidden_for(n, page),
            reveal: &|n| page == MainMenuPage::Single && is_faction_emblem(n),
            label: &|_: &str| None::<String>,
            checked: &|_: &str| None::<bool>,
            tint: &|_: &str| None::<bevy::prelude::Color>,
            fraction: &|_: &str| None::<f32>,
        };
        for (name, entity) in gadget::render(&mut commands, root, &scene) {
            let short = name.rsplit(':').next().unwrap_or("");
            let action = match (page, short) {
                (MainMenuPage::Main, "ButtonSinglePlayer") => Some(MainMenuAction::SinglePlayer),
                (MainMenuPage::Main, "ButtonOptions") => Some(MainMenuAction::Options),
                (MainMenuPage::Main, "ButtonExit") => Some(MainMenuAction::Exit),
                (MainMenuPage::Single, "ButtonSkirmish") => Some(MainMenuAction::Skirmish),
                (MainMenuPage::Single, "ButtonSingleBack") => Some(MainMenuAction::Back),
                _ => None,
            };
            match action {
                Some(action) => gadget::make_clickable(&mut commands, entity, action),
                // Buttons that lead nowhere in this port still highlight like the engine's.
                None => gadget::make_hoverable(&mut commands, entity),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn main_menu_input(
    mut page: ResMut<MainMenuPage>,
    mut dirty: ResMut<MainMenuDirty>,
    mut options: ResMut<OptionsState>,
    mut next: ResMut<NextState<ShellState>>,
    mut exit: MessageWriter<AppExit>,
    actions: Query<(&Interaction, &MainMenuAction), Changed<Interaction>>,
) {
    for (interaction, action) in &actions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match action {
            // `GBM_SELECTED` `buttonSinglePlayerID`: show `DROPDOWN_SINGLE`.
            MainMenuAction::SinglePlayer => {
                *page = MainMenuPage::Single;
                dirty.0 = true;
            }
            // `skirmishID`: `TheShell->push("Menus/SkirmishGameOptionsMenu.wnd")`.
            MainMenuAction::Skirmish => next.set(ShellState::Setup),
            // `optionsID`: open the options layout (F10 toggles it too).
            MainMenuAction::Options => {
                options.open = true;
                options.dirty = true;
                options.request_rebuild();
            }
            // `quitCallback`: quit the application.
            MainMenuAction::Exit => {
                exit.write(AppExit::Success);
            }
            // `buttonSingleBackID`: back to `DROPDOWN_MAIN`.
            MainMenuAction::Back => {
                *page = MainMenuPage::Main;
                dirty.0 = true;
            }
        }
    }
}
