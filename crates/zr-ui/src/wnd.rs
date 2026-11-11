//! `.wnd` window layout files (`GameWindowManagerScript.cpp`,
//! `GameWindowManager::winCreateFromScript` and `parseWindow`).
//!
//! A file is `FILE_VERSION = N;`, optionally a `STARTLAYOUTBLOCK … ENDLAYOUTBLOCK`
//! block for version 2+, some default colors/font, then a sequence of `WINDOW … END`
//! trees. Fields are `KEY = value;` and may span lines; a window's children live
//! behind a `CHILD` keyword as a list of nested `WINDOW … END` blocks terminated
//! by `ENDALLCHILDREN` (or the parent's own `END`).
//!
//! The parser takes untrusted bytes: every read is bounds-checked and malformed
//! input is a [`ParseError`] with a line number, never a panic.

use std::collections::BTreeMap;

/// `WindowStatusNames` (`GameWindowManagerScript.cpp`); bit index = position.
pub mod status {
    pub const ACTIVE: u32 = 1 << 0;
    pub const TOGGLE: u32 = 1 << 1;
    pub const DRAGABLE: u32 = 1 << 2;
    pub const ENABLED: u32 = 1 << 3;
    pub const HIDDEN: u32 = 1 << 4;
    pub const ABOVE: u32 = 1 << 5;
    pub const BELOW: u32 = 1 << 6;
    pub const IMAGE: u32 = 1 << 7;
    pub const TABSTOP: u32 = 1 << 8;
    pub const NOINPUT: u32 = 1 << 9;
    pub const NOFOCUS: u32 = 1 << 10;
    pub const DESTROYED: u32 = 1 << 11;
    pub const BORDER: u32 = 1 << 12;
    pub const SMOOTH_TEXT: u32 = 1 << 13;
    pub const ONE_LINE: u32 = 1 << 14;
    pub const NO_FLUSH: u32 = 1 << 15;
    pub const SEE_THRU: u32 = 1 << 16;
    pub const RIGHT_CLICK: u32 = 1 << 17;
    pub const WRAP_CENTERED: u32 = 1 << 18;
    pub const CHECK_LIKE: u32 = 1 << 19;
    pub const HOTKEY_TEXT: u32 = 1 << 20;
    pub const USE_OVERLAY_STATES: u32 = 1 << 21;
    pub const NOT_READY: u32 = 1 << 22;
    pub const FLASHING: u32 = 1 << 23;
    pub const ALWAYS_COLOR: u32 = 1 << 24;
    pub const ON_MOUSE_DOWN: u32 = 1 << 25;

    pub const NAMES: &[&str] = &[
        "ACTIVE", "TOGGLE", "DRAGABLE", "ENABLED", "HIDDEN", "ABOVE", "BELOW", "IMAGE", "TABSTOP",
        "NOINPUT", "NOFOCUS", "DESTROYED", "BORDER", "SMOOTH_TEXT", "ONE_LINE", "NO_FLUSH",
        "SEE_THRU", "RIGHT_CLICK", "WRAP_CENTERED", "CHECK_LIKE", "HOTKEY_TEXT",
        "USE_OVERLAY_STATES", "NOT_READY", "FLASHING", "ALWAYS_COLOR", "ON_MOUSE_DOWN",
    ];
}

/// `WindowStyleNames` (`GameWindowManagerScript.cpp`).
pub mod style {
    pub const PUSHBUTTON: u32 = 1 << 0;
    pub const RADIOBUTTON: u32 = 1 << 1;
    pub const CHECKBOX: u32 = 1 << 2;
    pub const VERTSLIDER: u32 = 1 << 3;
    pub const HORZSLIDER: u32 = 1 << 4;
    pub const SCROLLLISTBOX: u32 = 1 << 5;
    pub const ENTRYFIELD: u32 = 1 << 6;
    pub const STATICTEXT: u32 = 1 << 7;
    pub const PROGRESSBAR: u32 = 1 << 8;
    pub const USER: u32 = 1 << 9;
    pub const MOUSETRACK: u32 = 1 << 10;
    pub const ANIMATED: u32 = 1 << 11;
    pub const TABSTOP: u32 = 1 << 12;
    pub const TABCONTROL: u32 = 1 << 13;
    pub const TABPANE: u32 = 1 << 14;
    pub const COMBOBOX: u32 = 1 << 15;

    pub const NAMES: &[&str] = &[
        "PUSHBUTTON", "RADIOBUTTON", "CHECKBOX", "VERTSLIDER", "HORZSLIDER", "SCROLLLISTBOX",
        "ENTRYFIELD", "STATICTEXT", "PROGRESSBAR", "USER", "MOUSETRACK", "ANIMATED", "TABSTOP",
        "TABCONTROL", "TABPANE", "COMBOBOX",
    ];
}

/// One RGBA color, as written in the file (the engine packs with `GameMakeColor`,
/// the components are what matters here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// `TEXTCOLOR` slot: text color plus drop-shadow/border color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextState {
    pub color: Rgba,
    pub border_color: Rgba,
}

/// The enabled / disabled / hilite `TEXTCOLOR` triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextColors {
    pub enabled: TextState,
    pub disabled: TextState,
    pub hilite: TextState,
}

/// One `*DRAWDATA` entry: `IMAGE`, `COLOR`, `BORDERCOLOR`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DrawData {
    /// `None` for `NoImage`.
    pub image: Option<String>,
    pub color: Rgba,
    pub border_color: Rgba,
}

/// `FONT = NAME: "…", SIZE: n, BOLD: n`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FontSpec {
    pub name: String,
    pub size: i32,
    pub bold: i32,
}

/// `SCREENRECT` (unscaled, `CREATIONRESOLUTION` kept for the caller to scale).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScreenRect {
    pub upper_left: (i32, i32),
    pub bottom_right: (i32, i32),
    pub creation_resolution: (i32, i32),
}

impl ScreenRect {
    pub fn width(&self) -> i32 {
        self.bottom_right.0 - self.upper_left.0
    }
    pub fn height(&self) -> i32 {
        self.bottom_right.1 - self.upper_left.1
    }
}

/// Type-specific gadget data (`getDataTemplate` / the `parse*Data` functions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GadgetData {
    ListBox(ListBoxData),
    ComboBox(ComboBoxData),
    Slider(SliderData),
    RadioButton(RadioButtonData),
    TextEntry(TextEntryData),
    StaticText(StaticTextData),
    TabControl(TabControlData),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListBoxData {
    pub length: i32,
    pub auto_scroll: bool,
    pub scroll_if_at_end: bool,
    pub auto_purge: bool,
    pub scroll_bar: bool,
    pub multi_select: bool,
    pub columns: i32,
    pub column_width_percentages: Vec<i32>,
    pub force_select: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComboBoxData {
    pub is_editable: bool,
    pub max_chars: i32,
    pub max_display: i32,
    pub ascii_only: bool,
    pub letters_and_numbers_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SliderData {
    pub min_val: i32,
    pub max_val: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RadioButtonData {
    pub group: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextEntryData {
    pub max_len: i32,
    pub secret_text: bool,
    pub numerical_only: bool,
    pub alpha_numerical_only: bool,
    pub ascii_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StaticTextData {
    pub centered: bool,
    /// `parseStaticTextData` hard-codes these after `CENTERED`.
    pub centered_vertically: bool,
    pub left_margin: i32,
    pub top_margin: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TabControlData {
    pub tab_orientation: i32,
    pub tab_edge: i32,
    pub tab_width: i32,
    pub tab_height: i32,
    pub tab_count: i32,
    pub pane_border: i32,
    pub sub_pane_disabled: Vec<bool>,
}

/// The named `*DRAWDATA` sets. The three window states are typed; the many
/// list-box / combo-box / slider sets are kept by their INI key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrawDataSets {
    pub enabled: Vec<DrawData>,
    pub disabled: Vec<DrawData>,
    pub hilite: Vec<DrawData>,
    pub extra: BTreeMap<String, Vec<DrawData>>,
}

impl DrawDataSets {
    /// Every image name referenced by any slot (deduplicated on demand).
    pub fn image_names(&self) -> impl Iterator<Item = &str> {
        self.enabled
            .iter()
            .chain(&self.disabled)
            .chain(&self.hilite)
            .chain(self.extra.values().flatten())
            .filter_map(|d| d.image.as_deref())
    }
}

/// One `WINDOW … END` node.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Window {
    pub window_type: String,
    pub rect: ScreenRect,
    pub name: Option<String>,
    pub status: u32,
    pub style: u32,
    pub system_callback: Option<String>,
    pub input_callback: Option<String>,
    pub tooltip_callback: Option<String>,
    pub draw_callback: Option<String>,
    pub font: Option<FontSpec>,
    pub header_template: Option<String>,
    pub tooltip_text: Option<String>,
    pub tooltip_delay: Option<i32>,
    pub text: Option<String>,
    pub text_color: Option<TextColors>,
    pub draw_data: DrawDataSets,
    pub image_offset: Option<(i32, i32)>,
    pub gadget: Option<GadgetData>,
    pub children: Vec<Window>,
}

impl Window {
    /// This window and all descendants, depth-first.
    pub fn walk(&self) -> WindowWalk<'_> {
        WindowWalk { stack: vec![self] }
    }
}

/// Depth-first iterator over a window tree.
pub struct WindowWalk<'a> {
    stack: Vec<&'a Window>,
}

impl<'a> Iterator for WindowWalk<'a> {
    type Item = &'a Window;
    fn next(&mut self) -> Option<Self::Item> {
        let w = self.stack.pop()?;
        self.stack.extend(w.children.iter().rev());
        Some(w)
    }
}

/// `STARTLAYOUTBLOCK` callbacks (`[None]` when absent).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutBlock {
    pub init: Option<String>,
    pub update: Option<String>,
    pub shutdown: Option<String>,
}

/// File-level default colors and font.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowDefaults {
    pub colors: BTreeMap<String, Rgba>,
    pub font: Option<FontSpec>,
}

/// A parsed `.wnd` file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WndFile {
    pub file_version: u32,
    pub layout: LayoutBlock,
    pub defaults: WindowDefaults,
    pub windows: Vec<Window>,
    /// Recoverable oddities (unknown fields, bad values) with their line.
    pub warnings: Vec<(u32, String)>,
}

/// A parse failure with the 1-based line where it was detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: u32,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

fn err<T>(line: u32, message: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError { line, message: message.into() })
}

// ------------------------------------------------------------------------------------------------
// Reading

/// First integer in `s` (skipping labels), `sscanf("%d")`-like.
fn leading_int(s: &str) -> i32 {
    let s = s.trim_start();
    let Some(start) = s.find(|c: char| c.is_ascii_digit() || c == '-' || c == '+') else {
        return 0;
    };
    let s = &s[start..];
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '+'))
        .unwrap_or(s.len());
    s[..end].parse().unwrap_or(0)
}

/// `parseBitString`: tokens separated by `+`, `NULL` means none.
fn parse_bits(s: &str, names: &[&str]) -> u32 {
    let mut bits = 0;
    for tok in s.split('+').map(str::trim).filter(|t| !t.is_empty()) {
        if tok.eq_ignore_ascii_case("NULL") {
            continue;
        }
        if let Some(i) = names.iter().position(|n| n.eq_ignore_ascii_case(tok)) {
            bits |= 1 << i;
        }
    }
    bits
}

/// Quoted string value, or the trimmed raw text when there are no quotes.
fn parse_quoted(s: &str) -> String {
    let s = s.trim();
    if let Some(a) = s.find('"') {
        let rest = &s[a + 1..];
        if let Some(b) = rest.find('"') {
            return rest[..b].to_owned();
        }
    }
    s.to_owned()
}

/// The text after the first `=`.
fn after_eq(stmt: &str) -> String {
    match stmt.split_once('=') {
        Some((_, v)) => v.trim().to_owned(),
        None => stmt.trim().to_owned(),
    }
}

/// Splits a statement `KEY = VALUE` into its key and the raw value.
fn split_kv(stmt: &str) -> (&str, &str) {
    match stmt.split_once('=') {
        Some((k, v)) => (k.trim(), v.trim()),
        None => (stmt.trim(), ""),
    }
}

struct Reader<'a> {
    s: &'a [u8],
    pos: usize,
    line: u32,
}

impl<'a> Reader<'a> {
    fn new(s: &'a str) -> Self {
        let s = s.trim_start_matches('\u{feff}');
        Reader { s: s.as_bytes(), pos: 0, line: 1 }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.pos += 1;
        if c == b'\n' {
            self.line += 1;
        }
        Some(c)
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.bump();
        }
    }

    /// Next whitespace/`=`-delimited token, without consuming the `=`.
    fn scan_word(&mut self) -> Option<(&'a str, u32)> {
        self.skip_ws();
        let start = self.pos;
        let line = self.line;
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() || c == b'=' {
                break;
            }
            self.bump();
        }
        if self.pos == start {
            return None;
        }
        Some((std::str::from_utf8(&self.s[start..self.pos]).unwrap_or(""), line))
    }

    /// Reads up to and including the next `;` (or EOF), returned trimmed.
    fn statement(&mut self) -> (String, u32) {
        let line = self.line;
        let start = self.pos;
        while let Some(c) = self.bump() {
            if c == b';' {
                break;
            }
        }
        let text = String::from_utf8_lossy(&self.s[start..self.pos]);
        (text.trim().trim_end_matches(';').trim().to_owned(), line)
    }
}

/// Parses a `.wnd` file.
pub fn parse(bytes: &[u8]) -> Result<WndFile, ParseError> {
    let text = String::from_utf8_lossy(bytes);
    let mut r = Reader::new(&text);
    let mut file = WndFile::default();

    // FILE_VERSION
    let (stmt, line) = r.statement();
    let (key, value) = split_kv(&stmt);
    if !key.eq_ignore_ascii_case("FILE_VERSION") {
        return err(line, format!("expected `FILE_VERSION`, found `{}`", key));
    }
    if !value.trim_start().starts_with(|c: char| c.is_ascii_digit()) {
        return err(line, format!("bad FILE_VERSION `{value}`"));
    }
    file.file_version = leading_int(value).max(0) as u32;

    if file.file_version >= 2 {
        let (word, line) = r.scan_word().ok_or(ParseError { line, message: "missing STARTLAYOUTBLOCK".into() })?;
        if !word.eq_ignore_ascii_case("STARTLAYOUTBLOCK") {
            return err(line, format!("expected `STARTLAYOUTBLOCK`, found `{word}`"));
        }
        loop {
            let Some((word, line)) = r.scan_word() else {
                return err(r.line, "unterminated layout block");
            };
            if word.eq_ignore_ascii_case("ENDLAYOUTBLOCK") {
                break;
            }
            let (stmt, _) = r.statement();
            let v = after_eq(&stmt);
            let name = if v.eq_ignore_ascii_case("[none]") { None } else { Some(v) };
            match word.to_ascii_uppercase().as_str() {
                "LAYOUTINIT" => file.layout.init = name,
                "LAYOUTUPDATE" => file.layout.update = name,
                "LAYOUTSHUTDOWN" => file.layout.shutdown = name,
                _ => file.warnings.push((line, format!("unknown layout field `{word}`"))),
            }
        }
    }

    // Top level: default colors/font, then WINDOW trees.
    while let Some((word, line)) = r.scan_word() {
        match word.to_ascii_uppercase().as_str() {
            "WINDOW" => file.windows.push(parse_window(&mut r, &mut file.warnings)?),
            "END" => {}
            "ENABLEDCOLOR" | "DISABLEDCOLOR" | "HILITECOLOR" | "SELECTEDCOLOR" | "TEXTCOLOR"
            | "BACKGROUNDCOLOR" => {
                let (stmt, _) = r.statement();
                let v = after_eq(&stmt);
                file.defaults.colors.insert(word.to_ascii_lowercase(), parse_color(&v));
            }
            "FONT" => {
                let (stmt, _) = r.statement();
                file.defaults.font = Some(parse_font(&after_eq(&stmt)));
            }
            _ => {
                let _ = r.statement();
                file.warnings.push((line, format!("unknown top-level token `{word}`")));
            }
        }
    }

    Ok(file)
}

fn parse_window(r: &mut Reader<'_>, warnings: &mut Vec<(u32, String)>) -> Result<Window, ParseError> {
    let (stmt, line) = r.statement();
    let (key, value) = split_kv(&stmt);
    if !key.eq_ignore_ascii_case("WINDOWTYPE") {
        return err(line, format!("window missing `WINDOWTYPE`, found `{key}`"));
    }
    let mut w = Window { window_type: value.to_owned(), ..Default::default() };

    let (stmt, line) = r.statement();
    let (key, value) = split_kv(&stmt);
    if !key.eq_ignore_ascii_case("SCREENRECT") {
        return err(line, format!("window missing `SCREENRECT`, found `{key}`"));
    }
    w.rect = parse_screen_rect(value);

    loop {
        let Some((word, line)) = r.scan_word() else {
            return err(r.line, "unterminated WINDOW block");
        };
        match word.to_ascii_uppercase().as_str() {
            "END" => return Ok(w),
            "CHILD" => {
                if parse_children(r, &mut w.children, warnings)? {
                    return Ok(w);
                }
            }
            _ => {
                let (stmt, _) = r.statement();
                apply_field(&mut w, word, &after_eq(&stmt), line, warnings);
            }
        }
    }
}

/// Reads a `CHILD` list. Returns `true` when the parent's own `END` was reached
/// (no `ENDALLCHILDREN`), so the caller must not expect another token.
fn parse_children(
    r: &mut Reader<'_>,
    out: &mut Vec<Window>,
    warnings: &mut Vec<(u32, String)>,
) -> Result<bool, ParseError> {
    loop {
        let Some((word, line)) = r.scan_word() else { return Ok(true) };
        match word.to_ascii_uppercase().as_str() {
            "ENDALLCHILDREN" => return Ok(false),
            "END" => return Ok(true),
            "WINDOW" => out.push(parse_window(r, warnings)?),
            // `CHILD` separates siblings and is skipped by `parseChildWindows`.
            "CHILD" => {}
            _ => {
                let _ = r.statement();
                warnings.push((line, format!("unknown child token `{word}`")));
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Field parsing

fn apply_field(w: &mut Window, key: &str, value: &str, line: u32, warnings: &mut Vec<(u32, String)>) {
    let quoted = || parse_quoted(value);
    match key.to_ascii_uppercase().as_str() {
        "NAME" => w.name = Some(quoted()),
        "STATUS" => w.status = parse_bits(value, status::NAMES),
        "STYLE" => w.style = parse_bits(value, style::NAMES),
        "SYSTEMCALLBACK" => w.system_callback = none_if(quoted()),
        "INPUTCALLBACK" => w.input_callback = none_if(quoted()),
        "TOOLTIPCALLBACK" => w.tooltip_callback = none_if(quoted()),
        "DRAWCALLBACK" => w.draw_callback = none_if(quoted()),
        "FONT" => w.font = Some(parse_font(value)),
        "HEADERTEMPLATE" => w.header_template = none_if(quoted()),
        "TOOLTIPTEXT" => w.tooltip_text = none_if(quoted()),
        "TOOLTIPDELAY" => w.tooltip_delay = Some(leading_int(value)),
        "TEXT" => w.text = Some(quoted()),
        "TEXTCOLOR" => w.text_color = Some(parse_text_color(value)),
        "ENABLEDDRAWDATA" => w.draw_data.enabled = parse_draw_data(value),
        "DISABLEDDRAWDATA" => w.draw_data.disabled = parse_draw_data(value),
        "HILITEDRAWDATA" => w.draw_data.hilite = parse_draw_data(value),
        "IMAGEOFFSET" => {
            let nums = collect_ints(value);
            if nums.len() >= 2 {
                w.image_offset = Some((nums[0], nums[1]));
            }
        }
        "LISTBOXDATA" => w.gadget = Some(GadgetData::ListBox(parse_listbox(value))),
        "COMBOBOXDATA" => w.gadget = Some(GadgetData::ComboBox(parse_combobox(value))),
        "SLIDERDATA" => w.gadget = Some(GadgetData::Slider(parse_slider(value))),
        "RADIOBUTTONDATA" => w.gadget = Some(GadgetData::RadioButton(parse_radio(value))),
        "TEXTENTRYDATA" => w.gadget = Some(GadgetData::TextEntry(parse_text_entry(value))),
        "STATICTEXTDATA" => w.gadget = Some(GadgetData::StaticText(parse_static_text(value))),
        "TABCONTROLDATA" => w.gadget = Some(GadgetData::TabControl(parse_tab_control(value))),
        "TOOLTIP" | "DATA" => {}
        other if other.ends_with("DRAWDATA") => {
            w.draw_data.extra.insert(other.to_owned(), parse_draw_data(value));
        }
        other => warnings.push((line, format!("unknown window field `{other}`"))),
    }
}

fn none_if(s: String) -> Option<String> {
    if s.is_empty() || s.eq_ignore_ascii_case("[none]") || s.eq_ignore_ascii_case("noimage") {
        None
    } else {
        Some(s)
    }
}

/// All integers in `s`, in order (labels and punctuation skipped).
fn collect_ints(s: &str) -> Vec<i32> {
    let mut out = Vec::new();
    let mut i = 0;
    let b = s.as_bytes();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() || ((c == b'-' || c == b'+') && i + 1 < b.len() && b[i + 1].is_ascii_digit()) {
            let start = i;
            if c == b'-' || c == b'+' {
                i += 1;
            }
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            out.push(s[start..i].parse().unwrap_or(0));
        } else {
            i += 1;
        }
    }
    out
}

fn parse_color(s: &str) -> Rgba {
    let n = collect_ints(s);
    Rgba {
        r: n.first().copied().unwrap_or(0).clamp(0, 255) as u8,
        g: n.get(1).copied().unwrap_or(0).clamp(0, 255) as u8,
        b: n.get(2).copied().unwrap_or(0).clamp(0, 255) as u8,
        a: n.get(3).copied().unwrap_or(255).clamp(0, 255) as u8,
    }
}

/// `UPPERLEFT: x y, BOTTOMRIGHT: x y, CREATIONRESOLUTION: w h`.
fn parse_screen_rect(s: &str) -> ScreenRect {
    let n = collect_ints(s);
    ScreenRect {
        upper_left: (n.first().copied().unwrap_or(0), n.get(1).copied().unwrap_or(0)),
        bottom_right: (n.get(2).copied().unwrap_or(0), n.get(3).copied().unwrap_or(0)),
        creation_resolution: (n.get(4).copied().unwrap_or(800), n.get(5).copied().unwrap_or(600)),
    }
}

/// 24 integers: for each of enabled/disabled/hilite, 4 color then 4 border.
fn parse_text_color(s: &str) -> TextColors {
    let n = collect_ints(s);
    let state = |i: usize| TextState {
        color: Rgba {
            r: n.get(i * 8).copied().unwrap_or(0).clamp(0, 255) as u8,
            g: n.get(i * 8 + 1).copied().unwrap_or(0).clamp(0, 255) as u8,
            b: n.get(i * 8 + 2).copied().unwrap_or(0).clamp(0, 255) as u8,
            a: n.get(i * 8 + 3).copied().unwrap_or(0).clamp(0, 255) as u8,
        },
        border_color: Rgba {
            r: n.get(i * 8 + 4).copied().unwrap_or(0).clamp(0, 255) as u8,
            g: n.get(i * 8 + 5).copied().unwrap_or(0).clamp(0, 255) as u8,
            b: n.get(i * 8 + 6).copied().unwrap_or(0).clamp(0, 255) as u8,
            a: n.get(i * 8 + 7).copied().unwrap_or(0).clamp(0, 255) as u8,
        },
    };
    TextColors { enabled: state(0), disabled: state(1), hilite: state(2) }
}

/// `IMAGE: X, COLOR: r g b a, BORDERCOLOR: r g b a` repeated.
fn parse_draw_data(s: &str) -> Vec<DrawData> {
    let toks: Vec<&str> = s.split([':', ',', ' ', '\t', '\n', '\r', '=']).filter(|t| !t.is_empty()).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        match toks[i].to_ascii_uppercase().as_str() {
            "IMAGE" => {
                let name = toks.get(i + 1).copied().unwrap_or("NoImage");
                let mut d = DrawData { image: none_if(name.to_owned()), ..Default::default() };
                // COLOR
                let mut j = i + 2;
                if toks.get(j).is_some_and(|t| t.eq_ignore_ascii_case("COLOR")) {
                    d.color = read4(&toks, j + 1);
                    j += 5;
                }
                if toks.get(j).is_some_and(|t| t.eq_ignore_ascii_case("BORDERCOLOR")) {
                    d.border_color = read4(&toks, j + 1);
                    j += 5;
                }
                out.push(d);
                i = j;
            }
            _ => i += 1,
        }
    }
    out
}

fn read4(toks: &[&str], start: usize) -> Rgba {
    let g = |k: usize| toks.get(start + k).and_then(|t| t.parse::<i32>().ok()).unwrap_or(0).clamp(0, 255) as u8;
    Rgba { r: g(0), g: g(1), b: g(2), a: g(3) }
}

/// `NAME: "…", SIZE: n, BOLD: n`.
fn parse_font(s: &str) -> FontSpec {
    let mut f = FontSpec::default();
    // The font name may contain spaces, so pull the quoted string directly.
    if let Some(a) = s.find('"') {
        if let Some(b) = s[a + 1..].find('"') {
            f.name = s[a + 1..a + 1 + b].to_owned();
        }
    }
    let upper = s.to_ascii_uppercase();
    if let Some(i) = upper.find("SIZE") {
        f.size = leading_int(&s[i + 4..]);
    }
    if let Some(i) = upper.find("BOLD") {
        f.bold = leading_int(&s[i + 4..]);
    }
    f
}

/// `LABEL: value` pairs of a gadget data block.
fn pairs(s: &str) -> Vec<(String, String)> {
    s.split(',')
        .filter_map(|part| {
            let (k, v) = part.split_once(':')?;
            Some((k.trim().to_ascii_uppercase(), v.trim().to_owned()))
        })
        .collect()
}

fn parse_listbox(s: &str) -> ListBoxData {
    let mut d = ListBoxData { columns: 1, ..Default::default() };
    for (k, v) in pairs(s) {
        match k.as_str() {
            "LENGTH" => d.length = leading_int(&v),
            "AUTOSCROLL" => d.auto_scroll = leading_int(&v) != 0,
            "SCROLLIFATEND" => d.scroll_if_at_end = leading_int(&v) != 0,
            "AUTOPURGE" => d.auto_purge = leading_int(&v) != 0,
            "SCROLLBAR" => d.scroll_bar = leading_int(&v) != 0,
            "MULTISELECT" => d.multi_select = leading_int(&v) != 0,
            "COLUMNS" => d.columns = leading_int(&v),
            "FORCESELECT" => d.force_select = leading_int(&v) != 0,
            _ if k.contains("COLUMNWIDTH") => d.column_width_percentages.push(leading_int(&v)),
            _ => {}
        }
    }
    d
}

fn parse_combobox(s: &str) -> ComboBoxData {
    let mut d = ComboBoxData::default();
    for (k, v) in pairs(s) {
        match k.as_str() {
            "ISEDITABLE" | "EDITABLE" => d.is_editable = leading_int(&v) != 0,
            "MAXCHARS" => d.max_chars = leading_int(&v),
            "MAXDISPLAY" => d.max_display = leading_int(&v),
            "ASCIIONLY" => d.ascii_only = leading_int(&v) != 0,
            "LETTERSANDNUMBERS" | "LETTERSANDNUMBERSONLY" => d.letters_and_numbers_only = leading_int(&v) != 0,
            _ => {}
        }
    }
    d
}

fn parse_slider(s: &str) -> SliderData {
    let mut d = SliderData::default();
    for (k, v) in pairs(s) {
        match k.as_str() {
            "MINVALUE" => d.min_val = leading_int(&v),
            "MAXVALUE" => d.max_val = leading_int(&v),
            _ => {}
        }
    }
    d
}

fn parse_radio(s: &str) -> RadioButtonData {
    let mut d = RadioButtonData::default();
    for (k, v) in pairs(s) {
        if k == "GROUP" {
            d.group = leading_int(&v);
        }
    }
    d
}

fn parse_text_entry(s: &str) -> TextEntryData {
    let mut d = TextEntryData::default();
    for (k, v) in pairs(s) {
        match k.as_str() {
            "MAXLEN" => d.max_len = leading_int(&v),
            "SECRETTEXT" => d.secret_text = leading_int(&v) != 0,
            "NUMERICALONLY" => d.numerical_only = leading_int(&v) != 0,
            "ALPHANUMERICALONLY" => d.alpha_numerical_only = leading_int(&v) != 0,
            "ASCIIONLY" => d.ascii_only = leading_int(&v) != 0,
            _ => {}
        }
    }
    d
}

fn parse_static_text(s: &str) -> StaticTextData {
    let mut d = StaticTextData { centered_vertically: true, left_margin: 7, top_margin: 7, ..Default::default() };
    for (k, v) in pairs(s) {
        if k == "CENTERED" {
            d.centered = leading_int(&v) != 0;
        }
    }
    d
}

fn parse_tab_control(s: &str) -> TabControlData {
    let mut d = TabControlData::default();
    for (k, v) in pairs(s) {
        match k.as_str() {
            "TABORIENTATION" => d.tab_orientation = leading_int(&v),
            "TABEDGE" => d.tab_edge = leading_int(&v),
            "TABWIDTH" => d.tab_width = leading_int(&v),
            "TABHEIGHT" => d.tab_height = leading_int(&v),
            "TABCOUNT" => d.tab_count = leading_int(&v),
            "PANEBORDER" => d.pane_border = leading_int(&v),
            "PANEDISABLED" | "SUBPANEDISABLED" => d.sub_pane_disabled.push(leading_int(&v) != 0),
            _ => {}
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"FILE_VERSION = 2;
STARTLAYOUTBLOCK
  LAYOUTINIT = [None];
  LAYOUTUPDATE = [None];
  LAYOUTSHUTDOWN = [None];
ENDLAYOUTBLOCK
FONT = NAME: "Times New Roman", SIZE: 14, BOLD: 0;
TEXTCOLOR = 250 250 250 255;
WINDOW
  WINDOWTYPE = USER;
  SCREENRECT = UPPERLEFT: 0 416,
               BOTTOMRIGHT: 799 599,
               CREATIONRESOLUTION: 800 600;
  NAME = "ControlBar.wnd:Root";
  STATUS = ENABLED+BORDER+SEE_THRU;
  STYLE = USER;
  SYSTEMCALLBACK = "ControlBarSystem";
  HEADERTEMPLATE = "[None]";
  TEXT = "GUI:Root";
  TEXTCOLOR = ENABLED: 255 255 255 255, ENABLEDBORDER: 0 0 0 255,
              DISABLED: 192 192 192 255, DISABLEDBORDER: 64 64 64 255,
              HILITE: 128 128 255 255, HILITEBORDER: 0 0 128 255;
  ENABLEDDRAWDATA = IMAGE: NoImage, COLOR: 2 2 2 255, BORDERCOLOR: 1 1 1 255,
                    IMAGE: Buttons-Left, COLOR: 255 0 0 255, BORDERCOLOR: 0 0 0 255;
  CHILD
  WINDOW
    WINDOWTYPE = PUSHBUTTON;
    SCREENRECT = UPPERLEFT: 10 10, BOTTOMRIGHT: 60 40, CREATIONRESOLUTION: 800 600;
    NAME = "ControlBar.wnd:Btn";
    STATUS = ENABLED+IMAGE;
    STYLE = PUSHBUTTON+MOUSETRACK;
    TEXTENTRYDATA = MAXLEN: 64, SECRETTEXT: 0, NUMERICALONLY: 0, ALPHANUMERICALONLY: 0, ASCIIONLY: 0;
  END
  ENDALLCHILDREN
END
"#;

    #[test]
    fn parses_layout_defaults_and_tree() {
        let f = parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(f.file_version, 2);
        assert_eq!(f.layout.init, None);
        assert_eq!(f.defaults.font.as_ref().unwrap().size, 14);
        assert_eq!(f.windows.len(), 1);

        let root = &f.windows[0];
        assert_eq!(root.window_type, "USER");
        assert_eq!(root.name.as_deref(), Some("ControlBar.wnd:Root"));
        assert_eq!(root.rect.upper_left, (0, 416));
        assert_eq!((root.rect.width(), root.rect.height()), (799, 183));
        assert_eq!(root.status, status::ENABLED | status::BORDER | status::SEE_THRU);
        assert_eq!(root.style, style::USER);
        assert_eq!(root.system_callback.as_deref(), Some("ControlBarSystem"));
        assert_eq!(root.header_template, None);
        assert_eq!(root.text.as_deref(), Some("GUI:Root"));
        let tc = root.text_color.unwrap();
        assert_eq!(tc.enabled.color, Rgba { r: 255, g: 255, b: 255, a: 255 });
        assert_eq!(tc.hilite.border_color, Rgba { r: 0, g: 0, b: 128, a: 255 });
        assert_eq!(root.draw_data.enabled.len(), 2);
        assert_eq!(root.draw_data.enabled[0].image, None);
        assert_eq!(root.draw_data.enabled[1].image.as_deref(), Some("Buttons-Left"));

        assert_eq!(root.children.len(), 1);
        let btn = &root.children[0];
        assert_eq!(btn.window_type, "PUSHBUTTON");
        assert_eq!(btn.style, style::PUSHBUTTON | style::MOUSETRACK);
        assert_eq!(btn.gadget, Some(GadgetData::TextEntry(TextEntryData { max_len: 64, ..Default::default() })));
        assert_eq!(root.walk().count(), 2);
    }

    #[test]
    fn child_without_endallchildren_uses_parent_end() {
        let src = r#"FILE_VERSION = 2;
STARTLAYOUTBLOCK
ENDLAYOUTBLOCK
WINDOW
  WINDOWTYPE = USER;
  SCREENRECT = UPPERLEFT: 0 0, BOTTOMRIGHT: 1 1, CREATIONRESOLUTION: 1 1;
  CHILD
  WINDOW
    WINDOWTYPE = USER;
    SCREENRECT = UPPERLEFT: 0 0, BOTTOMRIGHT: 1 1, CREATIONRESOLUTION: 1 1;
  END
END
"#;
        let f = parse(src.as_bytes()).unwrap();
        assert_eq!(f.windows.len(), 1);
        assert_eq!(f.windows[0].children.len(), 1);
    }

    #[test]
    fn bad_version_is_an_error_with_a_line() {
        let e = parse(b"FILE_VERSION = x;\n").unwrap_err();
        assert_eq!(e.line, 1);
        let e = parse(b"not a wnd file").unwrap_err();
        assert_eq!(e.line, 1);
    }

    #[test]
    fn malformed_input_does_not_panic() {
        for src in [&b"FILE_VERSION = 2;"[..], b"", b"FILE_VERSION = 2;\nSTARTLAYOUTBLOCK\n", b"FILE_VERSION = 2;\nWINDOW\n"] {
            let _ = parse(src);
        }
    }
}
