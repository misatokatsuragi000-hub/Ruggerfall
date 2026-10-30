#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
}

// 1. Array a 56 fette (BC7 HD di DREAM)
@group(2) @binding(100) var tile_array: texture_2d_array<f32>;
@group(2) @binding(101) var tile_sampler: sampler;

// 2. Tilemap 128x128 del chunk (in formato Float R8Unorm)
@group(2) @binding(102) var tilemap: texture_2d<f32>;

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let chunk_uv = in.uv;
    let tile_coord = vec2<u32>(clamp(vec2<i32>(floor(chunk_uv * 128.0)), vec2<i32>(0), vec2<i32>(127)));

    // Riconverte il valore [0.0, 1.0] nel byte intero originale [0..255]
    let tile_data = u32(round(textureLoad(tilemap, tile_coord, 0).r * 255.0));
    let tile_idx = i32(tile_data >> 2u);
    let rotate = (tile_data & 1u) != 0u;
    let flip = (tile_data & 2u) != 0u;

    var local_uv = fract(chunk_uv * 128.0);

    if (rotate) {
        local_uv = vec2<f32>(1.0 - local_uv.y, local_uv.x);
    }
    if (flip) {
        local_uv.x = 1.0 - local_uv.x;
    }

    let albedo = textureSample(tile_array, tile_sampler, local_uv, tile_idx);
    pbr_input.material.base_color = albedo;

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
