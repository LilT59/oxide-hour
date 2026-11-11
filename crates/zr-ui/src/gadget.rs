//! Gadget artwork layout from a `.wnd` window's draw data.
//!
//! Ported from `Core/GameEngineDevice/Source/W3DDevice/GameClient/GUI/Gadget/W3DPushButton.cpp`
//! (`W3DGadgetPushButtonImageDrawOne` / `W3DGadgetPushButtonImageDrawThree`) and the draw-data
//! slot accessors in `Core/GameEngine/Include/GameClient/GadgetPushButton.h`, plus the generic
//! image window draw in `W3DGameWindow.cpp` (`WIN_STATUS_IMAGE`: the image replaces the colour fill).
//!
//! A push button has two artwork layouts:
//! * **one image**: the enabled / hilite / disabled slot 0 image is stretched over the whole button;
//! * **three pieces** (when the enabled middle slot 5 has an image, as `MainMenu.wnd`'s
//!   `Buttons-Left` / `Buttons-Middle` / `Buttons-Right` buttons do): the left and right caps keep
//!   their width and the middle image repeats between them, the last repeat clipped.
//!
//! Slot numbers (`GadgetPushButton.h`): 0 left, 1 left-pressed, 3 middle-pressed, 4 right-pressed,
//! 5 middle, 6 right. Pressed (`WIN_STATE_SELECTED`) uses the pressed slots of the hilite set, which
//! is what the engine's `Hilite*Selected` accessors read.

use crate::image::ImageDb;
use crate::wnd::{status, DrawData, DrawDataSets, Window};

/// Visual state of a push button, as `W3DGadgetPushButtonImageDraw*` picks its image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonState {
    Enabled,
    /// `WIN_STATE_HILITED` (the pointer is over the button).
    Hilite,
    /// `WIN_STATE_SELECTED` while hilited (the button is held down).
    Pressed,
    /// `WIN_STATUS_ENABLED` clear.
    Disabled,
}

/// Which piece of a three-piece button an image belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    Left,
    Middle,
    Right,
}

/// One drawn piece of a three-piece button, in the window's own (unscaled) units.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub piece: Piece,
    /// Offset from the window's left edge.
    pub x: i32,
    /// Drawn width.
    pub w: i32,
    /// Fraction of the image width shown from its left edge (`1.0` for the whole image; less for
    /// the clipped last middle repeat, `winDrawImage` with the `reg` clip in the engine).
    pub src_frac: f32,
}

/// The image names a push button draws in one state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ButtonArt {
    /// `true` when drawn as three pieces (`left`, `middle`, `right`).
    pub three_piece: bool,
    /// Left (or the single) image, then middle, then right; `None` where the slot is empty.
    pub images: [Option<String>; 3],
}

/// Whether a window is drawn from images (`WIN_STATUS_IMAGE`), else from its colour fill.
pub fn draws_image(w: &Window) -> bool {
    w.status & status::IMAGE != 0
}

/// The image name in draw-data slot `i` of a set (`NoImage` and missing slots give `None`).
fn slot(set: &[DrawData], i: usize) -> Option<String> {
    set.get(i).and_then(|d| d.image.clone())
}

/// `W3DGadgetPushButtonImageDrawThree` / `...One`: the images for `state`.
///
/// The three-piece layout is chosen by the enabled middle slot, not by the state, as
/// `W3DGadgetPushButtonImageDraw` does (`GadgetButtonGetMiddleEnabledImage`).
pub fn button_art(w: &Window, state: ButtonState) -> ButtonArt {
    let d: &DrawDataSets = &w.draw_data;
    let three_piece = slot(&d.enabled, 5).is_some();
    // Slot numbers: `[left, middle, right]` for the plain and the pressed variants.
    let (set, idx): (&[DrawData], [usize; 3]) = match state {
        ButtonState::Enabled => (&d.enabled, [0, 5, 6]),
        ButtonState::Hilite => (&d.hilite, [0, 5, 6]),
        ButtonState::Pressed => (&d.hilite, [1, 3, 4]),
        ButtonState::Disabled => (&d.disabled, [0, 5, 6]),
    };
    let images = if three_piece {
        [slot(set, idx[0]), slot(set, idx[1]), slot(set, idx[2])]
    } else {
        // One-image buttons: slot 0 of the set; the pressed look reads the pressed slot 1.
        let single = match state {
            ButtonState::Pressed => slot(&d.hilite, 1),
            _ => slot(set, 0),
        };
        [single, None, None]
    };
    ButtonArt { three_piece, images }
}

/// The layout of a three-piece button `width` wide (`W3DGadgetPushButtonImageDrawThree`).
///
/// The caps keep their pixel widths; the middle repeats `middle_w` wide, and the last repeat is
/// clipped to fit. A window narrower than its caps shows the left half and the right half.
pub fn three_piece_tiles(width: i32, left_w: i32, middle_w: i32, right_w: i32) -> Vec<Tile> {
    let width = width.max(0);
    let mut out = Vec::new();
    if width == 0 {
        return out;
    }
    let left_end = left_w;
    let right_start = width - right_w;
    let center = right_start - left_end;
    if center <= 0 || middle_w <= 0 {
        // Too narrow for the middle: left half, then right half (the engine stretches both).
        let half = width / 2;
        out.push(Tile { piece: Piece::Left, x: 0, w: half, src_frac: 1.0 });
        out.push(Tile { piece: Piece::Right, x: half, w: width - half, src_frac: 1.0 });
        return out;
    }
    // Whole repeats, then the clipped remainder (`reg.hi.x = rightStart.x`).
    let pieces = (center / middle_w).min(4096);
    out.push(Tile { piece: Piece::Left, x: 0, w: left_w, src_frac: 1.0 });
    for i in 0..pieces {
        out.push(Tile { piece: Piece::Middle, x: left_end + i * middle_w, w: middle_w, src_frac: 1.0 });
    }
    let start = left_end + pieces * middle_w;
    let visible = right_start - start;
    if visible > 0 {
        out.push(Tile { piece: Piece::Middle, x: start, w: visible, src_frac: visible as f32 / middle_w as f32 });
    }
    out.push(Tile { piece: Piece::Right, x: right_start, w: right_w, src_frac: 1.0 });
    out
}

/// The `COMBOBOX<part><state>DRAWDATA` set for one part of a combo box (`GadgetComboBox.cpp` gives
/// the edit box and drop-down button their own sets, keyed by the three window states).
fn combo_part_set<'a>(d: &'a DrawDataSets, part: &str, state: ButtonState) -> &'a [DrawData] {
    let st = match state {
        ButtonState::Enabled => "ENABLED",
        ButtonState::Hilite | ButtonState::Pressed => "HILITE",
        ButtonState::Disabled => "DISABLED",
    };
    d.extra.get(&format!("COMBOBOX{part}{st}DRAWDATA")).map_or(&[], Vec::as_slice)
}

/// The edit box of a combo box, drawn as `W3DGadgetTextEntryImageDraw` draws a text entry: the
/// left end (slot 0), the repeating centre (slot 2) and the right end (slot 1). The engine also
/// fills the gap with the small centre (slot 3); a clipped centre repeat covers that here.
pub fn combo_edit_art(w: &Window, state: ButtonState) -> ButtonArt {
    let set = combo_part_set(&w.draw_data, "EDITBOX", state);
    let pick = |i: usize| set.get(i).and_then(|d| d.image.clone());
    ButtonArt { three_piece: true, images: [pick(0), pick(2), pick(1)] }
}

/// The drop-down button of a combo box (`GadgetComboBox.cpp` creates it as a push button): one
/// image, slot 0 normally and slot 1 (`VSliderDownButtonHiliteSelected`) while it is held down.
pub fn combo_button_art(w: &Window, state: ButtonState) -> ButtonArt {
    let set = combo_part_set(&w.draw_data, "DROPDOWNBUTTON", state);
    let slot = if state == ButtonState::Pressed { 1 } else { 0 };
    ButtonArt { three_piece: false, images: [set.get(slot).and_then(|d| d.image.clone()), None, None] }
}

/// A text entry field's own art (`ENTRYFIELD` windows): left end, right end and centre repeat in
/// draw slots 0, 1 and 2, as for the combo box's edit box.
pub fn entry_art(w: &Window, state: ButtonState) -> ButtonArt {
    let set: &[DrawData] = match state {
        ButtonState::Enabled => &w.draw_data.enabled,
        ButtonState::Hilite | ButtonState::Pressed => &w.draw_data.hilite,
        ButtonState::Disabled => &w.draw_data.disabled,
    };
    let pick = |i: usize| set.get(i).and_then(|d| d.image.clone());
    ButtonArt { three_piece: true, images: [pick(0), pick(2), pick(1)] }
}

/// A check box's box image (`W3DGadgetCheckBoxImageDraw`): slot 1 unchecked, slot 2 checked, in
/// the enabled, hilite or disabled set. Returns the image name.
pub fn checkbox_image(w: &Window, state: ButtonState, checked: bool) -> Option<String> {
    let set: &[DrawData] = match state {
        ButtonState::Enabled => &w.draw_data.enabled,
        ButtonState::Hilite | ButtonState::Pressed => &w.draw_data.hilite,
        ButtonState::Disabled => &w.draw_data.disabled,
    };
    set.get(if checked { 2 } else { 1 }).and_then(|d| d.image.clone())
}

/// `(width, height)` of a mapped image in its atlas (`Image::getImageWidth` / `getImageHeight`).
pub fn image_size(images: &ImageDb, name: &str) -> Option<(i32, i32)> {
    images.get(name).map(|i| (i.width, i.height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wnd::{Rgba, Window};

    fn draw(image: &str) -> DrawData {
        DrawData { image: Some(image.to_owned()), color: Rgba::default(), border_color: Rgba::default() }
    }

    fn none() -> DrawData {
        DrawData::default()
    }

    /// A three-piece `MainMenu.wnd`-style button (slots as the file lists them).
    fn three_piece_button() -> Window {
        let mut w = Window::default();
        w.draw_data.enabled = vec![
            draw("Buttons-Left"),
            none(),
            none(),
            none(),
            none(),
            draw("Buttons-Middle"),
            draw("Buttons-Right"),
            none(),
        ];
        w.draw_data.hilite = vec![
            draw("Hi-Left"),
            draw("Push-Left"),
            none(),
            draw("Push-Middle"),
            draw("Push-Right"),
            draw("Hi-Middle"),
            draw("Hi-Right"),
            none(),
        ];
        w.draw_data.disabled = vec![
            draw("Dis-Left"),
            none(),
            none(),
            none(),
            none(),
            draw("Dis-Middle"),
            draw("Dis-Right"),
            none(),
        ];
        w
    }

    #[test]
    fn three_piece_selects_the_slot_set_per_state() {
        let w = three_piece_button();
        let enabled = button_art(&w, ButtonState::Enabled);
        assert!(enabled.three_piece);
        assert_eq!(enabled.images, [Some("Buttons-Left".into()), Some("Buttons-Middle".into()), Some("Buttons-Right".into())]);
        let hilite = button_art(&w, ButtonState::Hilite);
        assert_eq!(hilite.images, [Some("Hi-Left".into()), Some("Hi-Middle".into()), Some("Hi-Right".into())]);
        let pressed = button_art(&w, ButtonState::Pressed);
        assert_eq!(pressed.images, [Some("Push-Left".into()), Some("Push-Middle".into()), Some("Push-Right".into())]);
        let disabled = button_art(&w, ButtonState::Disabled);
        assert_eq!(disabled.images[1], Some("Dis-Middle".into()));
    }

    #[test]
    fn one_image_button_uses_slot_zero() {
        let mut w = Window::default();
        w.draw_data.enabled = vec![draw("Solo")];
        w.draw_data.hilite = vec![draw("Solo-Hi")];
        let art = button_art(&w, ButtonState::Enabled);
        assert!(!art.three_piece);
        assert_eq!(art.images[0], Some("Solo".into()));
        assert_eq!(button_art(&w, ButtonState::Hilite).images[0], Some("Solo-Hi".into()));
    }

    #[test]
    fn exact_fit_has_whole_repeats_only() {
        // 16 + 176 + 16 = 208: 22 whole 8-px middle repeats.
        let t = three_piece_tiles(208, 16, 8, 16);
        assert_eq!(t.iter().filter(|t| t.piece == Piece::Middle).count(), 22);
        assert!(t.iter().all(|t| t.src_frac == 1.0));
        assert_eq!(t.last().map(|t| (t.piece, t.x, t.w)), Some((Piece::Right, 192, 16)));
    }

    #[test]
    fn partial_repeat_is_clipped_and_covers_the_width() {
        // 16 + 178 + 16 = 210: 22 whole repeats then a 2-px clipped repeat.
        let t = three_piece_tiles(210, 16, 8, 16);
        let last_mid = t.iter().rev().find(|t| t.piece == Piece::Middle).unwrap();
        assert_eq!((last_mid.x, last_mid.w), (192, 2));
        assert!((last_mid.src_frac - 0.25).abs() < 1e-6);
        let covered: i32 = t.iter().map(|t| t.w).sum();
        assert_eq!(covered, 210);
    }

    #[test]
    fn narrow_window_splits_into_halves() {
        let t = three_piece_tiles(20, 16, 8, 16);
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].piece, t[0].x, t[0].w), (Piece::Left, 0, 10));
        assert_eq!((t[1].piece, t[1].x, t[1].w), (Piece::Right, 10, 10));
    }

    #[test]
    fn zero_width_has_no_tiles() {
        assert!(three_piece_tiles(0, 16, 8, 16).is_empty());
    }

    #[test]
    fn combo_edit_box_takes_ends_and_centre_from_its_own_set() {
        let mut w = Window::default();
        let set = |names: [&str; 4]| names.iter().map(|n| draw(n)).collect::<Vec<_>>();
        w.draw_data.extra.insert("COMBOBOXEDITBOXENABLEDDRAWDATA".into(), set(["Edit-Left", "Edit-Right", "Edit-Centre", "Edit-Small"]));
        w.draw_data.extra.insert("COMBOBOXEDITBOXHILITEDRAWDATA".into(), set(["Hi-Left", "Hi-Right", "Hi-Centre", "Hi-Small"]));
        let art = combo_edit_art(&w, ButtonState::Enabled);
        assert!(art.three_piece);
        assert_eq!(art.images, [Some("Edit-Left".into()), Some("Edit-Centre".into()), Some("Edit-Right".into())]);
        assert_eq!(combo_edit_art(&w, ButtonState::Hilite).images[0], Some("Hi-Left".into()));
        // A combo with no edit-box set draws nothing.
        assert!(combo_edit_art(&Window::default(), ButtonState::Enabled).images.iter().all(Option::is_none));
    }

    #[test]
    fn combo_button_pressed_uses_the_selected_slot() {
        let mut w = Window::default();
        w.draw_data.extra.insert(
            "COMBOBOXDROPDOWNBUTTONHILITEDRAWDATA".into(),
            vec![draw("ArrowHi"), draw("ArrowHiSelected")],
        );
        assert_eq!(combo_button_art(&w, ButtonState::Hilite).images[0], Some("ArrowHi".into()));
        assert_eq!(combo_button_art(&w, ButtonState::Pressed).images[0], Some("ArrowHiSelected".into()));
    }

    #[test]
    fn checkbox_image_follows_checked_state() {
        let mut w = Window::default();
        w.draw_data.enabled = vec![none(), draw("Box-Off"), draw("Box-On")];
        assert_eq!(checkbox_image(&w, ButtonState::Enabled, false), Some("Box-Off".into()));
        assert_eq!(checkbox_image(&w, ButtonState::Enabled, true), Some("Box-On".into()));
    }
}
