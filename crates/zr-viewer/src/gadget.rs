//! `.wnd` gadget renderer for the shell screens (main menu, setup, options, load screen).
//!
//! The plain `.wnd` renderer in [`crate::shell`] draws every window as one stretched image or colour
//! fill with the first draw slot and the bevy default font, so push buttons lost their caps and
//! hover art. This module draws the engine's gadget art instead:
//!
//! * push buttons with the `W3DGadgetPushButtonImageDraw*` piece layout ([`zr_ui::gadget`]): the
//!   enabled / hilite / pressed / disabled images per state, left and right caps at their own width,
//!   the middle repeated and clipped;
//! * the `.wnd` font (`Generals` = the install's `PLCC____.TTF`, Arial/Times from the OS) and the
//!   state text colours from `TEXTCOLOR`;
//! * horizontal sliders as a row of boxes filled to the position (`W3DGadgetHorizontalSliderImageDraw`,
//!   [`SliderBar`]); vertical sliders are not drawn yet;
//! * a [`GadgetRoot`] per gadget; a menu makes it clickable with [`make_clickable`].
//!
//! Every node is positioned absolutely in window coordinates (like [`crate::shell::render_wnd_filtered`]),
//! so a gadget's pieces, label and click target are siblings. Pieces and labels use
//! [`FocusPolicy::Pass`], so only the root receives the pointer.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::text::FontSource;
use bevy::ui::FocusPolicy;
use zr_text::GameText;
use zr_ui::gadget::{self, ButtonArt, ButtonState, Piece};
use zr_ui::wnd::{status, style, GadgetData, WndFile};

use crate::shell::{self, ImageEntry};

/// What a menu needs to draw one `.wnd`.
pub struct Scene<'a> {
    pub wnd: &'a WndFile,
    pub images: &'a HashMap<String, ImageEntry>,
    pub fonts: &'a HashMap<String, Handle<Font>>,
    pub text: &'a GameText,
    pub window: Option<&'a Window>,
    /// Windows to leave out (`GameWindow::winHide`, the dropdown panels of the main menu).
    pub hide: &'a dyn Fn(&str) -> bool,
    /// Windows the file starts hidden but the screen shows (`winHide(FALSE)` from a menu's own
    /// transition or init code, e.g. the faction emblems on the single-player page).
    pub reveal: &'a dyn Fn(&str) -> bool,
    /// Label overrides by full window name: the menu's own text (combo values, the map name).
    pub label: &'a dyn Fn(&str) -> Option<String>,
    /// Check-box state by full window name (`None` for windows that are not check boxes).
    pub checked: &'a dyn Fn(&str) -> Option<bool>,
    /// Text colour override by full window name (the player colour names in the setup list).
    pub tint: &'a dyn Fn(&str) -> Option<Color>,
    /// Horizontal slider position by full window name, as a fraction of its range (`None` = 0).
    pub fraction: &'a dyn Fn(&str) -> Option<f32>,
}

/// A gadget's root node: the window's rect, which becomes clickable with [`make_clickable`].
/// Its visual state comes from its [`Interaction`]; without one it shows the enabled art.
#[derive(Component)]
pub struct GadgetRoot {
    /// Which [`ButtonState`]s have art, indexed by [`state_index`].
    has: [bool; 4],
    /// `WIN_STATUS_ENABLED` clear: the disabled art shows whatever the pointer does.
    disabled: bool,
}

/// One image node of a gadget, shown while its root is in `state`.
#[derive(Component)]
pub struct GadgetPiece {
    root: Entity,
    state: ButtonState,
}

/// A gadget's label, with the text colour for each state.
#[derive(Component)]
pub struct GadgetText {
    root: Entity,
    /// Enabled, hilite / pressed, disabled.
    colors: [Color; 3],
}

fn state_index(s: ButtonState) -> usize {
    match s {
        ButtonState::Enabled => 0,
        ButtonState::Hilite => 1,
        ButtonState::Pressed => 2,
        ButtonState::Disabled => 3,
    }
}

const STATES: [ButtonState; 4] = [ButtonState::Enabled, ButtonState::Hilite, ButtonState::Pressed, ButtonState::Disabled];

impl GadgetRoot {
    /// The state to draw: the pointer state, falling back to the nearest state that has art.
    fn shown(&self, interaction: Option<&Interaction>) -> ButtonState {
        if self.disabled {
            return ButtonState::Disabled;
        }
        let order: &[ButtonState] = match interaction {
            Some(Interaction::Pressed) => &[ButtonState::Pressed, ButtonState::Hilite, ButtonState::Enabled],
            Some(Interaction::Hovered) => &[ButtonState::Hilite, ButtonState::Enabled],
            _ => &[ButtonState::Enabled],
        };
        order.iter().copied().find(|s| self.has[state_index(*s)]).unwrap_or(ButtonState::Enabled)
    }
}

/// Makes a gadget root clickable (`GameWindow` input) and attaches the menu's action component.
pub fn make_clickable<T: Component>(commands: &mut Commands, root: Entity, action: T) {
    commands.entity(root).insert((Button, Interaction::default(), action));
}

/// Gives a gadget root a pointer state (hover / pressed art) without an action.
pub fn make_hoverable(commands: &mut Commands, root: Entity) {
    commands.entity(root).insert((Button, Interaction::default()));
}

/// Draws `scene.wnd` under `root` and returns every gadget root by its full `.wnd` name
/// (`MainMenu.wnd:ButtonSinglePlayer`). Callers match the trailing name.
pub fn render(commands: &mut Commands, root: Entity, scene: &Scene) -> Vec<(String, Entity)> {
    let (sx, sy) = shell::wnd_scale(scene.wnd, scene.window);
    let mut out = Vec::new();
    for w in &scene.wnd.windows {
        spawn(commands, root, w, scene, (sx, sy), &mut out);
    }
    out
}

/// Whether a window is a gadget (drawn with [`gadget`] art and given a root).
fn is_gadget(w: &zr_ui::Window) -> bool {
    w.style & (style::PUSHBUTTON | style::COMBOBOX | style::ENTRYFIELD | style::CHECKBOX | style::RADIOBUTTON) != 0
}

fn spawn(commands: &mut Commands, parent: Entity, w: &zr_ui::Window, scene: &Scene, scale: (f32, f32), out: &mut Vec<(String, Entity)>) {
    let name = w.name.clone().unwrap_or_default();
    let hidden = w.status & status::HIDDEN != 0 && !(scene.reveal)(&name);
    if hidden || (scene.hide)(&name) {
        return;
    }
    let (x, y, width, height) = shell::scaled(w.rect, scale.0, scale.1);
    let draw = w.draw_callback.as_deref() != Some("W3DNoDraw");
    if draw && width > 0.0 && height > 0.0 {
        let rect = (x, y, width, height);
        if w.style & style::HORZSLIDER != 0 {
            if let Some(root) = spawn_slider(commands, parent, w, &name, scene, rect, scale) {
                out.push((name, root));
            }
        } else if w.style & style::VERTSLIDER == 0 {
            // Vertical sliders are not drawn yet.
            if is_gadget(w) {
                let root = spawn_gadget(commands, parent, w, &name, scene, rect, scale);
                out.push((name, root));
            } else {
                spawn_plain(commands, parent, w, &name, scene, rect, scale.1);
            }
        }
    }
    for c in &w.children {
        spawn(commands, parent, c, scene, scale, out);
    }
}

/// An absolutely positioned node covering `rect` (screen pixels).
fn rect_node(rect: (f32, f32, f32, f32)) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(rect.0),
        top: Val::Px(rect.1),
        width: Val::Px(rect.2),
        height: Val::Px(rect.3),
        ..default()
    }
}

/// Size of a mapped image in its atlas (unscaled pixels), from its UV rect.
fn image_px(entry: &ImageEntry) -> Option<(f32, f32)> {
    entry.rect.map(|r| (r.width(), r.height()))
}

/// The label text for `w`: the menu's override, else the `.wnd` `TEXT` label.
fn label_for(scene: &Scene, w: &zr_ui::Window, name: &str) -> Option<String> {
    (scene.label)(name).or_else(|| {
        let key = w.text.as_deref()?;
        scene.text.get(key).map(str::to_owned)
    })
}

/// The `.wnd` font (`FONT = NAME/SIZE`) as a bevy text font, and its pixel size at this scale.
///
/// The `Generals`, `Placard MT Condensed` and `Abadi MT Bold` faces are not installed with the game
/// (the English `LocalFontFile` names no file in the archives), so Windows substitutes its default
/// face for them; retail's buttons then read in Arial, and so do ours.
/// A loaded `.wnd` face by family name (`arial` is the substitute for the unavailable `Generals`).
pub fn ui_font(fonts: &HashMap<String, Handle<Font>>, family: &str) -> FontSource {
    match fonts.get(family) {
        Some(h) => FontSource::Handle(h.clone()),
        None => FontSource::default(),
    }
}

fn font_for(scene: &Scene, w: &zr_ui::Window, sy: f32) -> TextFont {
    let size = w.font.as_ref().map_or(12.0, |f| f.size as f32 * sy).clamp(8.0, 40.0);
    let name = w.font.as_ref().map_or(String::new(), |f| f.name.to_ascii_lowercase());
    let family = match name.as_str() {
        "generals" | "placard mt condensed" | "abadi mt bold" => "arial",
        other => other,
    };
    let font = match scene.fonts.get(family) {
        Some(h) => FontSource::Handle(h.clone()),
        None => FontSource::default(),
    };
    TextFont { font, font_size: FontSize::Px(size), ..default() }
}

/// A label covering `rect`, centred or left-aligned with `pad` on the left. Returns the container.
#[allow(clippy::too_many_arguments)]
fn spawn_label(commands: &mut Commands, parent: Entity, text: &str, font: TextFont, color: Color, rect: (f32, f32, f32, f32), centered: bool, pad: f32) -> Entity {
    let container = commands
        .spawn((
            Node {
                justify_content: if centered { JustifyContent::Center } else { JustifyContent::FlexStart },
                align_items: AlignItems::Center,
                padding: UiRect::left(Val::Px(pad)),
                ..rect_node(rect)
            },
            FocusPolicy::Pass,
            ChildOf(parent),
        ))
        .id();
    commands.spawn((
        Text::new(text.to_owned()),
        font,
        TextColor(color),
        TextLayout { justify: if centered { Justify::Center } else { Justify::Left }, ..default() },
        ChildOf(container),
    ));
    container
}

/// The image pieces of one button state, as `(entry, node rect, crop rect, state)` items. Empty
/// when an image is missing (the state then shows nothing, like the engine's `return` on a null
/// middle or cap image).
fn piece_specs(art: &ButtonArt, scene: &Scene, rect: (f32, f32, f32, f32), scale: (f32, f32)) -> Vec<(Handle<Image>, Node, Option<Rect>)> {
    let (rx, ry, rw, rh) = rect;
    let entries: Vec<Option<&ImageEntry>> = art.images.iter().map(|n| n.as_ref().and_then(|n| scene.images.get(n))).collect();
    if !art.three_piece {
        let Some(entry) = entries[0] else { return Vec::new() };
        return vec![(entry.handle.clone(), rect_node(rect), entry.rect)];
    }
    let (Some(left), Some(mid), Some(right)) = (entries[0], entries[1], entries[2]) else { return Vec::new() };
    let (Some(lw), Some(mw), Some(ew)) = (image_px(left), image_px(mid), image_px(right)) else { return Vec::new() };
    let sx = scale.0;
    let tiles = gadget::three_piece_tiles(rw.round() as i32, (lw.0 * sx).round() as i32, (mw.0 * sx).round() as i32, (ew.0 * sx).round() as i32);
    let mut out = Vec::new();
    for t in tiles {
        let entry = match t.piece {
            Piece::Left => left,
            Piece::Middle => mid,
            Piece::Right => right,
        };
        let Some(src) = entry.rect else { continue };
        // The clipped last middle repeat shows the left `src_frac` of the image.
        let crop = Rect::new(src.min.x, src.min.y, src.min.x + src.width() * t.src_frac, src.max.y);
        out.push((entry.handle.clone(), rect_node((rx + t.x as f32, ry, t.w as f32, rh)), Some(crop)));
    }
    out
}

/// The width of a combo box's drop-down button (`GadgetComboBox.cpp`: `buttonWidth = 21`).
const COMBO_BUTTON_W: f32 = 21.0;

/// One drawn image of a gadget part, in one state.
struct PartPiece {
    state: ButtonState,
    image: Handle<Image>,
    node: Node,
    crop: Option<Rect>,
}

/// The image pieces of every state of gadget `w`, by its kind. A combo box is an edit box plus a
/// drop-down button (`GadgetComboBox.cpp` creates both as children, the edit box at `(0,0)` and
/// the button at `width - 21`), each with its own art; an entry field and a check box draw their
/// own art (`W3DTextEntry.cpp`, `W3DCheckBox.cpp`); anything else is a push button.
/// Returns the pieces and the check box's box width (the label starts after it).
fn gadget_pieces(w: &zr_ui::Window, name: &str, scene: &Scene, rect: (f32, f32, f32, f32), scale: (f32, f32), filled: bool) -> (Vec<PartPiece>, f32) {
    let (rx, ry, rw, rh) = rect;
    let mut pieces = Vec::new();
    let mut box_w = 0.0;
    let checked = (scene.checked)(name).unwrap_or(false);
    for &state in &STATES {
        if !filled && state != ButtonState::Enabled {
            continue;
        }
        let parts: Vec<(gadget::ButtonArt, (f32, f32, f32, f32))> = if w.style & style::COMBOBOX != 0 {
            let drop = (COMBO_BUTTON_W * scale.0).min(rw);
            vec![
                (gadget::combo_edit_art(w, state), (rx, ry, rw - drop, rh)),
                (gadget::combo_button_art(w, state), (rx + rw - drop, ry, drop, rh)),
            ]
        } else if w.style & style::ENTRYFIELD != 0 {
            vec![(gadget::entry_art(w, state), rect)]
        } else if w.style & style::CHECKBOX != 0 {
            // The box keeps its own size, at the left and vertically centred.
            let Some(image) = gadget::checkbox_image(w, state, checked) else { continue };
            let Some((iw, ih)) = scene.images.get(&image).and_then(image_px) else { continue };
            let (bw, bh) = (iw * scale.0, ih * scale.1);
            if state == ButtonState::Enabled {
                box_w = bw;
            }
            let art = gadget::ButtonArt { three_piece: false, images: [Some(image), None, None] };
            vec![(art, (rx, ry + (rh - bh) / 2.0, bw, bh))]
        } else {
            vec![(gadget::button_art(w, state), rect)]
        };
        for (art, part_rect) in parts {
            for (image, node, crop) in piece_specs(&art, scene, part_rect, scale) {
                pieces.push(PartPiece { state, image, node, crop });
            }
        }
    }
    (pieces, box_w)
}

#[allow(clippy::too_many_arguments)]
fn spawn_gadget(
    commands: &mut Commands,
    parent: Entity,
    w: &zr_ui::Window,
    name: &str,
    scene: &Scene,
    rect: (f32, f32, f32, f32),
    scale: (f32, f32),
) -> Entity {
    let disabled = w.status & status::ENABLED == 0;
    let filled = w.status & status::IMAGE != 0;
    let (pieces, box_w) = gadget_pieces(w, name, scene, rect, scale, filled);
    let mut has = [false; 4];
    for p in &pieces {
        has[state_index(p.state)] = true;
    }
    let mut root_node = commands.spawn((rect_node(rect), GadgetRoot { has, disabled }, ChildOf(parent)));
    if !filled {
        // `WIN_STATUS_IMAGE` clear: the engine fills the window with its enabled colour.
        if let Some(d) = w.draw_data.enabled.first().filter(|d| d.color.a > 0) {
            root_node.insert(BackgroundColor(shell::rgba(d.color)));
        }
    }
    let root = root_node.id();
    for p in pieces {
        commands.spawn((
            p.node,
            ImageNode { image: p.image, rect: p.crop, image_mode: NodeImageMode::Stretch, ..default() },
            FocusPolicy::Pass,
            Visibility::Hidden,
            GadgetPiece { root, state: p.state },
            ChildOf(parent),
        ));
    }

    if let Some(text) = label_for(scene, w, name) {
        let font = font_for(scene, w, scale.1);
        let mut colors = w.text_color.map_or([Color::WHITE, Color::WHITE, Color::srgb(0.5, 0.5, 0.5)], |c| {
            [shell::rgba(c.enabled.color), shell::rgba(c.hilite.color), shell::rgba(c.disabled.color)]
        });
        if let Some(tint) = (scene.tint)(name) {
            colors = [tint, tint, colors[2]];
        }
        let (rx, ry, rw, rh) = rect;
        let sx = scale.0;
        // Where the label sits: a push button centres it; a combo box and an entry field left-align
        // it after the 5-px text margin (`winDrawText` in `W3DGadgetComboBoxDraw` / `TextEntry`); a
        // check box puts it right of the box.
        let (label_rect, centered, pad) = if w.style & style::PUSHBUTTON != 0 {
            (rect, true, 0.0)
        } else if w.style & style::COMBOBOX != 0 {
            ((rx, ry, rw - (COMBO_BUTTON_W * sx).min(rw), rh), false, 5.0 * sx)
        } else if w.style & style::ENTRYFIELD != 0 {
            (rect, false, 5.0 * sx)
        } else if w.style & style::CHECKBOX != 0 {
            ((rx + box_w, ry, (rw - box_w).max(0.0), rh), false, 4.0 * sx)
        } else {
            let centered = matches!(&w.gadget, Some(GadgetData::StaticText(s)) if s.centered);
            (rect, centered, if centered { 0.0 } else { 6.0 * sx })
        };
        let container = spawn_label(commands, parent, &text, font, colors[0], label_rect, centered, pad);
        commands.entity(container).insert(GadgetText { root, colors });
    }
    root
}

/// A `.wnd` window that is not a gadget: its image (`WIN_STATUS_IMAGE`) or colour fill, and its label.
fn spawn_plain(commands: &mut Commands, parent: Entity, w: &zr_ui::Window, name: &str, scene: &Scene, rect: (f32, f32, f32, f32), sy: f32) {
    // `W3DGameWinDefaultDraw` (`W3DGameWindow.cpp`): an image window draws its image, any other
    // window fills its rect with the enabled colour and outlines it with the border colour.
    // Windows with their own draw callback (`W3DMainMenuDraw`, `W3DShellMenuSchemeDraw`) draw
    // their own art instead, and static text draws its own text, so neither gets the default fill.
    let first = w.draw_data.enabled.first();
    let image_fill = first.and_then(|d| d.image.as_ref().and_then(|n| scene.images.get(n)));
    let default_draw = w.draw_callback.is_none() && w.style & style::STATICTEXT == 0;
    let mut base = rect_node(rect);
    let border = first.map(|d| d.border_color).filter(|c| c.a > 0 && default_draw && !(w.status & status::IMAGE != 0 && image_fill.is_some()));
    if border.is_some() {
        base.border = UiRect::all(Val::Px(1.0));
    }
    let mut node = commands.spawn((base, FocusPolicy::Pass, ChildOf(parent)));
    if let Some(border) = border {
        node.insert(BorderColor::all(shell::rgba(border)));
    }
    if let Some(d) = first {
        match (w.status & status::IMAGE != 0, image_fill) {
            (true, Some(entry)) => {
                node.insert(ImageNode { image: entry.handle.clone(), rect: entry.rect, image_mode: NodeImageMode::Stretch, ..default() });
            }
            _ => {
                if d.color.a > 0 && default_draw {
                    node.insert(BackgroundColor(shell::rgba(d.color)));
                }
            }
        }
    }
    if let Some(text) = label_for(scene, w, name) {
        let font = font_for(scene, w, sy);
        let color = w.text_color.map_or(Color::srgb(0.95, 0.95, 0.85), |c| shell::rgba(c.enabled.color));
        let centered = matches!(&w.gadget, Some(GadgetData::StaticText(s)) if s.centered);
        spawn_label(commands, parent, &text, font, color, rect, centered, 0.0);
    }
}

/// The gap between the boxes of a horizontal slider (`W3DGadgetHorizontalSliderImageDraw`, `boxPadding`).
const SLIDER_GAP: f32 = 2.0;

/// One mapped image of a slider box, with its atlas crop.
#[derive(Clone)]
struct BoxArt {
    image: Handle<Image>,
    crop: Option<Rect>,
}

/// A horizontal slider's bar, on its root. The boxes up to [`SliderBar::fraction`] of the bar take
/// the fill art, the rest the blank art ([`update_sliders`] redraws them).
#[derive(Component)]
pub struct SliderBar {
    /// The filled part of the range, 0 to 1 (`(position - min) / (max - min)`). The menu sets it.
    pub fraction: f32,
    fill: BoxArt,
    blank: BoxArt,
    /// Box width plus gap, and the bar width, in screen pixels.
    pitch: f32,
    width: f32,
}

impl SliderBar {
    /// Whether box `index` shows the fill art (`W3DGadgetHorizontalSliderImageDraw`: a box is filled
    /// when its left edge is at or before the position, and nothing is filled at the minimum).
    fn filled(&self, index: usize) -> bool {
        self.fraction > 0.0 && index as f32 * self.pitch <= self.fraction * self.width
    }
}

/// One box of a [`SliderBar`].
#[derive(Component)]
pub struct SliderBox {
    root: Entity,
    index: usize,
}

/// A horizontal slider drawn as a row of square boxes (`W3DGadgetHorizontalSliderImageDraw`,
/// `W3DHorizontalSlider.cpp`). The fill and blank boxes are the disabled draw slots 0 and 1
/// (`GadgetSliderGetDisabledImageLeft/Right`); each box is the fill image's width (scaled) and as
/// tall, the boxes are centred in the bar and as many as fit are drawn. The hover highlight is not
/// drawn. Returns `None` when either image is missing, as the engine then draws nothing.
fn spawn_slider(
    commands: &mut Commands,
    parent: Entity,
    w: &zr_ui::Window,
    name: &str,
    scene: &Scene,
    rect: (f32, f32, f32, f32),
    scale: (f32, f32),
) -> Option<Entity> {
    let entry = |i: usize| scene.images.get(w.draw_data.disabled.get(i)?.image.as_ref()?);
    let fill_entry = entry(0)?;
    let blank_entry = entry(1)?;
    let (fill_w, _) = image_px(fill_entry)?;
    let box_w = fill_w * scale.0;
    let (rx, ry, rw, _) = rect;
    if box_w <= 0.0 {
        return None;
    }
    let pitch = box_w + SLIDER_GAP;
    // Box `k` is drawn while its right edge is still inside the bar (`end.x < origin.x + size.x`).
    let count = (0..).take_while(|&k| k as f32 * pitch + box_w < rw).count();
    let left = rx + ((rw - count as f32 * pitch) / 2.0).floor();
    let bar = SliderBar {
        fraction: (scene.fraction)(name).unwrap_or(0.0).clamp(0.0, 1.0),
        fill: BoxArt { image: fill_entry.handle.clone(), crop: fill_entry.rect },
        blank: BoxArt { image: blank_entry.handle.clone(), crop: blank_entry.rect },
        pitch,
        width: rw,
    };
    let root = commands.spawn((rect_node(rect), ChildOf(parent))).id();
    for index in 0..count {
        let art = if bar.filled(index) { &bar.fill } else { &bar.blank };
        commands.spawn((
            rect_node((left + index as f32 * pitch, ry, box_w, box_w)),
            ImageNode { image: art.image.clone(), rect: art.crop, image_mode: NodeImageMode::Stretch, ..default() },
            FocusPolicy::Pass,
            SliderBox { root, index },
            ChildOf(parent),
        ));
    }
    commands.entity(root).insert(bar);
    Some(root)
}

/// Redraws every slider box from its bar's fraction (runs every frame; cheap).
pub fn update_sliders(bars: Query<&SliderBar>, mut boxes: Query<(&SliderBox, &mut ImageNode)>) {
    for (b, mut node) in &mut boxes {
        let Ok(bar) = bars.get(b.root) else { continue };
        let art = if bar.filled(b.index) { &bar.fill } else { &bar.blank };
        if node.image != art.image || node.rect != art.crop {
            node.image = art.image.clone();
            node.rect = art.crop;
        }
    }
}

/// Keeps each piece and label in the state its root shows (runs every frame; cheap).
pub fn update_visuals(
    roots: Query<(&GadgetRoot, Option<&Interaction>)>,
    mut pieces: Query<(&GadgetPiece, &mut Visibility)>,
    mut texts: Query<(&GadgetText, &mut TextColor)>,
) {
    for (piece, mut vis) in &mut pieces {
        let Ok((root, interaction)) = roots.get(piece.root) else { continue };
        let want = if piece.state == root.shown(interaction) { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
    }
    for (text, mut color) in &mut texts {
        let Ok((root, interaction)) = roots.get(text.root) else { continue };
        let index = match root.shown(interaction) {
            ButtonState::Enabled => 0,
            ButtonState::Hilite | ButtonState::Pressed => 1,
            ButtonState::Disabled => 2,
        };
        let want = text.colors[index];
        if color.0 != want {
            color.0 = want;
        }
    }
}
