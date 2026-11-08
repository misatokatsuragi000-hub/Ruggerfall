// Vertex shader per le vele e le bandiere del veliero elfico.
//
// La mesh è statica. Questo shader applica uno spostamento locale in base al
// tempo (uniform `sail.time`) e ai parametri `kind` / `amp` / `phase`:
//
// - kind = 0 (vela): rigonfiamento lungo la normale, massimo al centro della
//   vela, zero ai bordi. L'ampiezza segue una sinusoide lenta (~18 s).
// - kind = 1 (bandiera): ondeggio in X/Y/Z, con ampiezza che cresce con
//   `uv.x` (0 = asta, 1 = punta).
//
// Il `VertexOutput` è quello standard di `bevy_pbr::forward_io`. Se la tua
// versione di Bevy 0.15 ha campi extra (es. `color`, `world_tangent`),
// aggiungili in fondo qui sotto — ma di default la mesh non ha quei canali,
// quindi non servono.

#import bevy_pbr::forward_io::{Vertex, VertexOutput}
#import bevy_pbr::mesh_functions
#import bevy_pbr::view_transformations::position_world_to_clip

struct SailParams {
    time: f32,
    sail_amp: f32,
    flag_amp: f32,
    phase: f32,
    kind: u32,
    // Nessun padding: il buffer uniform lato Rust è di 5 campi (20 byte).
    // Con `_pad0.._pad2` la struct sarebbe di 32 byte e wgpu rifiuta la
    // pipeline ("min_binding_size 20").
};

@group(2) @binding(100) var<uniform> sail: SailParams;

@vertex
fn vertex(input: Vertex) -> VertexOutput {
    var out: VertexOutput;

    var local_position = input.position;
    let uv = input.uv;
    let n = input.normal;

    if sail.kind == 0u {
        // Vela: rigonfiamento sinusoidale lungo la normale.
        let cycle = sin(sail.time * 0.35 + sail.phase);
        let sag = 1.0 + 0.35 * cycle;
        let bulge = sin(uv.y * 3.14159) * sail.sail_amp * sag;
        local_position += n * bulge;
    } else {
        // Bandiera: ondeggio in tre direzioni, ampiezza ~ u^2.
        let u = uv.x;
        let v = uv.y;
        let a = sail.flag_amp;
        let p = sail.phase;
        let t = sail.time;
        local_position.x += u * u * a * sin(t * 1.4 + p);
        local_position.y += u * u * a * 0.4 * sin(t * 1.1 + p + v * 4.0);
        local_position.z += u * u * a * 0.7 * sin(t * 1.4 + p + v * 2.0);
    }

    // Bevy 0.14+: `get_model_matrix` è stata rinominata `get_world_from_local`.
    let world_from_local = mesh_functions::get_world_from_local(input.instance_index);
    let world_pos = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(local_position, 1.0)
    );

    out.position = position_world_to_clip(world_pos.xyz);
    out.world_position = world_pos;
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        n,
        input.instance_index
    );
    out.uv = uv;

    // Come il vertex shader standard di `bevy_pbr`: il fragment usa
    // `instance_index` (flag della mesh) e il dither di visibilità, se la
    // pipeline li abilita. Senza, leggerebbe valori non inizializzati.
    #ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = input.instance_index;
    #endif
    #ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        input.instance_index, world_from_local[3]);
    #endif

    return out;
}
