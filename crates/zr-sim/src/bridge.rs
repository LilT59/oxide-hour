//! Bridges (`TerrainLogic` `Bridge`/`BridgeInfo`, `PathfindLayer`).
//!
//! Ported from `Core/.../Map/TerrainLogic.cpp` (`Bridge::Bridge(BridgeInfo&)`,
//! `Bridge::Bridge(Object*)`, `isPointOnBridge`, `getBridgeHeight`, `isCellOnEnd`) and
//! `PathfindLayer`. A bridge is a rectangle (from/to sides, a width) sitting at a non-ground
//! pathfind layer. In this sim the pathfinder keeps one grid and marks the bridge's cells as a
//! passable surface above the ground (see [`crate::pathfind`]); the deck height is what an object
//! snaps to when it is closer to the deck than to the ground (`TerrainLogic::getLayerForDestination`).

use crate::math::{cos, sin, sqrt, Coord3D};
use crate::object::ObjectId;

/// `LAYER_GROUND` (`PathfindLayerEnum`). Bridge layers are `>= 2`.
pub const LAYER_GROUND: u32 = 1;
/// `LAYER_Z_CLOSE_ENOUGH_F`: how close in z an object must be to interact with a bridge.
pub const LAYER_Z_CLOSE_ENOUGH: f32 = 5.0;

/// One bridge (`Bridge` + `BridgeInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct Bridge {
    /// `PathfindLayerEnum` (`>= 2`).
    pub layer: u32,
    /// Midpoints of the from / to sides.
    pub from: Coord3D,
    pub to: Coord3D,
    pub width: f32,
    pub destroyed: bool,
    /// The `GenericBridge`/landmark object representing this bridge, if any.
    pub bridge_object: Option<ObjectId>,
    pub from_left: Coord3D,
    pub from_right: Coord3D,
    pub to_left: Coord3D,
    pub to_right: Coord3D,
    lo: [f32; 2],
    hi: [f32; 2],
    /// Unit vector along the bridge (from -> to) and its length.
    axis: [f32; 2],
    length: f32,
}

impl Bridge {
    /// `Bridge::Bridge(BridgeInfo&)` geometry: `from`/`to` midpoints plus a perpendicular width.
    pub fn new(from: Coord3D, to: Coord3D, width: f32, layer: u32) -> Self {
        let (dx, dy) = (to.x - from.x, to.y - from.y);
        let length = sqrt(dx * dx + dy * dy);
        let (ax, ay) = if length > 0.0 { (dx / length, dy / length) } else { (1.0, 0.0) };
        let (nx, ny) = (-ay, ax);
        let hw = width * 0.5;
        let from_left = Coord3D::new(from.x + nx * hw, from.y + ny * hw, from.z);
        let from_right = Coord3D::new(from.x - nx * hw, from.y - ny * hw, from.z);
        let to_left = Coord3D::new(to.x + nx * hw, to.y + ny * hw, to.z);
        let to_right = Coord3D::new(to.x - nx * hw, to.y - ny * hw, to.z);
        Self::assemble(layer, from, to, width, from_left, from_right, to_left, to_right, [ax, ay], length)
    }

    /// `Bridge::Bridge(Object*)`: derive the rectangle from a box object's transform.
    pub fn from_object(pos: Coord3D, angle: f32, half_x: f32, half_y: f32, layer: u32, id: ObjectId) -> Self {
        let (c, s) = (cos(angle), sin(angle));
        let from_left = Coord3D::new(pos.x - half_x * c - half_y * s, pos.y + half_y * c - half_x * s, pos.z);
        let to_left = Coord3D::new(pos.x + half_x * c - half_y * s, pos.y + half_y * c + half_x * s, pos.z);
        let from_right = Coord3D::new(pos.x - half_x * c + half_y * s, pos.y - half_y * c - half_x * s, pos.z);
        let to_right = Coord3D::new(pos.x + half_x * c + half_y * s, pos.y - half_y * c + half_x * s, pos.z);
        let from = Coord3D::new((from_left.x + from_right.x) * 0.5, (from_left.y + from_right.y) * 0.5, pos.z);
        let to = Coord3D::new((to_left.x + to_right.x) * 0.5, (to_left.y + to_right.y) * 0.5, pos.z);
        let (dx, dy) = (to.x - from.x, to.y - from.y);
        let length = sqrt(dx * dx + dy * dy);
        let axis = if length > 0.0 { [dx / length, dy / length] } else { [1.0, 0.0] };
        let mut b = Self::assemble(layer, from, to, 2.0 * half_y, from_left, from_right, to_left, to_right, axis, length);
        b.bridge_object = Some(id);
        b
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        layer: u32,
        from: Coord3D,
        to: Coord3D,
        width: f32,
        from_left: Coord3D,
        from_right: Coord3D,
        to_left: Coord3D,
        to_right: Coord3D,
        axis: [f32; 2],
        length: f32,
    ) -> Self {
        let xs = [from_left.x, from_right.x, to_left.x, to_right.x];
        let ys = [from_left.y, from_right.y, to_left.y, to_right.y];
        let lo = [xs.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MAX, f32::min)];
        let hi = [xs.iter().copied().fold(f32::MIN, f32::max), ys.iter().copied().fold(f32::MIN, f32::max)];
        Bridge { layer, from, to, width, destroyed: false, bridge_object: None, from_left, from_right, to_left, to_right, lo, hi, axis, length }
    }

    /// `Bridge::isPointOnBridge` for a rectangle: project into the bridge's local frame.
    pub fn contains_2d(&self, x: f32, y: f32) -> bool {
        if x < self.lo[0] || x > self.hi[0] || y < self.lo[1] || y > self.hi[1] {
            return false;
        }
        let (u, v) = self.local(x, y);
        (0.0..=1.0).contains(&u) && v.abs() <= self.width * 0.5
    }

    /// `(along 0..1, lateral signed distance)` of a point in the bridge frame.
    fn local(&self, x: f32, y: f32) -> (f32, f32) {
        let (px, py) = (x - self.from.x, y - self.from.y);
        let u = if self.length > 0.0 { (px * self.axis[0] + py * self.axis[1]) / self.length } else { 0.0 };
        let v = px * -self.axis[1] + py * self.axis[0];
        (u, v)
    }

    /// `Bridge::getBridgeHeight`: the deck plane's z at `(x, y)`.
    pub fn deck_height(&self, x: f32, y: f32) -> f32 {
        let (u, _) = self.local(x, y);
        self.from.z + u.clamp(0.0, 1.0) * (self.to.z - self.from.z)
    }

    /// `Bridge::isCellOnEnd`: near the from or to side within `radius` of the bridge width.
    pub fn is_cell_on_end(&self, x: f32, y: f32, radius: f32) -> bool {
        let (u, v) = self.local(x, y);
        let end = radius / self.length.max(1.0);
        (u <= end || u >= 1.0 - end) && v.abs() <= self.width * 0.5 + radius
    }

    /// `Bridge::isCellOnSide`: near one of the long sides (both sides).
    pub fn is_cell_on_side(&self, x: f32, y: f32, radius: f32) -> bool {
        let (u, v) = self.local(x, y);
        (0.0..=1.0).contains(&u) && (v.abs() - self.width * 0.5).abs() <= radius
    }

    pub fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        (self.lo, self.hi)
    }

    pub(crate) fn crc(&self, c: &mut crate::crc::Crc) {
        c.add_u32(self.layer);
        c.add_coord(&self.from);
        c.add_coord(&self.to);
        c.add_f32(self.width);
        c.add_bool(self.destroyed);
    }
}
