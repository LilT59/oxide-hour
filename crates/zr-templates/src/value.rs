//! INI value parsing: the engine's `INI::parse*` / `INI::scan*` helpers.
//!
//! Every function cites the C++ it ports (`Core/GameEngine/Source/Common/INI/INI.cpp` unless noted).
//! All numeric work is `f32` with plain IEEE operations, matching the engine's `Real`.

use crate::flags::NameEnum;

/// `LOGICFRAMES_PER_SECOND` (`WWSyncPerSecond`, `Core/Libraries/Source/WWVegas/WWLib/WWCommon.h`).
pub const LOGICFRAMES_PER_SECOND: u32 = 30;

/// `LOGICFRAMES_PER_MSEC_REAL` (`GameCommon.h`): `30.0f / 1000.0f`.
pub const LOGICFRAMES_PER_MSEC_REAL: f32 = LOGICFRAMES_PER_SECOND as f32 / 1000.0f32;
/// `SECONDS_PER_LOGICFRAME_REAL` (`GameCommon.h`): `1.0f / 30.0f`.
pub const SECONDS_PER_LOGICFRAME_REAL: f32 = 1.0f32 / LOGICFRAMES_PER_SECOND as f32;
/// `PI` (`BaseMacros.h`): `3.14159265359f`, which is the same `f32` as `std::f32::consts::PI`.
pub const PI: f32 = std::f32::consts::PI;
/// `RADS_PER_DEGREE` as written in `INI::parseAngleReal`: `PI / 180.0f`.
pub const RADS_PER_DEGREE: f32 = PI / 180.0f32;

/// `ConvertDurationFromMsecsToFrames` (`GameCommon.h`): a fractional frame count (`parseDurationReal`).
pub fn duration_ms_to_frames(msec: f32) -> f32 {
    msec * LOGICFRAMES_PER_MSEC_REAL
}

/// `INI::parseDurationUnsignedInt`: milliseconds to whole frames, rounding UP (`ceilf`).
pub fn duration_ms_to_frames_ceil(msec: u32) -> u32 {
    duration_ms_to_frames(msec as f32).ceil() as u32
}

/// `ConvertVelocityInSecsToFrames`: distance per second to distance per frame (`parseVelocityReal`).
pub fn velocity_per_sec_to_per_frame(v: f32) -> f32 {
    v * SECONDS_PER_LOGICFRAME_REAL
}

/// `ConvertAccelerationInSecsToFrames`: distance per second^2 to per frame^2 (`parseAccelerationReal`).
pub fn accel_per_sec2_to_per_frame2(a: f32) -> f32 {
    a * (SECONDS_PER_LOGICFRAME_REAL * SECONDS_PER_LOGICFRAME_REAL)
}

/// `INI::parseAngleReal`: degrees to radians.
pub fn degrees_to_radians(deg: f32) -> f32 {
    deg * RADS_PER_DEGREE
}

/// `ConvertAngularVelocityInDegreesPerSecToRadsPerFrame` (`parseAngularVelocityReal`).
pub fn deg_per_sec_to_rad_per_frame(deg: f32) -> f32 {
    deg * (SECONDS_PER_LOGICFRAME_REAL * RADS_PER_DEGREE)
}

/// Locomotor/physics `parseFrictionPerSec`: per second to per frame.
pub fn friction_per_sec_to_per_frame(f: f32) -> f32 {
    f * SECONDS_PER_LOGICFRAME_REAL
}

/// The default token separators (`INI::getSeps`): space, tab, CR, LF and `=`.
pub const SEPS: &[char] = &[' ', '\t', '\r', '\n', '='];
/// `INI::getSepsPercent`: default separators plus `%`.
pub const SEPS_PERCENT: &[char] = &[' ', '\t', '\r', '\n', '=', '%'];
/// `INI::getSepsColon`: default separators plus `:`.
pub const SEPS_COLON: &[char] = &[' ', '\t', '\r', '\n', '=', ':'];

/// `strtok` style split: runs of separators collapse, no empty tokens.
pub fn split<'a>(s: &'a str, seps: &[char]) -> Vec<&'a str> {
    s.split(seps).filter(|t| !t.is_empty()).collect()
}

/// Length of the longest prefix of `s` that `sscanf("%f")` would accept as a decimal float.
fn float_prefix_len(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut ndigits = i - digits_start;
    if i < b.len() && b[i] == b'.' {
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        ndigits += j - i - 1;
        i = j;
    }
    if ndigits == 0 {
        return 0;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let es = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > es {
            i = j;
        }
    }
    i
}

/// `INI::scanReal`: `sscanf("%f")`, so trailing junk (`50%`, `1.5f`) is ignored.
pub fn scan_real(tok: &str) -> Result<f32, String> {
    let n = float_prefix_len(tok);
    if n == 0 {
        return Err(format!("`{tok}` is not a number"));
    }
    tok[..n]
        .trim_start_matches('+')
        .parse::<f32>()
        .map_err(|_| format!("`{tok}` is not a number"))
}

fn scan_int_wide(tok: &str) -> Result<i64, String> {
    let b = tok.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let s = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == s {
        return Err(format!("`{tok}` is not an integer"));
    }
    tok[..i]
        .trim_start_matches('+')
        .parse()
        .map_err(|_| format!("`{tok}` is out of range"))
}

/// `INI::scanInt`: `sscanf("%d")`; leading digits are used, trailing junk ignored.
pub fn scan_int(tok: &str) -> Result<i32, String> {
    i32::try_from(scan_int_wide(tok)?).map_err(|_| format!("`{tok}` is out of range"))
}

/// `INI::scanUnsignedInt`: `sscanf("%u")`.
pub fn scan_uint(tok: &str) -> Result<u32, String> {
    u32::try_from(scan_int_wide(tok)?).map_err(|_| format!("`{tok}` is out of range"))
}

/// `INI::scanBool`: `yes` / `no`, case-insensitive.
pub fn scan_bool(tok: &str) -> Result<bool, String> {
    if tok.eq_ignore_ascii_case("yes") {
        Ok(true)
    } else if tok.eq_ignore_ascii_case("no") {
        Ok(false)
    } else {
        Err(format!("invalid boolean `{tok}`, expected Yes or No"))
    }
}

/// `INI::scanPercentToReal`: `"23%"` to `0.23` (`scanReal(token) / 100.0f`).
pub fn scan_percent(tok: &str) -> Result<f32, String> {
    Ok(scan_real(tok)? / 100.0f32)
}

/// A cursor over the tokens of a field value, mirroring successive `INI::getNextToken` calls.
#[derive(Debug, Clone)]
pub struct Args<'a> {
    toks: Vec<&'a str>,
    pos: usize,
}

impl<'a> Args<'a> {
    pub fn new(value: &'a str) -> Self {
        Self::with_seps(value, SEPS)
    }
    pub fn with_seps(value: &'a str, seps: &[char]) -> Self {
        Args { toks: split(value, seps), pos: 0 }
    }
    /// `getNextToken`: errors if the line is exhausted (`INI_INVALID_DATA`).
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<&'a str, String> {
        let t = self
            .toks
            .get(self.pos)
            .copied()
            .ok_or_else(|| "missing value".to_string())?;
        self.pos += 1;
        Ok(t)
    }
    /// `getNextTokenOrNull`.
    pub fn next_opt(&mut self) -> Option<&'a str> {
        let t = self.toks.get(self.pos).copied();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }
    /// All remaining tokens.
    pub fn rest(&mut self) -> &[&'a str] {
        let r = &self.toks[self.pos.min(self.toks.len())..];
        self.pos = self.toks.len();
        r
    }
    pub fn real(&mut self) -> Result<f32, String> {
        scan_real(self.next()?)
    }
    pub fn int(&mut self) -> Result<i32, String> {
        scan_int(self.next()?)
    }
    pub fn uint(&mut self) -> Result<u32, String> {
        scan_uint(self.next()?)
    }
    pub fn boolean(&mut self) -> Result<bool, String> {
        scan_bool(self.next()?)
    }
    pub fn percent(&mut self) -> Result<f32, String> {
        scan_percent(self.next()?)
    }
    /// `INI::parseUnsignedByte`.
    pub fn u8(&mut self) -> Result<u8, String> {
        let v = self.int()?;
        u8::try_from(v).map_err(|_| format!("{v} is out of range for an unsigned byte"))
    }
    /// `INI::parseUnsignedShort`.
    pub fn u16(&mut self) -> Result<u16, String> {
        let v = self.int()?;
        u16::try_from(v).map_err(|_| format!("{v} is out of range for an unsigned short"))
    }
    /// `INI::parseAngleReal`: degrees in, radians out.
    pub fn angle(&mut self) -> Result<f32, String> {
        Ok(degrees_to_radians(self.real()?))
    }
    /// `INI::parseAngularVelocityReal`: degrees/sec in, radians/frame out.
    pub fn angular_velocity(&mut self) -> Result<f32, String> {
        Ok(deg_per_sec_to_rad_per_frame(self.real()?))
    }
    /// `INI::parseVelocityReal`: units/sec in, units/frame out.
    pub fn velocity(&mut self) -> Result<f32, String> {
        Ok(velocity_per_sec_to_per_frame(self.real()?))
    }
    /// `INI::parseAccelerationReal`: units/sec^2 in, units/frame^2 out.
    pub fn acceleration(&mut self) -> Result<f32, String> {
        Ok(accel_per_sec2_to_per_frame2(self.real()?))
    }
    /// `INI::parseDurationReal`: ms in, fractional frames out.
    pub fn duration_real(&mut self) -> Result<f32, String> {
        Ok(duration_ms_to_frames(self.real()?))
    }
    /// `INI::parseDurationUnsignedInt`: ms in, frames (rounded up) out.
    pub fn duration_frames(&mut self) -> Result<u32, String> {
        Ok(duration_ms_to_frames_ceil(self.uint()?))
    }
    /// `getNextAsciiString`: first token, or empty if absent.
    pub fn string(&mut self) -> String {
        self.next_opt().unwrap_or("").to_string()
    }
    /// `scanIndexList` against an enum's name table.
    pub fn index<E: crate::flags::NameEnum>(&mut self) -> Result<E, String> {
        let t = self.next()?;
        E::from_name(t).ok_or_else(|| format!("`{t}` is not a valid member of the index list"))
    }
}

/// `INI::parseColorInt`: `R:100 G:114 B:245 [A:233]` to `[r, g, b, a]` (A defaults to 255).
pub fn parse_color(value: &str) -> Result<[u8; 4], String> {
    let mut a = Args::with_seps(value, SEPS_COLON);
    let mut out = [0u8; 4];
    for (i, name) in ["R", "G", "B", "A"].into_iter().enumerate() {
        match a.next_opt() {
            None if i == 3 => out[3] = 255,
            None => return Err("color needs R:, G: and B: components".into()),
            Some(t) => {
                if !t.eq_ignore_ascii_case(name) {
                    return Err(format!("expected `{name}:` in color, found `{t}`"));
                }
                let v = a.int()?;
                out[i] = u8::try_from(v).map_err(|_| format!("color component {v} out of range 0..=255"))?;
            }
        }
    }
    Ok(out)
}

/// `INI::parseCoord2D`: `X:1.0 Y:2.0`.
pub fn parse_coord2d(value: &str) -> Result<(f32, f32), String> {
    let mut a = Args::with_seps(value, SEPS_COLON);
    let mut get = |name: &str| -> Result<f32, String> {
        let t = a.next()?;
        if !t.eq_ignore_ascii_case(name) {
            return Err(format!("expected `{name}:`, found `{t}`"));
        }
        a.real()
    };
    let x = get("X")?;
    let y = get("Y")?;
    Ok((x, y))
}

/// `INI::parseCoord3D`: `X:1.0 Y:2.0 Z:3.0`; the labels are case-insensitive (`getNextSubToken`).
pub fn parse_coord3d(value: &str) -> Result<(f32, f32, f32), String> {
    let mut a = Args::with_seps(value, SEPS_COLON);
    let mut get = |name: &str| -> Result<f32, String> {
        let t = a.next()?;
        if !t.eq_ignore_ascii_case(name) {
            return Err(format!("expected `{name}:`, found `{t}`"));
        }
        a.real()
    };
    let x = get("X")?;
    let y = get("Y")?;
    let z = get("Z")?;
    Ok((x, y, z))
}

/// `INI::parseDeathTypeFlags` / `INI::parseDamageTypeFlags`: starts at ALL, then `ALL`, `NONE`,
/// `+TYPE`, `-TYPE`; a bare name (other than ALL/NONE) is rejected. Unlike [`Mask::parse`], a bare
/// list does not replace the current value.
pub fn parse_flag_list_all<E: crate::flags::NameEnum>(toks: &[&str]) -> Result<crate::flags::Mask<E>, String> {
    let mut m = crate::flags::Mask::from_bits(u32::MAX as u128);
    for t in toks {
        if t.eq_ignore_ascii_case("ALL") {
            m = crate::flags::Mask::from_bits(u32::MAX as u128);
        } else if t.eq_ignore_ascii_case("NONE") {
            m = crate::flags::Mask::EMPTY;
        } else if let Some(n) = t.strip_prefix('+') {
            m.insert(E::from_name(n).ok_or_else(|| format!("`{n}` is not a valid member of the index list"))?);
        } else if let Some(n) = t.strip_prefix('-') {
            m.remove(E::from_name(n).ok_or_else(|| format!("`{n}` is not a valid member of the index list"))?);
        } else {
            return Err(format!("unexpected token `{t}` (expected ALL, NONE, +TYPE or -TYPE)"));
        }
    }
    Ok(m)
}

/// `INI::parseVeterancyLevelFlags`: like [`parse_flag_list_all`] but the engine's
/// `VeterancyLevelFlags` uses bit index `level - 1` (`GameCommon.h`), so `REGULAR` has no bit.
pub fn parse_veterancy_level_flags(toks: &[&str]) -> Result<u32, String> {
    use crate::names::VeterancyLevel;
    let mut flags: u32 = u32::MAX;
    for t in toks {
        if t.eq_ignore_ascii_case("ALL") {
            flags = u32::MAX;
        } else if t.eq_ignore_ascii_case("NONE") {
            flags = 0;
        } else if let Some(n) = t.strip_prefix('+') {
            if let Some(bit) = VeterancyLevel::from_name(n).and_then(|v| v.index().checked_sub(1)) {
                flags |= 1u32 << bit;
            }
        } else if let Some(n) = t.strip_prefix('-') {
            if let Some(bit) = VeterancyLevel::from_name(n).and_then(|v| v.index().checked_sub(1)) {
                flags &= !(1u32 << bit);
            }
        } else {
            return Err(format!("unexpected token `{t}` (expected ALL, NONE, +TYPE or -TYPE)"));
        }
    }
    Ok(flags)
}
