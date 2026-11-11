//! Bridges: ported from `W3DBridgeBuffer.cpp` (`W3DBridge::load`, `getIndicesNVertices`,
//! `getModelVertices`). The bridge model (INI `BridgeModelName`) has `BRIDGE_LEFT`,
//! `BRIDGE_SPAN` and `BRIDGE_RIGHT` pieces; they are laid out along the line between the two
//! map points, repeating the span piece, and every vertex is mapped into world space (the
//! length is stretched to fit exactly, the width and height are scaled by `BridgeScale`).

use anyhow::Result;
use zr_assets::{RgbaImage, Vfs};
use zr_ini::IniDb;
use zr_map::MapFile;
use zr_w3d::ModelMesh;

use crate::{FLAG_BRIDGE_POINT1, FLAG_BRIDGE_POINT2};

/// `BRIDGE_FLOAT_AMT`: bridge end points sit slightly above the ground.
const BRIDGE_FLOAT_AMT: f32 = 0.25;

/// One mesh of a placed bridge, already in world space.
#[derive(Debug, Clone)]
pub struct BridgePiece {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub texture: Option<RgbaImage>,
    pub alpha_blend: bool,
    pub two_sided: bool,
}

#[derive(Debug, Clone)]
pub struct BridgeInstance {
    /// INI `Bridge` template name.
    pub template: String,
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub pieces: Vec<BridgePiece>,
}

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn len(a: V3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

struct Basis {
    start: V3,
    /// Full start-to-end vector divided by the length of the model to be spanned.
    vec: V3,
    normal: V3,
    z: V3,
}

impl Basis {
    fn map(&self, p: V3, x_offset: f32) -> V3 {
        let (x, y, z) = (p[0] + x_offset, p[1], p[2]);
        [
            x * self.vec[0] + y * self.normal[0] + z * self.z[0] + self.start[0],
            x * self.vec[1] + y * self.normal[1] + z * self.z[1] + self.start[1],
            x * self.vec[2] + y * self.normal[2] + z * self.z[2] + self.start[2],
        ]
    }
    fn map_normal(&self, n: V3) -> V3 {
        let v = [
            n[0] * self.vec[0] + n[1] * self.normal[0] + n[2] * self.z[0],
            n[0] * self.vec[1] + n[1] * self.normal[1] + n[2] * self.z[1],
            n[0] * self.vec[2] + n[1] * self.normal[2] + n[2] * self.z[2],
        ];
        let l = len(v);
        if l > 0.0 {
            scale(v, 1.0 / l)
        } else {
            [0.0, 0.0, 1.0]
        }
    }
}

fn x_range(m: &ModelMesh) -> (f32, f32) {
    m.positions.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])))
}

fn place(m: &ModelMesh, basis: &Basis, x_offset: f32, texture: &Option<RgbaImage>) -> BridgePiece {
    BridgePiece {
        positions: m.positions.iter().map(|&p| basis.map(p, x_offset)).collect(),
        normals: m.normals.iter().map(|&n| basis.map_normal(n)).collect(),
        uvs: m.uvs.clone(),
        indices: m.indices.clone(),
        texture: texture.clone(),
        alpha_blend: m.alpha_blend,
        two_sided: m.two_sided,
    }
}

/// The last mesh whose name starts with `prefix` (`<model>.BRIDGE_LEFT` ... in the engine).
fn find_piece<'a>(meshes: &'a [ModelMesh], prefix: &str) -> Option<&'a ModelMesh> {
    meshes.iter().rev().find(|m| {
        let n = m.name.to_ascii_lowercase();
        let n = n.rsplit('.').next().unwrap_or(&n);
        n.starts_with(prefix)
    })
}

/// Builds every bridge of the map. `ground(x, y)` is the terrain height; bridge ends are
/// placed `BRIDGE_FLOAT_AMT` above it. Bridges whose template or model is missing are skipped.
pub fn build_bridges(map: &MapFile, vfs: &Vfs, ini: &IniDb, ground: &dyn Fn(f32, f32) -> f32) -> Result<Vec<BridgeInstance>> {
    let mut out = Vec::new();
    let objs = &map.objects;
    let mut i = 0;
    while i < objs.len() {
        let o = &objs[i];
        if o.flags & FLAG_BRIDGE_POINT1 == 0 {
            i += 1;
            continue;
        }
        let Some(o2) = objs.get(i + 1) else { break };
        if o2.flags & FLAG_BRIDGE_POINT2 == 0 {
            i += 1;
            continue;
        }
        i += 2;
        let from = [o.pos[0], o.pos[1], ground(o.pos[0], o.pos[1]) + BRIDGE_FLOAT_AMT];
        let to = [o2.pos[0], o2.pos[1], ground(o2.pos[0], o2.pos[1]) + BRIDGE_FLOAT_AMT];
        if let Some(b) = place_bridge(vfs, ini, &o.template, from, to) {
            out.push(b);
        }
    }
    Ok(out)
}

fn place_bridge(vfs: &Vfs, ini: &IniDb, template: &str, from: V3, to: V3) -> Option<BridgeInstance> {
    let block = ini.of_kind("Bridge").find(|b| b.name().is_some_and(|n| n.eq_ignore_ascii_case(template)))?;
    let tok = |k: &str| block.value(k).and_then(|v| zr_ini::tokens(v).next().map(str::to_string));
    let bscale: f32 = tok("BridgeScale").and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let model_name = tok("BridgeModelName")?;
    let model = zr_w3d::load_model(vfs, &model_name).ok()?;
    let texture = tok("Texture").and_then(|t| zr_w3d::load_texture(vfs, &t).ok());

    let left = find_piece(&model.meshes, "bridge_left")?;
    let (mut section, mut right) = (find_piece(&model.meshes, "bridge_span"), find_piece(&model.meshes, "bridge_right"));
    if section.is_none() || right.is_none() {
        section = None;
        right = None;
    }

    let (left_min_x, left_max_x) = x_range(left);
    let mut fixed = section.is_none();
    let (mut section_min_x, mut section_max_x, mut right_min_x, mut right_max_x) = (left_max_x, left_max_x, left_max_x, left_max_x);
    if let (Some(s), Some(r)) = (section, right) {
        (section_min_x, section_max_x) = x_range(s);
        (right_min_x, right_max_x) = x_range(r);
    }
    let length = (right_max_x - left_min_x).max(1.0);
    if !fixed {
        let allowable = 0.05 * length;
        if left_max_x > section_min_x + allowable || right_min_x < section_max_x - allowable {
            fixed = true;
        }
    }

    // Basis (getIndicesNVertices / getModelVerticesFixed).
    let mut vec = sub(to, from);
    if vec[0] * vec[0] + vec[1] * vec[1] + vec[2] * vec[2] < 1.0 {
        let l = len(vec);
        if l > 0.0 {
            vec = scale(vec, 1.0 / l);
        }
    }
    let desired = len(vec);
    let nl = (vec[0] * vec[0] + vec[1] * vec[1]).sqrt();
    let normal = if nl > 0.0 { scale([-vec[1], vec[0], 0.0], bscale / nl) } else { [0.0, bscale, 0.0] };
    let delta_z = if desired > 0.0 { (to[2] - from[2]) / desired } else { 0.0 };
    let delta_x = (1.0 - delta_z * delta_z).max(0.0).sqrt();
    let zvec = scale([-delta_z, 0.0, delta_x], bscale);

    let x_offset = -left_min_x;
    let mut pieces = Vec::new();
    match (fixed, section, right) {
        (false, Some(section), Some(right)) => {
            let span_length = right_min_x - left_max_x;
            let spannable = desired - (length - span_length);
            let num_spans = ((spannable + span_length / 2.0) / span_length).floor().max(0.0) as i32;
            let bridge_length = length + (num_spans - 1) as f32 * span_length;
            let basis = Basis { start: from, vec: scale(vec, 1.0 / bridge_length), normal, z: zvec };
            pieces.push(place(left, &basis, x_offset, &texture));
            for k in 0..num_spans {
                pieces.push(place(section, &basis, x_offset + k as f32 * span_length, &texture));
            }
            pieces.push(place(right, &basis, x_offset + (num_spans - 1) as f32 * span_length, &texture));
        }
        _ => {
            // Fixed bridge: only the left piece, stretched over the whole length.
            let basis = Basis { start: from, vec: scale(vec, 1.0 / length), normal, z: zvec };
            pieces.push(place(left, &basis, x_offset, &texture));
        }
    }
    Some(BridgeInstance { template: template.to_string(), from, to, pieces })
}
