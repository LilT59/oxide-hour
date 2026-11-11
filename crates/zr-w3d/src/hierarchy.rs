//! W3D skeleton hierarchy (HTree) and animated pose evaluation.
//!
//! Pivot layout and base pose: `WW3D2/w3d_file.h` (`W3dPivotStruct`) and
//! `htree.cpp` (`HTreeClass::Load_W3D`, `Base_Update`, `Anim_Update`). The
//! animation is applied exactly like `HTreeClass::Anim_Update`: start from the
//! bind world transform, translate by the channel delta in the pivot's local
//! frame, then post-multiply the animated rotation.

use crate::anim::Animation;
use crate::error::{invalid, Error};
use cnc_formats::w3d::{W3dChunk, W3dFile};
use zr_assets::{normalize_path, Vfs};

const CHUNK_HIERARCHY: u32 = 0x100;
const CHUNK_HIERARCHY_HEADER: u32 = 0x101;
const CHUNK_PIVOTS: u32 = 0x102;

const PIVOT_SIZE: usize = 60;
const MAX_PIVOTS: usize = 8192;

/// 3x4 affine matrix, row-major; translation in column 3 (row-vector
/// convention, matching `WWMath::Matrix3D`).
pub type Mat34 = [[f32; 4]; 3];

pub const IDENTITY34: Mat34 = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
/// Identity orientation (`Quaternion::Make_Identity`), `(x, y, z, w)`.
pub const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// A rigid world transform as `(translation, orientation)`.
pub type Trs = ([f32; 3], [f32; 4]);

/// Rotation of `angle` radians about a unit `axis`, as `(x, y, z, w)` (`Matrix3D::Rotate_X/Y/Z`).
pub fn quat_about_axis(axis: [f32; 3], angle: f32) -> [f32; 4] {
    let s = libm::sinf(angle * 0.5);
    [axis[0] * s, axis[1] * s, axis[2] * s, libm::cosf(angle * 0.5)]
}

/// Hamilton product of two `(x, y, z, w)` quaternions.
pub fn quat_mul(a: &[f32; 4], b: &[f32; 4]) -> [f32; 4] {
    let (ax, ay, az, aw) = (a[0], a[1], a[2], a[3]);
    let (bx, by, bz, bw) = (b[0], b[1], b[2], b[3]);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// Rotates `v` by the `(x, y, z, w)` quaternion.
pub fn quat_rotate(q: &[f32; 4], v: [f32; 3]) -> [f32; 3] {
    // v' = v + 2 * cross(q.xyz, cross(q.xyz, v) + q.w * v)
    let u = [q[0], q[1], q[2]];
    let c1 = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let t = [c1[0] + q[3] * v[0], c1[1] + q[3] * v[1], c1[2] + q[3] * v[2]];
    let c2 = [u[1] * t[2] - u[2] * t[1], u[2] * t[0] - u[0] * t[2], u[0] * t[1] - u[1] * t[0]];
    [v[0] + 2.0 * c2[0], v[1] + 2.0 * c2[1], v[2] + 2.0 * c2[2]]
}

/// Rotation part of a quaternion (`Build_Matrix3D(Quaternion)`).
pub fn quat_to_mat(q: [f32; 4]) -> Mat34 {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (z * x + y * w), 0.0],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (z * z + x * x), 2.0 * (y * z - x * w), 0.0],
        [2.0 * (z * x - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y), 0.0],
    ]
}

pub fn xform_point(m: &Mat34, p: [f32; 3]) -> [f32; 3] {
    let f = |r: usize| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3];
    [f(0), f(1), f(2)]
}

pub fn xform_dir(m: &Mat34, p: [f32; 3]) -> [f32; 3] {
    let f = |r: usize| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2];
    [f(0), f(1), f(2)]
}

/// One skeleton joint: bind pose local to its parent.
#[derive(Debug, Clone)]
pub struct Pivot {
    pub name: String,
    pub parent: Option<usize>,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
}

/// A parsed W3D hierarchy (`W3D_CHUNK_HIERARCHY`), possibly with a scale applied.
#[derive(Debug, Clone)]
pub struct Hierarchy {
    pub name: String,
    pub pivots: Vec<Pivot>,
    scale: f32,
}

/// A relative transform post-multiplied onto a bone after its animated pose, in the bone's own
/// frame: `HTreeClass::Control_Bone` (`PivotClass::Capture_Update`, `pivot.cpp`). The turret yaw and
/// pitch and the weapon recoil offset use it, and children inherit it because they are composed from
/// the controlled parent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoneControl {
    pub bone: usize,
    pub translation: [f32; 3],
    /// `(x, y, z, w)`.
    pub rotation: [f32; 4],
}

fn rd_u32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?))
}

fn rd_f32(d: &[u8], o: usize) -> Option<f32> {
    rd_u32(d, o).map(f32::from_bits)
}

fn cstr(d: &[u8]) -> String {
    let n = d.iter().position(|&b| b == 0).unwrap_or(d.len());
    String::from_utf8_lossy(&d[..n]).into_owned()
}

impl Hierarchy {
    /// Parses the `HIERARCHY` chunk of a file. Returns `None` when the chunk is
    /// absent or truncated (the caller falls back to another file).
    pub fn parse(h: &W3dChunk) -> Option<Hierarchy> {
        let hdr = h.find_child(CHUNK_HIERARCHY_HEADER)?;
        let name = cstr(hdr.data.get(4..20)?);
        let n = rd_u32(hdr.data, 20)? as usize;
        let piv = h.find_child(CHUNK_PIVOTS)?.data;
        let n = n.min(piv.len() / PIVOT_SIZE).min(MAX_PIVOTS);
        let mut pivots = Vec::with_capacity(n);
        for i in 0..n {
            let b = piv.get(i * PIVOT_SIZE..(i + 1) * PIVOT_SIZE)?;
            let parent = rd_u32(b, 16).filter(|&p| p != u32::MAX).map(|p| p as usize);
            let translation = [rd_f32(b, 20)?, rd_f32(b, 24)?, rd_f32(b, 28)?];
            let rotation = [rd_f32(b, 44)?, rd_f32(b, 48)?, rd_f32(b, 52)?, rd_f32(b, 56)?];
            pivots.push(Pivot { name: cstr(b.get(..16)?), parent, translation, rotation });
        }
        Some(Hierarchy { name, pivots, scale: 1.0 })
    }

    pub fn pivot_count(&self) -> usize {
        self.pivots.len()
    }

    /// The one-pivot tree a model without a hierarchy gets (`HTreeClass::Init_Default`: a
    /// `RootTransform` at the identity).
    pub fn default_tree() -> Hierarchy {
        Hierarchy {
            name: String::new(),
            pivots: vec![Pivot { name: "RootTransform".into(), parent: None, translation: [0.0; 3], rotation: IDENTITY_QUAT }],
            scale: 1.0,
        }
    }

    /// Index of the named bone (case-insensitive), or `None`.
    pub fn bone_index(&self, name: &str) -> Option<usize> {
        self.pivots.iter().position(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// The first bone matching `names` in priority order: an exact case-insensitive name first,
    /// then a case-insensitive substring (so `BIP01 HEAD` matches a `HEAD` request). Used by the
    /// possession first-person camera to find a turret / cockpit / head bone.
    pub fn find_bone(&self, names: &[&str]) -> Option<usize> {
        for name in names {
            if let Some(i) = self.bone_index(name) {
                return Some(i);
            }
        }
        for name in names {
            let needle = name.to_ascii_uppercase();
            if let Some(i) = self.pivots.iter().position(|p| p.name.to_ascii_uppercase().contains(&needle)) {
                return Some(i);
            }
        }
        None
    }

    /// World transform of every pivot in the rest pose (`Base_Update` with an
    /// identity root), as translation/orientation.
    pub fn bind_pose_trs(&self) -> Vec<Trs> {
        self.pose_trs_controlled(None, 0.0, &[])
    }

    /// World TRS of every pivot for `anim` at `frame`, following
    /// `HTreeClass::Anim_Update`: `world = parent * bind`, then translate by the
    /// channel delta rotated into the pivot's frame, then post-multiply the
    /// animated rotation. Pivots without animation data keep the bind pose and
    /// `frame` is clamped by [`Animation::sample`].
    pub fn pose_trs(&self, anim: &Animation, frame: f32) -> Vec<Trs> {
        self.pose_trs_controlled(Some(anim), frame, &[])
    }

    /// World TRS of every pivot: the rest pose (`anim` `None`, `Base_Update`) or `anim` at `frame`
    /// (`Anim_Update`), with each pivot's [`BoneControl`]s post-multiplied in its own frame before its
    /// children are composed (`Capture_Update` runs inside the same pivot loop). The root pivot stays
    /// at the identity root, as `Base_Update` / `Anim_Update` set `Pivot[0].Transform = root`.
    pub fn pose_trs_controlled(&self, anim: Option<&Animation>, frame: f32, controls: &[BoneControl]) -> Vec<Trs> {
        let mut out = vec![([0.0; 3], IDENTITY_QUAT); self.pivots.len()];
        for i in 1..self.pivots.len() {
            let p = &self.pivots[i];
            let (pt, pq) = p.parent.filter(|&q| q < i).map_or(([0.0; 3], IDENTITY_QUAT), |q| out[q]);
            let local = [p.translation[0] * self.scale, p.translation[1] * self.scale, p.translation[2] * self.scale];
            let base = quat_mul(&pq, &p.rotation);
            let (mut t, mut q) = (add3(pt, quat_rotate(&pq, local)), base);
            if let Some((at, aq)) = anim.and_then(|a| a.sample(i, frame)) {
                t = add3(t, quat_rotate(&base, at));
                q = quat_mul(&base, &aq);
            }
            for c in controls.iter().filter(|c| c.bone == i) {
                t = add3(t, quat_rotate(&q, c.translation));
                q = quat_mul(&q, &c.rotation);
            }
            out[i] = (t, q);
        }
        out
    }

    /// World transform of every pivot in the rest pose (`Base_Update`).
    pub fn bind_pose(&self) -> Vec<Mat34> {
        self.bind_pose_trs().iter().map(|&(t, q)| trs_to_mat(t, q)).collect()
    }

    /// World transform of every pivot for `anim` at `frame` (see [`Self::pose_trs`]).
    pub fn pose(&self, anim: &Animation, frame: f32) -> Vec<Mat34> {
        self.pose_trs(anim, frame).iter().map(|&(t, q)| trs_to_mat(t, q)).collect()
    }
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn trs_to_mat(t: [f32; 3], q: [f32; 4]) -> Mat34 {
    let mut m = quat_to_mat(q);
    m[0][3] = t[0];
    m[1][3] = t[1];
    m[2][3] = t[2];
    m
}

fn w3d_path(name: &str) -> String {
    let n = normalize_path(name.trim());
    let n = n.strip_suffix(".w3d").unwrap_or(&n);
    format!("art\\w3d\\{n}.w3d")
}

/// Loads `art\w3d\<name>.w3d` and returns its hierarchy (e.g. `AIRngr_SKL`).
pub fn load_hierarchy(vfs: &Vfs, name: &str) -> Result<Hierarchy, Error> {
    let path = w3d_path(name);
    let bytes = vfs.read(&path).ok_or_else(|| Error::NotFound(path.clone()))?;
    let file = W3dFile::parse(&bytes).map_err(|e| invalid(format!("{path}: {e}")))?;
    let h = file
        .chunks
        .iter()
        .find(|c| c.chunk_type == CHUNK_HIERARCHY)
        .ok_or_else(|| invalid(format!("{path}: no hierarchy chunk")))?;
    Hierarchy::parse(h).ok_or_else(|| invalid(format!("{path}: malformed hierarchy")))
}

const CHUNK_HLOD_HEADER: u32 = 0x701;

/// The skeleton a model's sub-objects are bound to: the file's own hierarchy chunk, else the
/// `HierarchyName` of its HLOD header loaded from `art\w3d\<name>.w3d` (the lookup order of
/// `zr_w3d::load_model`, `HLodDefClass::Load_W3D` / `WW3DAssetManager::Get_HTree`). Clips are applied
/// to this skeleton by pivot index (`HTreeClass::Anim_Update`), so the viewer's rig must use it.
/// `None` when the model has none (the engine then uses a one-pivot default tree).
pub fn model_hierarchy(vfs: &Vfs, name: &str) -> Option<Hierarchy> {
    let bytes = vfs.read(&w3d_path(name))?;
    let file = W3dFile::parse(&bytes).ok()?;
    if let Some(h) = file.hierarchy().and_then(Hierarchy::parse) {
        return Some(h);
    }
    let hier = file
        .hlod()
        .and_then(|h| h.find_child(CHUNK_HLOD_HEADER))
        .and_then(|h| h.data.get(24..40))
        .map(cstr)
        .filter(|s| !s.is_empty())?;
    load_hierarchy(vfs, &hier).ok()
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

    /// Root at origin, child bound at (1,0,0).
    fn two_pivot_tree() -> Vec<u8> {
        let mut hdr = vec![0u8; 36];
        hdr[4..20].copy_from_slice(&name16("SKL"));
        hdr[20..24].copy_from_slice(&2u32.to_le_bytes());
        let pivot = |parent: u32, t: [f32; 3], q: [f32; 4]| {
            let mut p = name16("P");
            p.extend_from_slice(&parent.to_le_bytes());
            p.extend_from_slice(&f32s(&t));
            p.extend_from_slice(&f32s(&[0.0; 3]));
            p.extend_from_slice(&f32s(&q));
            p
        };
        let pivots = [pivot(u32::MAX, [0.0; 3], [0.0, 0.0, 0.0, 1.0]), pivot(0, [1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0])].concat();
        chunk(CHUNK_HIERARCHY, &[chunk(CHUNK_HIERARCHY_HEADER, &hdr, false), chunk(CHUNK_PIVOTS, &pivots, false)].concat(), true)
    }

    #[test]
    fn parse_and_bind_pose() {
        let data = two_pivot_tree();
        let f = W3dFile::parse(&data).unwrap();
        let h = Hierarchy::parse(f.chunks.iter().find(|c| c.chunk_type == CHUNK_HIERARCHY).unwrap()).unwrap();
        assert_eq!(h.name, "SKL");
        assert_eq!(h.pivot_count(), 2);
        let w = h.bind_pose();
        assert_eq!(xform_point(&w[1], [0.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
    }

    /// Root, then `arm` at (1,0,0) and `barrel` at (1,0,0) under the arm.
    fn arm_chain() -> Hierarchy {
        let p = |name: &str, parent: Option<usize>, t: [f32; 3]| Pivot { name: name.into(), parent, translation: t, rotation: IDENTITY_QUAT };
        Hierarchy {
            name: "CHAIN".into(),
            pivots: vec![p("ROOT", None, [0.0; 3]), p("ARM", Some(0), [1.0, 0.0, 0.0]), p("BARREL", Some(1), [1.0, 0.0, 0.0])],
            scale: 1.0,
        }
    }

    #[test]
    fn bone_control_moves_children() {
        let h = arm_chain();
        // No controls: the bind pose (the barrel sits 2 units out along X).
        let bind = h.pose_trs_controlled(None, 0.0, &[]);
        assert_eq!(bind[2].0, [2.0, 0.0, 0.0]);
        // A 90 degree yaw on the arm (`Rotate_Z`) swings the barrel with it (Control_Bone inherits).
        let yaw = BoneControl { bone: 1, translation: [0.0; 3], rotation: quat_about_axis([0.0, 0.0, 1.0], std::f32::consts::FRAC_PI_2) };
        let c = h.pose_trs_controlled(None, 0.0, &[yaw]);
        assert!((c[2].0[0] - 1.0).abs() < 1e-5 && (c[2].0[1] - 1.0).abs() < 1e-5 && c[2].0[2].abs() < 1e-5, "{:?}", c[2].0);
        // Recoil: a local -X offset on the arm moves the arm and its barrel back along the arm's X.
        let recoil = BoneControl { bone: 1, translation: [-0.5, 0.0, 0.0], rotation: IDENTITY_QUAT };
        let r = h.pose_trs_controlled(None, 0.0, &[recoil]);
        assert_eq!(r[1].0, [0.5, 0.0, 0.0]);
        assert_eq!(r[2].0, [1.5, 0.0, 0.0]);
    }

    #[test]
    fn find_bone_matches_by_priority_and_substring() {
        let data = two_pivot_tree();
        let f = W3dFile::parse(&data).unwrap();
        let h = Hierarchy::parse(f.chunks.iter().find(|c| c.chunk_type == CHUNK_HIERARCHY).unwrap()).unwrap();
        // Exact name (case-insensitive) wins over the priority list order.
        assert_eq!(h.find_bone(&["HEAD", "P"]), Some(0));
        // No match returns None.
        assert_eq!(h.find_bone(&["TURRET"]), None);
    }
}
