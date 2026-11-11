//! Water for a Zero Hour map as plain data in Generals space (X east, Y north, Z up, world
//! units; triangles CCW seen from +Z), like `zr_roads`.
//!
//! Ported from `WaterRenderObjClass` (`Core/GameEngineDevice/Source/W3DDevice/GameClient/Water/W3DWater.cpp`):
//! * `renderWater`: every water trigger with more than 2 points; rivers go to `drawRiverWater`,
//!   other areas are walked as a fan around point 0 (one quad `p[k+2], p[k+1], p[k], p[0]` per
//!   step of 2, drawn by `drawTrapezoidWater` as a bilinear patch).
//! * Colour: `StandingWaterColor` (1,1,1 = "not overridden" -> terrain ambient + upward
//!   diffuse light, times the `WaterSet` `DiffuseColor`; 0,0,0 = unlit white).
//! * Texture: `WaterTransparency` `StandingWaterTexture`, UVs `x / 150`, `y / 150` for areas.
//! * Soft edges: the engine writes terrain-under-water depth into destination alpha
//!   (`BaseHeightMapRenderObjClass::initDestAlphaLUT` / `updateShorelineTile`) so that opacity
//!   is `clamp(depth / TransparentWaterDepth, 0, TransparentWaterMinOpacity)`. We bake that
//!   into per-vertex alpha on a fine grid (areas are subdivided to [`GRID`] units).
//!
//! Not ported: reflection / bump / sparkle pixel shaders, the shroud, `m_featherWater`, the
//! script-driven water grid (`renderWaterMesh`), the sea plane, map.ini `WaterTransparency`
//! overrides, the `TWAlphaEdge` river bank texture (approximated by a bank fade).

use anyhow::Result;
use zr_assets::{RgbaImage, Vfs};
use zr_ini::IniDb;
use zr_map::{GlobalLighting, MapFile, PolygonTrigger};

/// Cell size (world units) the water polygons are subdivided to for depth-based alpha.
pub const GRID: f32 = 5.0;
/// `drawTrapezoidWater`: texture repeat is one per `waterFactor` units.
pub const WATER_FACTOR: f32 = 150.0;
/// Columns across a river used to fade its banks (stands in for the `TWAlphaEdge` texture).
const RIVER_COLUMNS: usize = 8;

/// Look of the water for the map's time of day.
#[derive(Debug, Clone)]
pub struct WaterLook {
    pub texture: RgbaImage,
    /// Vertex RGB, 0..1, gamma space (modulates the texture).
    pub shade: [f32; 3],
    /// `WaterSet` `DiffuseColor` alpha, 0..1 (used when depth fading is off).
    pub diffuse_alpha: f32,
    pub transparent_depth: f32,
    pub min_opacity: f32,
    pub additive: bool,
    /// `WaterSet` scroll per millisecond (texture units).
    pub u_scroll_per_ms: f32,
    pub v_scroll_per_ms: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaterKind {
    Area,
    River,
}

/// One water polygon's mesh.
#[derive(Debug, Clone)]
pub struct WaterMesh {
    pub kind: WaterKind,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Per-vertex opacity 0..1.
    pub alpha: Vec<f32>,
    pub indices: Vec<u32>,
}

impl WaterMesh {
    fn new(kind: WaterKind) -> Self {
        Self { kind, positions: vec![], uvs: vec![], alpha: vec![], indices: vec![] }
    }
}

#[derive(Debug, Clone)]
pub struct Water {
    pub look: WaterLook,
    pub meshes: Vec<WaterMesh>,
}

fn real(b: &zr_ini::Block, key: &str) -> Option<f32> {
    zr_ini::tokens(b.value(key)?).next()?.parse().ok()
}

/// `R:255 G:128 B:0 [A:255]` -> components (0..255).
fn rgba(b: &zr_ini::Block, key: &str) -> Option<[f32; 4]> {
    let mut c = [0.0, 0.0, 0.0, 255.0];
    for t in zr_ini::tokens(b.value(key)?) {
        let Some((k, v)) = t.split_once(':') else { continue };
        let i = match k.to_ascii_lowercase().as_str() {
            "r" => 0,
            "g" => 1,
            "b" => 2,
            "a" => 3,
            _ => continue,
        };
        c[i] = v.parse().ok()?;
    }
    Some(c)
}

const TOD_NAMES: [&str; 4] = ["MORNING", "AFTERNOON", "EVENING", "NIGHT"];

/// Settings read from `WaterSet <TOD>` and `WaterTransparency` (defaults from `Water.h`).
pub fn look(vfs: &Vfs, ini: &IniDb, lighting: &GlobalLighting) -> Result<WaterLook> {
    let tod = lighting.tod_index().min(3);
    let set = ini.of_kind("WaterSet").filter(|b| b.name().is_some_and(|n| n.eq_ignore_ascii_case(TOD_NAMES[tod]))).last();
    let tr = ini.of_kind("WaterTransparency").last();

    let (mut depth, mut min_op, mut color, mut tex, mut additive) = (3.0, 1.0, [1.0f32; 3], "TWWater01.tga".to_string(), false);
    if let Some(t) = tr {
        depth = real(t, "TransparentWaterDepth").unwrap_or(depth);
        min_op = real(t, "TransparentWaterMinOpacity").unwrap_or(min_op);
        if let Some(c) = rgba(t, "StandingWaterColor") {
            color = [c[0] / 255.0, c[1] / 255.0, c[2] / 255.0];
        }
        if let Some(s) = t.value("StandingWaterTexture").and_then(|v| zr_ini::tokens(v).next()) {
            tex = s.to_string();
        }
        additive = t.value("AdditiveBlending").is_some_and(|v| v.trim().eq_ignore_ascii_case("yes"));
    }
    let diffuse = set.and_then(|s| rgba(s, "DiffuseColor")).unwrap_or([0.0; 4]);
    let wd = [diffuse[0] / 255.0, diffuse[1] / 255.0, diffuse[2] / 255.0];

    // Shared by drawTrapezoidWater / drawRiverWater.
    let shade = if color == [1.0; 3] {
        let lights = &lighting.terrain[lighting.tod_index()];
        let mut s = lights[0].ambient;
        for l in lights {
            let up = -l.direction[2];
            if up > 0.0 {
                for (sc, d) in s.iter_mut().zip(l.diffuse) {
                    *sc += up * d;
                }
            }
        }
        [(s[0] * wd[0]).clamp(0.0, 1.0), (s[1] * wd[1]).clamp(0.0, 1.0), (s[2] * wd[2]).clamp(0.0, 1.0)]
    } else if color == [0.0; 3] {
        [1.0; 3] // "special case where we disable lighting"
    } else {
        color
    };
    let texture = zr_w3d::load_texture(vfs, &tex).map_err(|e| anyhow::anyhow!("water texture {tex}: {e:?}"))?;
    Ok(WaterLook {
        texture,
        shade,
        diffuse_alpha: diffuse[3] / 255.0,
        transparent_depth: depth,
        min_opacity: min_op,
        additive,
        u_scroll_per_ms: set.and_then(|s| real(s, "UScrollPerMS")).unwrap_or(0.0),
        v_scroll_per_ms: set.and_then(|s| real(s, "VScrollPerMS")).unwrap_or(0.0),
    })
}

/// Opacity of water at height `wz` over terrain height `tz` (the dest-alpha LUT).
fn opacity(look: &WaterLook, wz: f32, tz: f32) -> f32 {
    if look.transparent_depth <= 0.0 {
        return look.diffuse_alpha;
    }
    ((wz - tz) / look.transparent_depth).clamp(0.0, look.min_opacity)
}

fn len2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    libm::sqrtf(dx * dx + dy * dy)
}

/// `drawTrapezoidWater` for one quad `p[0..4]`, subdivided to [`GRID`].
fn trapezoid(m: &mut WaterMesh, look: &WaterLook, p: [[f32; 3]; 4], ground: &dyn Fn(f32, f32) -> f32) {
    // p[0] is the origin, p[1] the u end, p[3] the v end, p[2] the far corner.
    let u_len = len2(p[1], p[0]) + len2(p[2], p[3]);
    let v_len = len2(p[3], p[0]) + len2(p[2], p[1]);
    let uc = ((u_len / (2.0 * GRID)) as usize).clamp(1, 256);
    let vc = ((v_len / (2.0 * GRID)) as usize).clamp(1, 256);
    let base = m.positions.len() as u32;
    for j in 0..=vc {
        let dv = j as f32 / vc as f32;
        for i in 0..=uc {
            let du = i as f32 / uc as f32;
            let mut v = [0.0f32; 3];
            for (k, vk) in v.iter_mut().enumerate() {
                let u1 = p[1][k] - p[0][k];
                let v1 = p[3][k] - p[0][k];
                let v2 = p[2][k] - p[1][k];
                *vk = p[0][k] + u1 * du + v1 * dv + du * dv * (v2 - v1);
            }
            m.uvs.push([v[0] / WATER_FACTOR, v[1] / WATER_FACTOR]);
            m.alpha.push(opacity(look, v[2], ground(v[0], v[1])));
            m.positions.push(v);
        }
    }
    let w = (uc + 1) as u32;
    // Both windings: the quad's orientation depends on the polygon's, and the water is two-sided.
    for j in 0..vc as u32 {
        for i in 0..uc as u32 {
            let a = base + j * w + i;
            m.indices.extend([a, a + 1, a + w + 1, a, a + w + 1, a + w]);
        }
    }
}

/// `drawRiverWater`: a strip pairing `point(river_start + 1 + i)` with `point(river_start - i)`.
fn river(look: &WaterLook, t: &PolygonTrigger, ground: &dyn Fn(f32, f32) -> f32) -> Option<WaterMesh> {
    let n = t.points.len();
    let pairs = n / 2;
    let start = t.river_start;
    if pairs < 2 || start < 0 || start as usize >= n - 1 {
        return None;
    }
    let pt = |i: usize| [t.points[i][0] as f32, t.points[i][1] as f32, t.points[i][2] as f32];
    let (mut end_len, mut total) = (0.0f32, 0.0f32);
    for i in 0..n - 1 {
        let l = len2(pt(i), pt(i + 1));
        total += l;
        if i as i32 == start {
            end_len = l;
        }
    }
    let rect_count = (pairs - 1) as f32;
    let repeat = ((total / 2.0) - end_len) / end_len.max(1.0);
    let v_scale = repeat / rect_count;
    let mut m = WaterMesh::new(WaterKind::River);
    let (mut inner, mut outer) = (start, start + 1);
    let base_alpha = look.diffuse_alpha.max(look.min_opacity.min(1.0));
    for i in 0..pairs {
        let a = pt(outer as usize); // u = HEIGHT_TO_USE (0.5)
        let b = pt(inner as usize); // u = 0
        let fi = v_scale * i as f32;
        let v = fi + libm::sinf(2.0 * std::f32::consts::PI * fi) / 22.0;
        for c in 0..=RIVER_COLUMNS {
            let f = c as f32 / RIVER_COLUMNS as f32;
            let pos = [b[0] + (a[0] - b[0]) * f, b[1] + (a[1] - b[1]) * f, b[2] + (a[2] - b[2]) * f];
            m.uvs.push([0.5 * f, v]);
            // Bank feathering (stand-in for TWAlphaEdge) and shallows over high ground.
            let edge = (f.min(1.0 - f) / 0.25).clamp(0.0, 1.0);
            let shallow = if look.transparent_depth > 0.0 { (opacity(look, pos[2], ground(pos[0], pos[1])) / look.min_opacity.max(0.01)).clamp(0.0, 1.0) } else { 1.0 };
            m.alpha.push(base_alpha * edge * shallow);
            m.positions.push(pos);
        }
        outer += 1;
        inner -= 1;
        if inner < 0 {
            inner = n as i32 - 1;
        }
        if outer as usize >= n {
            outer = 0;
        }
    }
    let w = (RIVER_COLUMNS + 1) as u32;
    for r in 0..(pairs as u32 - 1) {
        for c in 0..RIVER_COLUMNS as u32 {
            let a = r * w + c;
            m.indices.extend([a, a + w, a + w + 1, a, a + w + 1, a + 1]);
        }
    }
    Some(m)
}

/// Builds all water of the map. `ground` samples terrain height at a world position.
/// Returns `None` if the map has no water.
pub fn build_water(map: &MapFile, vfs: &Vfs, ini: &IniDb, lighting: &GlobalLighting, ground: &dyn Fn(f32, f32) -> f32) -> Result<Option<Water>> {
    let Some(trigs) = map.triggers.as_ref() else { return Ok(None) };
    let areas: Vec<_> = trigs.water_areas().filter(|t| t.points.len() > 2).collect();
    if areas.is_empty() {
        return Ok(None);
    }
    let look = look(vfs, ini, lighting)?;
    let mut meshes = Vec::new();
    for t in areas {
        if t.is_river {
            meshes.extend(river(&look, t, ground));
            continue;
        }
        let pt = |i: usize| [t.points[i][0] as f32, t.points[i][1] as f32, t.points[i][2] as f32];
        let mut m = WaterMesh::new(WaterKind::Area);
        let n = t.points.len();
        let mut k = 1;
        while k < n - 1 {
            let p0 = if k + 2 < n { pt(k + 2) } else { pt(k + 1) };
            trapezoid(&mut m, &look, [p0, pt(k + 1), pt(k), pt(0)], ground);
            k += 2;
        }
        meshes.push(m);
    }
    Ok(Some(Water { look, meshes }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_look() -> WaterLook {
        WaterLook {
            texture: RgbaImage::new(1, 1),
            shade: [1.0; 3],
            diffuse_alpha: 1.0,
            transparent_depth: 3.0,
            min_opacity: 1.0,
            additive: false,
            u_scroll_per_ms: 0.0,
            v_scroll_per_ms: 0.0,
        }
    }

    #[test]
    fn trapezoid_covers_quad_and_fades_by_depth() {
        let look = flat_look();
        let mut m = WaterMesh::new(WaterKind::Area);
        // 100x100 square at z = 10, terrain 0 on the left half and 10 (shore) on the right.
        let p = [[0.0, 100.0, 10.0], [100.0, 100.0, 10.0], [100.0, 0.0, 10.0], [0.0, 0.0, 10.0]];
        trapezoid(&mut m, &look, p, &|x, _| if x < 50.0 { 0.0 } else { 10.0 });
        let (lo, hi) = m.positions.iter().fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), v| ([lo[0].min(v[0]), lo[1].min(v[1])], [hi[0].max(v[0]), hi[1].max(v[1])]));
        assert_eq!((lo, hi), ([0.0, 0.0], [100.0, 100.0]));
        assert_eq!(m.indices.len() % 3, 0);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
        for (v, a) in m.positions.iter().zip(&m.alpha) {
            assert_eq!(*a, if v[0] < 50.0 { 1.0 } else { 0.0 });
        }
    }

    #[test]
    fn river_strip_pairs_points() {
        let look = flat_look();
        // Two banks of a straight river: points 0..3 go along one bank, 4..7 back along the other.
        let t = PolygonTrigger {
            name: "r".into(),
            layer: String::new(),
            id: 1,
            is_water: true,
            is_river: true,
            river_start: 0,
            points: vec![[0, 0, 5], [0, 100, 5], [0, 200, 5], [0, 300, 5], [50, 300, 5], [50, 200, 5], [50, 100, 5], [50, 0, 5]],
        };
        let m = river(&look, &t, &|_, _| 0.0).unwrap();
        assert_eq!(m.positions.len(), 4 * (RIVER_COLUMNS + 1));
        assert_eq!(m.indices.len(), 3 * 2 * RIVER_COLUMNS * 3);
        // Bank vertices are transparent, the middle is not.
        assert_eq!(m.alpha[0], 0.0);
        assert!(m.alpha[RIVER_COLUMNS / 2] > 0.9);
    }
}
