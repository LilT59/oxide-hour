//! Retail mouse cursor art: `Data\INI\Mouse.ini` definitions and the Win32 `.ANI` (RIFF `ACON`)
//! files the engine loads in its default `RM_WINDOWS` redraw mode.
//!
//! Retail uses the Windows cursors by default: `GlobalData::m_winCursors` is `TRUE`
//! (`GeneralsMD/.../Common/GlobalData.cpp:669`), so `Mouse::init` selects `RM_WINDOWS`
//! (`Core/.../GameClient/Input/Mouse.cpp:583`). `Win32Mouse::initCursorResources` then preloads
//! `data\cursors\<Texture>.ANE` (`Win32Mouse.cpp:377`), one file per `Directions` entry, and
//! `Win32Mouse::setCursor` hands them to the OS. The `Image` / `HotSpot` / `Frames` / `FPS` /
//! `W3DModel` / `W3DAnim` fields only matter for the polygon / DX8 / W3D modes, which the default
//! options never select, so this module ports the Windows path only.
//!
//! The `.ANI` format is a RIFF file: `RIFF ACON` containing an `anih` header (frame count, step
//! count, default display rate in 1/60 s jiffies, direction attributes), optional `rate` and
//! `seq ` arrays, and a `LIST fram` of `icon` chunks. Each `icon` is a complete `.cur` image
//! (an `ICONDIR` + `ICONDIRENTRY` directory followed by a DIB), and the cursor hotspot lives in
//! the `ICONDIRENTRY` (`wPlanes` = hotspot x, `wBitCount` = hotspot y, as for a `.cur` file).
//! Parsing is bounds-checked and never panics; allocation counts are capped.
//!
//! This is a viewer-side feature: the sim never sees it. `ZR_CURSOR_DUMP=<dir>` writes every
//! loaded cursor frame to a PNG there (a debug aid, since the OS cursor is not part of the
//! framebuffer); `ZR_CONTEXT_DEMO` / `ZR_CURSOR_LABEL` keep the textual cursor label.

use std::collections::BTreeMap;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{CursorIcon, CustomCursor, CustomCursorImage, PrimaryWindow, SystemCursorIcon};
use zr_assets::Vfs;

/// Allocation cap on decoded frames. The engine's `MAX_2D_CURSOR_ANIM_FRAMES` (21, `Mouse.h:93`)
/// only bounds the DX8/W3D texture arrays; the default Win32 loader hands the whole `.ANI` to the
/// OS, and shipped art exceeds 21 frames (`SCCHostile3` has 33), so this is a safety cap only.
const MAX_FRAMES: usize = 256;
/// `MAX_2D_CURSOR_DIRECTIONS` (`Mouse.h:94`).
const MAX_DIRECTIONS: usize = 8;
/// A generous per-side cap for a cursor DIB (retail art is 32x32).
const MAX_SIDE: u32 = 1024;
/// Cap on `nSteps` / `seq` / `rate` entries.
const MAX_STEPS: usize = 256;

/// A retail `MouseCursor` name used by the viewer (`Mouse::CursorININames` in `Mouse.cpp:1270`).
/// Only the names the context evaluation can select are modelled.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseCursorName {
    Arrow,
    Scroll,
    Move,
    Waypoint,
    AttackObj,
    ForceAttackObj,
    ForceAttackGround,
    Select,
    GenericInvalid,
    EnterFriendly,
    EnterAggressive,
    CaptureBuilding,
    DoRepair,
    ResumeConstruction,
    GetRepaired,
    Dock,
    GetHealed,
    OutRange,
    SetRallyPoint,
}

/// `Mouse::CursorININames` (`Core/GameEngine/Source/GameClient/Input/Mouse.cpp:1270`), the names
/// `Mouse::getCursorIndex` accepts. A command button's `CursorName` / `InvalidCursorName` must be one
/// of these, else retail falls back to `Mouse::CROSS` (`InGameUI.cpp:3091`). The `ALLOW_SURRENDER` and
/// `ALLOW_DEMORALIZE` names are left out: those defines are not set in this port's build.
pub const INI_NAMES: &[&str] = &[
    "Normal", "Arrow", "Scroll", "Target", "Move", "AttackMove", "AttackObj", "ForceAttackObj", "ForceAttackGround",
    "Build", "InvalidBuild", "GenericInvalid", "Select", "EnterFriendly", "EnterAggressive", "SetRallyPoint",
    "GetRepaired", "GetHealed", "DoRepair", "ResumeConstruction", "CaptureBuilding", "SnipeVehicle", "LaserGuidedMissiles",
    "TankHunterTNTAttack", "StabAttack", "PlaceRemoteCharge", "PlaceTimedCharge", "Defector", "Dock", "FireFlame", "FireBomb",
    "PlaceBeacon", "DisguiseAsVehicle", "Waypoint", "OutRange", "StabAttackInvalid", "PlaceChargeInvalid", "Hack",
    "ParticleUplinkCannon",
];

/// `Mouse::getCursorIndex` (`Mouse.cpp:1263`): the canonical retail name for `name` (case-insensitive),
/// or `None` for an empty or unknown name (`INVALID_MOUSE_CURSOR`).
pub fn mouse_cursor_name(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return None;
    }
    INI_NAMES.iter().copied().find(|n| n.eq_ignore_ascii_case(name))
}

/// What the viewer asks the OS for: a `Mouse.ini` cursor by its retail name, or `Mouse::CROSS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorRequest {
    /// A `Mouse.ini` cursor, named as in [`INI_NAMES`].
    Named(&'static str),
    /// `Mouse::CROSS`: the system crosshair (`Win32DIMouse.cpp:479`, `IDC_CROSS`), used when a
    /// pending command's cursor name does not resolve.
    Cross,
}

impl From<MouseCursorName> for CursorRequest {
    fn from(name: MouseCursorName) -> Self {
        CursorRequest::Named(name.ini_name())
    }
}

impl MouseCursorName {
    /// The name as written in `Mouse.ini` (case-insensitive lookup).
    pub fn ini_name(self) -> &'static str {
        match self {
            MouseCursorName::Arrow => "Arrow",
            MouseCursorName::Scroll => "Scroll",
            MouseCursorName::Move => "Move",
            MouseCursorName::Waypoint => "Waypoint",
            MouseCursorName::AttackObj => "AttackObj",
            MouseCursorName::ForceAttackObj => "ForceAttackObj",
            MouseCursorName::ForceAttackGround => "ForceAttackGround",
            MouseCursorName::Select => "Select",
            MouseCursorName::GenericInvalid => "GenericInvalid",
            MouseCursorName::EnterFriendly => "EnterFriendly",
            MouseCursorName::EnterAggressive => "EnterAggressive",
            MouseCursorName::CaptureBuilding => "CaptureBuilding",
            MouseCursorName::DoRepair => "DoRepair",
            MouseCursorName::ResumeConstruction => "ResumeConstruction",
            MouseCursorName::GetRepaired => "GetRepaired",
            MouseCursorName::Dock => "Dock",
            MouseCursorName::SetRallyPoint => "SetRallyPoint",
            MouseCursorName::GetHealed => "GetHealed",
            MouseCursorName::OutRange => "OutRange",
        }
    }
}

/// One `MouseCursor` block from `Mouse.ini`. Only the fields the Win32 path reads are kept:
/// `Texture` (the `.ANI` base name) and `Directions` (`Mouse.cpp:73` field table).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorDef {
    pub name: String,
    pub texture: String,
    pub directions: usize,
}

/// Parses the `MouseCursor` blocks of a `Mouse.ini`. `Mouse` and `MouseCursor` are both known
/// block types to `zr_ini`, so the whole file parses; unknown fields are ignored.
pub fn parse_mouse_ini(text: &str) -> Result<BTreeMap<String, CursorDef>, zr_ini::ParseError> {
    let blocks = zr_ini::parse("Mouse.ini", text)?;
    let mut out = BTreeMap::new();
    for b in blocks.iter().filter(|b| b.kind.eq_ignore_ascii_case("MouseCursor")) {
        let Some(name) = b.header.first() else { continue };
        // An empty `Texture` (or a missing one) means this cursor has no 2D Windows art.
        let texture = b.value("Texture").unwrap_or("").trim().to_owned();
        let directions = b.value("Directions").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, MAX_DIRECTIONS);
        out.insert(name.to_ascii_lowercase(), CursorDef { name: name.clone(), texture, directions });
    }
    Ok(out)
}

/// A decoded `.ANI`: one direction's frames plus a step table (frame index, seconds).
#[derive(Clone, Debug)]
pub struct AniCursor {
    pub width: u32,
    pub height: u32,
    /// `ICONDIRENTRY` hotspot (`wPlanes`, `wBitCount`).
    pub hotspot: (u16, u16),
    /// RGBA8 frames, top row first.
    pub frames: Vec<Vec<u8>>,
    /// `(frame index, duration seconds)` per step, in display order.
    pub steps: Vec<(usize, f32)>,
}

impl AniCursor {
    /// The total animation length in seconds.
    #[cfg(test)]
    fn total_secs(&self) -> f32 {
        self.steps.iter().map(|s| s.1).sum()
    }

    /// The frame for animation time `t` seconds, looping (`MouseInfo::loop` defaults to `TRUE`).
    #[cfg(test)]
    pub fn frame_at(&self, t: f32) -> usize {
        if self.steps.is_empty() {
            return 0;
        }
        let total = self.total_secs();
        if total <= 0.0 {
            return self.steps[0].0;
        }
        let mut acc = t.rem_euclid(total);
        for &(frame, dur) in &self.steps {
            if acc < dur {
                return frame;
            }
            acc -= dur;
        }
        self.steps.last().unwrap().0
    }
}

/// Bounds-checked little-endian reads over a byte slice.
struct R<'a> {
    d: &'a [u8],
}

impl<'a> R<'a> {
    fn new(d: &'a [u8]) -> Self {
        Self { d }
    }
    fn u16(&self, off: usize) -> Option<u16> {
        let b = self.d.get(off..off + 2)?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&self, off: usize) -> Option<u32> {
        let b = self.d.get(off..off + 4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i32(&self, off: usize) -> Option<i32> {
        self.u32(off).map(|v| v as i32)
    }
    fn tag(&self, off: usize) -> Option<[u8; 4]> {
        self.d.get(off..off + 4).map(|b| [b[0], b[1], b[2], b[3]])
    }
}

/// Parses one `.ANI` file. Returns `Err` (never panics) on malformed or truncated input.
pub fn parse_ani(bytes: &[u8]) -> Result<AniCursor, String> {
    let r = R::new(bytes);
    if r.tag(0) != Some(*b"RIFF") || r.tag(8) != Some(*b"ACON") {
        return Err("not a RIFF/ACON .ani".into());
    }
    let riff_size = r.u32(4).ok_or("truncated RIFF size")? as usize;
    // The declared size excludes the 8-byte RIFF header; clamp to the buffer.
    let end = (riff_size + 8).min(bytes.len());

    let mut anih: Option<[u32; 9]> = None;
    let mut rate: Vec<u32> = Vec::new();
    let mut seq: Vec<u32> = Vec::new();
    // Icon chunk bodies in file order.
    let mut icon_bodies: Vec<&[u8]> = Vec::new();

    let mut pos = 12;
    while pos + 8 <= end {
        let id = r.tag(pos).ok_or("chunk id")?;
        let size = r.u32(pos + 4).ok_or("chunk size")? as usize;
        let data = pos + 8;
        let data_end = data.checked_add(size).ok_or("chunk size overflow")?.min(end);
        match &id {
            b"anih" => {
                if data_end < data + 36 {
                    return Err("short anih".into());
                }
                let mut a = [0u32; 9];
                for (i, v) in a.iter_mut().enumerate() {
                    *v = r.u32(data + i * 4).ok_or("anih field")?;
                }
                anih = Some(a);
            }
            b"rate" => {
                let n = size / 4;
                for i in 0..n.min(MAX_STEPS) {
                    rate.push(r.u32(data + i * 4).ok_or("rate entry")?);
                }
            }
            b"seq " => {
                let n = size / 4;
                for i in 0..n.min(MAX_STEPS) {
                    seq.push(r.u32(data + i * 4).ok_or("seq entry")?);
                }
            }
            b"LIST" => {
                let list_type = r.tag(data).ok_or("LIST type")?;
                if &list_type == b"fram" {
                    let mut sub = data + 4;
                    while sub + 8 <= data_end {
                        let sid = r.tag(sub).ok_or("frame chunk id")?;
                        let ssize = r.u32(sub + 4).ok_or("frame chunk size")? as usize;
                        let sdata = sub + 8;
                        let sdata_end = sdata.checked_add(ssize).ok_or("frame size overflow")?.min(data_end);
                        if &sid == b"icon" {
                            icon_bodies.push(bytes.get(sdata..sdata_end).ok_or("icon out of bounds")?);
                        }
                        sub = sdata_end + (ssize & 1);
                    }
                }
            }
            b"icon" => {
                // Older `ACON` files store the first frame as a top-level icon chunk.
                icon_bodies.push(bytes.get(data..data_end).ok_or("icon out of bounds")?);
            }
            _ => {}
        }
        pos = data_end + (size & 1);
    }

    let a = anih.ok_or("missing anih")?;
    let n_frames = a[1] as usize;
    let n_steps = a[2] as usize;
    let disp_rate = a[7];
    if n_frames == 0 || n_frames > MAX_FRAMES {
        return Err(format!("frame count {n_frames} out of range"));
    }
    let n_steps = n_steps.clamp(1, MAX_STEPS);

    let mut frames = Vec::with_capacity(icon_bodies.len().min(n_frames));
    let mut hotspot = (0u16, 0u16);
    let mut width = 0u32;
    let mut height = 0u32;
    for (i, body) in icon_bodies.iter().enumerate() {
        if i >= n_frames {
            break;
        }
        let (rgba, w, h, ihotspot) = parse_icon(body)?;
        if i == 0 {
            hotspot = ihotspot;
            width = w;
            height = h;
        }
        frames.push(rgba);
    }
    if frames.is_empty() {
        return Err("no icon frames".into());
    }

    // Step -> frame, and per-step duration. `seq` defaults to the identity; `rate` defaults to
    // `iDispRate` jiffies (1/60 s) for every step.
    let mut steps = Vec::with_capacity(n_steps);
    for s in 0..n_steps {
        let f = if seq.is_empty() { s } else { *seq.get(s).ok_or("short seq")? as usize };
        if f >= frames.len() {
            return Err(format!("seq step {s} frame {f} out of range"));
        }
        let jiffies = if rate.is_empty() { disp_rate } else { *rate.get(s).ok_or("short rate")? };
        let secs = (jiffies.max(1) as f32) / 60.0;
        steps.push((f, secs));
    }
    Ok(AniCursor { width, height, hotspot, frames, steps })
}

/// Within an `icon` chunk body, returns the offset of the DIB relative to the body and the
/// hotspot from the `.cur` directory (if present).
fn icon_dib_offset(body: &[u8]) -> Result<(usize, (u16, u16)), String> {
    let r = R::new(body);
    if body.len() >= 6 {
        let reserved = r.u16(0).ok_or("icondir")?;
        let kind = r.u16(2).ok_or("icondir")?;
        let count = r.u16(4).ok_or("icondir")?;
        if reserved == 0 && (kind == 1 || kind == 2) && count >= 1 {
            // ICONDIRENTRY is 16 bytes (the `.cur` layout, with `dwImageOffset`).
            if body.len() < 6 + 16 {
                return Err("short icondir entry".into());
            }
            let hotspot = (r.u16(10).ok_or("hotspot")?, r.u16(12).ok_or("hotspot")?);
            let img_off = r.u32(6 + 12).ok_or("image offset")? as usize;
            if img_off < body.len() {
                return Ok((img_off, hotspot));
            }
            // Fall through to a raw DIB search if the pointer is bogus.
        }
    }
    // Raw DIB: `biSize` is 40 or a known larger `BITMAPV*` header size.
    if r.u32(0).is_some_and(|s| (40..=124).contains(&s)) {
        return Ok((0, (0, 0)));
    }
    Err("icon is neither a .cur directory nor a DIB".into())
}

/// A decoded icon: RGBA8 pixels, width, height and hotspot.
type ParsedIcon = (Vec<u8>, u32, u32, (u16, u16));

/// Decodes a `.cur` image body (ICONDIR optional + DIB) to `(rgba, width, height)`.
fn parse_icon(body: &[u8]) -> Result<ParsedIcon, String> {
    let (off, hotspot) = icon_dib_offset(body)?;
    let (rgba, w, h) = decode_dib(body, off)?;
    Ok((rgba, w, h, hotspot))
}

/// Decodes a DIB (`BITMAPINFOHEADER` + palette + XOR bitmap + optional AND mask) to RGBA8.
fn decode_dib(data: &[u8], off: usize) -> Result<(Vec<u8>, u32, u32), String> {
    let r = R::new(data);
    let bi_size = r.u32(off).ok_or("biSize")? as usize;
    if !(40..=124).contains(&bi_size) {
        return Err(format!("unsupported DIB header size {bi_size}"));
    }
    let width = r.i32(off + 4).ok_or("biWidth")?;
    let raw_h = r.i32(off + 8).ok_or("biHeight")?;
    let bpp = r.u16(off + 14).ok_or("biBitCount")?;
    let compression = r.u32(off + 16).ok_or("biCompression")?;
    let clr_used = r.u32(off + 32).ok_or("biClrUsed")?;
    if compression != 0 {
        return Err(format!("unsupported DIB compression {compression}"));
    }
    if width <= 0 || raw_h == 0 || width as i64 > MAX_SIDE as i64 || (raw_h as i64).abs() > MAX_SIDE as i64 {
        return Err(format!("bad DIB dimensions {width}x{raw_h}"));
    }
    if !matches!(bpp, 1 | 4 | 8 | 24 | 32) {
        return Err(format!("unsupported DIB bit depth {bpp}"));
    }
    let w = width as usize;
    let full_h = (raw_h as i64).unsigned_abs() as usize;
    let top_down = raw_h < 0;

    let palette_colors = if clr_used != 0 {
        clr_used as usize
    } else if bpp <= 8 {
        1usize << bpp
    } else {
        0
    };
    let palette_off = off + bi_size;
    let palette_end = palette_off.checked_add(palette_colors * 4).ok_or("palette overflow")?;
    if palette_end > data.len() {
        return Err("palette out of bounds".into());
    }
    let mut palette = Vec::with_capacity(palette_colors);
    for i in 0..palette_colors {
        let p = palette_off + i * 4;
        palette.push([data[p + 2], data[p + 1], data[p]]); // stored B,G,R -> r,g,b
    }

    let stride = (w * bpp as usize).div_ceil(32) * 4;
    let mask_stride = w.div_ceil(32) * 4;

    // Cursor DIBs store the XOR bitmap and the AND mask stacked, so `biHeight` is doubled. If the
    // data really holds both, split it; otherwise treat the whole height as the XOR bitmap.
    let (xor_h, has_mask) = if full_h % 2 == 0 {
        let xor_h = full_h / 2;
        let need = palette_end + stride * xor_h + mask_stride * xor_h;
        if xor_h > 0 && need <= data.len() {
            (xor_h, true)
        } else {
            (full_h, false)
        }
    } else {
        (full_h, false)
    };

    let xor_off = palette_end;
    let mask_off = xor_off + stride * xor_h;
    if xor_off + stride * xor_h > data.len() {
        return Err("XOR bitmap out of bounds".into());
    }


    let mut rgba = vec![0u8; w * xor_h * 4];
    let mut all_alpha_zero = bpp == 32;
    for y in 0..xor_h {
        let src_row = if top_down { y } else { xor_h - 1 - y };
        for x in 0..w {
            let (cr, cg, cb, mut ca) = match bpp {
                32 => {
                    let p = xor_off + src_row * stride + x * 4;
                    let ca = data[p + 3];
                    if ca != 0 {
                        all_alpha_zero = false;
                    }
                    (data[p + 2], data[p + 1], data[p], ca)
                }
                24 => {
                    let p = xor_off + src_row * stride + x * 3;
                    (data[p + 2], data[p + 1], data[p], 255)
                }
                _ => {
                    let idx = match bpp {
                        8 => data[xor_off + src_row * stride + x] as usize,
                        4 => {
                            let b = data[xor_off + src_row * stride + x / 2];
                            if x % 2 == 0 {
                                (b >> 4) as usize
                            } else {
                                (b & 0xf) as usize
                            }
                        }
                        1 => {
                            let b = data[xor_off + src_row * stride + x / 8];
                            ((b >> (7 - (x % 8))) & 1) as usize
                        }
                        _ => unreachable!(),
                    };
                    let c = palette.get(idx).copied().unwrap_or([0, 0, 0]);
                    (c[0], c[1], c[2], 255)
                }
            };
            if has_mask && bpp != 32 {
                let p = mask_off + src_row * mask_stride + x / 8;
                if data.get(p).is_some_and(|b| (b >> (7 - (x % 8))) & 1 == 1) {
                    ca = 0;
                }
            }
            let o = (y * w + x) * 4;
            rgba[o] = cr;
            rgba[o + 1] = cg;
            rgba[o + 2] = cb;
            rgba[o + 3] = ca;
        }
    }

    // 32bpp cursors that never set alpha are legacy 32bpp art: fall back to the AND mask so they
    // are not drawn fully transparent.
    if bpp == 32 && all_alpha_zero && has_mask {
        for y in 0..xor_h {
            let src_row = if top_down { y } else { xor_h - 1 - y };
            for x in 0..w {
                let p = mask_off + src_row * mask_stride + x / 8;
                let transparent = data.get(p).is_some_and(|b| (b >> (7 - (x % 8))) & 1 == 1);
                rgba[(y * w + x) * 4 + 3] = if transparent { 0 } else { 255 };
            }
        }
    }
    Ok((rgba, w as u32, xor_h as u32))
}

/// All cursors loaded for this game, keyed by the lowercased `Mouse.ini` name.
#[derive(Resource, Default)]
pub struct MouseCursors {
    pub cursors: BTreeMap<String, LoadedCursor>,
}

/// One `MouseCursor` with one animation per direction (`Directions`, default 1).
#[derive(Default, Clone)]
pub struct LoadedCursor {
    pub dirs: Vec<LoadedDir>,
}

/// One direction's animation: RGBA frames and the step table (`(frame index, seconds)`).
#[derive(Clone)]
pub struct LoadedDir {
    pub width: u32,
    pub height: u32,
    pub hotspot: (u16, u16),
    pub frames: Vec<Handle<Image>>,
    pub steps: Vec<(usize, f32)>,
}

impl MouseCursors {
    pub fn get(&self, name: MouseCursorName) -> Option<&LoadedCursor> {
        self.cursors.get(&name.ini_name().to_ascii_lowercase())
    }

    pub fn is_empty(&self) -> bool {
        self.cursors.is_empty()
    }

    /// The arrow, the universal fallback. `None` only when nothing loaded.
    pub fn arrow(&self) -> Option<&LoadedCursor> {
        self.get(MouseCursorName::Arrow)
    }

    /// The `Mouse.ini` cursor with this retail name (case-insensitive), or the arrow when it has no art.
    pub fn resolve_named(&self, name: &str) -> Option<&LoadedCursor> {
        self.cursors.get(&name.to_ascii_lowercase()).or_else(|| self.arrow())
    }
}

/// Loads every `MouseCursor` definition from the install. Missing art is skipped with a warning,
/// as the engine asserts but must not crash the viewer.
fn load_cursors(vfs: &Vfs, defs: &BTreeMap<String, CursorDef>, images: &mut Assets<Image>) -> MouseCursors {
    let mut out = MouseCursors::default();
    let mut loaded = 0usize;
    let mut missing = 0usize;
    let mut dump = std::env::var_os("ZR_CURSOR_DUMP").map(std::path::PathBuf::from);
    for (key, def) in defs {
        if def.texture.is_empty() {
            continue;
        }
        let mut dirs = Vec::new();
        for dir in 0..def.directions {
            let path = if def.directions > 1 {
                format!(r"data\cursors\{}{}.ani", def.texture, dir)
            } else {
                format!(r"data\cursors\{}.ani", def.texture)
            };
            let Some(bytes) = vfs.read(&path) else {
                missing += 1;
                continue;
            };
            match parse_ani(&bytes) {
                Ok(ani) => {
                    if let Some(dirpath) = dump.as_mut() {
                        dump_ani(dirpath, &def.name, dir, &ani);
                    }
                    let frames = ani
                        .frames
                        .iter()
                        .map(|f| {
                            images.add(Image::new(
                                Extent3d { width: ani.width.max(1), height: ani.height.max(1), depth_or_array_layers: 1 },
                                TextureDimension::D2,
                                f.clone(),
                                TextureFormat::Rgba8UnormSrgb,
                                RenderAssetUsages::default(),
                            ))
                        })
                        .collect();
                    dirs.push(LoadedDir { width: ani.width, height: ani.height, hotspot: ani.hotspot, frames, steps: ani.steps });
                    loaded += 1;
                }
                Err(e) => {
                    missing += 1;
                    warn!("cursor {}: {path}: {e}", def.name);
                }
            }
        }
        if !dirs.is_empty() {
            out.cursors.insert(key.clone(), LoadedCursor { dirs });
        }
    }
    info!("cursor: loaded {loaded} .ani directions across {} cursors ({missing} missing/unparsed)", out.cursors.len());
    out
}

/// `ZR_CURSOR_DUMP`: writes one PNG per frame (`<name>_d<dir>_f<frame>.png`). Debug aid only, so
/// the cursors can be checked without the OS cursor being part of a screenshot.
fn dump_ani(dir: &std::path::Path, name: &str, direction: usize, ani: &AniCursor) {
    let safe: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let png = std::path::Path::new(dir).join(format!("lane2b_{safe}_d{direction}.png"));
    if let Err(e) = write_png_montage(&png, ani) {
        warn!("cursor dump {}: {e}", png.display());
    }
}

// ---- viewer glue -------------------------------------------------------------------------------

/// The cursor the frame's hover asked for, with its scroll direction; `None` means "leave the OS cursor".
#[derive(Resource, Default)]
pub struct RequestedCursor(pub Option<(CursorRequest, usize)>);

/// Animation state: which cursor/direction is applied, the current step and its elapsed time.
#[derive(Resource, Default)]
struct CursorAnim {
    current: Option<(CursorRequest, usize)>,
    step: usize,
    elapsed: f32,
    applied_frame: Option<usize>,
}

/// Registers the cursor resources and the per-frame animation system.
pub fn add_systems(app: &mut App) {
    app.init_resource::<MouseCursors>();
    app.init_resource::<RequestedCursor>();
    app.init_resource::<CursorAnim>();
    app.add_systems(OnEnter(crate::shell::ShellState::InGame), setup.after(crate::hud::setup_hud));
    app.add_systems(
        Update,
        animate
            .after(crate::context_cmd::update_cursor)
            .after(crate::sim_view::sim_step)
            .run_if(in_state(crate::shell::ShellState::InGame)),
    );
}

/// `OnEnter(InGame)`: read `Mouse.ini` + load the `.ANI` files after the HUD has mounted the VFS.
fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>, hud: Option<Res<crate::hud::HudData>>) {
    // Fresh animation state each game build: the previous handles are gone, so force a re-insert.
    commands.insert_resource(CursorAnim::default());
    commands.insert_resource(RequestedCursor::default());
    // `hud::HudData` owns the mounted Zero Hour VFS; reuse it when present, otherwise mount one
    // (so the cursors still load with the HUD disabled, e.g. under a screenshot flag).
    let owned;
    let vfs: Option<&Vfs> = if let Some(h) = &hud {
        Some(&h.vfs)
    } else {
        owned = zr_assets::Installs::detect().zero_hour.and_then(|i| Vfs::zero_hour(&i).ok());
        owned.as_ref()
    };
    let Some(vfs) = vfs else {
        warn!("cursor: no Zero Hour install; using the system cursor");
        commands.insert_resource(MouseCursors::default());
        return;
    };
    let Some(bytes) = vfs.read(r"data\ini\mouse.ini") else {
        warn!("cursor: Mouse.ini not found; using the system cursor");
        commands.insert_resource(MouseCursors::default());
        return;
    };
    let text = String::from_utf8_lossy(&bytes);
    let cursors = match parse_mouse_ini(&text) {
        Ok(defs) => load_cursors(vfs, &defs, &mut images),
        Err(e) => {
            warn!("cursor: Mouse.ini parse failed: {e}");
            MouseCursors::default()
        }
    };
    commands.insert_resource(cursors);
}

/// Advances the animation and, when the frame changes, sets the Bevy custom cursor. Falls back to
/// the system arrow when no art is loaded.
#[allow(clippy::too_many_arguments)]
fn animate(
    time: Res<Time<Real>>,
    requested: Res<RequestedCursor>,
    cursors: Res<MouseCursors>,
    mut state: ResMut<CursorAnim>,
    window: Query<(Entity, &Window), With<PrimaryWindow>>,
    mut commands: Commands,
) {
    let Ok((entity, _)) = window.single() else { return };
    let Some((request, dir)) = requested.0 else { return };

    // `Mouse::CROSS` is the system crosshair, so it does not depend on the `.ANI` art.
    let name = match request {
        CursorRequest::Cross => {
            if state.current != Some((request, dir)) {
                state.current = Some((request, dir));
                state.applied_frame = None;
                info!("cursor: CROSS (system crosshair)");
                commands.entity(entity).insert(CursorIcon::System(SystemCursorIcon::Crosshair));
            }
            return;
        }
        CursorRequest::Named(name) => name,
    };

    if cursors.is_empty() {
        // No retail art: keep whatever system cursor the window already has.
        state.current = None;
        return;
    }
    let Some(cursor) = cursors.resolve_named(name) else { return };
    if cursor.dirs.is_empty() {
        return;
    }
    let dir = dir.min(cursor.dirs.len().saturating_sub(1));
    let anim = &cursor.dirs[dir];

    if state.current != Some((request, dir)) {
        state.current = Some((request, dir));
        state.step = 0;
        state.elapsed = 0.0;
        state.applied_frame = None;
        info!("cursor: {name} (dir {dir})");
    }
    state.elapsed += time.delta_secs();
    // Advance the step table, looping.
    if anim.steps.is_empty() {
        return;
    }
    loop {
        let (_, dur) = anim.steps[state.step.min(anim.steps.len() - 1)];
        if dur > 0.0 && state.elapsed >= dur {
            state.elapsed -= dur;
            state.step = (state.step + 1) % anim.steps.len();
        } else {
            break;
        }
    }
    let frame = anim.steps[state.step.min(anim.steps.len() - 1)].0.min(anim.frames.len().saturating_sub(1));
    // A cursor/direction change resets `applied_frame` to `None`, so the first frame after a change
    // is always inserted even when its index is the same as the previous cursor's.
    if state.applied_frame == Some(frame) {
        return;
    }
    state.applied_frame = Some(frame);
    let handle = anim.frames[frame].clone();
    let hotspot = anim.hotspot;
    // Custom cursors must have their hotspot inside the image.
    let hotspot = (hotspot.0.min(anim.width.saturating_sub(1) as u16), hotspot.1.min(anim.height.saturating_sub(1) as u16));
    commands.entity(entity).insert(CursorIcon::Custom(CustomCursor::Image(CustomCursorImage {
        handle,
        texture_atlas: None,
        flip_x: false,
        flip_y: false,
        rect: None,
        hotspot,
    })));
}

// ---- minimal PNG writer (debug dump only) ------------------------------------------------------

/// Writes every frame of `ani` into a single vertical montage PNG. Dependency-free: PNG with
/// stored (uncompressed) DEFLATE blocks, so it only needs crc32 and adler32.
fn write_png_montage(path: &std::path::Path, ani: &AniCursor) -> std::io::Result<()> {
    let w = ani.width as usize;
    let h = ani.height as usize;
    if w == 0 || h == 0 || ani.frames.is_empty() {
        return Ok(());
    }
    let n = ani.frames.len();
    let out_h = h * n;
    let mut raw = Vec::with_capacity((w * 4 + 1) * out_h);
    for f in &ani.frames {
        for y in 0..h {
            raw.push(0); // filter: none
            let row = &f[y * w * 4..(y + 1) * w * 4];
            raw.extend_from_slice(row);
        }
    }
    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(out_h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, png)
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc32::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// A zlib stream whose DEFLATE payload is all "stored" blocks (BTYPE=00). The 3-bit block header
/// pads to the next byte, so every block begins on a byte boundary and the header byte is just the
/// `BFINAL` bit (0 or 1).
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut blocks = data.chunks(0xffff).peekable();
    if blocks.peek().is_none() {
        out.extend_from_slice(&[1, 0x00, 0x00, 0xff, 0xff]);
    }
    while let Some(block) = blocks.next() {
        let final_block = blocks.peek().is_none();
        out.push(u8::from(final_block));
        out.extend_from_slice(&(block.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

struct Crc32(u32);

impl Crc32 {
    fn new() -> Self {
        Self(0xffff_ffff)
    }
    fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.0 ^= b as u32;
            for _ in 0..8 {
                let mask = (self.0 & 1).wrapping_neg();
                self.0 = (self.0 >> 1) ^ (0xedb8_8320 & mask);
            }
        }
    }
    fn finish(self) -> u32 {
        self.0 ^ 0xffff_ffff
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a one- or multi-frame `.ANI` with an `ICONDIR`/DIB icon per frame. Each frame is an
    /// `(xor, and_mask)` pair of raw bitmaps.
    #[allow(clippy::too_many_arguments)]
    fn build_ani(bit_count: u16, width: u16, height: u16, hotspot: (u16, u16), frames: &[(Vec<u8>, Vec<u8>)], rates: &[u32], seq: &[u32], disp: u32) -> Vec<u8> {
        let mut icon_bodies = Vec::new();
        for (xor, mask) in frames {
            let mut body = Vec::new();
            // ICONDIR + one ICONDIRENTRY pointing past the 22-byte directory.
            body.extend_from_slice(&0u16.to_le_bytes()); // reserved
            body.extend_from_slice(&2u16.to_le_bytes()); // type: cursor
            body.extend_from_slice(&1u16.to_le_bytes()); // count
            body.push(width as u8);
            body.push(height as u8);
            body.push(0);
            body.push(0);
            body.extend_from_slice(&hotspot.0.to_le_bytes());
            body.extend_from_slice(&hotspot.1.to_le_bytes());
            let dib = build_dib(bit_count, width, height, xor, mask);
            body.extend_from_slice(&(dib.len() as u32).to_le_bytes()); // dwBytesInRes
            body.extend_from_slice(&22u32.to_le_bytes()); // dwImageOffset
            body.extend_from_slice(&dib);
            icon_bodies.push(body);
        }
        let mut riff = Vec::new();
        riff.extend_from_slice(b"ACON");
        // anih
        riff.extend_from_slice(b"anih");
        riff.extend_from_slice(&36u32.to_le_bytes());
        riff.extend_from_slice(&36u32.to_le_bytes());
        riff.extend_from_slice(&(frames.len() as u32).to_le_bytes());
        let steps = if seq.is_empty() { frames.len() } else { seq.len() };
        riff.extend_from_slice(&(steps as u32).to_le_bytes());
        riff.extend_from_slice(&0u32.to_le_bytes());
        riff.extend_from_slice(&0u32.to_le_bytes());
        riff.extend_from_slice(&(bit_count as u32).to_le_bytes());
        riff.extend_from_slice(&1u32.to_le_bytes());
        riff.extend_from_slice(&disp.to_le_bytes());
        riff.extend_from_slice(&1u32.to_le_bytes());
        if !rates.is_empty() {
            riff.extend_from_slice(b"rate");
            riff.extend_from_slice(&((rates.len() * 4) as u32).to_le_bytes());
            for r in rates {
                riff.extend_from_slice(&r.to_le_bytes());
            }
        }
        if !seq.is_empty() {
            riff.extend_from_slice(b"seq ");
            riff.extend_from_slice(&((seq.len() * 4) as u32).to_le_bytes());
            for s in seq {
                riff.extend_from_slice(&s.to_le_bytes());
            }
        }
        // LIST fram { icon ... }
        let mut fram = Vec::new();
        fram.extend_from_slice(b"fram");
        for body in &icon_bodies {
            fram.extend_from_slice(b"icon");
            fram.extend_from_slice(&(body.len() as u32).to_le_bytes());
            fram.extend_from_slice(body);
            if body.len() % 2 == 1 {
                fram.push(0);
            }
        }
        riff.extend_from_slice(b"LIST");
        riff.extend_from_slice(&(fram.len() as u32).to_le_bytes());
        riff.extend_from_slice(&fram);

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(riff.len() as u32).to_le_bytes());
        out.extend_from_slice(&riff);
        out
    }

    /// A `BITMAPINFOHEADER` + palette + XOR bitmap + AND mask.
    fn build_dib(bit_count: u16, width: u16, height: u16, xor: &[u8], mask: &[u8]) -> Vec<u8> {
        let w = width as usize;
        let h = height as usize;
        let xor_stride = (w * bit_count as usize).div_ceil(32) * 4;
        let mask_stride = w.div_ceil(32) * 4;
        let palette = if bit_count <= 8 { 1usize << bit_count } else { 0 };
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&(width as i32).to_le_bytes());
        dib.extend_from_slice(&((height as i32) * 2).to_le_bytes()); // XOR + AND
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&bit_count.to_le_bytes());
        dib.extend_from_slice(&0u32.to_le_bytes());
        dib.extend_from_slice(&(((xor_stride + mask_stride) * h) as u32).to_le_bytes());
        dib.extend_from_slice(&0u32.to_le_bytes());
        dib.extend_from_slice(&0u32.to_le_bytes());
        dib.extend_from_slice(&(palette as u32).to_le_bytes());
        dib.extend_from_slice(&0u32.to_le_bytes());
        for _ in 0..palette {
            dib.extend_from_slice(&[0, 0, 0, 0]);
        }
        dib.extend_from_slice(xor);
        dib.resize(dib.len() - xor.len() + xor_stride * h, 0);
        let mut mask_padded = mask.to_vec();
        mask_padded.resize(mask_stride * h, 0);
        dib.extend_from_slice(&mask_padded);
        dib
    }

    /// A 4bpp, 2x1 cursor: palette index 1 on the left (opaque) and AND-masked on the right.
    fn one_pixel_4bpp() -> (Vec<u8>, Vec<u8>) {
        // Row is bottom-up; one row. Left pixel index 1 (palette), right pixel 0 with the mask's
        // last bit set -> transparent.
        let mut xor = vec![0u8; 4];
        xor[0] = 0x10; // left nibble 1, right nibble 0
        let mut mask = vec![0u8; 4];
        mask[0] = 0x40; // bit for x=1 set -> transparent
        (xor, mask)
    }

    #[test]
    fn parses_single_frame_4bpp() {
        let (xor, mask) = one_pixel_4bpp();
        let ani = build_ani(4, 2, 1, (1, 0), &[(xor, mask)], &[], &[], 10);
        let c = parse_ani(&ani).unwrap();
        assert_eq!((c.width, c.height), (2, 1));
        assert_eq!(c.hotspot, (1, 0));
        assert_eq!(c.frames.len(), 1);
        // Palette entry 1 is black in the synthetic palette; left is opaque, right transparent.
        assert_eq!(&c.frames[0][0..4], &[0, 0, 0, 255]);
        assert_eq!(c.frames[0][7], 0);
        assert_eq!(c.steps, vec![(0, 10.0 / 60.0)]);
    }

    #[test]
    fn follows_seq_and_rate() {
        let p = one_pixel_4bpp();
        let ani = build_ani(4, 2, 1, (0, 0), &[p.clone(), p.clone(), p], &[4, 8], &[2, 0], 10);
        let c = parse_ani(&ani).unwrap();
        assert_eq!(c.frames.len(), 3);
        assert_eq!(c.steps, vec![(2, 4.0 / 60.0), (0, 8.0 / 60.0)]);
        assert_eq!(c.frame_at(0.0), 2);
        assert_eq!(c.frame_at(5.0 / 60.0), 0);
        // Loops (13/60 s is one jiffy past the 12/60 s cycle).
        assert_eq!(c.frame_at(13.0 / 60.0), 2);
    }

    #[test]
    fn truncated_is_error_not_panic() {
        let p = one_pixel_4bpp();
        let ani = build_ani(4, 2, 1, (0, 0), &[p], &[], &[], 10);
        for n in 0..ani.len() {
            // Any prefix must either parse or error; it must never panic.
            let _ = parse_ani(&ani[..n]);
        }
        assert!(parse_ani(b"not an ani").is_err());
        assert!(parse_ani(&[]).is_err());
    }

    #[test]
    fn rejects_absurd_frame_count() {
        let p = one_pixel_4bpp();
        let mut ani = build_ani(4, 2, 1, (0, 0), &[p], &[], &[], 10);
        // Force `anih.nFrames` to 9999 (find the anih chunk body: after RIFF/ACON/anih/size).
        let body = 12 + 4 + 4 + 4; // RIFF(8) + ACON(4) + "anih"(4) + size(4)
        ani[body + 4..body + 8].copy_from_slice(&9999u32.to_le_bytes());
        assert!(parse_ani(&ani).is_err());
    }

    #[test]
    fn scroll_direction_index_is_clockwise_from_east() {
        use crate::context_cmd::scroll_dir_index as idx;
        // Frame 0 points right (`SCCScroll0` = "Scroll - E"); the rest continue clockwise.
        assert_eq!(idx(Vec2::new(1.0, 0.0), 8), 0);
        assert_eq!(idx(Vec2::new(1.0, 1.0), 8), 1);
        assert_eq!(idx(Vec2::new(0.0, 1.0), 8), 2);
        assert_eq!(idx(Vec2::new(-1.0, 1.0), 8), 3);
        assert_eq!(idx(Vec2::new(-1.0, 0.0), 8), 4);
        assert_eq!(idx(Vec2::new(-1.0, -1.0), 8), 5);
        assert_eq!(idx(Vec2::new(0.0, -1.0), 8), 6);
        assert_eq!(idx(Vec2::new(1.0, -1.0), 8), 7);
        assert_eq!(idx(Vec2::ZERO, 8), 0);
    }

    #[test]
    fn parses_mouse_ini_names_and_directions() {
        let text = "Mouse\n  TooltipWidth = 20\nEnd\n\nMouseCursor Arrow\n  Texture = SCCPointer\n  Image = SCCPointer\nEnd\n\nMouseCursor Scroll\n  Texture = SCCScroll\n  Directions = 8\nEnd\n";
        let defs = parse_mouse_ini(text).unwrap();
        assert_eq!(defs["arrow"].texture, "SCCPointer");
        assert_eq!(defs["arrow"].directions, 1);
        assert_eq!(defs["scroll"].directions, 8);
    }

    #[test]
    fn mouse_ini_rejects_unknown_block() {
        assert!(parse_mouse_ini("NotAThing A\nEnd\n").is_err());
    }

    #[test]
    fn zlib_stored_roundtrips_with_puff() {
        // The IDAT payload must begin with the zlib header and end with adler32; decode it back to
        // the raw bytes to prove the stored-block writer is correct.
        let raw: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        let z = zlib_stored(&raw);
        assert_eq!(&z[..2], &[0x78, 0x01]);
        let got = inflate_stored(&z[2..z.len() - 4]).unwrap();
        assert_eq!(got, raw);
    }

    /// Decodes a stream of stored DEFLATE blocks (test-only helper).
    fn inflate_stored(mut b: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        loop {
            if b.is_empty() {
                return Err("truncated".into());
            }
            let header = b[0];
            b = &b[1..];
            let final_block = header & 1 == 1;
            // The 3 header bits are ignored for stored blocks beyond BFINAL.
            if b.len() < 4 {
                return Err("truncated len".into());
            }
            let len = u16::from_le_bytes([b[0], b[1]]) as usize;
            let nlen = u16::from_le_bytes([b[2], b[3]]) as usize;
            if len != !nlen & 0xffff {
                return Err("len/nlen mismatch".into());
            }
            b = &b[4..];
            if b.len() < len {
                return Err("truncated data".into());
            }
            out.extend_from_slice(&b[..len]);
            b = &b[len..];
            if final_block {
                return Ok(out);
            }
        }
    }
}
