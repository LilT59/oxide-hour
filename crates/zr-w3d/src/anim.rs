//! W3D hierarchy animation decoding.
//!
//! Handles the classic raw `ANIMATION` chunk (`HRawAnimClass`,
//! `hrawanim.cpp` / `motchan.cpp`) and the `COMPRESSED_ANIMATION` chunk in its
//! time-coded and adaptive-delta flavours (`HCompressedAnimClass`,
//! `hcanim.cpp` / `motchan.cpp`). Channel layouts are from `w3d_file.h`
//! (`W3dAnimChannelStruct`, `W3dTimeCodedAnimChannelStruct`,
//! `W3dAdaptiveDeltaAnimChannelStruct`, the bit channels). Animations live in
//! separate files named `<hierarchy>.<anim>` in the INI; the on-disk file is
//! `art\w3d\<anim>.w3d` (`WW3DAssetManager::Get_HAnim` uses the part after the
//! first dot).

use crate::error::{invalid, Error};
use cnc_formats::w3d::{W3dChunk, W3dFile};
use zr_assets::{normalize_path, Vfs};

const CHUNK_ANIMATION: u32 = 0x200;
const CHUNK_ANIMATION_HEADER: u32 = 0x201;
const CHUNK_ANIMATION_CHANNEL: u32 = 0x202;
const CHUNK_BIT_CHANNEL: u32 = 0x203;
const CHUNK_COMPRESSED_ANIMATION: u32 = 0x280;
const CHUNK_COMPRESSED_ANIMATION_HEADER: u32 = 0x281;
const CHUNK_COMPRESSED_ANIMATION_CHANNEL: u32 = 0x282;
const CHUNK_COMPRESSED_BIT_CHANNEL: u32 = 0x283;

const ANIM_CHANNEL_X: u16 = 0;
const ANIM_CHANNEL_Y: u16 = 1;
const ANIM_CHANNEL_Z: u16 = 2;
const ANIM_CHANNEL_Q: u16 = 6;

const ANIM_FLAVOR_TIMECODED: u16 = 0;
const ANIM_FLAVOR_ADAPTIVE_DELTA: u16 = 1;

const TIMECODED_BINARY_FLAG: u32 = 0x8000_0000;
const TIMECODED_BIT_MASK: u32 = 0x8000_0000;

const MAX_PIVOTS: usize = 8192;
const MAX_FRAMES: u32 = 1 << 20;

/// Static part of the adaptive-delta filter table (`motchan.cpp`).
const FILTER_TABLE: [f32; 16] = [
    0.00000001, 0.0000001, 0.000001, 0.00001, 0.0001, 0.001, 0.01, 0.1, 1.0, 10.0, 100.0, 1000.0,
    10000.0, 100000.0, 1000000.0, 10000000.0,
];

fn filter_table(i: usize) -> f32 {
    if i < FILTER_TABLE.len() {
        FILTER_TABLE[i]
    } else {
        let ratio = (i - FILTER_TABLE.len()) as f32 / 240.0;
        1.0 - libm::sinf(ratio * core::f32::consts::FRAC_PI_2)
    }
}

/// How a hierarchy animation stores its channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimFlavor {
    /// Classic raw float channels (`W3D_CHUNK_ANIMATION`).
    Raw,
    /// `COMPRESSED_ANIMATION` with time-coded packets.
    TimeCoded,
    /// `COMPRESSED_ANIMATION` with adaptive-delta quantised packets.
    AdaptiveDelta,
}

/// A decoded hierarchy animation, keyed by pivot index.
#[derive(Debug, Clone)]
pub struct Animation {
    /// Internal name, `<HierarchyName>.<Name>` (matches `HAnimClass::Get_Name`).
    pub name: String,
    /// Skeleton this animation belongs to.
    pub hierarchy: String,
    pub frame_count: u32,
    pub frame_rate: u32,
    pub flavor: AnimFlavor,
    tracks: Vec<PivotTrack>,
}

#[derive(Debug, Clone, Default)]
struct PivotTrack {
    x: Option<Channel>,
    y: Option<Channel>,
    z: Option<Channel>,
    q: Option<Channel>,
    vis: Option<BitChannel>,
}

#[derive(Debug, Clone)]
enum Channel {
    Raw(RawChannel),
    TimeCoded(TimeCodedChannel),
    Adaptive(AdaptiveChannel),
}

#[derive(Debug, Clone)]
struct RawChannel {
    first: i32,
    last: i32,
    vlen: usize,
    data: Vec<f32>,
}

#[derive(Debug, Clone)]
struct TimeCodedChannel {
    vlen: usize,
    packets: Vec<TimePacket>,
}

#[derive(Debug, Clone, Copy)]
struct TimePacket {
    time: u32,
    binary: bool,
    v: [f32; 4],
}

#[derive(Debug, Clone)]
struct AdaptiveChannel {
    frame_count: u32,
    scale: f32,
    vlen: usize,
    stream: Vec<u8>,
}

#[derive(Debug, Clone)]
enum BitChannel {
    /// Raw `W3dBitChannelStruct`: packed bits over `[first, last]`.
    Raw { first: i32, last: i32, default: bool, data: Vec<u8> },
    /// Time-coded bits: each entry's MSB is the value, the rest the frame.
    TimeCoded { default: bool, bits: Vec<u32> },
}

// ── readers ───────────────────────────────────────────────────────────────────

fn rd_u32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

fn rd_u16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o.checked_add(2)?)?.try_into().ok()?))
}

fn rd_f32(d: &[u8], o: usize) -> Option<f32> {
    rd_u32(d, o).map(f32::from_bits)
}

fn cstr(d: &[u8]) -> String {
    let n = d.iter().position(|&b| b == 0).unwrap_or(d.len());
    String::from_utf8_lossy(&d[..n]).into_owned()
}

// ── channel sampling ──────────────────────────────────────────────────────────

fn raw_frames(frame: f32, num_frames: u32) -> (i32, i32, f32) {
    let n = num_frames.max(1) as i32;
    let clamped = frame.clamp(0.0, (n - 1) as f32);
    // `Float_To_Long(frame - 0.499999f)` (FPU round-to-nearest).
    let f0 = libm::roundf(clamped - 0.499999) as i32;
    let mut f1 = f0 + 1;
    if f1 >= n {
        f1 = 0;
    }
    (f0, f1, clamped - f0 as f32)
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// `Fast_Slerp` (`quat.cpp`) with the standard hemisphere flip.
fn slerp(p: [f32; 4], q: [f32; 4], t: f32) -> [f32; 4] {
    let dot = p[0] * q[0] + p[1] * q[1] + p[2] * q[2] + p[3] * q[3];
    let (mut q, mut cos) = (q, dot);
    if cos < 0.0 {
        cos = -cos;
        for v in q.iter_mut() {
            *v = -*v;
        }
    }
    if 1.0 - cos < 1e-6 {
        let mut r = [0.0; 4];
        for i in 0..4 {
            r[i] = p[i] + (q[i] - p[i]) * t;
        }
        return r;
    }
    let theta = libm::acosf(cos);
    let sin_t = libm::sinf(theta);
    if sin_t.abs() < 1e-9 {
        return p;
    }
    let beta = libm::sinf(theta - t * theta) / sin_t;
    let alpha = libm::sinf(t * theta) / sin_t;
    let mut r = [0.0; 4];
    for i in 0..4 {
        r[i] = beta * p[i] + alpha * q[i];
    }
    r
}

impl RawChannel {
    fn at(&self, frame: i32) -> [f32; 4] {
        if frame < self.first || frame > self.last {
            return identity_for(self.vlen);
        }
        let base = (frame - self.first) as usize * self.vlen;
        let mut v = [0.0; 4];
        for (i, slot) in v.iter_mut().enumerate().take(self.vlen) {
            if let Some(&x) = self.data.get(base + i) {
                *slot = x;
            }
        }
        v
    }
}

fn identity_for(vlen: usize) -> [f32; 4] {
    if vlen == 4 {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        [0.0; 4]
    }
}

impl TimeCodedChannel {
    fn index(&self, timecode: u32) -> usize {
        let n = self.packets.len();
        if n == 0 {
            return 0;
        }
        if timecode >= self.packets[n - 1].time {
            return n - 1;
        }
        let mut idx = 0;
        for (i, p) in self.packets.iter().enumerate() {
            if p.time <= timecode {
                idx = i;
            } else {
                break;
            }
        }
        idx
    }

    fn sample(&self, frame: f32) -> [f32; 4] {
        let n = self.packets.len();
        if n == 0 {
            return [0.0; 4];
        }
        let pidx = self.index(frame.max(0.0) as u32).min(n - 1);
        let p0 = self.packets[pidx];
        if pidx == n - 1 {
            return p0.v;
        }
        let p2 = self.packets[pidx + 1];
        if p2.binary || p2.time <= p0.time {
            return p0.v;
        }
        let ratio = (frame - p0.time as f32) / (p2.time - p0.time) as f32;
        let mut out = [0.0; 4];
        for (i, slot) in out.iter_mut().enumerate().take(self.vlen) {
            *slot = p0.v[i] + (p2.v[i] - p0.v[i]) * ratio;
        }
        out
    }

    fn sample_quat(&self, frame: f32) -> [f32; 4] {
        let n = self.packets.len();
        if n == 0 {
            return [0.0, 0.0, 0.0, 1.0];
        }
        let pidx = self.index(frame.max(0.0) as u32).min(n - 1);
        let p0 = self.packets[pidx];
        if pidx == n - 1 {
            return p0.v;
        }
        let p2 = self.packets[pidx + 1];
        if p2.binary || p2.time <= p0.time {
            return p0.v;
        }
        let ratio = (frame - p0.time as f32) / (p2.time - p0.time) as f32;
        slerp(p0.v, p2.v, ratio)
    }
}

impl AdaptiveChannel {
    fn base(&self, vi: usize) -> f32 {
        rd_f32(&self.stream, vi * 4).unwrap_or(0.0)
    }

    /// Adaptive-delta decompression (`AdaptiveDeltaMotionChannelClass::decompress`).
    fn getframe(&self, frame_idx: u32, vi: usize) -> f32 {
        if self.frame_count == 0 || vi >= self.vlen {
            return 0.0;
        }
        let frame_idx = frame_idx.min(self.frame_count - 1);
        if frame_idx == 0 {
            return self.base(vi);
        }
        let mut p = 4 * self.vlen + 9 * vi;
        let mut last = self.base(vi);
        let mut frame = 1u32;
        while frame <= frame_idx {
            let Some(&filter_index) = self.stream.get(p) else { break };
            p += 1;
            let filter = filter_table(filter_index as usize) * self.scale;
            let mut done = false;
            for fi in 0..16 {
                let Some(&byte) = self.stream.get(p + (fi >> 1)) else {
                    done = true;
                    break;
                };
                let raw = if fi & 1 != 0 { (byte >> 4) as i32 } else { (byte & 0x0f) as i32 };
                let factor = if raw & 0x8 != 0 { raw - 16 } else { raw };
                last += factor as f32 * filter;
                if frame == frame_idx {
                    done = true;
                    break;
                }
                frame += 1;
            }
            if done {
                break;
            }
            p += 9 * self.vlen - 1;
        }
        last
    }

    fn sample(&self, frame: f32) -> f32 {
        let f1 = frame.max(0.0) as u32;
        let ratio = frame.max(0.0) - f1 as f32;
        let a = self.getframe(f1, 0);
        let b = self.getframe(f1 + 1, 0);
        a + (b - a) * ratio
    }

    fn sample_quat(&self, frame: f32) -> [f32; 4] {
        let f1 = frame.max(0.0) as u32;
        let ratio = frame.max(0.0) - f1 as f32;
        let mut a = [0.0; 4];
        let mut b = [0.0; 4];
        for (i, (av, bv)) in a.iter_mut().zip(b.iter_mut()).enumerate() {
            *av = self.getframe(f1, i);
            *bv = self.getframe(f1 + 1, i);
        }
        slerp(a, b, ratio)
    }
}

impl Channel {
    fn raw_vec4(&self, frame: i32) -> [f32; 4] {
        match self {
            Channel::Raw(c) => c.at(frame),
            _ => [0.0; 4],
        }
    }

    fn sample_scalar(&self, frame: f32) -> f32 {
        match self {
            Channel::TimeCoded(c) => c.sample(frame)[0],
            Channel::Adaptive(c) => c.sample(frame),
            Channel::Raw(c) => c.at(frame as i32)[0],
        }
    }

    fn sample_quat(&self, frame: f32) -> [f32; 4] {
        match self {
            Channel::TimeCoded(c) => c.sample_quat(frame),
            Channel::Adaptive(c) => c.sample_quat(frame),
            Channel::Raw(c) => c.at(frame as i32),
        }
    }
}

impl BitChannel {
    fn visible(&self, frame: f32) -> bool {
        let frame = frame.max(0.0) as i32;
        match self {
            BitChannel::Raw { first, last, default, data } => {
                if frame < *first || frame > *last {
                    return *default;
                }
                let bit = (frame - first) as usize;
                data.get(bit / 8).is_some_and(|b| b & (1 << (bit % 8)) != 0)
            }
            BitChannel::TimeCoded { default, bits } => {
                if bits.is_empty() {
                    return *default;
                }
                let frame = frame.max(0) as u32;
                let mut idx = 0usize;
                let start = if (bits[0] & !TIMECODED_BIT_MASK) <= frame { 1 } else { 0 };
                for (i, b) in bits.iter().enumerate().skip(start) {
                    if frame < (b & !TIMECODED_BIT_MASK) {
                        break;
                    }
                    idx = i;
                }
                bits[idx] & TIMECODED_BIT_MASK != 0
            }
        }
    }
}

impl PivotTrack {
    fn sample(&self, flavor: AnimFlavor, frame: f32, num_frames: u32) -> Option<([f32; 3], [f32; 4])> {
        if self.x.is_none() && self.y.is_none() && self.z.is_none() && self.q.is_none() {
            return None;
        }
        if flavor == AnimFlavor::Raw {
            let (f0, f1, ratio) = raw_frames(frame, num_frames);
            let axis = |c: &Option<Channel>, i: i32| c.as_ref().map_or(0.0, |c| c.raw_vec4(i)[0]);
            let t0 = [axis(&self.x, f0), axis(&self.y, f0), axis(&self.z, f0)];
            let t1 = [axis(&self.x, f1), axis(&self.y, f1), axis(&self.z, f1)];
            let q = match &self.q {
                Some(c) => slerp(c.raw_vec4(f0), c.raw_vec4(f1), ratio),
                None => [0.0, 0.0, 0.0, 1.0],
            };
            Some((lerp3(t0, t1, ratio), q))
        } else {
            let axis = |c: &Option<Channel>| c.as_ref().map_or(0.0, |c| c.sample_scalar(frame));
            let q = self.q.as_ref().map_or([0.0, 0.0, 0.0, 1.0], |c| c.sample_quat(frame));
            Some(([axis(&self.x), axis(&self.y), axis(&self.z)], q))
        }
    }
}

impl Animation {
    /// Number of pivots the animation carries data for (`max(pivot)+1`).
    pub fn pivot_count(&self) -> usize {
        self.tracks.len()
    }

    /// Translation delta and orientation for `pivot` at `frame`, or `None`
    /// when the animation has no channel for that pivot (the bind pose is used).
    pub fn sample(&self, pivot: usize, frame: f32) -> Option<([f32; 3], [f32; 4])> {
        self.tracks.get(pivot)?.sample(self.flavor, frame, self.frame_count)
    }

    /// Visibility of an animated pivot (true unless a bit channel says otherwise).
    pub fn visible(&self, pivot: usize, frame: f32) -> bool {
        self.tracks.get(pivot).and_then(|t| t.vis.as_ref()).is_none_or(|v| v.visible(frame))
    }
}

// ── channel decoding ──────────────────────────────────────────────────────────

fn parse_raw_channel(outer: &W3dChunk) -> Result<Channel, Error> {
    let d = outer.data;
    if d.len() < 16 {
        return Err(invalid("raw animation channel too short"));
    }
    let first = rd_u16(d, 0).unwrap_or(0) as i32;
    let last = rd_u16(d, 2).unwrap_or(0) as i32;
    let vlen = rd_u16(d, 4).unwrap_or(0) as usize;
    if vlen == 0 || vlen > 4 {
        return Err(invalid("raw animation channel VectorLen out of range"));
    }
    let count = d.len().saturating_sub(12) / 4;
    let mut data = Vec::with_capacity(count);
    for i in 0..count {
        data.push(rd_f32(d, 12 + i * 4).unwrap_or(0.0));
    }
    Ok(Channel::Raw(RawChannel { first, last, vlen, data }))
}

fn parse_timecoded_channel(outer: &W3dChunk) -> Result<Channel, Error> {
    let d = outer.data;
    if d.len() < 8 {
        return Err(invalid("time-coded animation channel too short"));
    }
    let num_time_codes = rd_u32(d, 0).unwrap_or(0) as usize;
    let vlen = d.get(6).copied().unwrap_or(0) as usize;
    if vlen == 0 || vlen > 4 {
        return Err(invalid("time-coded animation channel VectorLen out of range"));
    }
    let words = d.len().saturating_sub(8) / 4;
    let packet_size = vlen + 1;
    let n = num_time_codes.min(words / packet_size);
    let mut packets = Vec::with_capacity(n);
    for i in 0..n {
        let base = i * packet_size;
        let raw = rd_u32(d, 8 + base * 4).unwrap_or(0);
        let mut v = [0.0f32; 4];
        for (k, slot) in v.iter_mut().enumerate().take(vlen) {
            *slot = rd_f32(d, 8 + (base + 1 + k) * 4).unwrap_or(0.0);
        }
        packets.push(TimePacket { time: raw & !TIMECODED_BINARY_FLAG, binary: raw & TIMECODED_BINARY_FLAG != 0, v });
    }
    Ok(Channel::TimeCoded(TimeCodedChannel { vlen, packets }))
}

fn parse_adaptive_channel(outer: &W3dChunk) -> Result<Channel, Error> {
    let d = outer.data;
    if d.len() < 16 {
        return Err(invalid("adaptive-delta animation channel too short"));
    }
    let frame_count = rd_u32(d, 0).unwrap_or(0).min(MAX_FRAMES);
    let vlen = d.get(6).copied().unwrap_or(0) as usize;
    if vlen == 0 || vlen > 4 {
        return Err(invalid("adaptive-delta animation channel VectorLen out of range"));
    }
    let scale = rd_f32(d, 8).unwrap_or(1.0);
    let stream = d.get(12..).unwrap_or(&[]).to_vec();
    Ok(Channel::Adaptive(AdaptiveChannel { frame_count, scale, vlen, stream }))
}

fn parse_raw_bit_channel(d: &[u8]) -> Result<BitChannel, Error> {
    if d.len() < 10 {
        return Err(invalid("raw bit channel too short"));
    }
    let first = rd_u16(d, 0).unwrap_or(0) as i32;
    let last = rd_u16(d, 2).unwrap_or(0) as i32;
    let default = d.get(8).is_some_and(|&b| b != 0);
    let bytes = d.get(9..).unwrap_or(&[]).to_vec();
    Ok(BitChannel::Raw { first, last, default, data: bytes })
}

fn parse_timecoded_bit_channel(d: &[u8]) -> Result<BitChannel, Error> {
    if d.len() < 12 {
        return Err(invalid("time-coded bit channel too short"));
    }
    let default = d.get(7).is_some_and(|&b| b != 0);
    let count = d.len().saturating_sub(8) / 4;
    let mut bits = Vec::with_capacity(count);
    for i in 0..count {
        bits.push(rd_u32(d, 8 + i * 4).unwrap_or(0));
    }
    Ok(BitChannel::TimeCoded { default, bits })
}

fn put_channel(track: &mut PivotTrack, ty: u16, ch: Channel) {
    match ty {
        ANIM_CHANNEL_X => track.x = Some(ch),
        ANIM_CHANNEL_Y => track.y = Some(ch),
        ANIM_CHANNEL_Z => track.z = Some(ch),
        ANIM_CHANNEL_Q => track.q = Some(ch),
        _ => {}
    }
}

fn ensure_track(tracks: &mut Vec<PivotTrack>, pivot: usize) -> Option<&mut PivotTrack> {
    if pivot >= MAX_PIVOTS {
        return None;
    }
    if pivot >= tracks.len() {
        tracks.resize(pivot + 1, PivotTrack::default());
    }
    tracks.get_mut(pivot)
}

fn parse_raw(chunk: &W3dChunk) -> Result<Animation, Error> {
    let hdr = chunk.find_child(CHUNK_ANIMATION_HEADER).ok_or_else(|| invalid("animation without header"))?;
    if hdr.data.len() < 44 {
        return Err(invalid("animation header too short"));
    }
    let name = cstr(hdr.data.get(4..20).unwrap_or(&[]));
    let hierarchy = cstr(hdr.data.get(20..36).unwrap_or(&[]));
    let frame_count = rd_u32(hdr.data, 36).unwrap_or(0).min(MAX_FRAMES);
    let frame_rate = rd_u32(hdr.data, 40).unwrap_or(0).max(1);
    let mut tracks: Vec<PivotTrack> = Vec::new();
    for c in chunk.find_children(CHUNK_ANIMATION_CHANNEL) {
        let ty = rd_u16(c.data, 6).unwrap_or(u16::MAX);
        let pivot = rd_u16(c.data, 8).unwrap_or(u16::MAX) as usize;
        let ch = parse_raw_channel(c)?;
        if let Some(t) = ensure_track(&mut tracks, pivot) {
            put_channel(t, ty, ch);
        }
    }
    for c in chunk.find_children(CHUNK_BIT_CHANNEL) {
        let pivot = rd_u16(c.data, 6).unwrap_or(u16::MAX) as usize;
        let bit = parse_raw_bit_channel(c.data)?;
        if let Some(t) = ensure_track(&mut tracks, pivot) {
            t.vis = Some(bit);
        }
    }
    Ok(Animation {
        name: join_name(&hierarchy, &name),
        hierarchy,
        frame_count,
        frame_rate,
        flavor: AnimFlavor::Raw,
        tracks,
    })
}

fn parse_compressed(chunk: &W3dChunk) -> Result<Animation, Error> {
    let hdr = chunk.find_child(CHUNK_COMPRESSED_ANIMATION_HEADER).ok_or_else(|| invalid("compressed animation without header"))?;
    if hdr.data.len() < 44 {
        return Err(invalid("compressed animation header too short"));
    }
    let name = cstr(hdr.data.get(4..20).unwrap_or(&[]));
    let hierarchy = cstr(hdr.data.get(20..36).unwrap_or(&[]));
    let frame_count = rd_u32(hdr.data, 36).unwrap_or(0).min(MAX_FRAMES);
    let frame_rate = rd_u16(hdr.data, 40).unwrap_or(0).max(1) as u32;
    let flavor = match rd_u16(hdr.data, 42) {
        Some(ANIM_FLAVOR_TIMECODED) => AnimFlavor::TimeCoded,
        Some(ANIM_FLAVOR_ADAPTIVE_DELTA) => AnimFlavor::AdaptiveDelta,
        _ => return Err(invalid("compressed animation: unknown flavor")),
    };
    let mut tracks: Vec<PivotTrack> = Vec::new();
    for c in chunk.find_children(CHUNK_COMPRESSED_ANIMATION_CHANNEL) {
        let ty = c.data.get(7).copied().unwrap_or(u8::MAX) as u16;
        let pivot = rd_u16(c.data, 4).unwrap_or(u16::MAX) as usize;
        let ch = if flavor == AnimFlavor::TimeCoded {
            parse_timecoded_channel(c)?
        } else {
            parse_adaptive_channel(c)?
        };
        if let Some(t) = ensure_track(&mut tracks, pivot) {
            put_channel(t, ty, ch);
        }
    }
    for c in chunk.find_children(CHUNK_COMPRESSED_BIT_CHANNEL) {
        let pivot = rd_u16(c.data, 4).unwrap_or(u16::MAX) as usize;
        let bit = parse_timecoded_bit_channel(c.data)?;
        if let Some(t) = ensure_track(&mut tracks, pivot) {
            t.vis = Some(bit);
        }
    }
    Ok(Animation {
        name: join_name(&hierarchy, &name),
        hierarchy,
        frame_count,
        frame_rate,
        flavor,
        tracks,
    })
}

fn join_name(hierarchy: &str, name: &str) -> String {
    if hierarchy.is_empty() {
        name.to_string()
    } else if name.is_empty() {
        hierarchy.to_string()
    } else {
        format!("{hierarchy}.{name}")
    }
}

/// Decodes the first animation chunk of a W3D file.
pub fn parse_animation(bytes: &[u8], source: &str) -> Result<Animation, Error> {
    let file = W3dFile::parse(bytes).map_err(|e| invalid(format!("{source}: {e}")))?;
    if let Some(c) = file.chunks.iter().find(|c| c.chunk_type == CHUNK_ANIMATION) {
        parse_raw(c)
    } else if let Some(c) = file.chunks.iter().find(|c| c.chunk_type == CHUNK_COMPRESSED_ANIMATION) {
        parse_compressed(c)
    } else {
        Err(invalid(format!("{source}: no animation chunk")))
    }
}

fn w3d_anim_path(name: &str) -> String {
    let part = name.split_once('.').map(|(_, a)| a).unwrap_or(name);
    let n = normalize_path(part.trim());
    let n = n.strip_suffix(".w3d").unwrap_or(&n);
    format!("art\\w3d\\{n}.w3d")
}

/// Loads an animation by its INI name, e.g. `AIRngr_SKL.AIRngr_STA`.
pub fn load_animation(vfs: &Vfs, name: &str) -> Result<Animation, Error> {
    let path = w3d_anim_path(name);
    let bytes = vfs.read(&path).ok_or_else(|| Error::NotFound(path.clone()))?;
    parse_animation(&bytes, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: u32, payload: &[u8], container: bool) -> Vec<u8> {
        let mut v = id.to_le_bytes().to_vec();
        let sz = payload.len() as u32 | if container { 0x8000_0000 } else { 0 };
        v.extend_from_slice(&sz.to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    fn f32s(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    fn name16(s: &str) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.resize(16, 0);
        v
    }

    fn header(hier: &str, name: &str, frames: u32, rate: u32, flavor: u32) -> Vec<u8> {
        let mut h = 0x0004_0002u32.to_le_bytes().to_vec();
        h.extend_from_slice(&name16(name));
        h.extend_from_slice(&name16(hier));
        h.extend_from_slice(&frames.to_le_bytes());
        h.extend_from_slice(&(rate as u16).to_le_bytes());
        h.extend_from_slice(&(flavor as u16).to_le_bytes());
        assert_eq!(h.len(), 44);
        h
    }

    fn raw_anim_header(hier: &str, name: &str, frames: u32, rate: u32) -> Vec<u8> {
        let mut h = 0x0004_0002u32.to_le_bytes().to_vec();
        h.extend_from_slice(&name16(name));
        h.extend_from_slice(&name16(hier));
        h.extend_from_slice(&frames.to_le_bytes());
        h.extend_from_slice(&rate.to_le_bytes());
        assert_eq!(h.len(), 44);
        h
    }

    fn raw_motion(ty: u16, pivot: u16, first: u16, last: u16, vlen: u16, vals: &[f32]) -> Vec<u8> {
        let mut p = first.to_le_bytes().to_vec();
        p.extend_from_slice(&last.to_le_bytes());
        p.extend_from_slice(&vlen.to_le_bytes());
        p.extend_from_slice(&ty.to_le_bytes());
        p.extend_from_slice(&pivot.to_le_bytes());
        p.extend_from_slice(&0u16.to_le_bytes());
        p.extend_from_slice(&f32s(vals));
        chunk(CHUNK_ANIMATION_CHANNEL, &p, false)
    }

    #[test]
    fn raw_animation_samples_and_interpolates() {
        // Pivot 1: X from 0..2 = 0,10,20; Q constant identity.
        let mut body = chunk(CHUNK_ANIMATION_HEADER, &raw_anim_header("SKL", "STA", 3, 30), false);
        body.extend(raw_motion(ANIM_CHANNEL_X, 1, 0, 2, 1, &[0.0, 10.0, 20.0]));
        body.extend(raw_motion(ANIM_CHANNEL_Q, 1, 0, 2, 4, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]));
        let data = chunk(CHUNK_ANIMATION, &body, true);
        let a = parse_animation(&data, "t").unwrap();
        assert_eq!(a.flavor, AnimFlavor::Raw);
        assert_eq!(a.name, "SKL.STA");
        assert_eq!(a.frame_count, 3);
        assert_eq!(a.sample(1, 0.0).unwrap().0[0], 0.0);
        assert!((a.sample(1, 1.0).unwrap().0[0] - 10.0).abs() < 1e-4);
        assert!((a.sample(1, 2.0).unwrap().0[0] - 20.0).abs() < 1e-4);
        assert!(a.sample(2, 1.0).is_none());
    }

    #[test]
    fn raw_no_panic_on_truncated() {
        let data = chunk(CHUNK_ANIMATION, &chunk(CHUNK_ANIMATION_HEADER, &[0u8; 20], false), true);
        assert!(parse_animation(&data, "t").is_err());
    }

    #[test]
    fn timecoded_compressed_packets() {
        // One X channel, packets at frame 0 (value 5) and frame 10 (value 15).
        let mut c = 2u32.to_le_bytes().to_vec();
        c.extend_from_slice(&1u16.to_le_bytes());
        c.push(1);
        c.push(ANIM_CHANNEL_X as u8);
        c.extend_from_slice(&0u32.to_le_bytes());
        c.extend_from_slice(&5.0f32.to_bits().to_le_bytes());
        c.extend_from_slice(&10u32.to_le_bytes());
        c.extend_from_slice(&15.0f32.to_bits().to_le_bytes());
        let body = [chunk(CHUNK_COMPRESSED_ANIMATION_HEADER, &header("SKL", "MVA", 11, 30, 0), false), chunk(CHUNK_COMPRESSED_ANIMATION_CHANNEL, &c, false)].concat();
        let data = chunk(CHUNK_COMPRESSED_ANIMATION, &body, true);
        let a = parse_animation(&data, "t").unwrap();
        assert_eq!(a.flavor, AnimFlavor::TimeCoded);
        assert!((a.sample(1, 5.0).unwrap().0[0] - 10.0).abs() < 1e-4);
        assert!((a.sample(1, 10.0).unwrap().0[0] - 15.0).abs() < 1e-4);
        assert!((a.sample(1, 0.0).unwrap().0[0] - 5.0).abs() < 1e-4);
    }

    #[test]
    fn adaptive_delta_decompresses() {
        // Scale 1, filter index 8 (=1.0): frame1 = +1, frame2 = +2 more -> 3.
        let mut c = 3u32.to_le_bytes().to_vec();
        c.extend_from_slice(&1u16.to_le_bytes());
        c.push(1);
        c.push(ANIM_CHANNEL_X as u8);
        c.extend_from_slice(&1.0f32.to_le_bytes());
        // base value 0.0, then packet: header filter 8, nibbles 1, 2, ...
        c.extend_from_slice(&0.0f32.to_le_bytes());
        c.push(8);
        c.push(0x21);
        c.extend_from_slice(&[0u8; 7]);
        let body = [chunk(CHUNK_COMPRESSED_ANIMATION_HEADER, &header("SKL", "ADA", 3, 30, 1), false), chunk(CHUNK_COMPRESSED_ANIMATION_CHANNEL, &c, false)].concat();
        let data = chunk(CHUNK_COMPRESSED_ANIMATION, &body, true);
        let a = parse_animation(&data, "t").unwrap();
        assert_eq!(a.flavor, AnimFlavor::AdaptiveDelta);
        assert!((a.sample(1, 1.0).unwrap().0[0] - 1.0).abs() < 1e-4);
        assert!((a.sample(1, 2.0).unwrap().0[0] - 3.0).abs() < 1e-4);
    }
}
