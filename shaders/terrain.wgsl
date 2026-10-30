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

// Se i tile risultano ancora girati/specchiati, prova le altre 3 combinazioni di queste due costanti.
const TILEMAP_Y_SOUTH: bool = true;
const TEXTURE_V_TOPDOWN: bool = false;

// Correzione di rotazione (in passi da 90 gradi, 0..3) per categoria di tile di transizione.
// Le tre tabelle di transizione della lookup usano i record:
//   singolo vertice : 5, 10, 15     (angolo "convesso")
//   tre vertici     : 7, 12, 17     (angolo "concavo")
//   bordo           : 6, 11, 16
//   sella (diagonale): 48, 51, 53
// Con 3u gli angoli interni si sono allineati; cambia un valore alla volta.
const SHIFT_SINGLE: u32 = 0u;
const SHIFT_TRIPLE: u32 = 0u;
const SHIFT_EDGE: u32 = 0u;
const SHIFT_SADDLE: u32 = 0u;

// Modalita' di debug:
//   0 = spenta
//   1 = tinta sopra la texture (difficile da leggere sull'acqua/sabbia)
//   2 = colori PIENI, senza texture: rosso = singolo, verde = tre vertici, blu = bordo,
//       giallo = sella, magenta = altro record di transizione (>3), grigio = tile base (0..3).
//       La luminosita' cresce con l'indice di rotazione originale (0 scuro .. 3 chiaro).
const DEBUG_MODE: u32 = 0u;

// 0 = altro, 1 = singolo, 2 = tre vertici, 3 = bordo, 4 = sella
fn tile_category(r: i32) -> u32 {
    if (r == 5 || r == 10 || r == 15) { return 1u; }
    if (r == 7 || r == 12 || r == 17) { return 2u; }
    if (r == 6 || r == 11 || r == 16) { return 3u; }
    if (r == 48 || r == 51 || r == 53) { return 4u; }
    return 0u;
}

fn category_shift(c: u32) -> u32 {
    switch c {
        case 1u: { return SHIFT_SINGLE; }
        case 2u: { return SHIFT_TRIPLE; }
        case 3u: { return SHIFT_EDGE; }
        case 4u: { return SHIFT_SADDLE; }
        default: { return 0u; }
    }
}

// Matrici e traslazioni di DFU (rotations[] / translations[]):
//   0: (x, y)          1: (y, 1-x)          2: (1-x, 1-y)          3: (1-y, x)
fn dfu_tile_transform(idx: u32, uv: vec2<f32>) -> vec2<f32> {
    switch idx {
        case 1u: { return vec2<f32>(uv.y, 1.0 - uv.x); }
        case 2u: { return vec2<f32>(1.0 - uv.x, 1.0 - uv.y); }
        case 3u: { return vec2<f32>(1.0 - uv.y, uv.x); }
        default: { return uv; }
    }
}

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
    var transform_idx = tile_data & 3u; // 0 = normale, 1 = rotate, 2 = flip, 3 = rotate+flip
    let category = tile_category(tile_idx);
    transform_idx = (transform_idx + category_shift(category)) & 3u;

    var local_uv = fract(chunk_uv * 128.0);

    // La tilemap ha Y verso sud (specchiata rispetto a Daggerfall Unity, dove Y e' verso nord):
    // riporta la v locale nel sistema di DFU prima della trasformazione.
    if (TILEMAP_Y_SOUTH) {
        local_uv.y = 1.0 - local_uv.y;
    }

    // Trasformazione identica a DaggerfallTilemapTextureArray.shader.
    // Attenzione: in DFU "flip" NON e' uno specchio ma una rotazione di 180 gradi.
    local_uv = dfu_tile_transform(transform_idx, local_uv);

    // Unity ha v=0 in basso, wgpu/Bevy ha v=0 in alto: se le immagini dei tile sono
    // caricate "dall'alto in basso" serve invertire la v dopo la trasformazione.
    if (TEXTURE_V_TOPDOWN) {
        local_uv.y = 1.0 - local_uv.y;
    }

    let albedo = textureSample(tile_array, tile_sampler, local_uv, tile_idx);
    var tinted = albedo;
    if (DEBUG_MODE == 1u) {
        if (category == 1u) { tinted = vec4<f32>(albedo.rgb * vec3<f32>(1.0, 0.3, 0.3), albedo.a); }
        else if (category == 2u) { tinted = vec4<f32>(albedo.rgb * vec3<f32>(0.3, 1.0, 0.3), albedo.a); }
        else if (category == 3u) { tinted = vec4<f32>(albedo.rgb * vec3<f32>(0.3, 0.3, 1.0), albedo.a); }
        else if (category == 4u) { tinted = vec4<f32>(albedo.rgb * vec3<f32>(1.0, 1.0, 0.2), albedo.a); }
        else if (tile_idx > 3) { tinted = vec4<f32>(albedo.rgb * vec3<f32>(1.0, 0.3, 1.0), albedo.a); }
    } else if (DEBUG_MODE == 2u) {
        var flat_color = vec3<f32>(0.5, 0.5, 0.5);
        if (category == 1u) { flat_color = vec3<f32>(1.0, 0.0, 0.0); }
        else if (category == 2u) { flat_color = vec3<f32>(0.0, 1.0, 0.0); }
        else if (category == 3u) { flat_color = vec3<f32>(0.0, 0.0, 1.0); }
        else if (category == 4u) { flat_color = vec3<f32>(1.0, 1.0, 0.0); }
        else if (tile_idx > 3) { flat_color = vec3<f32>(1.0, 0.0, 1.0); }
        let brightness = 0.4 + 0.2 * f32(tile_data & 3u);
        tinted = vec4<f32>(flat_color * brightness, 1.0);
    }
    pbr_input.material.base_color = tinted;

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
