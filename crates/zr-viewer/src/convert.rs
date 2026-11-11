//! The only place plain-data structs from `source` become Bevy meshes / images / materials.
//!
//! Coordinate convention: Generals (x east, y north, z up) -> Bevy (x, z, -y). That is a
//! proper rotation, so triangle winding is preserved.

use std::sync::atomic::{AtomicU8, Ordering};

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use zr_assets::RgbaImage;

use crate::source::{ModelPart, TerrainData};

/// Generals-space point to Bevy space.
pub fn to_bevy(p: [f32; 3]) -> Vec3 {
    Vec3::new(p[0], p[2], -p[1])
}

/// Bevy-space point back to Generals space (inverse of [`to_bevy`]).
pub fn from_bevy(p: Vec3) -> [f32; 3] {
    [p.x, -p.z, p.y]
}

fn conv(v: &[[f32; 3]]) -> Vec<[f32; 3]> {
    v.iter().map(|&p| to_bevy(p).to_array()).collect()
}

/// Anisotropic trilinear filtering: the possession build's addition for the terrain atlas and the non-alpha-tested
/// model textures (`Visual fidelity` in POSSESSION.md). No engine texture family uses it; those take the engine's
/// `TextureFilter` mode ([`texture_sampler`]), which is also why this is possession-only.
#[cfg(feature = "possession")]
fn aniso_descriptor() -> ImageSamplerDescriptor {
    let mut d = ImageSamplerDescriptor::linear();
    d.set_anisotropic_filter(8);
    d
}

/// The engine's `TextureFilter` mode indices (`TextureFilterClass::TextureFilterMode`). The default is BILINEAR
/// (`GlobalData.cpp:943`).
pub const FILTER_NONE: u8 = 0;
pub const FILTER_POINT: u8 = 1;
pub const FILTER_BILINEAR: u8 = 2;
pub const FILTER_TRILINEAR: u8 = 3;
pub const FILTER_ANISOTROPIC: u8 = 4;

/// The `TextureFilter` and `AnisotropyLevel` options, from `options.ini` at launch (`OptionPreferences.cpp:83-99`,
/// applied by `GlobalData.cpp:1231` and `W3DDisplay.cpp:899`). Textures created after [`set_texture_filter`] use them.
static FILTER_MODE: AtomicU8 = AtomicU8::new(FILTER_BILINEAR);
static ANISOTROPY: AtomicU8 = AtomicU8::new(2);

/// Sets the engine's texture filter mode and anisotropy level (`WW3D::Set_Texture_Filter`,
/// `WW3D::Set_Anisotropy_Level`). The level is clamped to 2..16 as `OptionPreferences` clamps it.
pub fn set_texture_filter(mode: u8, anisotropy: u16) {
    FILTER_MODE.store(mode.min(FILTER_ANISOTROPIC), Ordering::Relaxed);
    ANISOTROPY.store(anisotropy.clamp(2, 16) as u8, Ordering::Relaxed);
}

fn filter_mode() -> u8 {
    FILTER_MODE.load(Ordering::Relaxed)
}

fn anisotropy() -> u8 {
    ANISOTROPY.load(Ordering::Relaxed)
}

/// The `TextureReduction` level: the number of times every file texture is halved. The engine sets it at launch
/// from the game detail level (`W3DGameClient::setTextureLOD` -> `WW3D::Set_Texture_Reduction(level, 32)`,
/// `W3DGameClient.cpp:199-206`, from `GameLOD.cpp:565-580` / `:633`), and the loader reads it back in
/// `Get_Requested_Reduction` (`textureloader.cpp:1304-1328`). Textures created after [`set_texture_reduction`]
/// use it.
static TEXTURE_REDUCTION: AtomicU8 = AtomicU8::new(0);

/// Sets the engine's texture reduction: 0 keeps file resolution, 1 halves a texture once, 2 twice (the option's
/// range, `OptionPreferences::getTextureReduction` `OptionPreferences.cpp:726-736`). Like the texture filter, a
/// change needs a restart: the viewer builds its images once and has no `WW3D::_Invalidate_Textures` re-upload.
pub fn set_texture_reduction(level: u8) {
    TEXTURE_REDUCTION.store(level.min(2), Ordering::Relaxed);
}

fn texture_reduction() -> u8 {
    TEXTURE_REDUCTION.load(Ordering::Relaxed)
}

/// The sampler of a texture's default filter under a `TextureFilter` mode (`TextureFilterClass::_Init_Filters`,
/// `texturefilter.cpp:125-274`, the BEST column that `FILTER_TYPE_DEFAULT` maps to). Min and mag follow the mode;
/// the mip level is picked POINT (nearest) for every mode but TRILINEAR and ANISOTROPIC, and NONE uses the top level
/// only. The POINT mip matters for alpha tests: a trilinear sample blends two levels, so the blended alpha of a
/// chain-link cut-out clears the 0x60 reference and the fence reads solid; a point sample tests the alpha of the one
/// level it picks.
fn mode_sampler(mode: u8, anisotropy: u8) -> ImageSamplerDescriptor {
    let mut d = ImageSamplerDescriptor::linear();
    match mode {
        FILTER_NONE => {
            d.min_filter = ImageFilterMode::Nearest;
            d.mag_filter = ImageFilterMode::Nearest;
            d.lod_max_clamp = 0.0;
        }
        FILTER_POINT => {
            d.min_filter = ImageFilterMode::Nearest;
            d.mag_filter = ImageFilterMode::Nearest;
            d.mipmap_filter = ImageFilterMode::Nearest;
        }
        FILTER_BILINEAR => d.mipmap_filter = ImageFilterMode::Nearest,
        FILTER_TRILINEAR => {}
        _ => {
            d.set_anisotropic_filter(u16::from(anisotropy));
        }
    }
    d
}

fn address(clamp: bool) -> ImageAddressMode {
    if clamp {
        ImageAddressMode::ClampToEdge
    } else {
        ImageAddressMode::Repeat
    }
}

/// A texture's engine surface family. Every family is a `TextureClass` and so takes the global `TextureFilter`
/// mode for min/mag (`WW3D::Set_Texture_Filter`, `texturefilter.cpp:270-275`); the families differ in the number
/// of mip levels their loader keeps, the address mode their draw code sets, and (scorch) in a filter override.
///
/// | family | C++ | filter | mips | address |
/// |---|---|---|---|---|
/// | [`Particle`](Self::Particle) | `W3DParticleSys.cpp:213` `Get_Texture(name)` | mode | all | repeat |
/// | [`Laser`](Self::Laser) | `W3DLaserDraw.cpp:124`, `W3DProjectileStreamDraw.cpp:94` | mode | all | repeat |
/// | [`Water`](Self::Water) | `W3DWater.cpp:1133`; `:218-220` sets `FILTER_TYPE_BEST` | mode | all | repeat |
/// | [`Radar`](Self::Radar) | `W3DRadar.cpp:910-928` `MIP_LEVELS_1`, shroud `FILTER_TYPE_DEFAULT` | mode | 1 | repeat |
/// | [`Shroud`](Self::Shroud) | `W3DShroud.cpp:241-257` `MIP_LEVELS_1`, min/mag `FILTER_TYPE_DEFAULT` (`:88`, `:683-686`) | mode | 1 | clamp |
/// | [`Scorch`](Self::Scorch) | `TerrainTex.cpp:1106-1140` `ScorchTextureClass::Apply` | terrain flags | 3 | clamp |
/// | [`DecalShadow`](Self::DecalShadow) | `W3DProjectedShadow.cpp:1516-1519` (also 1624-1626, 1761-1764, 1947-1950) | mode | none | clamp |
/// | [`Bridge`](Self::Bridge) | `W3DBridgeBuffer.cpp:248` `MIP_LEVELS_3` | mode | 3 | repeat |
/// | [`Ui`](Self::Ui) | `W3DDisplay.cpp:2798` `Get_Texture(filename, MIP_LEVELS_1)` | mode | 1 | repeat |
///
/// `W3DShadow` (the stencil-volume pass) loads no texture at all, so it has no row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureUse {
    /// A particle system texture (`W3DParticleSys` -> `PointGroupClass` / `SegLineRendererClass`), all levels.
    Particle,
    /// A weapon laser, projectile stream or `RayEffect` texture (`W3DLaserDraw`, `W3DProjectileStreamDraw`).
    Laser,
    /// The standing/river water texture (`WaterRenderObjClass::m_riverTexture`).
    Water,
    /// A radar texture: one level, so the mip filter is off (`TextureFilterClass` constructor for
    /// `MIP_LEVELS_1`, `texturefilter.cpp:74-81`).
    Radar,
    /// The fog-of-war shroud overlay (`W3DShroud::ReAcquireResources`): `MIP_LEVELS_1`, U/V CLAMP and
    /// `Set_Mip_Mapping(FILTER_TYPE_NONE)` (`W3DShroud.cpp:241-257`), min/mag the default filter the shroud asks
    /// for (`m_shroudFilter`, `:88`, applied at `:683-686`).
    Shroud,
    /// The scorch decal sheet (`ScorchTextureClass`). `Apply` (`TerrainTex.cpp:1106-1140`) overrides the global
    /// filter with `GameData`'s `BilinearTerrainTex` / `TrilinearTerrainTex` (both `Yes` in the shipped
    /// `Data\INI\gamedata.ini`), whose defaults are FALSE (`GlobalData.cpp:647-648`).
    Scorch { bilinear: bool, trilinear: bool },
    /// A projected decal shadow texture: clamp and `FILTER_TYPE_NONE` mip mapping, min/mag from the mode.
    DecalShadow,
    /// A bridge piece texture: `MIP_LEVELS_3`.
    Bridge,
    /// A shell/UI image (`W3DDisplay::drawImage`): one level, so the mip filter is off.
    Ui,
}

impl TextureUse {
    /// `MIP_LEVELS_1` (`TextureFilterClass` sets `MipMapFilter = FILTER_TYPE_NONE` for it).
    fn one_level(self) -> bool {
        matches!(self, TextureUse::Radar | TextureUse::Shroud | TextureUse::Ui)
    }

    /// The texture's `MipCountType`: `MIP_LEVELS_1` for the one-level families and the engine's
    /// `MIP_LEVELS_3` cap, `None` for `MIP_LEVELS_ALL` (`Apply_Mip_Reduction`, `textureloader.cpp:1480-1511`,
    /// takes as many levels as the file has).
    fn mip_cap(self) -> Option<u32> {
        match self {
            TextureUse::Radar | TextureUse::Shroud | TextureUse::Ui => Some(1),
            TextureUse::Scorch { .. } | TextureUse::Bridge => Some(3),
            _ => None,
        }
    }
}

/// The sampler a family's `TextureClass` ends up with.
pub fn texture_sampler(family: TextureUse) -> ImageSampler {
    let mut d = match family {
        // `ScorchTextureClass::Apply`: linear min/mag when either terrain flag is set, linear mip only for
        // trilinear; otherwise point/point/point.
        TextureUse::Scorch { bilinear, trilinear } => {
            let mut d = ImageSamplerDescriptor::linear();
            if !(bilinear || trilinear) {
                d.min_filter = ImageFilterMode::Nearest;
                d.mag_filter = ImageFilterMode::Nearest;
            }
            if !trilinear {
                d.mipmap_filter = ImageFilterMode::Nearest;
            }
            d
        }
        _ => mode_sampler(filter_mode(), anisotropy()),
    };
    match family {
        // `Set_U/V_Addr_Mode(TEXTURE_ADDRESS_CLAMP)`: the decal shadows, the shroud overlay
        // (`W3DShroud.cpp:255-256`) and `ScorchTextureClass::Apply` (`TerrainTex.cpp:1125-1126`).
        TextureUse::Scorch { .. } | TextureUse::DecalShadow | TextureUse::Shroud => {
            d.address_mode_u = ImageAddressMode::ClampToEdge;
            d.address_mode_v = ImageAddressMode::ClampToEdge;
        }
        // Every other family passes a `TextureFilterClass` whose constructor defaults to
        // `TEXTURE_ADDRESS_REPEAT` (`texturefilter.cpp:71-72`).
        _ => {
            d.address_mode_u = ImageAddressMode::Repeat;
            d.address_mode_v = ImageAddressMode::Repeat;
        }
    }
    // A decal shadow's `Set_Mip_Mapping(FILTER_TYPE_NONE)` and a one-level texture (`MIP_LEVELS_1`) both mean
    // "sample level 0 only": D3D has no level to filter.
    if matches!(family, TextureUse::DecalShadow) || family.one_level() {
        d.mipmap_filter = ImageFilterMode::Nearest;
        d.lod_max_clamp = 0.0;
    }
    ImageSampler::Descriptor(d)
}

/// A family texture as the engine uploads it: the levels a DDS file has, a generated chain for a TGA
/// (`Load_Uncompressed_Mipmap`'s mip generation, `textureloader.cpp:1718-1846`; every texture these families
/// reference in `Data\INI` is a TGA and none is a DDS), and the family's `MipCountType`. `TextureReduction` applies
/// to a DDS only (`Begin_Uncompressed_Load` resets `Reduction = 0`, `textureloader.cpp:1602-1603`), so a TGA family
/// texture keeps full resolution; see [`reduce_file`].
pub fn texture_image(img: &RgbaImage, family: TextureUse) -> Image {
    let requested = family.mip_cap();
    let image = if requested == Some(1) {
        // `MIP_LEVELS_1`: the loader asks D3D for a single level, so no chain is built.
        level_zero(img)
    } else if img.mips.is_empty() {
        generated_levels(img)
    } else {
        authored_levels(img)
    };
    let image = keep_levels(image, requested);
    let mut image = reduce_file(image, requested, !img.mips.is_empty(), texture_reduction());
    image.sampler = texture_sampler(family);
    image
}

/// Which engine `TextureClass` a W3D *file* texture belongs to, for its sampler: a model/texture surface (the
/// possession aniso addition) or the road buffer's own texture (not covered by the addition, so it keeps the plain
/// build's sampler; POSSESSION.md "Visual fidelity").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum W3dUse {
    /// A model part texture. `alpha_test` keeps the engine mode sampler in the possession build: a trilinear
    /// sample blends two levels, so the blended alpha of a cut-out would clear the alpha test.
    Model { alpha_test: bool },
    /// A road texture (`RoadType::loadTexture`'s `TextureClass`, `W3DRoadBuffer.cpp:177-184`).
    Road,
}

/// The sampler of a W3D texture (`TextureClass` filter and wrap, `texture.cpp` `Load_Texture`): the engine's
/// `TextureFilter` mode (`WW3D::Set_Texture_Filter`) and the texture's wrap: repeat unless `W3DTEXTURE_CLAMP_U/V`
/// (`texturefilter.cpp` constructor: `TEXTURE_ADDRESS_REPEAT`; `Apply` `D3DTADDRESS_WRAP`). [`W3dUse::Model`]
/// textures that do not alpha-test take the possession build's addition ([`sampler_for`]).
pub fn w3d_sampler(params: zr_w3d::TextureParams, use_: W3dUse) -> ImageSampler {
    let mut d = match use_ {
        W3dUse::Model { alpha_test } => sampler_for(alpha_test),
        // The road buffer is not part of the possession aniso addition, so it takes the engine mode in both builds.
        W3dUse::Road => mode_sampler(filter_mode(), anisotropy()),
    };
    d.address_mode_u = address(params.clamp_u);
    d.address_mode_v = address(params.clamp_v);
    ImageSampler::Descriptor(d)
}

/// The plain build: the engine's filter for every texture; `alpha_test` changes nothing.
#[cfg(not(feature = "possession"))]
fn sampler_for(_alpha_test: bool) -> ImageSamplerDescriptor {
    mode_sampler(filter_mode(), anisotropy())
}

/// The possession build's addition (POSSESSION.md "Visual fidelity"): anisotropic trilinear for textures that do not
/// alpha-test. Alpha-tested textures keep the engine's mode sampler, as the plain build does (see [`mode_sampler`]).
#[cfg(feature = "possession")]
fn sampler_for(alpha_test: bool) -> ImageSamplerDescriptor {
    if alpha_test {
        mode_sampler(filter_mode(), anisotropy())
    } else {
        aniso_descriptor()
    }
}

/// The terrain atlas sampler: the engine's terrain takes the default filter (`FILTER_TYPE_DEFAULT`, no override in
/// `TerrainTex.cpp` / `BaseHeightMap.cpp`); the tiles share one image, so it clamps. The possession build keeps the
/// anisotropic addition. `TextureReduction` reaches the terrain as D3D's `SetLOD`
/// (`BaseHeightMapRenderObjClass::setTextureLOD` -> `WorldHeightMap::setTextureLOD` -> `TerrainTextureClass::setLOD`,
/// `TerrainTex.cpp:365-368`): the sampler starts at that level instead of dropping uploaded levels.
pub fn terrain_sampler() -> ImageSampler {
    terrain_sampler_at(texture_reduction())
}

#[cfg(feature = "possession")]
fn terrain_descriptor() -> ImageSamplerDescriptor {
    aniso_descriptor()
}

#[cfg(not(feature = "possession"))]
fn terrain_descriptor() -> ImageSamplerDescriptor {
    mode_sampler(filter_mode(), anisotropy())
}

fn terrain_sampler_at(reduction: u8) -> ImageSampler {
    let mut d = terrain_descriptor();
    d.address_mode_u = ImageAddressMode::ClampToEdge;
    d.address_mode_v = ImageAddressMode::ClampToEdge;
    // `SetLOD` keeps the sampler in the range it already has: a `FILTER_NONE` mode (`lod_max_clamp` 0) samples the
    // top level and has no coarser level to start from.
    d.lod_min_clamp = f32::from(reduction).min(d.lod_max_clamp);
    ImageSampler::Descriptor(d)
}

/// Append a full mip chain (box-filtered 2x2 averages) so the anisotropic sampler has mip levels
/// to use. Bevy does not generate mipmaps for procedurally built images.
pub fn with_mipmaps(image: Image) -> Image {
    with_mipmaps_capped(image, None)
}

/// [`with_mipmaps`] but stops after `levels` levels when given (`None` = down to 1x1). The engine's
/// `D3DXFilterTexture(.., D3DX_FILTER_BOX)` only builds the levels its `MipCountType` asked for.
fn with_mipmaps_capped(mut image: Image, levels: Option<u32>) -> Image {
    let Some(src) = image.data.clone() else { return image };
    let (w, h) = (image.texture_descriptor.size.width, image.texture_descriptor.size.height);
    if w <= 1 || h <= 1 || src.len() < (w as usize * h as usize * 4) {
        return image;
    }
    let mut out = src.clone();
    let mut prev = src;
    let (mut cw, mut ch) = (w, h);
    let mut count = 1u32;
    while (cw > 1 || ch > 1) && levels.is_none_or(|n| count < n) {
        let nw = (cw / 2).max(1);
        let nh = (ch / 2).max(1);
        let mut next = vec![0u8; nw as usize * nh as usize * 4];
        for y in 0..nh {
            for x in 0..nw {
                let mut acc = [0u32; 4];
                let mut n = 0u32;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (sx, sy) = (x * 2 + dx, y * 2 + dy);
                        if sx >= cw || sy >= ch {
                            continue;
                        }
                        let i = ((sy * cw + sx) * 4) as usize;
                        for c in 0..4 {
                            acc[c] += u32::from(prev[i + c]);
                        }
                        n += 1;
                    }
                }
                let o = ((y * nw + x) * 4) as usize;
                for c in 0..4 {
                    next[o + c] = (acc[c] / n) as u8;
                }
            }
        }
        out.extend_from_slice(&next);
        prev = next;
        cw = nw;
        ch = nh;
        count += 1;
    }
    image.data = Some(out);
    image.texture_descriptor.mip_level_count = count;
    image
}

/// A W3D texture as the engine uploads it (`textureloader.cpp`). A DDS keeps the levels its file has, after the two
/// lowest are dropped (`ddsfile.cpp:107-109`, already applied by `zr_w3d::dds::decode`); a DDS with no levels left
/// uploads one level. A TGA gets a generated chain. Then the texture's `MipCountType` caps that chain
/// (`texture.cpp` `MipCountType`, `Apply_Mip_Reduction`). `TextureReduction` applies to a DDS only: TheSuperHackers'
/// `Begin_Uncompressed_Load` resets `Reduction = 0` (`textureloader.cpp:1602-1603`), so TGA files upload at full
/// resolution; the DDS path takes `Get_Requested_Reduction` / `Apply_Dim_Reduction` / `Apply_Mip_Reduction`
/// (`:1304-1328`, `:1536-1543`). `authored` is true for a DDS file; `use_` picks the sampler ([`w3d_sampler`]).
pub fn w3d_image(img: &RgbaImage, params: zr_w3d::TextureParams, authored: bool, use_: W3dUse) -> Image {
    let image = if authored { authored_levels(img) } else { generated_levels(img) };
    let image = keep_levels(image, params.levels);
    let mut image = reduce_file(image, params.levels, authored, texture_reduction());
    image.sampler = w3d_sampler(params, use_);
    image
}

/// The file's own levels: `RgbaImage::mips` after the top. An empty chain is one level, which is what the engine
/// has for a DDS whose file keeps no extra levels. A chain that does not halve cleanly is generated instead.
fn authored_levels(img: &RgbaImage) -> Image {
    if img.mips.is_empty() {
        return level_zero(img);
    }
    let mut w = img.width;
    let mut h = img.height;
    let mut consistent = true;
    for m in &img.mips {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        consistent &= m.width == w && m.height == h && m.pixels.len() == w as usize * h as usize * 4;
    }
    if !consistent {
        return generated_levels(img);
    }
    // `Image::new` checks the length against the top level alone, so the chain is set after construction (as
    // `with_mipmaps` does).
    let mut data = img.pixels.clone();
    for m in &img.mips {
        data.extend_from_slice(&m.pixels);
    }
    let mut image = level_zero(img);
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = 1 + img.mips.len() as u32;
    image
}

/// The top level alone, no mip chain.
fn level_zero(img: &RgbaImage) -> Image {
    Image::new(
        Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        img.pixels.clone(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// The top level with a box-filtered chain down to 1x1 ([`with_mipmaps`]). No sampler set.
fn generated_levels(img: &RgbaImage) -> Image {
    with_mipmaps(level_zero(img))
}

/// The first `keep` levels of `image` (`Apply_Mip_Reduction`: a texture's mip count caps the file's chain). `None`
/// keeps all of them. Levels are stored largest first, each `max(1, w >> i) x max(1, h >> i)`.
fn keep_levels(mut image: Image, keep: Option<u32>) -> Image {
    let Some(keep) = keep else { return image };
    let total = image.texture_descriptor.mip_level_count.max(1);
    let keep = keep.clamp(1, total);
    if keep >= total {
        return image;
    }
    let (w, h) = (image.texture_descriptor.size.width, image.texture_descriptor.size.height);
    let bytes: usize = (0..keep).map(|i| ((w >> i).max(1) * (h >> i).max(1)) as usize * 4).sum();
    if let Some(data) = image.data.as_mut() {
        data.truncate(bytes);
    }
    image.texture_descriptor.mip_level_count = keep;
    image
}

/// The requested reduction of a file texture: `Get_Requested_Reduction` (`textureloader.cpp:1304-1328`) and then
/// `Validate_Reduction` (`textureloader.cpp:1407-1422`). `level` is the launch's `TextureReduction`;
/// `W3DGameClient::setTextureLOD` passes `Set_Texture_Reduction(level, 32)` (`W3DGameClient.cpp:204`), so a texture
/// with a side of 32 or less is not reduced. `file_mips` is the file's level count and `requested` the texture's
/// `MipCountType` (`None` = `MIP_LEVELS_ALL`).
fn requested_reduction(width: u32, height: u32, file_mips: u32, requested: Option<u32>, level: u8) -> u32 {
    let mut reduction = u32::from(level);
    // `reqReduction >= max(mip_count, 1)` leaves only the lowest level.
    if reduction >= file_mips {
        reduction = file_mips.saturating_sub(1);
    }
    // Keep the texture above the minimum dimension (32 from `Set_Texture_Reduction`).
    let (mut w, mut h, mut cur) = (width, height, 0);
    while cur < reduction && w > 32 && h > 32 {
        w >>= 1;
        h >>= 1;
        cur += 1;
    }
    reduction = cur;
    // `Validate_Reduction`: a `MIP_LEVELS_1` texture is never reduced, and never drop every requested level.
    if requested == Some(1) {
        return 0;
    }
    if let Some(n) = requested {
        if reduction >= n {
            reduction = n - 1;
        }
    }
    if reduction >= file_mips {
        reduction = 0;
    }
    reduction
}

/// Upload `reduction` levels coarser: the engine halves the dimensions and drops that many mip levels
/// (`Apply_Dim_Reduction`'s `width >>= reduction`; `Apply_Mip_Reduction`'s `mip_level_count -= reduction`).
fn drop_top_levels(mut image: Image, reduction: u32) -> Image {
    let total = image.texture_descriptor.mip_level_count.max(1);
    let reduction = reduction.min(total - 1);
    if reduction == 0 {
        return image;
    }
    let (w, h) = (image.texture_descriptor.size.width, image.texture_descriptor.size.height);
    let offset: usize = (0..reduction).map(|i| ((w >> i).max(1) * (h >> i).max(1)) as usize * 4).sum();
    let Some(mut data) = image.data.take() else { return image };
    data.drain(..offset.min(data.len()));
    image.data = Some(data);
    let depth_or_array_layers = image.texture_descriptor.size.depth_or_array_layers;
    image.texture_descriptor.size = Extent3d { width: (w >> reduction).max(1), height: (h >> reduction).max(1), depth_or_array_layers };
    image.texture_descriptor.mip_level_count = total - reduction;
    image
}

/// A file texture after the launch's `TextureReduction`, for a compressed (DDS) file only:
/// `Begin_Uncompressed_Load` sets `Reduction = 0` (`textureloader.cpp:1602-1603`), so a TGA is uploaded at full
/// resolution in TheSuperHackers. (`texture_image` treats an image with no authored levels as the TGA case; a
/// single-level DDS is not reducible anyway, `Get_Requested_Reduction` leaves the last level,
/// `textureloader.cpp:1310-1311`.) `requested` is the texture's `MipCountType`, see [`requested_reduction`].
fn reduce_file(image: Image, requested: Option<u32>, compressed: bool, level: u8) -> Image {
    if !compressed {
        return image;
    }
    reduced(image, requested, level)
}

/// A file texture after the launch's `TextureReduction` (`requested` is its `MipCountType`, see
/// [`requested_reduction`]).
fn reduced(image: Image, requested: Option<u32>, level: u8) -> Image {
    let total = image.texture_descriptor.mip_level_count.max(1);
    let (w, h) = (image.texture_descriptor.size.width, image.texture_descriptor.size.height);
    let reduction = requested_reduction(w, h, total, requested, level);
    drop_top_levels(image, reduction)
}

/// A road texture (`RoadType::loadTexture`, `W3DRoadBuffer.cpp:177-184`): `MIP_LEVELS_3`, REPEAT (the road UVs run
/// past 1: `zr_roads` gives u up to 6.7 on Alpine). Roads are TGA here, so the chain is generated, and they keep
/// the plain-build sampler in the possession build too ([`W3dUse::Road`]).
pub fn road_image(img: &RgbaImage) -> Image {
    w3d_image(img, zr_w3d::TextureParams { levels: Some(3), ..Default::default() }, false, W3dUse::Road)
}

/// The terrain atlas as an image with its engine mip chain and [`terrain_sampler`]. The engine's atlas is a
/// `TerrainTextureClass` created with `MIP_LEVELS_3` (`TerrainTex.cpp:70-74`): three levels, the top one filled by
/// `update` and the rest box-filtered by `D3DXFilterTexture(.., D3DX_FILTER_BOX)` (`:198`). Three levels are what
/// the 4-texel block borders (`TILE_OFFSET`, `TerrainTex.h:38`) are sized for, and deeper levels would average
/// across unrelated blocks. The atlas is procedural, so `TextureReduction` reaches it as `SetLOD` on the sampler
/// rather than as dropped levels ([`terrain_sampler`]).
pub fn terrain_image(img: &RgbaImage) -> Image {
    let mut image = with_mipmaps_capped(level_zero(img), Some(TERRAIN_MIP_LEVELS));
    image.sampler = terrain_sampler();
    image
}

/// `MIP_LEVELS_3` (`TerrainTex.cpp:70-74`).
const TERRAIN_MIP_LEVELS: u32 = 3;

/// A bridge piece texture (`W3DBridgeBuffer.cpp:248`: `Get_Texture(textureFile, MIP_LEVELS_3)`, the default
/// `TextureFilterClass` filter and wrap).
pub fn bridge_image(img: &RgbaImage) -> Image {
    texture_image(img, TextureUse::Bridge)
}

/// A grayscale copy of a decoded image, for the engine's disabled-button draw
/// (`W3DGadgetPushButtonImageDrawOne` `DRAW_IMAGE_GRAYSCALE`; the weights are the engine's 2D
/// grayscale constants, `W3DShaderManager.cpp:428` `0.3, 0.59, 0.11`). It is a UI image, so it keeps the UI
/// sampler and its single level.
pub fn gray_image(img: &RgbaImage) -> Image {
    let mut pixels = img.pixels.clone();
    for c in pixels.chunks_exact_mut(4) {
        let l = (0.3 * c[0] as f32 + 0.59 * c[1] as f32 + 0.11 * c[2] as f32).round().clamp(0.0, 255.0) as u8;
        c[0] = l;
        c[1] = l;
        c[2] = l;
    }
    let mut image = Image::new(
        Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = texture_sampler(TextureUse::Ui);
    image
}

fn mesh(positions: &[[f32; 3]], normals: &[[f32; 3]], uvs: &[[f32; 2]], indices: &[u32]) -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, conv(positions))
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, conv(normals))
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs.to_vec())
        .with_inserted_indices(Indices::U32(indices.to_vec()))
}

/// The terrain's render pieces: the two-layer terrain mesh, and the separate
/// `renderExtraBlendTiles` pass when the map has cells with a third layer.
pub struct TerrainPieces {
    pub mesh: Mesh,
    pub material: crate::terrain::TerrainMaterial,
    pub extra: Option<(Mesh, crate::terrain::TerrainMaterial)>,
}

pub fn terrain(t: &TerrainData, images: &mut Assets<Image>, shadow_color: Vec4) -> TerrainPieces {
    let texture = t.atlas.as_ref().map(|a| images.add(terrain_image(a)));
    let mut m = mesh(&t.positions, &t.normals, &t.uvs, &t.indices);
    if let Some(c) = &t.colors {
        m.insert_attribute(Mesh::ATTRIBUTE_COLOR, c.clone());
    }
    // The engine's second UV set (`u2`,`v2`, `getAlphaUVData`); the shader samples the blend
    // layer through it and takes the vertex alpha from the colour attribute.
    if !t.blend_uvs.is_empty() {
        m.insert_attribute(Mesh::ATTRIBUTE_UV_1, t.blend_uvs.clone());
    }
    let material = crate::terrain::TerrainMaterial {
        base_color: Vec4::ONE,
        base_color_texture: texture.clone(),
        shadow_color,
        alpha_mode: AlphaMode::Opaque,
        depth_bias: 0.0,
    };
    let extra = t.extra.as_ref().map(|e| {
        let mut em = mesh(&e.positions, &e.normals, &e.uvs, &e.indices);
        if e.colors.len() == e.positions.len() {
            em.insert_attribute(Mesh::ATTRIBUTE_COLOR, e.colors.clone());
        }
        // The cell's three layers are composited into its atlas block, so the quad is opaque
        // terrain. The engine draws this pass inside the terrain render (`HeightMap.cpp:2076`,
        // before roads and props); the bias beats the coplanar two-layer terrain quad.
        let mat = crate::terrain::TerrainMaterial {
            base_color: Vec4::ONE,
            base_color_texture: texture.clone(),
            shadow_color,
            alpha_mode: AlphaMode::Opaque,
            depth_bias: 4.0,
        };
        (em, mat)
    });
    TerrainPieces { mesh: m, material, extra }
}

/// `pivots`: when `Some(n)`, the mesh gets Bevy skin attributes (one unit-weight
/// joint per vertex) so a `SkinnedMesh` can drive it; parts bound outside `n`
/// pivots are left unskinned. `None` keeps a plain mesh.
pub fn model_part(p: &ModelPart, images: &mut Assets<Image>, pivots: Option<usize>) -> (Mesh, StandardMaterial) {
    let mut m = mesh(&p.positions, &p.normals, &p.uvs, &p.indices);
    // W3D has one bone per vertex (no weights), so skins are a unit-weight joint
    // index per vertex; Bevy's `SkinnedMesh` applies `joint * inverse_bind`.
    let n = p.positions.len();
    if let Some(pivots) = pivots {
        if crate::anim::part_bound(p, pivots) {
            let index: Vec<[u16; 4]> = match &p.vertex_bones {
                Some(vb) => vb.iter().map(|&b| [b, 0, 0, 0]).collect(),
                None => vec![[p.bone.unwrap_or(0) as u16, 0, 0, 0]; n],
            };
            m.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(index));
            m.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, VertexAttributeValues::Float32x4(vec![[1.0f32, 0.0, 0.0, 0.0]; n]));
        }
    }
    let mat = StandardMaterial {
        base_color: Color::linear_rgba(p.color[0], p.color[1], p.color[2], p.color[3]),
        base_color_texture: p.texture.as_ref().map(|a| {
            // A DDS texture keeps the levels its file has; `texture_name` is the resolved VFS path.
            let authored = p.texture_name.as_deref().is_some_and(|n| n.to_ascii_lowercase().ends_with(".dds"));
            images.add(w3d_image(
                a,
                p.texture_params,
                authored,
                W3dUse::Model { alpha_test: p.blend == zr_w3d::BlendMode::AlphaTest },
            ))
        }),
        // Ported from ShaderClass::Apply: alpha test (ref 0x60) cuts out; other blends composite.
        alpha_mode: match p.blend {
            zr_w3d::BlendMode::Opaque => AlphaMode::Opaque,
            zr_w3d::BlendMode::AlphaTest => AlphaMode::Mask(zr_w3d::ALPHA_TEST_REF),
            zr_w3d::BlendMode::AlphaBlend => AlphaMode::Blend,
            zr_w3d::BlendMode::Additive => AlphaMode::Add,
            zr_w3d::BlendMode::Multiply => AlphaMode::Multiply,
        },
        cull_mode: if p.two_sided { None } else { Some(bevy::render::render_resource::Face::Back) },
        double_sided: p.two_sided,
        perceptual_roughness: 0.9,
        ..default()
    };
    (m, mat)
}

/// Bevy transform for a map object: Generals position + yaw about Z (CCW) = Bevy rotation about +Y.
pub fn object_transform(pos: [f32; 3], angle: f32) -> Transform {
    Transform::from_translation(to_bevy(pos)).with_rotation(Quat::from_rotation_y(angle))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `TerrainData` with the atlas path's fields, as `source::terrain` returns them.
    fn terrain_data(blend_uvs: Vec<[f32; 2]>, extra: bool) -> TerrainData {
        let n = 4;
        TerrainData {
            positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; n],
            uvs: vec![[0.0, 0.0]; n],
            blend_uvs,
            blend_alpha: vec![255; n],
            extra: extra.then(|| crate::source::TerrainExtra {
                positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; n],
                uvs: vec![[0.0, 0.0]; n],
                indices: vec![0, 2, 3, 0, 1, 2],
                colors: vec![[1.0, 1.0, 1.0, 1.0]; n],
            }),
            colors: Some(vec![[0.5, 0.5, 0.5, 1.0]; n]),
            indices: vec![0, 2, 3, 0, 1, 2],
            atlas: Some(RgbaImage::new(2048, 1024)),
        }
    }

    #[test]
    fn the_terrain_mesh_carries_the_second_uv_set_only_when_the_atlas_has_a_blend_layer() {
        let mut images = Assets::<Image>::default();
        let p = terrain(&terrain_data(vec![[0.0, 0.0]; 4], false), &mut images, Vec4::ONE);
        assert!(p.mesh.attribute(Mesh::ATTRIBUTE_UV_1).is_some());
        assert!(p.mesh.attribute(Mesh::ATTRIBUTE_COLOR).is_some());
        assert!(p.extra.is_none());
        // The heightfield fallback has no class atlas: one layer, no second UV set.
        let mut fallback = terrain_data(Vec::new(), false);
        fallback.atlas = None;
        fallback.colors = Some(vec![[0.5, 0.5, 0.5, 1.0]; 4]);
        let p = terrain(&fallback, &mut images, Vec4::ONE);
        assert!(p.mesh.attribute(Mesh::ATTRIBUTE_UV_1).is_none());
    }

    #[test]
    fn the_extra_blend_pass_is_a_separate_opaque_terrain_mesh() {
        let mut images = Assets::<Image>::default();
        let p = terrain(&terrain_data(vec![[0.0, 0.0]; 4], true), &mut images, Vec4::ONE);
        let (mesh, material) = p.extra.expect("an extra blend layer must get its own mesh");
        assert_eq!(mesh.count_vertices(), 4);
        assert!(mesh.attribute(Mesh::ATTRIBUTE_COLOR).is_some());
        assert!(mesh.attribute(Mesh::ATTRIBUTE_UV_1).is_none());
        // The three layers are already composited into the cell's atlas block, so this pass is
        // opaque terrain (the engine draws it inside the terrain render, before roads and props).
        assert!(matches!(material.alpha_mode, AlphaMode::Opaque));
        assert!(matches!(p.material.alpha_mode, AlphaMode::Opaque));
        // Both passes sample the same atlas (one upload).
        assert_eq!(p.material.base_color_texture, material.base_color_texture);
    }

    fn descriptor(s: ImageSampler) -> ImageSamplerDescriptor {
        match s {
            ImageSampler::Descriptor(d) => d,
            other => panic!("expected a descriptor sampler, got {other:?}"),
        }
    }

    /// A `w x h` top level with the given authored levels (each `w >> i x h >> i`), as a DDS decode gives them.
    fn authored(w: u32, h: u32, levels: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        img.mips = (1..levels).map(|i| RgbaImage::new((w >> i).max(1), (h >> i).max(1))).collect();
        img
    }

    fn bytes(image: &Image) -> usize {
        image.data.as_ref().map_or(0, |d| d.len())
    }

    fn levels(image: &Image) -> u32 {
        image.texture_descriptor.mip_level_count
    }

    fn image_size(image: &Image) -> (u32, u32) {
        (image.texture_descriptor.size.width, image.texture_descriptor.size.height)
    }

    #[test]
    fn the_engine_default_filter_is_bilinear_with_point_mips() {
        // `_Init_Filters` (texturefilter.cpp:125-274): BILINEAR min/mag linear, mip POINT, no anisotropy.
        let d = mode_sampler(FILTER_BILINEAR, 8);
        assert_eq!((d.min_filter, d.mag_filter, d.mipmap_filter), (ImageFilterMode::Linear, ImageFilterMode::Linear, ImageFilterMode::Nearest));
        assert_eq!(d.anisotropy_clamp, 1);
        let p = mode_sampler(FILTER_POINT, 8);
        assert_eq!((p.min_filter, p.mag_filter, p.mipmap_filter), (ImageFilterMode::Nearest, ImageFilterMode::Nearest, ImageFilterMode::Nearest));
        let t = mode_sampler(FILTER_TRILINEAR, 8);
        assert_eq!(t.mipmap_filter, ImageFilterMode::Linear);
        assert_eq!(t.anisotropy_clamp, 1);
        let a = mode_sampler(FILTER_ANISOTROPIC, 8);
        assert_eq!((a.anisotropy_clamp, a.mipmap_filter), (8, ImageFilterMode::Linear));
        assert_eq!(mode_sampler(FILTER_NONE, 8).lod_max_clamp, 0.0, "NONE samples the top level only");
    }

    #[test]
    fn w3d_textures_repeat_unless_the_texture_clamps() {
        // `TEXTURE_ADDRESS_REPEAT` is the default (texturefilter.cpp constructor); `W3DTEXTURE_CLAMP_U/V` clamp.
        let repeat = descriptor(w3d_sampler(zr_w3d::TextureParams::default(), W3dUse::Road));
        assert_eq!((repeat.address_mode_u, repeat.address_mode_v), (ImageAddressMode::Repeat, ImageAddressMode::Repeat));
        let u = zr_w3d::TextureParams { clamp_u: true, ..Default::default() };
        let d = descriptor(w3d_sampler(u, W3dUse::Road));
        assert_eq!((d.address_mode_u, d.address_mode_v), (ImageAddressMode::ClampToEdge, ImageAddressMode::Repeat));
    }

    #[test]
    fn the_plain_build_samples_alpha_tests_like_every_other_texture() {
        // The engine's filter is global (`WW3D::Set_Texture_Filter`); nothing splits by alpha test.
        #[cfg(not(feature = "possession"))]
        assert_eq!(
            w3d_sampler(zr_w3d::TextureParams::default(), W3dUse::Model { alpha_test: true }),
            w3d_sampler(zr_w3d::TextureParams::default(), W3dUse::Model { alpha_test: false })
        );
    }

    #[cfg(feature = "possession")]
    #[test]
    fn possession_routes_alpha_tests_to_the_point_mip_sampler() {
        // The possession addition (anisotropic trilinear) skips alpha-tested textures, which keep the mode sampler.
        let alpha = descriptor(w3d_sampler(zr_w3d::TextureParams::default(), W3dUse::Model { alpha_test: true }));
        assert_eq!((alpha.mipmap_filter, alpha.anisotropy_clamp), (ImageFilterMode::Nearest, 1));
        let other = descriptor(w3d_sampler(zr_w3d::TextureParams::default(), W3dUse::Model { alpha_test: false }));
        assert_eq!((other.mipmap_filter, other.anisotropy_clamp), (ImageFilterMode::Linear, 8));
    }

    #[test]
    fn roads_keep_the_engine_sampler_in_both_builds() {
        // The possession aniso addition covers the terrain atlas and non-alpha-tested model textures only
        // (POSSESSION.md "Visual fidelity"), so `W3DRoadBuffer`'s texture stays on the `TextureFilter` mode.
        let d = descriptor(road_image(&RgbaImage::new(64, 64)).sampler);
        assert_eq!(
            (d.min_filter, d.mag_filter, d.mipmap_filter, d.anisotropy_clamp),
            (ImageFilterMode::Linear, ImageFilterMode::Linear, ImageFilterMode::Nearest, 1)
        );
        assert_eq!((d.address_mode_u, d.address_mode_v), (ImageAddressMode::Repeat, ImageAddressMode::Repeat));
    }

    #[test]
    fn an_authored_chain_uploads_the_files_levels() {
        // 8x8 with levels 4, 2, 1: 64 + 16 + 4 + 1 texels.
        let image = authored_levels(&authored(8, 8, 4));
        assert_eq!(levels(&image), 4);
        assert_eq!(bytes(&image), (64 + 16 + 4 + 1) * 4);
    }

    #[test]
    fn a_dds_with_no_extra_levels_uploads_one_level() {
        // The empty-chain case: `dds::decode` keeps no extra levels for a file of three levels or fewer.
        let image = w3d_image(&authored(4, 4, 1), zr_w3d::TextureParams::default(), true, W3dUse::Road);
        assert_eq!(levels(&image), 1);
        assert_eq!(bytes(&image), 16 * 4);
    }

    #[test]
    fn a_tga_gets_a_generated_chain_capped_by_the_mip_count() {
        // 16x16 generated: 16, 8, 4, 2, 1 (five levels). Capped at four: 256 + 64 + 16 + 4 texels.
        let generated = w3d_image(&RgbaImage::new(16, 16), zr_w3d::TextureParams::default(), false, W3dUse::Road);
        assert_eq!(levels(&generated), 5);
        let capped = w3d_image(&RgbaImage::new(16, 16), zr_w3d::TextureParams { levels: Some(4), ..Default::default() }, false, W3dUse::Road);
        assert_eq!(levels(&capped), 4);
        assert_eq!(bytes(&capped), (256 + 64 + 16 + 4) * 4);
    }

    #[test]
    fn non_square_chains_halve_with_a_floor_of_one() {
        // 8x2 generated: 8x2, 4x1, 2x1, 1x1 (23 texels). Capped at two: 16 + 4 texels.
        let full = w3d_image(&RgbaImage::new(8, 2), zr_w3d::TextureParams::default(), false, W3dUse::Road);
        assert_eq!(levels(&full), 4);
        assert_eq!(bytes(&full), (16 + 4 + 2 + 1) * 4);
        let two = w3d_image(&RgbaImage::new(8, 2), zr_w3d::TextureParams { levels: Some(2), ..Default::default() }, false, W3dUse::Road);
        assert_eq!((levels(&two), bytes(&two)), (2, (16 + 4) * 4));
        // An authored non-square chain: 8x2 with 4x1, 2x1, 1x1.
        let authored_ns = authored_levels(&authored(8, 2, 4));
        assert_eq!(levels(&authored_ns), 4);
        assert_eq!(bytes(&authored_ns), (16 + 4 + 2 + 1) * 4);
    }

    #[test]
    fn a_cap_above_the_chain_keeps_the_chain() {
        // A file with three levels under a cap of four: three levels.
        let image = w3d_image(&authored(4, 4, 3), zr_w3d::TextureParams { levels: Some(4), ..Default::default() }, true, W3dUse::Road);
        assert_eq!(levels(&image), 3);
    }

    #[test]
    fn a_chain_that_does_not_halve_falls_back_to_generated() {
        // A 3x3 level under a 4x4 top is malformed: the 4x4 is generated (4, 2, 1).
        let mut img = RgbaImage::new(4, 4);
        img.mips = vec![RgbaImage::new(3, 3)];
        let image = authored_levels(&img);
        assert_eq!(levels(&image), 3);
    }

    #[test]
    fn keep_levels_is_the_identity_without_a_cap_and_at_or_above_the_chain() {
        let image = w3d_image(&authored(8, 8, 4), zr_w3d::TextureParams::default(), true, W3dUse::Road);
        assert_eq!(levels(&image), 4);
        assert_eq!(bytes(&image), (64 + 16 + 4 + 1) * 4);
        assert_eq!(levels(&keep_levels(image.clone(), Some(0))), 1, "a cap of zero is one level");
    }

    /// Every family's sampler, from the C++ each row of [`TextureUse`] cites. The global mode is the engine's
    /// default BILINEAR (`GlobalData.cpp:943`), so min/mag are linear, the mip level is picked POINT and nothing
    /// is anisotropic -- in the plain build and in the possession build (the possession aniso addition covers the
    /// terrain atlas and model textures only, POSSESSION.md "Visual fidelity").
    #[test]
    fn every_family_samples_like_its_engine_texture_class() {
        let families = [
            TextureUse::Particle,
            TextureUse::Laser,
            TextureUse::Water,
            TextureUse::Bridge,
            TextureUse::Radar,
            TextureUse::Shroud,
            TextureUse::Ui,
            TextureUse::DecalShadow,
        ];
        for family in families {
            let d = descriptor(texture_sampler(family));
            assert_eq!(
                (d.min_filter, d.mag_filter, d.anisotropy_clamp),
                (ImageFilterMode::Linear, ImageFilterMode::Linear, 1),
                "{family:?} takes the mode's min/mag with no anisotropy"
            );
        }
        // `TextureFilterClass` defaults U/V to `TEXTURE_ADDRESS_REPEAT` (`texturefilter.cpp:71-72`).
        for family in [TextureUse::Particle, TextureUse::Laser, TextureUse::Water, TextureUse::Bridge, TextureUse::Radar, TextureUse::Ui] {
            let d = descriptor(texture_sampler(family));
            assert_eq!(
                (d.address_mode_u, d.address_mode_v),
                (ImageAddressMode::Repeat, ImageAddressMode::Repeat),
                "{family:?} repeats"
            );
        }
        // The decal shadow's `Set_U/V_Addr_Mode(CLAMP)`, the shroud's (`W3DShroud.cpp:255-256`) and scorch's
        // `Apply` (`W3DProjectedShadow.cpp:1516-1519`, `TerrainTex.cpp:1125-1126`).
        for family in [
            TextureUse::DecalShadow,
            TextureUse::Shroud,
            TextureUse::Scorch { bilinear: true, trilinear: true },
        ] {
            let d = descriptor(texture_sampler(family));
            assert_eq!(
                (d.address_mode_u, d.address_mode_v),
                (ImageAddressMode::ClampToEdge, ImageAddressMode::ClampToEdge),
                "{family:?} clamps"
            );
        }
        // `Set_Mip_Mapping(FILTER_TYPE_NONE)` and `MIP_LEVELS_1` both sample the top level only.
        for family in [TextureUse::DecalShadow, TextureUse::Ui, TextureUse::Radar, TextureUse::Shroud] {
            let d = descriptor(texture_sampler(family));
            assert_eq!((d.mipmap_filter, d.lod_max_clamp), (ImageFilterMode::Nearest, 0.0), "{family:?} has no mip chain to pick from");
        }
        // The all-levels and `MIP_LEVELS_3` families keep the mode's point mip and all their levels.
        for family in [TextureUse::Particle, TextureUse::Laser, TextureUse::Water, TextureUse::Bridge] {
            let d = descriptor(texture_sampler(family));
            assert_eq!(
                (d.mipmap_filter, d.lod_max_clamp),
                (ImageFilterMode::Nearest, ImageSamplerDescriptor::linear().lod_max_clamp),
                "{family:?} keeps the mode's mip filter"
            );
        }
    }

    #[test]
    fn scorch_follows_the_terrain_texture_flags() {
        // `ScorchTextureClass::Apply` (`TerrainTex.cpp:1111-1122`) reads `GameData`'s `BilinearTerrainTex` /
        // `TrilinearTerrainTex` (both `Yes` in the shipped `Data\INI\gamedata.ini`) instead of the mode.
        let trilinear = descriptor(texture_sampler(TextureUse::Scorch { bilinear: true, trilinear: true }));
        assert_eq!(
            (trilinear.min_filter, trilinear.mag_filter, trilinear.mipmap_filter),
            (ImageFilterMode::Linear, ImageFilterMode::Linear, ImageFilterMode::Linear)
        );
        let bilinear = descriptor(texture_sampler(TextureUse::Scorch { bilinear: true, trilinear: false }));
        assert_eq!(
            (bilinear.min_filter, bilinear.mag_filter, bilinear.mipmap_filter),
            (ImageFilterMode::Linear, ImageFilterMode::Linear, ImageFilterMode::Nearest)
        );
        let point = descriptor(texture_sampler(TextureUse::Scorch { bilinear: false, trilinear: false }));
        assert_eq!(
            (point.min_filter, point.mag_filter, point.mipmap_filter),
            (ImageFilterMode::Nearest, ImageFilterMode::Nearest, ImageFilterMode::Nearest)
        );
    }

    #[test]
    fn family_images_keep_the_engines_mip_counts() {
        // `MIP_LEVELS_1` (`W3DDisplay.cpp:2798`, `W3DRadar.cpp:910-928`): one uploaded level.
        assert_eq!(levels(&texture_image(&RgbaImage::new(16, 16), TextureUse::Ui)), 1);
        assert_eq!(levels(&texture_image(&RgbaImage::new(16, 16), TextureUse::Shroud)), 1);
        // `MIP_LEVELS_3`: `ScorchTextureClass` (`TerrainTex.h:117`) and `W3DBridgeBuffer.cpp:248`.
        assert_eq!(levels(&texture_image(&RgbaImage::new(16, 16), TextureUse::Bridge)), 3);
        assert_eq!(levels(&texture_image(&RgbaImage::new(16, 16), TextureUse::Scorch { bilinear: true, trilinear: true })), 3);
        // A plain `Get_Texture(name)` keeps every level (16x16 generated: 16, 8, 4, 2, 1).
        assert_eq!(levels(&texture_image(&RgbaImage::new(16, 16), TextureUse::Particle)), 5);
        assert_eq!(levels(&texture_image(&RgbaImage::new(16, 16), TextureUse::Water)), 5);
        // The family's sampler is attached to the image.
        let image = texture_image(&RgbaImage::new(8, 8), TextureUse::DecalShadow);
        assert_eq!(image.sampler, texture_sampler(TextureUse::DecalShadow));
    }

    #[test]
    fn family_images_keep_an_authored_dds_chain_instead_of_generating_one() {
        // `Apply_Mip_Reduction` takes min(requested, file mips) (`textureloader.cpp:1480-1511`), and only the TGA
        // path generates levels (`Load_Uncompressed_Mipmap`, `textureloader.cpp:1718-1846`). A 16x16 DDS with three
        // levels (16, 8, 4) therefore uploads those three (256 + 64 + 16 texels), not a generated 16..1 chain.
        let image = texture_image(&authored(16, 16, 3), TextureUse::Particle);
        assert_eq!(levels(&image), 3);
        assert_eq!(bytes(&image), (256 + 64 + 16) * 4);
        // The family's `MIP_LEVELS_3` cap still applies to an authored chain longer than three levels.
        assert_eq!(levels(&texture_image(&authored(16, 16, 5), TextureUse::Water)), 5);
        assert_eq!(levels(&texture_image(&authored(16, 16, 5), TextureUse::Bridge)), 3);
    }

    #[test]
    fn texture_reduction_drops_the_top_levels_like_the_loader() {
        // `Get_Requested_Reduction` (`textureloader.cpp:1304-1328`) + `Validate_Reduction` (`:1407-1422`): a level
        // of 1 halves a 64x64 file once, so its 7-level chain uploads from 32x32 with 6 levels.
        let once = reduced(generated_levels(&RgbaImage::new(64, 64)), None, 1);
        assert_eq!((image_size(&once), levels(&once)), ((32, 32), 6));
        assert_eq!(bytes(&once), (1024 + 256 + 64 + 16 + 4 + 1) * 4);
        let twice = reduced(generated_levels(&RgbaImage::new(128, 128)), None, 2);
        assert_eq!((image_size(&twice), levels(&twice)), ((32, 32), 6));
        // The minimum dimension 32 stops the halving: 64x64 is reduced once even at level 2.
        assert_eq!(image_size(&reduced(generated_levels(&RgbaImage::new(64, 64)), None, 2)), (32, 32));
        // No reduction is the identity.
        let none = reduced(generated_levels(&RgbaImage::new(64, 64)), None, 0);
        assert_eq!((image_size(&none), levels(&none)), ((64, 64), 7));
        // `MIP_LEVELS_1` is never reduced (`Validate_Reduction`) and neither is a side of 32 or less
        // (`Set_Texture_Reduction(level, 32)`, `W3DGameClient.cpp:204`).
        let one_level = reduced(level_zero(&RgbaImage::new(64, 64)), Some(1), 2);
        assert_eq!((image_size(&one_level), levels(&one_level)), ((64, 64), 1));
        assert_eq!(image_size(&reduced(generated_levels(&RgbaImage::new(32, 32)), None, 2)), (32, 32));
        // A requested cap is reduced too, but keeps at least one level.
        assert_eq!(levels(&reduced(keep_levels(generated_levels(&RgbaImage::new(64, 64)), Some(3)), Some(3), 1)), 2);
        // TheSuperHackers resets `Reduction = 0` for uncompressed loads (`Begin_Uncompressed_Load`,
        // `textureloader.cpp:1602-1603`), so a TGA uploads at full resolution even at level 2 and only a DDS is
        // reduced.
        let tga = reduce_file(generated_levels(&RgbaImage::new(128, 128)), None, false, 2);
        assert_eq!((image_size(&tga), levels(&tga)), ((128, 128), 8));
        let dds = reduce_file(generated_levels(&RgbaImage::new(128, 128)), None, true, 2);
        assert_eq!((image_size(&dds), levels(&dds)), ((32, 32), 6));
    }

    #[test]
    fn the_terrain_atlas_reduction_starts_the_sampler_at_a_coarser_level() {
        // `TextureReduction` reaches the procedural terrain through `SetLOD` (`TerrainTex.cpp:365-368`), not by
        // dropping uploaded levels.
        assert_eq!(descriptor(terrain_sampler_at(0)).lod_min_clamp, 0.0);
        let d = descriptor(terrain_sampler_at(2));
        assert_eq!(d.lod_min_clamp, 2.0);
        assert_eq!(d.address_mode_u, ImageAddressMode::ClampToEdge);
        assert_eq!(d.address_mode_v, ImageAddressMode::ClampToEdge);
        // The possession addition is the aniso filter, and the clamp applies to it too.
        #[cfg(feature = "possession")]
        assert_eq!(d.anisotropy_clamp, 8);
        #[cfg(not(feature = "possession"))]
        assert_eq!(d.anisotropy_clamp, 1);
    }

    #[test]
    fn the_terrain_atlas_uploads_three_mip_levels() {
        // `TerrainTextureClass` is created with `MIP_LEVELS_3` (`TerrainTex.cpp:70-74`); `D3DXFilterTexture` boxes
        // levels 1..2 (`:198`). Deeper levels would filter across the 4-texel block borders (`TerrainTex.h:38`).
        let image = terrain_image(&RgbaImage::new(256, 128));
        assert_eq!(levels(&image), 3);
        let data = image.data.clone().unwrap();
        assert_eq!(data.len(), (256 * 128 + 128 * 64 + 64 * 32) * 4);
        // A one-pixel atlas still uploads one level.
        assert_eq!(levels(&terrain_image(&RgbaImage::new(1, 1))), 1);
    }
}
