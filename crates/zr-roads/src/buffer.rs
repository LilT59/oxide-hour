//! Road segment graph: links, curves, tee / Y / H / 4-way intersections, alpha joins and the
//! terrain-draped quad strips.
//!
//! Ported from `W3DRoadBuffer.cpp` (`references/GeneralsGameCode/GeneralsMD/Code/GameEngineDevice/
//! Source/W3DDevice/GameClient/`). Names follow the C++ (`addMapObject`, `insertCurveSegmentAt`,
//! `loadFloat4PtSection`, ...) so the two can be read side by side. Everything is 2D (x, y)
//! until the final strip is draped over the height map.

use std::f32::consts::PI;
use std::ops::{Add, AddAssign, Mul, Neg, Sub};

pub const MAP_XY_FACTOR: f32 = 10.0;
pub const MAP_HEIGHT_SCALE: f32 = MAP_XY_FACTOR / 16.0;

const TEE_WIDTH_ADJUSTMENT: f32 = 1.03;
/// Radius of the curve centre line, in road widths (`CORNER_RADIUS`, `TIGHT_CORNER_RADIUS`).
pub const CORNER_RADIUS: f32 = 1.5;
pub const TIGHT_CORNER_RADIUS: f32 = 0.5;
pub const DEFAULT_ROAD_SCALE: f32 = 8.0;
const MIN_ROAD_SEGMENT: f32 = 0.25;
const MAX_ROAD_SEGMENTS: usize = 2000;

/// Cap on strip vertices per segment (`MAX_SEG_VERTEX`).
const MAX_SEG_VERTEX: usize = 500;

pub const FLAG_ROAD_POINT1: i32 = 0x02;
pub const FLAG_ROAD_POINT2: i32 = 0x04;
pub const FLAG_ROAD_CORNER_ANGLED: i32 = 0x08;
pub const FLAG_ROAD_CORNER_TIGHT: i32 = 0x40;
pub const FLAG_ROAD_JOIN: i32 = 0x80;

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct V2 {
    pub x: f32,
    pub y: f32,
}

impl V2 {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    fn len(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    /// `Vector2::Normalize`: leaves a zero vector alone.
    fn norm(self) -> Self {
        let l2 = self.x * self.x + self.y * self.y;
        if l2 != 0.0 {
            let o = 1.0 / l2.sqrt();
            Self::new(self.x * o, self.y * o)
        } else {
            self
        }
    }
    fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y
    }
    /// Counter-clockwise rotation (`Vector2::Rotate`).
    fn rot(self, a: f32) -> Self {
        let (s, c) = a.sin_cos();
        Self::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }
    fn cross(self, o: Self) -> f32 {
        self.x * o.y - self.y * o.x
    }
}

impl Add for V2 {
    type Output = V2;
    fn add(self, o: V2) -> V2 {
        V2::new(self.x + o.x, self.y + o.y)
    }
}
impl Sub for V2 {
    type Output = V2;
    fn sub(self, o: V2) -> V2 {
        V2::new(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f32> for V2 {
    type Output = V2;
    fn mul(self, k: f32) -> V2 {
        V2::new(self.x * k, self.y * k)
    }
}
impl Neg for V2 {
    type Output = V2;
    fn neg(self) -> V2 {
        V2::new(-self.x, -self.y)
    }
}
impl AddAssign for V2 {
    fn add_assign(&mut self, o: V2) {
        *self = *self + o;
    }
}

/// `xpSign`: +1 when `b` is to the left of `a`, -1 to the right, 0 parallel.
fn xp_sign(a: V2, b: V2) -> i32 {
    let x = a.cross(b);
    if x < 0.0 {
        -1
    } else if x > 0.0 {
        1
    } else {
        0
    }
}

/// A 2D line through `p0`/`p1` (`LineSegClass`, z = 0).
#[derive(Clone, Copy)]
struct Line {
    p0: V2,
    dir: V2,
}

impl Line {
    fn new(p0: V2, p1: V2) -> Self {
        Self { p0, dir: (p1 - p0).norm() }
    }
    /// `LineSegClass::Find_Intersection` (unbounded lines): the point on `self` closest to
    /// `o`, or `None` when they are parallel.
    fn intersect(&self, o: &Line) -> Option<V2> {
        let cz = self.dir.cross(o.dir);
        if cz.abs() < 1e-6 {
            return None;
        }
        let c2 = (o.p0 - self.p0).cross(o.dir);
        Some(self.p0 + self.dir * (c2 / cz))
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Pt {
    pub loc: V2,
    pub top: V2,
    pub bottom: V2,
    pub count: i32,
    pub is_angled: bool,
    pub is_join: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Segment,
    Curve,
    Tee,
    FourWay,
    ThreeWayY,
    ThreeWayH,
    ThreeWayHFlip,
    AlphaJoin,
}

/// Draw order of the segment kinds within one road type (`TCorner`).
const KIND_ORDER: [Kind; 8] =
    [Kind::Segment, Kind::Curve, Kind::Tee, Kind::FourWay, Kind::ThreeWayY, Kind::ThreeWayH, Kind::ThreeWayHFlip, Kind::AlphaJoin];

#[derive(Clone, Copy, Debug)]
pub struct Vtx {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Clone, Debug)]
pub struct Seg {
    pub pt1: Pt,
    pub pt2: Pt,
    pub curve_radius: f32,
    pub kind: Kind,
    pub scale: f32,
    pub width_in_texture: f32,
    /// Road type (index into the INI road list).
    pub id: usize,
    pub verts: Vec<Vtx>,
    pub indices: Vec<u32>,
}

impl Seg {
    fn blank(kind: Kind) -> Self {
        Self {
            pt1: Pt::default(),
            pt2: Pt::default(),
            curve_radius: 0.0,
            kind,
            scale: 1.0,
            width_in_texture: 1.0,
            id: 0,
            verts: Vec::new(),
            indices: Vec::new(),
        }
    }
    fn flip(&mut self) {
        std::mem::swap(&mut self.pt1, &mut self.pt2);
    }
}

/// One road segment read from a pair of map objects.
pub struct RoadInput {
    pub loc1: [f32; 2],
    pub loc2: [f32; 2],
    pub scale: f32,
    pub width_in_texture: f32,
    pub id: usize,
    pub angled: [bool; 2],
    pub join: [bool; 2],
    pub tight: bool,
}

/// `(segment index, 0 = pt1 / 1 = pt2)`.
type PtRef = (usize, u8);

pub struct RoadBuffer<'a> {
    pub segs: Vec<Seg>,
    /// Per road type draw order (`RoadType::m_stackingOrder`).
    pub stacking: Vec<i32>,
    /// Maximum height of the four corners of the height cell containing (x, y).
    max_cell_height: &'a dyn Fn(f32, f32) -> f32,
}

impl<'a> RoadBuffer<'a> {
    pub fn new(num_types: usize, max_cell_height: &'a dyn Fn(f32, f32) -> f32) -> Self {
        Self { segs: Vec::new(), stacking: vec![0; num_types], max_cell_height }
    }

    /// `loadRoads`: runs the whole pipeline.
    pub fn load_roads(&mut self, inputs: &[RoadInput]) {
        self.add_map_objects(inputs);
        self.update_counts_and_flags();
        self.insert_tee_intersections();
        self.insert_curve_segments();
        self.insert_cross_type_joins();
        self.preload();
    }

    fn pt(&mut self, r: PtRef) -> &mut Pt {
        if r.1 == 0 {
            &mut self.segs[r.0].pt1
        } else {
            &mut self.segs[r.0].pt2
        }
    }
    fn loc(&self, r: PtRef) -> V2 {
        if r.1 == 0 {
            self.segs[r.0].pt1.loc
        } else {
            self.segs[r.0].pt2.loc
        }
    }
    fn other(r: PtRef) -> PtRef {
        (r.0, 1 - r.1)
    }

    // ---------------------------------------------------------------- linking

    fn add_map_objects(&mut self, inputs: &[RoadInput]) {
        for inp in inputs {
            if self.segs.len() >= MAX_ROAD_SEGMENTS {
                break;
            }
            let mut s = Seg::blank(Kind::Segment);
            s.scale = inp.scale;
            s.width_in_texture = inp.width_in_texture;
            s.id = inp.id;
            s.pt1.loc = V2::new(inp.loc1[0], inp.loc1[1]);
            s.pt2.loc = V2::new(inp.loc2[0], inp.loc2[1]);
            s.pt1.is_angled = inp.angled[0];
            s.pt1.is_join = inp.join[0];
            s.pt2.is_angled = inp.angled[1];
            s.pt2.is_join = inp.join[1];
            s.curve_radius = if inp.tight { TIGHT_CORNER_RADIUS } else { CORNER_RADIUS };
            self.add_map_object(s, true);
        }
        // Second pass: re-insert in the order produced by the first so links are contiguous.
        let old = std::mem::take(&mut self.segs);
        for s in old {
            self.add_map_object(s, false);
        }
    }

    fn update_counts(&mut self, cur: &mut Seg) {
        cur.pt1.count = 0;
        cur.pt2.count = 0;
        let (loc1, loc2) = (cur.pt1.loc, cur.pt2.loc);
        for r in &mut self.segs {
            if r.id != cur.id {
                continue;
            }
            if r.pt1.loc == loc1 {
                r.pt1.count += 1;
                cur.pt1.count += 1;
            }
            if r.pt1.loc == loc2 {
                r.pt1.count += 1;
                cur.pt2.count += 1;
            }
            if r.pt2.loc == loc1 {
                r.pt2.count += 1;
                cur.pt1.count += 1;
            }
            if r.pt2.loc == loc2 {
                r.pt2.count += 1;
                cur.pt2.count += 1;
            }
        }
    }

    fn add_map_object(&mut self, mut cur: Seg, update_counts: bool) {
        let (loc1, mut loc2) = (cur.pt1.loc, cur.pt2.loc);
        if update_counts && loc1 == loc2 {
            loc2.x += 0.25;
            cur.pt2.loc = loc2;
        }
        let rv = loc2 - loc1;
        let n = V2::new(-rv.y, rv.x).norm() * (cur.scale * cur.width_in_texture / 2.0);
        cur.pt1.top = loc1 + n;
        cur.pt1.bottom = loc1 - n;
        cur.pt2.top = loc2 + n;
        cur.pt2.bottom = loc2 - n;
        if update_counts {
            self.update_counts(&mut cur);
        }
        if self.segs.len() >= MAX_ROAD_SEGMENTS {
            return;
        }
        let (mut flip, mut add_before, mut add_after, mut both) = (false, false, false, false);
        let mut i = 0;
        while i < self.segs.len() {
            let r = &self.segs[i];
            both = false;
            if (r.pt1.loc == loc1 && r.pt2.loc == loc2) || (r.pt1.loc == loc2 && r.pt2.loc == loc1) {
                both = true; // identical segment: discard
                break;
            }
            if cur.pt1.count == 1 {
                if r.pt1.loc == loc1 {
                    flip = true;
                    add_after = true;
                }
                if r.pt2.loc == loc1 {
                    flip = false;
                    add_before = true;
                }
            }
            if cur.pt2.count == 1 {
                if r.pt1.loc == loc2 {
                    flip = false;
                    add_after = true;
                }
                if r.pt2.loc == loc2 {
                    flip = true;
                    add_before = true;
                }
            }
            if add_before || add_after {
                break;
            }
            i += 1;
        }
        if both {
            return;
        }
        let mut idx = i;
        if add_after {
            idx += 1;
        }
        if flip {
            cur.flip();
        }
        self.segs.insert(idx.min(self.segs.len()), cur);
        if add_before {
            self.check_link_before(idx);
        } else if add_after {
            self.check_link_after(idx);
        }
    }

    fn move_seg_to(&mut self, from: usize, to: usize) {
        if from >= self.segs.len() || to >= self.segs.len() || from == to {
            return;
        }
        let s = self.segs.remove(from);
        self.segs.insert(to, s);
    }

    fn check_link_before(&mut self, ndx: usize) {
        if self.segs[ndx].pt2.count != 1 {
            return;
        }
        let mut loc2 = self.segs[ndx].pt2.loc;
        let mut end_of_cur = ndx + 1;
        while end_of_cur + 1 < self.segs.len() {
            if self.segs[end_of_cur].pt1.loc != self.segs[end_of_cur + 1].pt2.loc {
                break;
            }
            end_of_cur += 1;
        }
        let mut check = end_of_cur + 1;
        while check < self.segs.len() {
            if self.segs[check].pt1.loc == loc2 {
                self.move_seg_to(check, ndx);
                loc2 = self.segs[ndx].pt2.loc;
                if self.segs[ndx].pt2.count != 1 {
                    return;
                }
                end_of_cur += 1;
            } else if self.segs[check].pt2.loc == loc2 {
                self.segs[check].flip();
                self.move_seg_to(check, ndx);
                loc2 = self.segs[ndx].pt2.loc;
                if self.segs[ndx].pt2.count != 1 {
                    return;
                }
                end_of_cur += 1;
            } else {
                check += 1;
            }
            if check <= end_of_cur {
                check = end_of_cur + 1;
            }
        }
    }

    fn check_link_after(&mut self, mut ndx: usize) {
        if self.segs[ndx].pt1.count != 1 || ndx + 1 >= self.segs.len() {
            return;
        }
        let mut loc1 = self.segs[ndx].pt1.loc;
        let mut check = ndx + 1;
        while check < self.segs.len() && ndx + 1 < self.segs.len() {
            if self.segs[check].pt2.loc == loc1 {
                ndx += 1;
                self.move_seg_to(check, ndx);
                loc1 = self.segs[ndx].pt1.loc;
                if self.segs[ndx].pt1.count != 1 {
                    return;
                }
            } else if self.segs[check].pt1.loc == loc1 {
                self.segs[check].flip();
                ndx += 1;
                self.move_seg_to(check, ndx);
                loc1 = self.segs[ndx].pt1.loc;
                if self.segs[ndx].pt1.count != 1 {
                    return;
                }
            } else {
                check += 1;
            }
        }
    }

    fn update_counts_and_flags(&mut self) {
        for s in &mut self.segs {
            s.pt1.count = 0;
            s.pt2.count = 0;
        }
        for j in (1..self.segs.len()).rev() {
            let (loc1, loc2, id) = (self.segs[j].pt1.loc, self.segs[j].pt2.loc, self.segs[j].id);
            for i in 0..j {
                if self.segs[i].id != id {
                    continue;
                }
                if self.segs[i].pt1.loc == loc1 {
                    self.segs[i].pt1.count += 1;
                    self.segs[j].pt1.count += 1;
                }
                if self.segs[i].pt1.loc == loc2 {
                    self.segs[i].pt1.count += 1;
                    self.segs[j].pt2.count += 1;
                }
                if self.segs[i].pt2.loc == loc1 {
                    self.segs[i].pt2.count += 1;
                    self.segs[j].pt1.count += 1;
                }
                if self.segs[i].pt2.loc == loc2 {
                    self.segs[i].pt2.count += 1;
                    self.segs[j].pt2.count += 1;
                }
            }
        }
    }

    // ------------------------------------------------------------ intersections

    fn push_inserted(&mut self, loc: V2, loc2: V2, like: usize, kind: Kind, width_in_texture: f32) {
        if self.segs.len() >= MAX_ROAD_SEGMENTS {
            return;
        }
        let mut s = Seg::blank(kind);
        s.pt1.loc = loc;
        s.pt2.loc = loc2;
        s.scale = self.segs[like].scale;
        s.width_in_texture = width_in_texture;
        s.pt1.count = -3;
        s.id = self.segs[like].id;
        self.segs.push(s);
    }

    fn insert_tee_intersections(&mut self) {
        let n = self.segs.len();
        for i in 0..n {
            if self.segs[i].kind != Kind::Segment {
                continue;
            }
            let scale = self.segs[i].scale;
            for p in 0..2u8 {
                let (count, loc) = if p == 0 { (self.segs[i].pt1.count, self.segs[i].pt1.loc) } else { (self.segs[i].pt2.count, self.segs[i].pt2.loc) };
                if count == 2 {
                    self.insert_tee(loc, i, scale);
                }
                if count == 3 {
                    self.insert_4way(loc, i, scale);
                }
            }
        }
    }

    /// Collects the other segments touching `loc` after `index1` (`pr` = far points, `pc` =
    /// points at `loc`), marking them with count -2 as the original does.
    fn gather(&mut self, loc: V2, index1: usize, max: usize) -> (Vec<PtRef>, Vec<PtRef>) {
        let (mut pr, mut pc) = (Vec::new(), Vec::new());
        let own = if self.segs[index1].pt1.loc == loc { (index1, 0u8) } else { (index1, 1u8) };
        pc.push(own);
        pr.push(Self::other(own));
        for i in index1 + 1..self.segs.len() {
            for w in 0..2u8 {
                if self.loc((i, w)) == loc {
                    self.pt((i, w)).count = -2;
                    if pc.len() < max {
                        pc.push((i, w));
                        pr.push((i, 1 - w));
                    }
                }
            }
        }
        (pr, pc)
    }

    fn insert_tee(&mut self, loc: V2, index1: usize, scale: f32) {
        if self.insert_y(loc, index1, scale) {
            return;
        }
        let (pr, pc) = self.gather(loc, index1, 3);
        if pr.len() < 3 {
            return;
        }
        let (pc1, pc2, pc3) = (pc[0], pc[1], pc[2]);
        let v1 = (self.loc(pr[0]) - loc).norm();
        let v2 = (self.loc(pr[1]) - loc).norm();
        let v3 = (self.loc(pr[2]) - loc).norm();
        let (dot12, dot13, dot32) = (v1.dot(v2), v1.dot(v3), v3.dot(v2));
        let (mut do12, mut do13, mut do32) = (false, false, false);
        if dot12 < dot13 {
            if dot12 < dot32 {
                do12 = true;
            } else {
                do32 = true;
            }
        } else if dot13 < dot32 {
            do13 = true;
        } else {
            do32 = true;
        }
        let (mut up, mut decider) = (V2::default(), V2::default());
        if do12 {
            up = v2 - v1;
            decider = v3;
        }
        if do13 {
            up = v3 - v1;
            decider = v2;
        }
        if do32 {
            up = v2 - v3;
            decider = v1;
        }
        up = up.norm();
        let wit = self.segs[index1].width_in_texture;
        let cos60 = 0.5;
        let dot = up.dot(decider).abs();
        let xp = up.cross(decider);
        if dot > cos60 {
            // The arm of the tee is slanted: an H tee.
            let mut angle = PI / 2.0;
            let mut mirror = false;
            if xp < 0.0 {
                angle = -angle;
                mirror = true;
            }
            up = up.norm() * (0.5 * scale);
            let tee = up.rot(angle);
            let flip;
            if do12 {
                flip = xp_sign(tee, v3) == 1;
                self.offset_h(pc1, pc2, pc3, loc, up, tee, flip, mirror, wit);
            } else if do13 {
                flip = xp_sign(tee, v2) == 1;
                self.offset_h(pc1, pc3, pc2, loc, up, tee, flip, mirror, wit);
            } else {
                flip = xp_sign(tee, v1) == 1;
                self.offset_h(pc3, pc2, pc1, loc, up, tee, flip, mirror, wit);
            }
            for p in [pc1, pc2, pc3] {
                self.pt(p).count = 0;
            }
            let kind = if flip { Kind::ThreeWayHFlip } else { Kind::ThreeWayH };
            self.push_inserted(loc, loc + tee, index1, kind, wit);
        } else {
            let mut angle = PI / 2.0;
            if xp < 0.0 {
                angle = -angle;
            }
            up = up.norm() * (0.5 * scale);
            let tee = up.rot(angle);
            if do12 {
                self.offset_3way(pc1, pc2, pc3, loc, up, tee, wit);
            } else if do13 {
                self.offset_3way(pc1, pc3, pc2, loc, up, tee, wit);
            } else {
                self.offset_3way(pc3, pc2, pc1, loc, up, tee, wit);
            }
            for p in [pc1, pc2, pc3] {
                self.pt(p).count = 0;
            }
            self.push_inserted(loc, loc + tee, index1, Kind::Tee, wit);
        }
    }

    fn insert_y(&mut self, loc: V2, index1: usize, scale: f32) -> bool {
        let (pr, pc) = self.gather(loc, index1, 3);
        if pr.len() < 3 {
            return false;
        }
        let (pc1, pc2, pc3) = (pc[0], pc[1], pc[2]);
        let v1 = (self.loc(pr[0]) - loc).norm();
        let v2 = (self.loc(pr[1]) - loc).norm();
        let v3 = (self.loc(pr[2]) - loc).norm();
        let (dot12, dot13, dot32) = (v1.dot(v2), v1.dot(v3), v3.dot(v2));
        let (mut do12, mut do13, mut do32) = (false, false, false);
        let (mut score12, mut score13, mut score32) = (2.0f32, 2.0f32, 2.0f32);
        let cos30 = 0.866;
        let cos45 = 0.707;
        if dot12 < -cos30 || dot13 < -cos30 || dot32 < -cos30 {
            return false; // too close to a straight line: straight-sided tee
        }
        let s2 = xp_sign(v1, v2);
        let s3 = xp_sign(v1, v3);
        if s2 != s3 && s2 + s3 == 0 {
            let v1_90 = V2::new(-v1.y, v1.x);
            if xp_sign(v1_90, v2) == 1 && xp_sign(v1_90, v3) == 1 {
                do32 = true;
                score32 = (dot12 + cos45).abs() + (dot13 + cos45).abs();
            }
        }
        let s1 = xp_sign(v3, v1);
        let s2 = xp_sign(v3, v2);
        if s2 != s1 && s2 + s1 == 0 {
            let v3_90 = V2::new(-v3.y, v3.x);
            if xp_sign(v3_90, v2) == 1 && xp_sign(v3_90, v1) == 1 {
                do12 = true;
                score12 = (dot13 + cos45).abs() + (dot32 + cos45).abs();
            }
        }
        let s1 = xp_sign(v2, v1);
        let s3 = xp_sign(v2, v3);
        if s3 != s1 && s3 + s1 == 0 {
            let v2_90 = V2::new(-v2.y, v2.x);
            if xp_sign(v2_90, v3) == 1 && xp_sign(v2_90, v1) == 1 {
                do13 = true;
                score13 = (dot12 + cos45).abs() + (dot32 + cos45).abs();
            }
        }
        if score12 < score13 {
            do13 = false;
            if score12 < score32 {
                do32 = false;
            } else {
                do12 = false;
            }
        } else {
            do12 = false;
            if score13 < score32 {
                do32 = false;
            } else {
                do13 = false;
            }
        }
        let mut up = if do12 {
            v3
        } else if do13 {
            v2
        } else if do32 {
            v1
        } else {
            return false;
        };
        up = up.norm() * (0.5 * scale);
        let tee = up.rot(-PI / 2.0);
        let wit = self.segs[index1].width_in_texture;
        if do12 {
            if xp_sign(v3, v1) == -1 {
                self.offset_y(pc1, pc2, pc3, loc, up, wit);
            } else {
                self.offset_y(pc2, pc1, pc3, loc, up, wit);
            }
        }
        if do13 {
            if xp_sign(v2, v1) == -1 {
                self.offset_y(pc1, pc3, pc2, loc, up, wit);
            } else {
                self.offset_y(pc3, pc1, pc2, loc, up, wit);
            }
        }
        if do32 {
            if xp_sign(v1, v3) == -1 {
                self.offset_y(pc3, pc2, pc1, loc, up, wit);
            } else {
                self.offset_y(pc2, pc3, pc1, loc, up, wit);
            }
        }
        for p in [pc1, pc2, pc3] {
            self.pt(p).count = 0;
        }
        self.push_inserted(loc, loc + tee, index1, Kind::ThreeWayY, wit);
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn offset_3way(&mut self, pc1: PtRef, pc2: PtRef, pc3: PtRef, loc: V2, up: V2, tee: V2, wit: f32) {
        // pc3 keeps its own top/bottom orientation test below, so read before moving.
        self.pt(pc1).loc = loc - up;
        self.pt(pc2).loc = loc + up;
        self.pt(pc3).loc = loc + tee;
        let mut right_tee = tee;
        if up.cross(tee) < 0.0 {
            right_tee = -tee;
        }
        right_tee = right_tee * wit;
        for p in [pc1, pc2] {
            let pt = self.pt(p);
            if up.cross(pt.top - pt.loc) > 0.0 {
                pt.bottom = pt.loc - right_tee;
                pt.top = pt.loc + right_tee;
            } else {
                pt.bottom = pt.loc + right_tee;
                pt.top = pt.loc - right_tee;
            }
        }
        let up_w = up * wit;
        let pt = self.pt(pc3);
        if right_tee.cross(pt.top - pt.loc) < 0.0 {
            pt.bottom = pt.loc - up_w;
            pt.top = pt.loc + up_w;
        } else {
            pt.bottom = pt.loc + up_w;
            pt.top = pt.loc - up_w;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn offset_h(&mut self, pc1: PtRef, pc2: PtRef, pc3: PtRef, loc: V2, up: V2, tee: V2, flip: bool, mirror: bool, wit: f32) {
        if flip != mirror {
            self.pt(pc1).loc = loc - up * 2.05;
            self.pt(pc2).loc = loc + up * 0.46;
        } else {
            self.pt(pc1).loc = loc - up * 0.46;
            self.pt(pc2).loc = loc + up * 2.05;
        }
        let mut right_tee = tee;
        if up.cross(tee) < 0.0 {
            right_tee = -tee;
        }
        right_tee = right_tee * wit;
        for p in [pc1, pc2] {
            let pt = self.pt(p);
            if up.cross(pt.top - pt.loc) > 0.0 {
                pt.bottom = pt.loc - right_tee;
                pt.top = pt.loc + right_tee;
            } else {
                pt.bottom = pt.loc + right_tee;
                pt.top = pt.loc - right_tee;
            }
        }
        let arm = tee.rot(if flip { PI / 4.0 } else { -PI / 4.0 });
        let arm_normal = V2::new(-arm.y, arm.x) * wit;
        let pt = self.pt(pc3);
        pt.loc += arm * 2.10;
        if xp_sign(arm, pt.top - loc) == 1 {
            pt.top = pt.loc + arm_normal;
            pt.bottom = pt.loc - arm_normal;
        } else {
            pt.top = pt.loc - arm_normal;
            pt.bottom = pt.loc + arm_normal;
        }
    }

    fn offset_y(&mut self, pc1: PtRef, pc2: PtRef, pc3: PtRef, loc: V2, up: V2, wit: f32) {
        {
            let pt = self.pt(pc3);
            pt.loc += up * 0.55;
            pt.top += up * 0.55;
            pt.bottom += up * 0.55;
        }
        let arm = up.rot(3.0 * PI / 4.0);
        let arm_normal = V2::new(-arm.y, arm.x) * wit;
        {
            let pt = self.pt(pc2);
            pt.loc += arm * 1.1;
            if xp_sign(arm, pt.top - loc) == 1 {
                pt.top = pt.loc + arm_normal;
                pt.bottom = pt.loc - arm_normal;
            } else {
                pt.top = pt.loc - arm_normal;
                pt.bottom = pt.loc + arm_normal;
            }
        }
        let arm = up.rot(-3.0 * PI / 4.0);
        let arm_normal = V2::new(-arm.y, arm.x) * wit;
        let pt = self.pt(pc1);
        pt.loc += arm * 1.1;
        if xp_sign(arm, pt.top - loc) == 1 {
            pt.top = pt.loc + arm_normal;
            pt.bottom = pt.loc - arm_normal;
        } else {
            pt.top = pt.loc - arm_normal;
            pt.bottom = pt.loc + arm_normal;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn offset_4way(&mut self, pc1: PtRef, pc2: PtRef, pc3: PtRef, pr3: PtRef, pc4: PtRef, loc: V2, align: V2, wit: f32) {
        self.pt(pc1).loc = loc - align;
        self.pt(pc2).loc = loc + align;
        let v3 = self.loc(pr3) - loc;
        let mut angle = PI / 2.0;
        if align.cross(v3) < 0.0 {
            angle = -angle;
        }
        let tee = align.rot(angle);
        self.pt(pc3).loc = loc + tee;
        self.pt(pc4).loc = loc - tee;
        let real_tee = align.rot(PI / 2.0) * wit;
        for p in [pc1, pc2] {
            let pt = self.pt(p);
            if align.cross(pt.top - pt.loc) > 0.0 {
                pt.bottom = pt.loc - real_tee;
                pt.top = pt.loc + real_tee;
            } else {
                pt.bottom = pt.loc + real_tee;
                pt.top = pt.loc - real_tee;
            }
        }
        let align_w = align * wit;
        for p in [pc3, pc4] {
            let pt = self.pt(p);
            if real_tee.cross(pt.top - pt.loc) < 0.0 {
                pt.bottom = pt.loc - align_w;
                pt.top = pt.loc + align_w;
            } else {
                pt.bottom = pt.loc + align_w;
                pt.top = pt.loc - align_w;
            }
        }
        for p in [pc1, pc2, pc3, pc4] {
            self.pt(p).count = 0;
        }
    }

    fn insert_4way(&mut self, loc: V2, index1: usize, scale: f32) {
        let (pr, pc) = self.gather(loc, index1, 4);
        if pr.len() < 4 {
            return;
        }
        let v: Vec<V2> = pr.iter().map(|&r| (self.loc(r) - loc).norm()).collect();
        // The most opposite pair of legs defines the alignment.
        let pairs = [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];
        let mut best = pairs[0];
        let mut cur = v[0].dot(v[1]);
        for &(a, b) in &pairs[1..] {
            let d = v[a].dot(v[b]);
            if d < cur {
                cur = d;
                best = (a, b);
            }
        }
        let mut align = (v[best.1] - v[best.0]).norm() * (0.5 * scale);
        let wit = self.segs[index1].width_in_texture;
        // (pc_a, pc_b, pc_c, pr_c, pc_d) per original argument orders.
        let (a, b, c, d) = match best {
            (0, 1) => (0, 1, 2, 3),
            (0, 2) => (0, 2, 1, 3),
            (0, 3) => (0, 3, 2, 1),
            (1, 2) => (1, 2, 0, 3),
            (1, 3) => (1, 3, 0, 2),
            _ => (2, 3, 0, 1),
        };
        // pr3 argument: do12 -> pr3, do13 -> pr2, do14 -> pr3, do23 -> pr1, do24 -> pr1, do34 -> pr1.
        let pr_arg = match best {
            (0, 1) => pr[2],
            (0, 2) => pr[1],
            (0, 3) => pr[2],
            _ => pr[0],
        };
        self.offset_4way(pc[a], pc[b], pc[c], pr_arg, pc[d], loc, align, wit);
        if align.x < 0.0 {
            align = -align;
        }
        self.push_inserted(loc, loc + align, index1, Kind::FourWay, TEE_WIDTH_ADJUSTMENT);
        let last = self.segs.len() - 1;
        self.segs[last].pt1.count = -4;
    }

    // ------------------------------------------------------------------ curves

    fn insert_curve_segments(&mut self) {
        let n = self.segs.len();
        let mut start: Option<usize> = None;
        for i in 0..n {
            if i + 1 < n && self.segs[i].pt1.loc == self.segs[i + 1].pt2.loc {
                if self.segs[i + 1].pt2.count == 1 && self.segs[i].pt1.count == 1 {
                    self.insert_curve_segment_at(i, i + 1);
                    if start.is_none() {
                        start = Some(i);
                    }
                }
            } else if let Some(s) = start {
                if self.segs[i].pt1.loc == self.segs[s].pt2.loc && self.segs[s].pt2.count == 1 && self.segs[i].pt1.count == 1 {
                    self.insert_curve_segment_at(i, s);
                }
                start = None;
            }
        }
    }

    fn miter(&mut self, n1: usize, n2: usize) {
        let l1 = Line::new(self.segs[n1].pt1.top, self.segs[n1].pt2.top);
        let l2 = Line::new(self.segs[n2].pt1.top, self.segs[n2].pt2.top);
        if let Some(p) = l1.intersect(&l2) {
            self.segs[n2].pt2.top = p;
            self.segs[n1].pt1.top = p;
        }
        let l1 = Line::new(self.segs[n1].pt1.bottom, self.segs[n1].pt2.bottom);
        let l2 = Line::new(self.segs[n2].pt1.bottom, self.segs[n2].pt2.bottom);
        if let Some(p) = l1.intersect(&l2) {
            self.segs[n2].pt2.bottom = p;
            self.segs[n1].pt1.bottom = p;
        }
    }

    fn set_loc(&mut self, r: PtRef, v: V2) {
        self.pt(r).loc = v;
    }

    fn push_curve(&mut self, a: V2, b: V2, like: usize) {
        if self.segs.len() >= MAX_ROAD_SEGMENTS {
            return;
        }
        let mut s = Seg::blank(Kind::Curve);
        s.pt1.loc = a;
        s.pt2.loc = b;
        s.scale = self.segs[like].scale;
        s.width_in_texture = self.segs[like].width_in_texture;
        s.curve_radius = self.segs[like].curve_radius;
        s.id = self.segs[like].id;
        self.segs.push(s);
    }

    fn insert_curve_segment_at(&mut self, n1: usize, n2: usize) {
        const DOT_LIMIT: f32 = 0.5;
        let radius = self.segs[n1].curve_radius * self.segs[n1].scale;
        let original = self.segs[n1].pt1.loc;
        if self.segs[n1].id != self.segs[n2].id {
            return;
        }
        let mut line1 = Line::new(self.segs[n1].pt1.loc, self.segs[n1].pt2.loc);
        let mut line2 = Line::new(self.segs[n2].pt1.loc, self.segs[n2].pt2.loc);
        let cur_sin = line1.dir.dot(line2.dir);
        let xp = line1.dir.cross(line2.dir);
        let (pr1, pr2, pr3, pr4): (PtRef, PtRef, PtRef, PtRef);
        if xp > 0.0 {
            pr1 = (n1, 0);
            pr2 = (n1, 1);
            pr3 = (n2, 0);
            pr4 = (n2, 1);
        } else {
            pr4 = (n1, 0);
            pr3 = (n1, 1);
            pr2 = (n2, 0);
            pr1 = (n2, 1);
            line1 = Line::new(self.loc(pr1), self.loc(pr2));
            line2 = Line::new(self.loc(pr3), self.loc(pr4));
        }
        let angle = cur_sin.clamp(-1.0, 1.0).acos();
        let count = angle / (PI / 6.0);
        if count < 0.9 || self.segs[n1].pt1.is_angled {
            self.miter(n1, n2);
            return;
        }
        let off1 = line1.dir.rot(-PI / 2.0) * radius;
        let off2 = line2.dir.rot(-PI / 2.0) * radius;
        let o1 = Line::new(self.loc(pr1) + off1, self.loc(pr2) + off1);
        let o2 = Line::new(self.loc(pr3) + off2, self.loc(pr4) + off2);
        let Some(int1) = o1.intersect(&o2) else { return };
        let cross1 = Line::new(int1, int1 - off2);
        let cross2 = Line::new(int1, int1 - off1);
        let (Some(p_int1), Some(p_int3)) = (cross1.intersect(&line2), cross2.intersect(&line1)) else {
            return;
        };
        if line2.dir.dot(p_int1 - self.loc(pr3)) < DOT_LIMIT || line1.dir.dot(self.loc(pr2) - p_int3) < DOT_LIMIT {
            self.set_loc(pr1, original);
            self.set_loc(pr4, original);
            self.miter(n1, n2);
            return;
        }
        self.set_loc(pr4, p_int1);
        let ang = -PI / 6.0;
        let mut pt2 = self.loc(pr4);
        let mut pt1 = self.loc(pr3);
        let mut direction = pt1 - pt2;
        let mut center = V2::new(-direction.y, direction.x).norm() * (self.segs[n1].curve_radius * self.segs[n1].scale);
        center += pt2;
        pt2 = rotate_about(pt2, center, ang);
        direction = direction.rot(ang);
        pt1 = pt2 + direction;
        self.push_curve(pt2, pt1, n1);
        if count > 2.0 {
            let mut i = 2.0;
            while i < count {
                direction = direction.rot(ang);
                pt2 = rotate_about(pt2, center, ang);
                pt1 = pt2 + direction;
                self.push_curve(pt2, pt1, n1);
                i += 1.0;
            }
        }
        self.set_loc(pr1, p_int3);
        if count > 1.0 {
            let pt2 = self.loc(pr1);
            let pt1 = pt2 + (self.loc(pr1) - self.loc(pr2));
            self.push_curve(pt2, pt1, n1);
        }
        // Recalculate top & bottom.
        let rv = self.segs[n1].pt2.loc - self.segs[n1].pt1.loc;
        let n = V2::new(-rv.y, rv.x).norm() * (self.segs[n1].scale * self.segs[n1].width_in_texture / 2.0);
        let l = self.segs[n1].pt1.loc;
        self.segs[n1].pt1.top = l + n;
        self.segs[n1].pt1.bottom = l - n;
        let rv = self.segs[n2].pt2.loc - self.segs[n2].pt1.loc;
        let n = V2::new(-rv.y, rv.x).norm() * (self.segs[n2].scale * self.segs[n2].width_in_texture / 2.0);
        let l = self.segs[n2].pt2.loc;
        self.segs[n2].pt2.top = l + n;
        self.segs[n2].pt2.bottom = l - n;
    }

    // ------------------------------------------------------------- cross joins

    fn find_cross_type_join_vector(&self, loc: V2, join: &mut V2, id: usize) -> Option<usize> {
        for r in &self.segs {
            if r.id == id || r.kind != Kind::Segment {
                continue;
            }
            let (l1, l2) = (r.pt1.loc, r.pt2.loc);
            let half = r.scale / 2.0;
            let (lo_x, hi_x) = (l1.x.min(l2.x) - half, l1.x.max(l2.x) + half);
            let (lo_y, hi_y) = (l1.y.min(l2.y) - half, l1.y.max(l2.y) + half);
            if loc.x >= lo_x && loc.y >= lo_y && loc.x <= hi_x && loc.y <= hi_y {
                // Closest point on the segment (clamped, `Find_Point_Closest_To`).
                let d = l2 - l1;
                let len2 = d.dot(d);
                let t = if len2 > 0.0 { ((loc - l1).dot(d) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let on = l1 + d * t;
                if (on - loc).len() < r.scale * 0.55 {
                    let mut rv = l2 - l1;
                    rv = if xp_sign(rv, *join) == 1 { rv.rot(PI / 2.0) } else { rv.rot(-PI / 2.0) };
                    *join = rv;
                    return Some(r.id);
                }
            }
        }
        None
    }

    fn adjust_stacking(&mut self, top: usize, bottom: usize) {
        if top >= self.stacking.len() || bottom >= self.stacking.len() {
            return;
        }
        if self.stacking[top] > self.stacking[bottom] {
            return;
        }
        let new = self.stacking[bottom];
        for s in &mut self.stacking {
            if *s > new {
                *s += 1;
            }
        }
        self.stacking[top] = new + 1;
    }

    fn insert_cross_type_joins(&mut self) {
        let n = self.segs.len();
        for i in 0..n {
            let (loc1, loc2, is_pt1);
            if self.segs[i].pt2.count == 0 && self.segs[i].pt2.is_join {
                loc1 = self.segs[i].pt2.loc;
                loc2 = self.segs[i].pt1.loc;
                is_pt1 = false;
            } else if self.segs[i].pt1.count == 0 && self.segs[i].pt1.is_join {
                loc1 = self.segs[i].pt1.loc;
                loc2 = self.segs[i].pt2.loc;
                is_pt1 = true;
            } else {
                continue;
            }
            let road_vector = (loc2 - loc1).norm();
            let mut join = road_vector;
            let other = self.find_cross_type_join_vector(loc1, &mut join, self.segs[i].id);
            if other.is_none() {
                join = join * 100.0;
            }
            let road_normal = V2::new(-road_vector.y, road_vector.x);
            let join_normal = V2::new(-join.y, join.x);
            let half = self.segs[i].scale * self.segs[i].width_in_texture / 2.0;
            // Join line is a 1-unit segment from loc1 along join_normal (direction only matters).
            let jl = Line::new(loc1, loc1 + join_normal);
            let road_line = Line::new(loc1 + road_normal * half, loc2 + road_normal * half);
            let mut top = self.segs[i].pt1.top;
            if let Some(p) = jl.intersect(&road_line) {
                if is_pt1 {
                    self.segs[i].pt1.top = p;
                    top = p;
                } else {
                    self.segs[i].pt2.bottom = p;
                    top = p;
                }
            }
            let road_line = Line::new(loc1 - road_normal * half, loc2 - road_normal * half);
            let mut bottom = self.segs[i].pt1.bottom;
            if let Some(p) = jl.intersect(&road_line) {
                if is_pt1 {
                    self.segs[i].pt1.bottom = p;
                    bottom = p;
                } else {
                    self.segs[i].pt2.top = p;
                    bottom = p;
                }
            }
            let scale_adjustment = (bottom - top).len() / (self.segs[i].scale * self.segs[i].width_in_texture);
            if let Some(o) = other {
                let id = self.segs[i].id;
                self.adjust_stacking(id, o);
            }
            if self.segs.len() >= MAX_ROAD_SEGMENTS {
                return;
            }
            let mut s = Seg::blank(Kind::AlphaJoin);
            s.pt1.loc = loc1;
            s.pt2.loc = loc1 + join;
            s.scale = self.segs[i].scale;
            s.width_in_texture = self.segs[i].scale * scale_adjustment;
            s.id = self.segs[i].id;
            self.segs.push(s);
        }
    }

    // -------------------------------------------------------------- strip build

    fn preload(&mut self) {
        for kind in [Kind::Segment, Kind::Curve, Kind::ThreeWayY, Kind::ThreeWayH, Kind::Tee, Kind::AlphaJoin] {
            for i in 0..self.segs.len() {
                let k = self.segs[i].kind;
                let hit = match kind {
                    Kind::ThreeWayH => k == Kind::ThreeWayH || k == Kind::ThreeWayHFlip,
                    Kind::Tee => k == Kind::Tee || k == Kind::FourWay,
                    _ => k == kind,
                };
                if !hit {
                    continue;
                }
                let (loc1, loc2, scale) = (self.segs[i].pt1.loc, self.segs[i].pt2.loc, self.segs[i].scale);
                match k {
                    Kind::Segment => self.preload_segment(i),
                    Kind::Curve => self.load_curve(i, loc1, loc2, scale),
                    Kind::ThreeWayY => self.load_y(i, loc1, loc2, scale),
                    Kind::ThreeWayH | Kind::ThreeWayHFlip => self.load_h(i, loc1, loc2, k == Kind::ThreeWayHFlip, scale),
                    Kind::Tee | Kind::FourWay => self.load_tee(i, loc1, loc2, k == Kind::FourWay, scale),
                    Kind::AlphaJoin => self.load_alpha_join(i, loc1, loc2, scale),
                }
            }
        }
    }

    /// Segments of road type `id` in draw order (`loadRoadsInVertexAndIndexBuffers`).
    pub fn segments_of(&self, id: usize) -> impl Iterator<Item = &Seg> {
        KIND_ORDER.into_iter().flat_map(move |k| self.segs.iter().filter(move |s| s.id == id && s.kind == k))
    }

    fn preload_segment(&mut self, i: usize) {
        let s = &self.segs[i];
        let road_vector = s.pt2.loc - s.pt1.loc;
        let road_height = s.width_in_texture * s.scale / 2.0;
        let normal = V2::new(-road_vector.y, road_vector.x).norm() * road_height;
        let corners = [s.pt1.bottom, s.pt2.bottom, s.pt1.top, s.pt2.top]; // BL, BR, TL, TR
        let (loc, scale) = (s.pt1.loc, s.scale);
        self.float_4pt(i, loc, normal, road_vector, corners, 0.0, 85.0 / 512.0, scale, scale);
    }

    fn load_curve(&mut self, i: usize, loc1: V2, loc2: V2, scale: f32) {
        let tight = self.segs[i].curve_radius == TIGHT_CORNER_RADIUS;
        let u_off = 4.0 / 512.0;
        let v_off = if tight { 425.0 / 512.0 } else { 255.0 / 512.0 };
        let mut rv = loc2 - loc1;
        let mut rn = V2::new(-rv.y, rv.x);
        let curve_height = self.segs[i].width_in_texture * scale / 2.0;
        rv = rv.norm() * scale;
        rn = rn.norm() * curve_height.abs();
        // corners: [BL, BR, TL, TR]
        let mut bl = loc1 - rn;
        let mut br = bl;
        let mut tl;
        let mut tr;
        if tight {
            br += rv * 0.5;
            tr = br + rn * 2.0;
            tl = bl + rn * 2.0;
            br += rv * 0.1;
            br += rn * 0.2;
            bl = bl - rn * 0.1;
            bl = bl - rv * 0.02;
            tl = tl - rv * 0.02;
            tr = tr - rv * 0.4;
            tr += rn * 0.2;
        } else {
            br += rv;
            tr = br + rn * 2.0;
            tl = bl + rn * 2.0;
            br += rv * 0.1;
            br += rn * 0.4;
            bl = bl - rn * 0.2;
            bl = bl - rv * 0.02;
            tl = tl - rv * 0.02;
            tr = tr - rv * 0.4;
            tr += rn * 0.4;
        }
        self.float_4pt(i, loc1, rn, rv, [bl, br, tl, tr], u_off, v_off, scale, scale);
    }

    fn unit_or_default(rv: V2) -> (V2, V2) {
        if rv.x.abs() < MIN_ROAD_SEGMENT && rv.y.abs() < MIN_ROAD_SEGMENT {
            (V2::new(1.0, 0.0), V2::new(0.0, 1.0))
        } else {
            (rv.norm(), V2::new(-rv.y, rv.x).norm())
        }
    }

    fn load_tee(&mut self, i: usize, loc1: V2, loc2: V2, is_4way: bool, scale: f32) {
        let (u_off, v_off) = if is_4way { (425.0 / 512.0, 425.0 / 512.0) } else { (425.0 / 512.0, 255.0 / 512.0) };
        let tee_factor = scale * TEE_WIDTH_ADJUSTMENT / 2.0;
        let left = self.segs[i].width_in_texture * scale / 2.0;
        self.float_section(i, loc1, loc2 - loc1, tee_factor, left, tee_factor, u_off, v_off, scale);
    }

    fn load_alpha_join(&mut self, i: usize, loc1: V2, loc2: V2, scale: f32) {
        let (mut rv, mut rn) = Self::unit_or_default(loc2 - loc1);
        let (u_off, v_off) = (106.0 / 512.0, 425.0 / 512.0);
        let u_scale = self.segs[i].width_in_texture;
        rv = rv * (scale * 48.0 / 128.0);
        rn = rn * (u_scale * (1.0 + 8.0 / 128.0));
        let tl = loc1 + rn * 0.5 - rv * 0.65;
        let bl = tl - rn;
        let br = bl + rv;
        let tr = tl + rv;
        self.float_4pt(i, loc1, rn, rv, [bl, br, tl, tr], u_off, v_off, scale, u_scale);
    }

    fn load_y(&mut self, i: usize, loc1: V2, loc2: V2, scale: f32) {
        let (mut rv, mut rn) = Self::unit_or_default(loc2 - loc1);
        let (u_off, v_off) = (255.0 / 512.0, 226.0 / 512.0);
        rv = rv * scale;
        rn = rn * scale;
        rv = rv * 1.59;
        let tl = loc1 + rn * 0.29 - rv * 0.5;
        let bl = tl - rn * 1.08;
        let br = bl + rv;
        let tr = tl + rv;
        self.float_4pt(i, loc1, rn, rv, [bl, br, tl, tr], u_off, v_off, scale, scale);
    }

    fn load_h(&mut self, i: usize, loc1: V2, loc2: V2, flip: bool, scale: f32) {
        let (mut rv, mut rn) = Self::unit_or_default(loc2 - loc1);
        let (u_off, v_off) = (202.0 / 512.0, 364.0 / 512.0);
        let wit = self.segs[i].width_in_texture;
        rv = rv * scale;
        rn = rn * scale;
        rn = rn * 1.35;
        let bl = if flip { loc1 - rn * 0.20 - rv * (wit / 2.0) } else { loc1 - rn * 0.8 - rv * (wit / 2.0) };
        let mut width = rv * (wit / 2.0);
        width += rv * 1.2;
        let br = bl + width;
        let tr = br + rn;
        let tl = bl + rn;
        if flip {
            rn = -rn;
        }
        self.float_4pt(i, loc1, rn, rv, [bl, br, tl, tr], u_off, v_off, scale, scale);
    }

    #[allow(clippy::too_many_arguments)]
    fn float_section(&mut self, i: usize, loc: V2, road_vector: V2, half_height: f32, left: f32, right: f32, u_off: f32, v_off: f32, scale: f32) {
        let road_normal = V2::new(-road_vector.y, road_vector.x);
        let mut rv = road_vector.norm() * right;
        let rn = road_normal.norm() * half_height.abs();
        let road_left = rv.norm() * left;
        rv += road_left;
        let left_center = loc - road_left;
        let bl = left_center - rn;
        let br = bl + rv;
        let tr = br + rn * 2.0;
        let tl = bl + rn * 2.0;
        self.float_4pt(i, loc, rn, rv, [bl, br, tl, tr], u_off, v_off, scale, scale);
    }

    /// `loadFloat4PtSection`: a quad strip over the four corners (`[BL, BR, TL, TR]`), one
    /// column per height cell along the road, flat across the width at the highest terrain
    /// cell, floating slightly above ground. UVs follow `loc` / the road axes.
    #[allow(clippy::too_many_arguments)]
    fn float_4pt(&mut self, i: usize, loc: V2, road_normal: V2, road_vector: V2, c: [V2; 4], u_off: f32, v_off: f32, u_scale: f32, v_scale: f32) {
        const FLOAT_AMOUNT: f32 = MAP_HEIGHT_SCALE / 8.0;
        const MAX_ERROR: f32 = MAP_HEIGHT_SCALE * 1.1;
        let road_len = road_vector.len();
        let half_height = road_normal.len();
        let rn = road_normal.norm();
        let rv = road_vector.norm();
        let u_count = ((road_len / MAP_XY_FACTOR) as usize + 1).max(2);
        let v_count = (((2.0 * half_height / MAP_XY_FACTOR) as usize) + 1).clamp(2, 100);
        let [bl, br, tl, tr] = c;
        // Columns: (bottom point, top point, z, column index).
        struct Col {
            p0: V2,
            p1: V2,
            z: f32,
            u: usize,
        }
        let mut cols: Vec<Col> = Vec::with_capacity(u_count);
        for ci in 0..u_count {
            let f = ci as f32 / (u_count - 1) as f32;
            let mut max_h = f32::MIN;
            let mut p0 = V2::default();
            let mut p1 = V2::default();
            for j in 0..v_count {
                let g = j as f32 / (v_count - 1) as f32;
                let p = bl * ((1.0 - f) * (1.0 - g)) + br * (f * (1.0 - g)) + tl * ((1.0 - f) * g) + tr * (f * g);
                max_h = max_h.max((self.max_cell_height)(p.x, p.y));
                if j == 0 {
                    p0 = p;
                }
                p1 = p;
            }
            cols.push(Col { p0, p1, z: max_h, u: ci });
        }
        // Drop interior columns the neighbours interpolate within MAX_ERROR.
        let mut kept: Vec<usize> = vec![0];
        for k in 1..u_count.saturating_sub(1) {
            let prev = &cols[*kept.last().unwrap()];
            let (cur, next) = (&cols[k], &cols[k + 1]);
            let z = (prev.z * (cur.u - prev.u) as f32 + next.z * (next.u - cur.u) as f32) / (next.u - prev.u) as f32;
            if !(z >= cur.z && z < cur.z + MAX_ERROR) {
                kept.push(k);
            }
        }
        kept.push(u_count - 1);
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        for &k in &kept {
            if verts.len() + 2 > MAX_SEG_VERTEX {
                break;
            }
            let col = &cols[k];
            for p in [col.p0, col.p1] {
                let d = p - loc;
                let (v, u) = (rn.dot(d), rv.dot(d));
                verts.push(Vtx { pos: [p.x, p.y, col.z + FLOAT_AMOUNT], uv: [u_off + u / (u_scale * 4.0), v_off - v / (v_scale * 4.0)] });
            }
        }
        for c in 0..(verts.len() / 2).saturating_sub(1) {
            let (p0, p1, c0, c1) = (c as u32 * 2, c as u32 * 2 + 1, c as u32 * 2 + 2, c as u32 * 2 + 3);
            indices.extend_from_slice(&[p1, p0, c0, p1, c0, c1]);
        }
        self.segs[i].verts = verts;
        self.segs[i].indices = indices;
    }
}

fn rotate_about(p: V2, center: V2, angle: f32) -> V2 {
    let off = p - center;
    let rot = off.rot(angle);
    p + (rot - off)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(a: (f32, f32), b: (f32, f32)) -> RoadInput {
        RoadInput { loc1: [a.0, a.1], loc2: [b.0, b.1], scale: 30.0, width_in_texture: 0.9, id: 0, angled: [false; 2], join: [false; 2], tight: false }
    }

    fn build(inputs: &[RoadInput]) -> Vec<Seg> {
        let flat = |_: f32, _: f32| 5.0;
        let mut b = RoadBuffer::new(1, &flat);
        b.load_roads(inputs);
        b.segs
    }

    #[test]
    fn straight_segment_is_a_flat_strip_above_ground() {
        let segs = build(&[seg((0.0, 0.0), (100.0, 0.0))]);
        assert_eq!(segs.len(), 1);
        let s = &segs[0];
        assert!(s.verts.len() >= 4 && s.verts.len() % 2 == 0);
        // Width = scale * widthInTexture, centred on the segment, a hair above the ground.
        let ys: Vec<f32> = s.verts.iter().map(|v| v.pos[1]).collect();
        assert!((ys.iter().cloned().fold(f32::MIN, f32::max) - 13.5).abs() < 1e-3);
        assert!((ys.iter().cloned().fold(f32::MAX, f32::min) + 13.5).abs() < 1e-3);
        assert!(s.verts.iter().all(|v| v.pos[2] > 5.0 && v.pos[2] < 5.2));
        // Every triangle faces +Z.
        for t in s.indices.chunks(3) {
            let (a, b, c) = (s.verts[t[0] as usize].pos, s.verts[t[1] as usize].pos, s.verts[t[2] as usize].pos);
            assert!((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) > 0.0);
        }
    }

    #[test]
    fn corner_gets_curve_pieces() {
        // 90 degree turn: both ends link (count 1) so curve segments are inserted.
        let segs = build(&[seg((0.0, 0.0), (200.0, 0.0)), seg((200.0, 0.0), (200.0, 200.0))]);
        assert!(segs.iter().filter(|s| s.kind == Kind::Curve).count() >= 2);
        assert!(segs.iter().all(|s| !s.verts.is_empty()));
    }

    #[test]
    fn three_roads_meeting_make_a_tee() {
        let segs = build(&[seg((0.0, 0.0), (200.0, 0.0)), seg((200.0, 0.0), (400.0, 0.0)), seg((200.0, 0.0), (200.0, 200.0))]);
        assert_eq!(segs.iter().filter(|s| s.kind == Kind::Tee).count(), 1);
    }

    #[test]
    fn four_roads_meeting_make_a_four_way() {
        let segs = build(&[
            seg((0.0, 0.0), (200.0, 0.0)),
            seg((200.0, 0.0), (400.0, 0.0)),
            seg((200.0, 0.0), (200.0, 200.0)),
            seg((200.0, 0.0), (200.0, -200.0)),
        ]);
        assert_eq!(segs.iter().filter(|s| s.kind == Kind::FourWay).count(), 1);
    }

    #[test]
    fn duplicate_segments_are_discarded() {
        let segs = build(&[seg((0.0, 0.0), (100.0, 0.0)), seg((100.0, 0.0), (0.0, 0.0))]);
        assert_eq!(segs.len(), 1);
    }
}
