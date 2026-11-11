// Terrain material: the engine's baked `doTheLight` vertex colours drawn unlit, darkened by the
// directional light's shadow map exactly like the engine's stencil shadow volumes:
// `baked * lerp(1, shadow_color, shadow)`.
//
// The class atlas holds base and blend tiles (`WorldHeightMap` `getUVForNdx` /
// `getAlphaUVData`), so a mesh with a second UV set draws the engine's two passes here:
// pass 0 = `MODULATE(texture, diffuse)` opaque with `uv`, pass 1 = `MODULATE(blend texture,
// diffuse)` with alpha `MODULATE(texture.a, diffuse.a)` and `SRCALPHA`/`INVSRCALPHA` over it
// (`W3DShaderManager.cpp` `TerrainShader2Stage::set`). The vertex colour alpha is the blend
// alpha `doTheLight` packs into the engine's terrain diffuse (`BaseHeightMap.cpp:618-635`).
// Meshes without a second UV set (roads, the extra blend pass) keep the single-layer path.
#define_import_path zr_viewer::terrain

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings as view_bindings
#import bevy_pbr::shadows

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> base_color: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_color_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> shadow_color: vec4<f32>;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let diffuse = base_color * in.color;
    var albedo = textureSample(base_color_texture, base_color_sampler, in.uv) * diffuse;
#ifdef VERTEX_UVS_B
    // Second terrain pass: the blend tile's texel, times the vertex (lighting) colour, over the
    // base pass with the texture's alpha times the vertex blend alpha.
    let blend = textureSample(base_color_texture, base_color_sampler, in.uv_b);
    let alpha = blend.a * diffuse.a;
    albedo = vec4<f32>(mix(albedo.rgb, blend.rgb * diffuse.rgb, alpha), albedo.a);
#endif
    var tint = vec3<f32>(1.0);
#ifdef DIRECTIONAL_LIGHT_SHADOWS
    // `fetch_directional_shadow` returns 1.0 when lit and 0.0 when shadowed.
    let view_z = (view_bindings::view.view_from_world * in.world_position).z;
    let shadow = shadows::fetch_directional_shadow(
        0u,
        in.world_position,
        normalize(in.world_normal),
        view_z,
        in.position.xy,
    );
    tint = mix(vec3<f32>(1.0), shadow_color.rgb, (1.0 - shadow) * shadow_color.a);
#endif
    return vec4<f32>(albedo.rgb * tint, albedo.a);
}
