//! Rest-pose model assembly from the W3D chunk tree.
//!
//! Chunk layouts: `WW3D2/w3d_file.h`. Hierarchy base pose: `htree.cpp`
//! (`HTreeClass::Init_Default` / `Load_W3D`). Skin deformation (rest pose):
//! `meshgeometry.cpp` `get_deformed_vertices` (`bone_tm * vertex`).
//! HLOD sub-object -> bone binding: `hlod.cpp` (`HLodDefClass::Load_W3D`).
//! UV flip: `meshmdlio.cpp` (`uv.Y = 1.0f - texcoord.V`).

use crate::error::{invalid, Error};
use crate::texture;
use cnc_formats::w3d::{parse_mesh_header, parse_texture_name, parse_vertices, W3dChunk, W3dFile};
use zr_assets::{normalize_path, Vfs};

const CHUNK_VERTICES: u32 = 0x02;
const CHUNK_NORMALS: u32 = 0x03;
const CHUNK_VERTEX_INFLUENCES: u32 = 0x0E;
const CHUNK_MESH_HEADER3: u32 = 0x1F;
const CHUNK_TRIANGLES: u32 = 0x20;
const CHUNK_PRELIT_UNLIT: u32 = 0x23;
const CHUNK_PRELIT_VERTEX: u32 = 0x24;
const CHUNK_PRELIT_LIGHTMAP_MULTI_PASS: u32 = 0x25;
const CHUNK_PRELIT_LIGHTMAP_MULTI_TEXTURE: u32 = 0x26;
const CHUNK_SHADERS: u32 = 0x29;
const CHUNK_VERTEX_MATERIALS: u32 = 0x2A;
const CHUNK_VERTEX_MATERIAL: u32 = 0x2B;
const CHUNK_VERTEX_MATERIAL_INFO: u32 = 0x2D;
const CHUNK_TEXTURES: u32 = 0x30;
const CHUNK_TEXTURE: u32 = 0x31;
const CHUNK_TEXTURE_NAME: u32 = 0x32;
/// `W3D_CHUNK_TEXTURE_INFO` (`w3d_file.h:374`): optional `W3dTextureInfoStruct` inside a texture chunk.
const CHUNK_TEXTURE_INFO: u32 = 0x33;
const CHUNK_MATERIAL_PASS: u32 = 0x38;
const CHUNK_VERTEX_MATERIAL_IDS: u32 = 0x39;
const CHUNK_SHADER_IDS: u32 = 0x3A;
const CHUNK_TEXTURE_STAGE: u32 = 0x48;
const CHUNK_TEXTURE_IDS: u32 = 0x49;
const CHUNK_STAGE_TEXCOORDS: u32 = 0x4A;
const CHUNK_HIERARCHY_HEADER: u32 = 0x101;
const CHUNK_PIVOTS: u32 = 0x102;
const CHUNK_HLOD_HEADER: u32 = 0x701;
const CHUNK_HLOD_LOD_ARRAY: u32 = 0x702;
const CHUNK_HLOD_SUB_OBJECT_ARRAY_HEADER: u32 = 0x703;
const CHUNK_HLOD_SUB_OBJECT: u32 = 0x704;

const MESH_FLAG_COLLISION_BOX: u32 = 0x1;
const MESH_FLAG_HIDDEN: u32 = 0x1000;
const MESH_FLAG_TWO_SIDED: u32 = 0x2000;
const MESH_GEOMETRY_TYPE_MASK: u32 = 0x00FF_0000;
const GEOMETRY_SKIN: u32 = 0x0002_0000;
/// Obsolete shadow / AABox / OBBox geometry types: never drawn.
const GEOMETRY_NON_RENDER: [u32; 3] = [0x0003_0000, 0x0004_0000, 0x0005_0000];

const MAX_PIVOTS: usize = 8192;

const W3DTEXTURE_NO_LOD: u32 = 0x0004;
const W3DTEXTURE_CLAMP_U: u32 = 0x0008;
const W3DTEXTURE_CLAMP_V: u32 = 0x0010;
const W3DTEXTURE_MIP_LEVELS_MASK: u32 = 0x00C0;

/// How the engine samples one W3D texture, from its `W3dTextureInfoStruct::Attributes` (`w3d_file.h`
/// `W3DTEXTURE_*`, applied by `texture.cpp` `Load_Texture` to each `TextureClass`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextureParams {
    /// `W3DTEXTURE_CLAMP_U` / `_V`: `TEXTURE_ADDRESS_CLAMP`. Otherwise the engine repeats (`TEXTURE_ADDRESS_REPEAT`).
    pub clamp_u: bool,
    pub clamp_v: bool,
    /// Mip levels the engine keeps: `None` for all, `Some(n)` under the `MIP_LEVELS_2/3/4` cap, `Some(1)` under
    /// `W3DTEXTURE_NO_LOD` (`texture.cpp` `Load_Texture`: `MIP_LEVELS_1` and mip filter `NONE`).
    pub levels: Option<u32>,
}

impl TextureParams {
    /// From a texture chunk's info attributes. `None` (no info chunk) is the engine's `Get_Texture(name)` default:
    /// repeat, all levels (`texture.cpp` `Load_Texture`, the branch without `hastexinfo`).
    pub fn from_attributes(attrs: Option<u16>) -> TextureParams {
        let Some(a) = attrs.map(u32::from) else { return TextureParams::default() };
        let levels = if a & W3DTEXTURE_NO_LOD != 0 {
            Some(1)
        } else {
            match a & W3DTEXTURE_MIP_LEVELS_MASK {
                0x40 => Some(2),
                0x80 => Some(3),
                0xC0 => Some(4),
                _ => None,
            }
        };
        TextureParams { clamp_u: a & W3DTEXTURE_CLAMP_U != 0, clamp_v: a & W3DTEXTURE_CLAMP_V != 0, levels }
    }
}

/// Base-pass compositing, from the W3D shader's src/dest blend and alpha-test fields.
/// Ported from `ShaderClass::Apply` (WW3D2/shader.cpp): blending is on unless the pair is
/// (ONE, ZERO); alpha test uses reference `0x60` (`ALPHA_TEST_REF`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlendMode {
    #[default]
    Opaque,
    /// Alpha test only (or test + blend, where the cut-out dominates): discard alpha < `ALPHA_TEST_REF`.
    AlphaTest,
    /// SRC_ALPHA / ONE_MINUS_SRC_ALPHA style blending.
    AlphaBlend,
    /// Destination factor ONE (additive glows, beams).
    Additive,
    /// Source ZERO, destination SRC_COLOR (multiply / shadow decals).
    Multiply,
}

/// The engine's alpha-test reference (`0x60` of 255) in 0..1; "produces best results with mip-mapped textures".
pub const ALPHA_TEST_REF: f32 = 0x60 as f32 / 255.0;

/// One drawable mesh of a model.
#[derive(Debug, Clone)]
pub struct ModelMesh {
    pub name: String,
    /// `container.name` (the render object's `Get_Name`, which the engine's tread test splits on `.`);
    /// just `name` when the mesh has no container.
    pub full_name: String,
    /// Already transformed into model space by the rest-pose bone transform.
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Base (first non-environment textured) pass / stage; V is flipped so they index a top-row-first image.
    pub uvs: Vec<[f32; 2]>,
    /// CCW front faces in W3D's Z-up right-handed space.
    pub indices: Vec<u32>,
    /// Normalized texture file name as found in the VFS (or the normalized
    /// reference if it could not be found).
    pub texture: Option<String>,
    /// The texture's sampling attributes (`TextureParams`); the default when the mesh has no texture.
    pub texture_params: TextureParams,
    /// True when the base pass blends or alpha-tests (`blend != Opaque`).
    pub alpha_blend: bool,
    /// How the base pass composites, classified like `ShaderClass::Apply`.
    pub blend: BlendMode,
    pub two_sided: bool,
    /// Hidden-by-default, collision box or shadow-only mesh: should not render.
    pub hidden: bool,
    /// Rigid binding: for a non-skinned HLOD sub-object, the bone (pivot) the
    /// mesh is attached to. `positions`/`normals` are already in bind model
    /// space; a renderer recreates the rest pose with this bone's bind transform.
    pub bone: Option<usize>,
    /// Skinned binding: for a `GEOMETRY_SKIN` mesh, the bone index of each
    /// (compacted) vertex. W3D stores exactly one bone per vertex
    /// (`W3dVertInfStruct`, `meshgeometry.cpp` `get_deformed_vertices`).
    pub vertex_bones: Option<Vec<u16>>,
}

#[derive(Debug, Clone)]
pub struct Model {
    pub name: String,
    pub meshes: Vec<ModelMesh>,
}

/// 3x4 affine matrix, row-major (`m[row][col]`, translation in column 3).
type Mat = [[f32; 4]; 3];
const IDENTITY: Mat = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];

fn mat_mul(a: &Mat, b: &Mat) -> Mat {
    let mut o = [[0.0; 4]; 3];
    for (r, row) in o.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
        }
        row[3] += a[r][3];
    }
    o
}

fn xform_point(m: &Mat, p: [f32; 3]) -> [f32; 3] {
    let f = |r: usize| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3];
    [f(0), f(1), f(2)]
}

fn xform_dir(m: &Mat, p: [f32; 3]) -> [f32; 3] {
    let f = |r: usize| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2];
    [f(0), f(1), f(2)]
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

/// Base-pose world transform of every pivot (`HTreeClass::Base_Update`).
fn parse_htree(h: &W3dChunk) -> Option<Vec<Mat>> {
    let hdr = h.find_child(CHUNK_HIERARCHY_HEADER)?;
    let n = rd_u32(hdr.data, 20)? as usize;
    let piv = h.find_child(CHUNK_PIVOTS)?.data;
    const SZ: usize = 60;
    let n = n.min(piv.len() / SZ).min(MAX_PIVOTS);
    let mut local: Vec<(Option<usize>, Mat)> = Vec::with_capacity(n);
    for i in 0..n {
        let b = &piv[i * SZ..(i + 1) * SZ];
        let parent = rd_u32(b, 16).filter(|&p| p != u32::MAX).map(|p| p as usize);
        let t = [rd_f32(b, 20)?, rd_f32(b, 24)?, rd_f32(b, 28)?];
        let (x, y, z, w) = (rd_f32(b, 44)?, rd_f32(b, 48)?, rd_f32(b, 52)?, rd_f32(b, 56)?);
        // Build_Matrix3D(Quaternion): standard rotation matrix.
        let m: Mat = [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (z * x + y * w), t[0]],
            [2.0 * (x * y + z * w), 1.0 - 2.0 * (z * z + x * x), 2.0 * (y * z - x * w), t[1]],
            [2.0 * (z * x - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y), t[2]],
        ];
        local.push((parent, m));
    }
    let mut world: Vec<Option<Mat>> = vec![None; n];
    for i in 0..n {
        // Walk up to the first resolved ancestor, then fold down.
        let mut chain = vec![i];
        let mut cur = i;
        while world[cur].is_none() {
            match local[cur].0 {
                Some(p) if p < n && chain.len() <= n => {
                    chain.push(p);
                    cur = p;
                }
                _ => break,
            }
        }
        let mut acc = if world[cur].is_some() { world[cur] } else { None };
        for &c in chain.iter().rev() {
            if world[c].is_none() {
                let m = match acc {
                    Some(p) => mat_mul(&p, &local[c].1),
                    None => local[c].1,
                };
                world[c] = Some(m);
            }
            acc = world[c];
        }
    }
    Some(world.into_iter().map(|m| m.unwrap_or(IDENTITY)).collect())
}

#[derive(Clone, Copy)]
struct Shader {
    src_blend: u8,
    dest_blend: u8,
    alpha_test: u8,
}

impl Shader {
    /// `W3dShaderStruct` values: src 0 ZERO, 1 ONE, 2 SRC_ALPHA, 3 ONE_MINUS_SRC_ALPHA;
    /// dest 0 ZERO, 1 ONE, 2 SRC_COLOR, 3 ONE_MINUS_SRC_COLOR, 4 SRC_ALPHA, 5 ONE_MINUS_SRC_ALPHA.
    fn mode(&self) -> BlendMode {
        let blend_on = !(self.src_blend == 1 && self.dest_blend == 0);
        if self.alpha_test != 0 {
            return BlendMode::AlphaTest;
        }
        if !blend_on {
            return BlendMode::Opaque;
        }
        match (self.src_blend, self.dest_blend) {
            (_, 1) => BlendMode::Additive,
            (0, 2) => BlendMode::Multiply,
            _ => BlendMode::AlphaBlend,
        }
    }
}

/// A W3D "single or per-element array of u32 ids" chunk (`W3D_CHUNK_TEXTURE_IDS`,
/// `_SHADER_IDS`, `_VERTEX_MATERIAL_IDS`): a single entry applies to everything,
/// otherwise the array is per triangle (textures, shaders) or per vertex
/// (vertex materials). Ported from `meshmdlio.cpp` `read_*_ids`.
enum Ids {
    Single(u32),
    Many(Vec<u32>),
}

impl Ids {
    fn parse(data: &[u8]) -> Option<Ids> {
        if data.len() < 4 {
            None
        } else if data.len() == 4 {
            rd_u32(data, 0).map(Ids::Single)
        } else {
            Some(Ids::Many(data.chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()))
        }
    }

    fn get(&self, i: usize) -> Option<u32> {
        match self {
            Ids::Single(v) => Some(*v),
            Ids::Many(v) => v.get(i).copied(),
        }
    }
}

struct Stage {
    tex: Option<Ids>,
    /// Per-vertex coordinates (V already flipped).
    uvs: Option<Vec<[f32; 2]>>,
}

struct Pass {
    vmat: Option<Ids>,
    shader: Option<Ids>,
    stages: Vec<Stage>,
}

struct Material {
    textures: Vec<String>,
    /// Sampling attributes of each texture (same index as `textures`).
    tex_info: Vec<TextureParams>,
    shaders: Vec<Shader>,
    /// `W3dVertexMaterialStruct::Attributes` of each vertex material.
    vmat_attrs: Vec<u32>,
    passes: Vec<Pass>,
}

/// Stage `s` (0 or 1) of a vertex material uses an environment / reflection
/// mapper (`vertmaterial.cpp` `Parse_Mapping_Args`): ENVIRONMENT, CHEAP_ENVIRONMENT,
/// WS_CLASSIC_ENV, WS_ENVIRONMENT, GRID_CLASSIC_ENV, GRID_ENVIRONMENT, BUMPENV,
/// GRID_WS_CLASSIC_ENV, GRID_WS_ENVIRONMENT.
fn is_env_mapping(attrs: u32, s: usize) -> bool {
    let m = if s == 0 { (attrs >> 16) & 0xFF } else { (attrs >> 8) & 0xFF };
    matches!(m, 0x01 | 0x02 | 0x0C | 0x0D | 0x0E | 0x0F | 0x12 | 0x13 | 0x14)
}

impl Material {
    fn tex_name(&self, id: u32) -> Option<&String> {
        self.textures.get(id as usize).filter(|n| !n.is_empty())
    }

    fn tex_params(&self, id: u32) -> TextureParams {
        self.tex_info.get(id as usize).copied().unwrap_or_default()
    }

    fn pass_is_env(&self, pass: &Pass, stage: usize) -> bool {
        // Per-vertex material arrays are sampled at vertex 0.
        pass.vmat
            .as_ref()
            .and_then(|v| v.get(0))
            .and_then(|i| self.vmat_attrs.get(i as usize))
            .is_some_and(|&a| is_env_mapping(a, stage))
    }

    fn stage_textured(&self, st: &Stage) -> bool {
        match &st.tex {
            Some(Ids::Single(i)) => self.tex_name(*i).is_some(),
            Some(Ids::Many(v)) => v.iter().any(|&i| self.tex_name(i).is_some()),
            None => false,
        }
    }

    /// The pass/stage that provides the diffuse texture: the first textured
    /// pass whose mapper is not an environment/reflection one; failing that
    /// (the env pass is the only textured one) the first textured pass.
    fn base(&self) -> Option<(&Pass, &Stage)> {
        let mut fallback = None;
        for pass in &self.passes {
            let Some((s, st)) = pass.stages.iter().enumerate().find(|(_, st)| self.stage_textured(st)) else {
                continue;
            };
            if self.pass_is_env(pass, s) {
                fallback.get_or_insert((pass, st));
            } else {
                return Some((pass, st));
            }
        }
        fallback
    }
}

/// The chunk that holds the material chunks: the mesh itself or, failing that,
/// one of the PRELIT wrappers.
fn material_container<'a, 'b>(mesh: &'a W3dChunk<'b>) -> &'a W3dChunk<'b> {
    if mesh.find_child(CHUNK_MATERIAL_PASS).is_some() {
        return mesh;
    }
    for id in [
        CHUNK_PRELIT_VERTEX,
        CHUNK_PRELIT_UNLIT,
        CHUNK_PRELIT_LIGHTMAP_MULTI_PASS,
        CHUNK_PRELIT_LIGHTMAP_MULTI_TEXTURE,
    ] {
        if let Some(c) = mesh.find_child(id) {
            if c.find_child(CHUNK_MATERIAL_PASS).is_some() {
                return c;
            }
        }
    }
    mesh
}

fn parse_material(mesh: &W3dChunk) -> Material {
    let mc = material_container(mesh);
    let (textures, tex_info): (Vec<String>, Vec<TextureParams>) = mc
        .find_child(CHUNK_TEXTURES)
        .map(|t| {
            t.find_children(CHUNK_TEXTURE)
                .iter()
                .map(|c| {
                    let name = c.find_child(CHUNK_TEXTURE_NAME).map(|n| parse_texture_name(n.data).to_string()).unwrap_or_default();
                    // `W3dTextureInfoStruct`: `Attributes` is its first u16 (`w3d_file.h:1066-1069`).
                    let attrs = c.find_child(CHUNK_TEXTURE_INFO).and_then(|i| i.data.get(..2)).map(|d| u16::from_le_bytes([d[0], d[1]]));
                    (name, TextureParams::from_attributes(attrs))
                })
                .unzip()
        })
        .unwrap_or_default();
    let shaders: Vec<Shader> = mc
        .find_child(CHUNK_SHADERS)
        .map(|s| {
            s.data
                .chunks_exact(16)
                .map(|b| Shader { dest_blend: b[3], src_blend: b[7], alpha_test: b[12] })
                .collect()
        })
        .unwrap_or_default();
    let vmat_attrs: Vec<u32> = mc
        .find_child(CHUNK_VERTEX_MATERIALS)
        .map(|v| {
            v.find_children(CHUNK_VERTEX_MATERIAL)
                .iter()
                .map(|m| m.find_child(CHUNK_VERTEX_MATERIAL_INFO).and_then(|i| rd_u32(i.data, 0)).unwrap_or(0))
                .collect()
        })
        .unwrap_or_default();
    let passes = mc
        .find_children(CHUNK_MATERIAL_PASS)
        .iter()
        .map(|pass| Pass {
            vmat: pass.find_child(CHUNK_VERTEX_MATERIAL_IDS).and_then(|c| Ids::parse(c.data)),
            shader: pass.find_child(CHUNK_SHADER_IDS).and_then(|c| Ids::parse(c.data)),
            stages: pass
                .find_children(CHUNK_TEXTURE_STAGE)
                .iter()
                .map(|stage| Stage {
                    tex: stage.find_child(CHUNK_TEXTURE_IDS).and_then(|c| Ids::parse(c.data)),
                    uvs: stage.find_child(CHUNK_STAGE_TEXCOORDS).map(|c| {
                        c.data
                            .chunks_exact(8)
                            .map(|b| {
                                let u = f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                                let v = f32::from_bits(u32::from_le_bytes([b[4], b[5], b[6], b[7]]));
                                [u, 1.0 - v]
                            })
                            .collect()
                    }),
                })
                .collect(),
        })
        .collect();
    Material { textures, tex_info, shaders, vmat_attrs, passes }
}

fn build_mesh(mesh: &W3dChunk, htree: Option<&[Mat]>, bone: Option<usize>, vfs: &Vfs) -> Result<Vec<ModelMesh>, Error> {
    let header = parse_mesh_header(mesh.find_child(CHUNK_MESH_HEADER3).ok_or_else(|| invalid("mesh without header"))?.data)
        .map_err(|e| invalid(format!("mesh header: {e}")))?;
    let attrs = header.attributes;
    let verts = parse_vertices(mesh.find_child(CHUNK_VERTICES).map_or(&[][..], |c| c.data))
        .map_err(|e| invalid(format!("vertices: {e}")))?;
    let n = verts.len();
    let mut positions: Vec<[f32; 3]> = verts.iter().map(|v| [v.x, v.y, v.z]).collect();
    let mut normals: Vec<[f32; 3]> = match mesh.find_child(CHUNK_NORMALS) {
        Some(c) => parse_vertices(c.data).map(|v| v.iter().map(|v| [v.x, v.y, v.z]).collect()).unwrap_or_default(),
        None => Vec::new(),
    };
    if normals.len() != n {
        normals.clear();
    }

    // Triangles: 3 x u32 indices, attributes, plane normal + dist (32 bytes).
    let tri_data = mesh.find_child(CHUNK_TRIANGLES).map_or(&[][..], |c| c.data);
    let mut indices = Vec::with_capacity(tri_data.len() / 32 * 3);
    let mut face_of: Vec<usize> = Vec::new();
    for (fi, t) in tri_data.chunks_exact(32).enumerate() {
        let i = [rd_u32(t, 0).unwrap_or(0), rd_u32(t, 4).unwrap_or(0), rd_u32(t, 8).unwrap_or(0)];
        if i.iter().all(|&x| (x as usize) < n) {
            indices.extend_from_slice(&i);
            face_of.push(fi);
        }
    }

    // Rest-pose transform. The raw vertices are in the bound bone's local space;
    // the returned `positions` are baked into bind model space, and the binding
    // (`bone`/`vertex_bones`) lets a renderer re-apply an animated pose.
    let skin = attrs & MESH_GEOMETRY_TYPE_MASK == GEOMETRY_SKIN;
    let influences = mesh.find_child(CHUNK_VERTEX_INFLUENCES).map(|c| c.data);
    let mut skin_bones: Option<Vec<u16>> = None;
    match (htree, skin, influences) {
        (Some(ht), true, Some(inf)) if inf.len() >= n * 8 => {
            let mut bones = Vec::with_capacity(n);
            for i in 0..n {
                let b = u16::from_le_bytes([inf[i * 8], inf[i * 8 + 1]]);
                bones.push(b);
                if let Some(m) = ht.get(b as usize) {
                    positions[i] = xform_point(m, positions[i]);
                    if !normals.is_empty() {
                        normals[i] = xform_dir(m, normals[i]);
                    }
                }
            }
            skin_bones = Some(bones);
        }
        (Some(ht), false, _) => {
            if let Some(m) = bone.and_then(|b| ht.get(b)) {
                for p in &mut positions {
                    *p = xform_point(m, *p);
                }
                for nrm in &mut normals {
                    *nrm = xform_dir(m, *nrm);
                }
            }
        }
        _ => {}
    }

    if normals.is_empty() {
        normals = vertex_normals(&positions, &indices);
    }

    let mat = parse_material(mesh);
    let base = mat.base();
    // Texcoords of the chosen stage (per vertex).
    let uvs_all: Vec<[f32; 2]> = base
        .and_then(|(_, st)| st.uvs.clone())
        .filter(|u| u.len() == n)
        .unwrap_or_else(|| vec![[0.0, 0.0]; n]);
    let name = header.name_str().to_string();
    let container = cstr(&header.container_name);
    let full_name = if container.is_empty() { name.clone() } else { format!("{container}.{name}") };
    let hidden =attrs & (MESH_FLAG_HIDDEN | MESH_FLAG_COLLISION_BOX) != 0
        || GEOMETRY_NON_RENDER.contains(&(attrs & MESH_GEOMETRY_TYPE_MASK))
        || name.eq_ignore_ascii_case("boundingbox")
        || is_fx_mesh(&name);
    let two_sided = attrs & MESH_FLAG_TWO_SIDED != 0;

    // Group the (valid) triangles by their per-face texture id and whether their
    // shader blends; one ModelMesh per group, in first-appearance order.
    type Key = (Option<u32>, BlendMode);
    let mut groups: Vec<(Key, Vec<u32>)> = Vec::new();
    for (face, i) in indices.chunks_exact(3).enumerate() {
        let src = face_of[face];
        let tex = base
            .and_then(|(_, st)| st.tex.as_ref())
            .and_then(|t| t.get(if matches!(t, Ids::Single(_)) { 0 } else { src }))
            .filter(|&t| mat.tex_name(t).is_some());
        let blend = base
            .and_then(|(p, _)| p.shader.as_ref())
            .and_then(|s| s.get(if matches!(s, Ids::Single(_)) { 0 } else { src }))
            .and_then(|s| mat.shaders.get(s as usize))
            .map_or(BlendMode::Opaque, |s| s.mode());
        let key = (tex, blend);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.extend_from_slice(i),
            None => groups.push((key, i.to_vec())),
        }
    }
    if groups.is_empty() {
        groups.push(((None, BlendMode::Opaque), Vec::new()));
    }
    let single = groups.len() == 1;
    let mut out = Vec::with_capacity(groups.len());
    for (gi, ((tex, blend), idx)) in groups.into_iter().enumerate() {
        let texture = tex
            .and_then(|t| mat.tex_name(t))
            .map(|t| texture::resolve(vfs, t).unwrap_or_else(|| normalize_path(t)));
        let (positions, normals, uvs, idx, vertex_bones) = if single {
            (positions.clone(), normals.clone(), uvs_all.clone(), idx, skin_bones.clone())
        } else {
            // Compact the vertex arrays to the ones this group uses.
            let mut remap = vec![u32::MAX; n];
            let (mut p, mut nr, mut uv) = (Vec::new(), Vec::new(), Vec::new());
            let mut bones = skin_bones.as_ref().map(|_| Vec::new());
            let idx: Vec<u32> = idx
                .iter()
                .map(|&v| {
                    let v = v as usize;
                    if remap[v] == u32::MAX {
                        remap[v] = p.len() as u32;
                        p.push(positions[v]);
                        nr.push(normals[v]);
                        uv.push(uvs_all[v]);
                        if let (Some(src), Some(dst)) = (&skin_bones, bones.as_mut()) {
                            dst.push(src[v]);
                        }
                    }
                    remap[v]
                })
                .collect();
            (p, nr, uv, idx, bones)
        };
        out.push(ModelMesh {
            name: if gi == 0 { name.clone() } else { format!("{name}#{gi}") },
            full_name: full_name.clone(),
            positions,
            normals,
            uvs,
            indices: idx,
            texture,
            texture_params: tex.map(|t| mat.tex_params(t)).unwrap_or_default(),
            alpha_blend: blend != BlendMode::Opaque,
            blend,
            two_sided,
            hidden,
            bone: if skin { None } else { bone },
            vertex_bones,
        });
    }
    Ok(out)
}

/// Muzzle-flash style sub-objects (`TURRETFX01`, `MUZZLEFX02`): the engine
/// shows them only while firing (W3DModelDraw hides them by default).
fn is_fx_mesh(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    let stem = n.trim_end_matches(|c: char| c.is_ascii_digit());
    n.starts_with("MUZZLEFX") || (stem.len() < n.len() && stem.ends_with("FX"))
}

/// Area-weighted smooth normals, used only when the file has none.
fn vertex_normals(pos: &[[f32; 3]], idx: &[u32]) -> Vec<[f32; 3]> {
    let mut acc = vec![[0.0f32; 3]; pos.len()];
    for t in idx.chunks_exact(3) {
        let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
        let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        for &i in t {
            for k in 0..3 {
                acc[i as usize][k] += n[k];
            }
        }
    }
    for n in &mut acc {
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if l > 0.0 {
            *n = [n[0] / l, n[1] / l, n[2] / l];
        } else {
            *n = [0.0, 0.0, 1.0];
        }
    }
    acc
}

fn w3d_path(name: &str) -> String {
    let n = normalize_path(name.trim());
    let n = n.strip_suffix(".w3d").unwrap_or(&n);
    format!("art\\w3d\\{n}.w3d")
}

fn mesh_key(m: &W3dChunk) -> Option<String> {
    let h = parse_mesh_header(m.find_child(CHUNK_MESH_HEADER3)?.data).ok()?;
    let (c, n) = (cstr(&h.container_name), cstr(&h.mesh_name));
    Some(if c.is_empty() { n } else { format!("{c}.{n}") }.to_ascii_lowercase())
}

/// Picks the highest-detail LOD array: the one with the largest
/// `MaxScreenSize` (the last on ties), per `hlod.cpp`'s `Lod[LodCount - 1]`.
fn top_lod<'a, 'b>(hlod: &'a W3dChunk<'b>) -> Option<&'a W3dChunk<'b>> {
    let mut best: Option<(&W3dChunk, f32)> = None;
    for arr in hlod.find_children(CHUNK_HLOD_LOD_ARRAY) {
        let size = arr
            .find_child(CHUNK_HLOD_SUB_OBJECT_ARRAY_HEADER)
            .and_then(|h| rd_f32(h.data, 4))
            .unwrap_or(0.0);
        if best.is_none_or(|(_, s)| size >= s) {
            best = Some((arr, size));
        }
    }
    best.map(|(a, _)| a)
}

/// Loads `art\w3d\<name>.w3d`, assembling the top LOD in the rest pose.
pub fn load_model(vfs: &Vfs, name: &str) -> Result<Model, Error> {
    let path = w3d_path(name);
    let bytes = vfs.read(&path).ok_or_else(|| Error::NotFound(path.clone()))?;
    let file = W3dFile::parse(&bytes).map_err(|e| invalid(format!("{path}: {e}")))?;
    let hlod = file.hlod();

    // Skeleton: in this file, else a separate `<HierarchyName>.w3d`.
    let mut htree = file.hierarchy().and_then(parse_htree);
    if htree.is_none() {
        let hier = hlod
            .and_then(|h| h.find_child(CHUNK_HLOD_HEADER))
            .and_then(|h| h.data.get(24..40))
            .map(cstr)
            .filter(|s| !s.is_empty());
        if let Some(hn) = hier {
            if let Some(b) = vfs.read(&w3d_path(&hn)) {
                if let Ok(f) = W3dFile::parse(&b) {
                    htree = f.hierarchy().and_then(parse_htree);
                }
            }
        }
    }

    let meshes = file.meshes();
    let mut out = Vec::new();
    match hlod.and_then(top_lod) {
        Some(lod) => {
            for so in lod.find_children(CHUNK_HLOD_SUB_OBJECT) {
                let Some(bone) = rd_u32(so.data, 0) else { continue };
                let sub = cstr(so.data.get(4..).unwrap_or(&[])).to_ascii_lowercase();
                // Sub-objects may also be non-meshes (nulls, emitters): skipped.
                let Some(mesh) = meshes.iter().find(|m| mesh_key(m).as_deref() == Some(sub.as_str())) else {
                    continue;
                };
                out.extend(build_mesh(mesh, htree.as_deref(), Some(bone as usize), vfs)?);
            }
        }
        None => {
            for mesh in &meshes {
                out.extend(build_mesh(mesh, htree.as_deref(), None, vfs)?);
            }
        }
    }
    if out.is_empty() {
        return Err(invalid(format!("{path}: no meshes")));
    }
    Ok(Model { name: name.to_string(), meshes: out })
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

    fn mesh_chunk(name: &str, container: &str, attrs: u32) -> Vec<u8> {
        mesh_chunk_info(name, container, attrs, None)
    }

    /// [`mesh_chunk`] whose texture carries a `W3dTextureInfoStruct` with these `Attributes`.
    fn mesh_chunk_info(name: &str, container: &str, attrs: u32, info: Option<u16>) -> Vec<u8> {
        let mut h = Vec::new();
        h.extend_from_slice(&0x0004_0002u32.to_le_bytes());
        h.extend_from_slice(&attrs.to_le_bytes());
        h.extend_from_slice(&name16(name));
        h.extend_from_slice(&name16(container));
        h.extend_from_slice(&1u32.to_le_bytes());
        h.extend_from_slice(&3u32.to_le_bytes());
        h.resize(116, 0);
        assert_eq!(h.len(), 116);
        let mut tri = Vec::new();
        for i in [0u32, 1, 2, 0] {
            tri.extend_from_slice(&i.to_le_bytes());
        }
        tri.extend_from_slice(&f32s(&[0.0, 0.0, 1.0, 0.0]));
        let mut body = chunk(CHUNK_MESH_HEADER3, &h, false);
        body.extend(chunk(CHUNK_VERTICES, &f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]), false));
        body.extend(chunk(CHUNK_NORMALS, &f32s(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]), false));
        body.extend(chunk(CHUNK_TRIANGLES, &tri, false));
        // Texture "T.TGA", UVs (0,0.25) each.
        let mut tex = chunk(CHUNK_TEXTURE_NAME, b"T.TGA\0", false);
        if let Some(a) = info {
            // `W3dTextureInfoStruct`: Attributes u16, AnimType u16, FrameCount u32, FrameRate f32.
            let mut inf = a.to_le_bytes().to_vec();
            inf.resize(12, 0);
            tex.extend(chunk(CHUNK_TEXTURE_INFO, &inf, false));
        }
        body.extend(chunk(CHUNK_TEXTURES, &chunk(CHUNK_TEXTURE, &tex, true), true));
        let mut sh = [0u8; 16];
        sh[3] = 5;
        sh[7] = 2;
        body.extend(chunk(CHUNK_SHADERS, &sh, false));
        let stage = [
            chunk(CHUNK_TEXTURE_IDS, &0u32.to_le_bytes(), false),
            chunk(CHUNK_STAGE_TEXCOORDS, &f32s(&[0.0, 0.25, 0.0, 0.25, 0.0, 0.25]), false),
        ]
        .concat();
        let pass = [chunk(CHUNK_SHADER_IDS, &0u32.to_le_bytes(), false), chunk(CHUNK_TEXTURE_STAGE, &stage, true)].concat();
        body.extend(chunk(CHUNK_MATERIAL_PASS, &pass, true));
        chunk(0, &body, true)
    }

    #[test]
    fn plain_mesh_without_hlod() {
        let data = mesh_chunk("M", "C", 0x2000);
        let file = W3dFile::parse(&data).unwrap();
        let m = file.meshes();
        let vfs = Vfs::empty();
        let mm = &build_mesh(m[0], None, None, &vfs).unwrap()[0];
        assert_eq!(mm.indices, vec![0, 1, 2]);
        assert_eq!(mm.uvs[0], [0.0, 0.75]);
        assert_eq!(mm.texture.as_deref(), Some("t.tga"));
        assert!(mm.alpha_blend && mm.two_sided && !mm.hidden);
        assert_eq!(mesh_key(m[0]).as_deref(), Some("c.m"));
    }


    #[test]
    fn texture_attributes_map_to_the_engine_sampling() {
        // `W3DTEXTURE_*` (w3d_file.h:1034-1044): NO_LOD 0x4, CLAMP_U 0x8, CLAMP_V 0x10, MIP_LEVELS 0x40/0x80/0xC0.
        let none = TextureParams::default();
        assert_eq!(TextureParams::from_attributes(None), none, "no info chunk: Get_Texture(name)");
        assert_eq!(TextureParams::from_attributes(Some(0)), none);
        assert_eq!(TextureParams::from_attributes(Some(0x0008)), TextureParams { clamp_u: true, ..none });
        assert_eq!(TextureParams::from_attributes(Some(0x0010)), TextureParams { clamp_v: true, ..none });
        assert_eq!(TextureParams::from_attributes(Some(0x0040)).levels, Some(2));
        assert_eq!(TextureParams::from_attributes(Some(0x0080)).levels, Some(3));
        assert_eq!(TextureParams::from_attributes(Some(0x00C0)).levels, Some(4));
        // NO_LOD wins over the mip-level cap (`texture.cpp` `Load_Texture` checks it first).
        assert_eq!(TextureParams::from_attributes(Some(0x00C4)).levels, Some(1));
        assert_eq!(TextureParams::from_attributes(Some(0x0004)).levels, Some(1));
    }

    #[test]
    fn texture_info_chunk_reaches_the_mesh() {
        let parse = |info: Option<u16>| {
            let data = mesh_chunk_info("M", "C", 0x2000, info);
            let file = W3dFile::parse(&data).unwrap();
            let m = file.meshes();
            build_mesh(m[0], None, None, &Vfs::empty()).unwrap()[0].texture_params
        };
        let none = TextureParams::default();
        assert_eq!(parse(None), none);
        assert_eq!(parse(Some(0x0018)), TextureParams { clamp_u: true, clamp_v: true, levels: None });
        assert_eq!(parse(Some(0x000C)), TextureParams { clamp_u: true, levels: Some(1), ..none });
    }

    /// Two triangles over four vertices, textures "A.TGA"/"B.TGA"/"ENV.TGA".
    /// `per_face`: pass 0 stage 0 holds one texture id per triangle (0, 1).
    /// `env_first`: an extra leading pass uses ENV.TGA with an ENVIRONMENT mapper.
    fn two_tri_mesh(per_face: bool, env_first: bool) -> Vec<u8> {
        let mut h = Vec::new();
        h.extend_from_slice(&0x0004_0002u32.to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(&name16("M"));
        h.extend_from_slice(&name16(""));
        h.extend_from_slice(&2u32.to_le_bytes());
        h.extend_from_slice(&4u32.to_le_bytes());
        h.resize(116, 0);
        let mut tri = Vec::new();
        for t in [[0u32, 1, 2], [1, 3, 2]] {
            for i in t {
                tri.extend_from_slice(&i.to_le_bytes());
            }
            tri.extend_from_slice(&0u32.to_le_bytes());
            tri.extend_from_slice(&f32s(&[0.0, 0.0, 1.0, 0.0]));
        }
        let mut body = chunk(CHUNK_MESH_HEADER3, &h, false);
        body.extend(chunk(CHUNK_VERTICES, &f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0]), false));
        body.extend(chunk(CHUNK_TRIANGLES, &tri, false));
        let tex = |n: &[u8]| chunk(CHUNK_TEXTURE, &chunk(CHUNK_TEXTURE_NAME, n, false), true);
        body.extend(chunk(CHUNK_TEXTURES, &[tex(b"A.TGA\0"), tex(b"B.TGA\0"), tex(b"ENV.TGA\0")].concat(), true));
        // Shader 0 opaque (src ONE, dest ZERO), shader 1 blends (src SRC_ALPHA=2... dest 5).
        let mut sh = [0u8; 32];
        sh[7] = 1;
        sh[16 + 3] = 5;
        sh[16 + 7] = 2;
        body.extend(chunk(CHUNK_SHADERS, &sh, false));
        let vm = |attrs: u32| chunk(CHUNK_VERTEX_MATERIAL, &chunk(CHUNK_VERTEX_MATERIAL_INFO, &attrs.to_le_bytes(), false), true);
        body.extend(chunk(CHUNK_VERTEX_MATERIALS, &[vm(0), vm(0x0001_0000)].concat(), true));
        let uv = f32s(&[0.0, 0.25, 0.5, 0.25, 0.0, 0.5, 0.5, 0.5]);
        let ids = |v: &[u32]| v.iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<u8>>();
        if env_first {
            let stage = chunk(CHUNK_TEXTURE_IDS, &ids(&[2]), false);
            let pass = [chunk(CHUNK_VERTEX_MATERIAL_IDS, &ids(&[1]), false), chunk(CHUNK_TEXTURE_STAGE, &stage, true)].concat();
            body.extend(chunk(CHUNK_MATERIAL_PASS, &pass, true));
        }
        let tex_ids = if per_face { ids(&[0, 1]) } else { ids(&[0]) };
        let stage = [chunk(CHUNK_TEXTURE_IDS, &tex_ids, false), chunk(CHUNK_STAGE_TEXCOORDS, &uv, false)].concat();
        let pass = [
            chunk(CHUNK_VERTEX_MATERIAL_IDS, &ids(&[0]), false),
            chunk(CHUNK_SHADER_IDS, &ids(&[0, 1]), false),
            chunk(CHUNK_TEXTURE_STAGE, &stage, true),
        ]
        .concat();
        body.extend(chunk(CHUNK_MATERIAL_PASS, &pass, true));
        chunk(0, &body, true)
    }

    #[test]
    fn per_face_texture_ids_split_meshes() {
        let data = two_tri_mesh(true, false);
        let file = W3dFile::parse(&data).unwrap();
        let ms = build_mesh(file.meshes()[0], None, None, &Vfs::empty()).unwrap();
        assert_eq!(ms.len(), 2);
        assert_eq!((ms[0].name.as_str(), ms[0].texture.as_deref()), ("M", Some("a.tga")));
        assert_eq!((ms[1].name.as_str(), ms[1].texture.as_deref()), ("M#1", Some("b.tga")));
        // Per-face shader ids: face 0 opaque, face 1 blends.
        assert!(!ms[0].alpha_blend && ms[1].alpha_blend);
        assert_eq!(ms[0].indices, vec![0, 1, 2]);
        // Vertices compacted per group (face 1 uses original 1, 3, 2) with their own UVs.
        assert_eq!(ms[1].indices, vec![0, 1, 2]);
        assert_eq!(ms[1].positions.len(), 3);
        assert_eq!(ms[1].positions[1], [1.0, 1.0, 0.0]);
        assert_eq!(ms[1].uvs[1], [0.5, 0.5]);
    }

    #[test]
    fn single_texture_id_stays_one_mesh() {
        let data = two_tri_mesh(false, false);
        let file = W3dFile::parse(&data).unwrap();
        let ms = build_mesh(file.meshes()[0], None, None, &Vfs::empty()).unwrap();
        // Same texture, but the per-face shaders differ in blending: still two groups.
        assert_eq!(ms.len(), 2);
        assert!(ms.iter().all(|m| m.texture.as_deref() == Some("a.tga")));
    }

    #[test]
    fn environment_pass_is_not_the_diffuse_texture() {
        let data = two_tri_mesh(true, true);
        let file = W3dFile::parse(&data).unwrap();
        let ms = build_mesh(file.meshes()[0], None, None, &Vfs::empty()).unwrap();
        assert_eq!(ms.len(), 2);
        assert!(ms.iter().all(|m| m.texture.as_deref() != Some("env.tga")));
        assert_eq!(ms[0].texture.as_deref(), Some("a.tga"));
    }
    #[test]
    fn fx_names() {
        assert!(is_fx_mesh("TURRETFX01") && is_fx_mesh("MuzzleFX") && !is_fx_mesh("CHASSIS"));
    }

    #[test]
    fn hidden_flags() {
        let data = mesh_chunk("BOX", "", 0x1000);
        let file = W3dFile::parse(&data).unwrap();
        assert!(build_mesh(file.meshes()[0], None, None, &Vfs::empty()).unwrap()[0].hidden);
    }

    #[test]
    fn htree_chain_and_bone_transform() {
        // Two pivots: root at origin, child translated (1,2,3) rotated 90deg about Z.
        let mut hdr = vec![0u8; 36];
        hdr[20..24].copy_from_slice(&2u32.to_le_bytes());
        let piv = |parent: u32, t: [f32; 3], q: [f32; 4]| {
            let mut p = name16("P");
            p.extend_from_slice(&parent.to_le_bytes());
            p.extend_from_slice(&f32s(&t));
            p.extend_from_slice(&f32s(&[0.0; 3]));
            p.extend_from_slice(&f32s(&q));
            p
        };
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let pivots = [piv(u32::MAX, [0.0; 3], [0.0, 0.0, 0.0, 1.0]), piv(0, [1.0, 2.0, 3.0], [0.0, 0.0, s, s])].concat();
        let h = [chunk(CHUNK_HIERARCHY_HEADER, &hdr, false), chunk(CHUNK_PIVOTS, &pivots, false)].concat();
        let data = chunk(0x100, &h, true);
        let file = W3dFile::parse(&data).unwrap();
        let ht = parse_htree(file.hierarchy().unwrap()).unwrap();
        assert_eq!(ht.len(), 2);
        let p = xform_point(&ht[1], [1.0, 0.0, 0.0]);
        assert!((p[0] - 1.0).abs() < 1e-5 && (p[1] - 3.0).abs() < 1e-5 && (p[2] - 3.0).abs() < 1e-5);
    }

    #[test]
    fn mesh_binding_metadata_and_bind_pose_agreement() {
        // Two-pivot hierarchy (root + child at (1,2,3), 90deg about Z).
        let mut hdr = vec![0u8; 36];
        hdr[20..24].copy_from_slice(&2u32.to_le_bytes());
        let piv = |parent: u32, t: [f32; 3], q: [f32; 4]| {
            let mut p = name16("P");
            p.extend_from_slice(&parent.to_le_bytes());
            p.extend_from_slice(&f32s(&t));
            p.extend_from_slice(&f32s(&[0.0; 3]));
            p.extend_from_slice(&f32s(&q));
            p
        };
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let pivots = [piv(u32::MAX, [0.0; 3], [0.0, 0.0, 0.0, 1.0]), piv(0, [1.0, 2.0, 3.0], [0.0, 0.0, s, s])].concat();
        let hchunk = chunk(0x100, &[chunk(CHUNK_HIERARCHY_HEADER, &hdr, false), chunk(CHUNK_PIVOTS, &pivots, false)].concat(), true);
        let hfile = W3dFile::parse(&hchunk).unwrap();
        let ht = parse_htree(hfile.hierarchy().unwrap()).unwrap();

        // Skinned mesh: every vertex bound to bone 1.
        let mut skin = mesh_chunk("SKIN", "", 0x0002_0000);
        let mut inf = Vec::new();
        for _ in 0..3 {
            inf.extend_from_slice(&1u16.to_le_bytes());
            inf.extend_from_slice(&[0u8; 6]);
        }
        let inf_chunk = chunk(CHUNK_VERTEX_INFLUENCES, &inf, false);
        let old = (u32::from_le_bytes([skin[4], skin[5], skin[6], skin[7]]) & 0x7fff_ffff) as usize;
        skin.extend_from_slice(&inf_chunk);
        skin[4..8].copy_from_slice(&((old + inf_chunk.len()) as u32 | 0x8000_0000).to_le_bytes());
        let sf = W3dFile::parse(&skin).unwrap();
        let mm = &build_mesh(sf.meshes()[0], Some(&ht), None, &Vfs::empty()).unwrap()[0];
        assert_eq!(mm.vertex_bones.as_deref(), Some(&[1u16, 1, 1][..]));
        assert_eq!(mm.bone, None);

        // Rigid mesh attached to bone 1.
        let rigid = mesh_chunk("M", "", 0);
        let rf = W3dFile::parse(&rigid).unwrap();
        let mm = &build_mesh(rf.meshes()[0], Some(&ht), Some(1), &Vfs::empty()).unwrap()[0];
        assert_eq!(mm.bone, Some(1));
        assert_eq!(mm.vertex_bones, None);

        // The viewer's `Hierarchy` (bind_pose_trs) must agree with the model baker.
        let hier = crate::hierarchy::Hierarchy::parse(hfile.hierarchy().unwrap()).unwrap();
        let b = hier.bind_pose();
        for i in 0..2 {
            for r in 0..3 {
                for c in 0..4 {
                    assert!((b[i][r][c] - ht[i][r][c]).abs() < 1e-4, "pivot {i} [{r}][{c}]");
                }
            }
        }
    }

    #[test]
    fn malformed_is_error_not_panic() {
        let vfs = Vfs::empty();
        assert!(matches!(load_model(&vfs, "nope"), Err(Error::NotFound(_))));
        let data = chunk(0, &[], true);
        let file = W3dFile::parse(&data).unwrap();
        assert!(build_mesh(file.meshes()[0], None, None, &vfs).is_err());
    }
}
