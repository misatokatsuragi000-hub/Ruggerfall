// Finto rilievo 3D per pareti piatte: porting di DFU_FakeRelief.fx (ReShade) per Bevy 0.15.
//
// Passate:
//   fragment_down    luminanza a 1/4 di risoluzione           -> texture A
//   fragment_blur_h  blur gaussiano orizzontale di A          -> texture B
//   fragment_blur_v  blur gaussiano verticale di B (mean)     -> texture Mean
//   fragment_height  mappa bump (solo modo 1 o debug 5)       -> texture Height
//   fragment_relief  rilievo finale                           -> schermo

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

// Deve coincidere byte-per-byte con `struct FakeRelief` in relief_post.rs.
struct Settings {
    mode: u32,
    coarse_scale: u32,
    skin_enable: u32,
    debug: u32,
    enabled: u32,
    strength: f32,
    detail_size: f32,
    cavity: f32,
    light_scale: f32,
    bump_contrast: f32,
    bump_smooth: f32,
    mortar: f32,
    mortar_threshold: f32,
    parallax: f32,
    spec: f32,
    shininess: f32,
    light_az: f32,
    light_el: f32,
    torch_blend: f32,
    skin_hue: f32,
    skin_hue_tol: f32,
    skin_sat: f32,
    skin_sat_tol: f32,
    skin_val: f32,
    skin_val_tol: f32,
    far_plane: f32,
    fade_distance: f32,
    near: f32,
    tan_half_fov: f32,
    mean_div: f32,
}

@group(0) @binding(0) var scene_tex: texture_2d<f32>;
@group(0) @binding(1) var depth_tex: texture_depth_2d;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var src_tex: texture_2d<f32>;
@group(0) @binding(4) var mean_tex: texture_2d<f32>;
@group(0) @binding(5) var height_tex: texture_2d<f32>;
@group(0) @binding(6) var<uniform> S: Settings;

const GAMMA: f32 = 2.2;

// ------------------------------------------------------------- Utility
fn to_gamma(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / GAMMA));
}

fn from_gamma(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(GAMMA));
}

fn out_g(c: vec3<f32>) -> vec4<f32> {
    return vec4<f32>(from_gamma(c), 1.0);
}

fn sat(x: f32) -> f32 { return clamp(x, 0.0, 1.0); }

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn px_size() -> vec2<f32> {
    return 1.0 / vec2<f32>(textureDimensions(scene_tex));
}

fn scene_lin(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(scene_tex, samp, uv, 0.0).rgb;
}

fn lum_tap(uv: vec2<f32>) -> f32 {
    return luma(to_gamma(scene_lin(uv)));
}

fn mean_tap(uv: vec2<f32>) -> f32 {
    return textureSampleLevel(mean_tex, samp, uv, 0.0).r;
}

fn height_tap(uv: vec2<f32>) -> f32 {
    return textureSampleLevel(height_tex, samp, uv, 0.0).r;
}

// Profondità lineare in unità di gioco (reverse-Z infinito: d = near / z).
fn depth_z(uv: vec2<f32>) -> f32 {
    let dim_u = textureDimensions(depth_tex);
    let dim = vec2<i32>(i32(dim_u.x), i32(dim_u.y));
    let p = clamp(vec2<i32>(vec2<f32>(dim) * uv), vec2<i32>(0, 0), dim - vec2<i32>(1, 1));
    let d = textureLoad(depth_tex, p, 0);
    if d <= 0.0 {
        return S.far_plane * 2.0;
    }
    return min(S.near / d, S.far_plane * 2.0);
}

fn view_pos_z(uv: vec2<f32>, z: f32, t: f32) -> vec3<f32> {
    let dim = textureDimensions(depth_tex);
    let aspect = max(f32(dim.x), 1.0) / max(f32(dim.y), 1.0);
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    return vec3<f32>(ndc.x * t * aspect * z, ndc.y * t * z, z);
}

fn get_view_pos(uv: vec2<f32>, t: f32) -> vec3<f32> {
    return view_pos_z(uv, depth_z(uv), t);
}

// Gradiente Sobel 3x3 della luminanza
fn sobel_lum(uv: vec2<f32>, d: vec2<f32>) -> vec2<f32> {
    let tl = lum_tap(uv + vec2<f32>(-d.x, -d.y));
    let t  = lum_tap(uv + vec2<f32>(0.0, -d.y));
    let tr = lum_tap(uv + vec2<f32>(d.x, -d.y));
    let l  = lum_tap(uv + vec2<f32>(-d.x, 0.0));
    let r  = lum_tap(uv + vec2<f32>(d.x, 0.0));
    let bl = lum_tap(uv + vec2<f32>(-d.x, d.y));
    let b  = lum_tap(uv + vec2<f32>(0.0, d.y));
    let br = lum_tap(uv + vec2<f32>(d.x, d.y));
    let gx = (tr + 2.0 * r + br) - (tl + 2.0 * l + bl);
    let gy = (bl + 2.0 * b + br) - (tl + 2.0 * t + tr);
    return vec2<f32>(gx, gy) * 0.125;
}

fn sobel_height(uv: vec2<f32>, d: vec2<f32>) -> vec2<f32> {
    let tl = height_tap(uv + vec2<f32>(-d.x, -d.y));
    let t  = height_tap(uv + vec2<f32>(0.0, -d.y));
    let tr = height_tap(uv + vec2<f32>(d.x, -d.y));
    let l  = height_tap(uv + vec2<f32>(-d.x, 0.0));
    let r  = height_tap(uv + vec2<f32>(d.x, 0.0));
    let bl = height_tap(uv + vec2<f32>(-d.x, d.y));
    let b  = height_tap(uv + vec2<f32>(0.0, d.y));
    let br = height_tap(uv + vec2<f32>(d.x, d.y));
    let gx = (tr + 2.0 * r + br) - (tl + 2.0 * l + bl);
    let gy = (bl + 2.0 * b + br) - (tl + 2.0 * t + tr);
    return vec2<f32>(gx, gy) * 0.125;
}

fn multi_grad_lum(uv: vec2<f32>, d: vec2<f32>) -> vec2<f32> {
    var g = sobel_lum(uv, d);
    if S.coarse_scale != 0u {
        g += 0.5 * sobel_lum(uv, d * 2.0);
    } else {
        g *= 1.5;
    }
    return g;
}

fn multi_grad_height(uv: vec2<f32>, d: vec2<f32>) -> vec2<f32> {
    var g = sobel_height(uv, d);
    if S.coarse_scale != 0u {
        g += 0.5 * sobel_height(uv, d * 2.0);
    } else {
        g *= 1.5;
    }
    return g;
}

// ------------------------------------------------------------- Pass 1
@fragment
fn fragment_down(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let p = px_size();
    var s = 0.0;
    s += lum_tap(in.uv + vec2<f32>(-p.x, -p.y));
    s += lum_tap(in.uv + vec2<f32>( p.x, -p.y));
    s += lum_tap(in.uv + vec2<f32>(-p.x,  p.y));
    s += lum_tap(in.uv + vec2<f32>( p.x,  p.y));
    return vec4<f32>(s * 0.25, 0.0, 0.0, 1.0);
}

// ------------------------------------------------------------- Pass 2/3
fn blur_dir(uv: vec2<f32>, dir: vec2<f32>) -> f32 {
    let texel = 1.0 / vec2<f32>(textureDimensions(src_tex));
    let step_texels = clamp(S.light_scale * 0.25, 1.0, 300.0) / max(S.mean_div, 1.0);

    let w0 = 0.2270270;
    let w1 = 0.1945946;
    let w2 = 0.1216216;
    let w3 = 0.0540541;
    let w4 = 0.0162162;

    let o1 = dir * texel * (1.0 * step_texels);
    let o2 = dir * texel * (2.0 * step_texels);
    let o3 = dir * texel * (3.0 * step_texels);
    let o4 = dir * texel * (4.0 * step_texels);

    var sum = textureSampleLevel(src_tex, samp, uv, 0.0).r * w0;
    sum += (textureSampleLevel(src_tex, samp, uv + o1, 0.0).r
          + textureSampleLevel(src_tex, samp, uv - o1, 0.0).r) * w1;
    sum += (textureSampleLevel(src_tex, samp, uv + o2, 0.0).r
          + textureSampleLevel(src_tex, samp, uv - o2, 0.0).r) * w2;
    sum += (textureSampleLevel(src_tex, samp, uv + o3, 0.0).r
          + textureSampleLevel(src_tex, samp, uv - o3, 0.0).r) * w3;
    sum += (textureSampleLevel(src_tex, samp, uv + o4, 0.0).r
          + textureSampleLevel(src_tex, samp, uv - o4, 0.0).r) * w4;
    return sum;
}

@fragment
fn fragment_blur_h(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(blur_dir(in.uv, vec2<f32>(1.0, 0.0)), 0.0, 0.0, 1.0);
}

@fragment
fn fragment_blur_v(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(blur_dir(in.uv, vec2<f32>(0.0, 1.0)), 0.0, 0.0, 1.0);
}

// ------------------------------------------------------------- Pass 4
@fragment
fn fragment_height(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    if S.mode != 1u && S.debug != 5u {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let uv = in.uv;
    let o = px_size() * S.bump_smooth;
    var s = lum_tap(uv) * 4.0;
    s += (lum_tap(uv + vec2<f32>(o.x, 0.0))
        + lum_tap(uv - vec2<f32>(o.x, 0.0))
        + lum_tap(uv + vec2<f32>(0.0, o.y))
        + lum_tap(uv - vec2<f32>(0.0, o.y))) * 2.0;
    s += lum_tap(uv + vec2<f32>( o.x,  o.y))
       + lum_tap(uv + vec2<f32>(-o.x,  o.y))
       + lum_tap(uv + vec2<f32>( o.x, -o.y))
       + lum_tap(uv + vec2<f32>(-o.x, -o.y));
    s *= 1.0 / 16.0;

    let mean = mean_tap(uv);
    let v = clamp((s - mean) / (mean + 0.08), -1.0, 1.0);
    let h = sign(v) * pow(abs(v), 1.0 / max(S.bump_contrast, 0.01));
    return vec4<f32>(h, 0.0, 0.0, 1.0);
}

// ------------------------------------------------------------- Filtro pelle
fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    let d = mx - mn;
    var h = 0.0;
    if d > 1e-5 {
        if mx == c.r {
            let q = (c.g - c.b) / d + 6.0;
            h = q - 6.0 * floor(q / 6.0);
        } else if mx == c.g {
            h = (c.b - c.r) / d + 2.0;
        } else {
            h = (c.r - c.g) / d + 4.0;
        }
    }
    var sat_v = 0.0;
    if mx > 1e-5 {
        sat_v = d / mx;
    }
    return vec3<f32>(h * 60.0, sat_v, mx);
}

fn tol_mask(dist: f32, tol: f32) -> f32 {
    if tol <= 0.0 {
        return 0.0;
    }
    return 1.0 - smoothstep(tol, tol * 1.5, dist);
}

fn skin_mask(col: vec3<f32>) -> f32 {
    if S.skin_enable == 0u {
        return 0.0;
    }
    let hsv = rgb_to_hsv(col);
    var hd = abs(hsv.x - S.skin_hue);
    hd = min(hd, 360.0 - hd);
    return tol_mask(hd, S.skin_hue_tol)
         * tol_mask(abs(hsv.y - S.skin_sat), S.skin_sat_tol)
         * tol_mask(abs(hsv.z - S.skin_val), S.skin_val_tol);
}

// ------------------------------------------------------------- Pass 5
@fragment
fn fragment_relief(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let lin = scene_lin(uv);
    var col = to_gamma(lin);

    if S.debug == 4u {
        return out_g(mix(col * 0.4, vec3<f32>(1.0, 0.2, 0.8), skin_mask(col)));
    }
    if S.debug == 5u {
        return out_g(vec3<f32>(height_tap(uv) * 0.5 + 0.5));
    }

    let z = depth_z(uv);
    if z / S.far_plane > 0.999 {
        return vec4<f32>(lin, 1.0);
    }

    let dist_fade = sat(1.0 - z / max(S.fade_distance, 0.01));
    if S.debug == 0u && dist_fade <= 0.0 {
        return vec4<f32>(lin, 1.0);
    }
    let skin = skin_mask(col);
    if S.debug == 0u && skin >= 0.999 {
        return vec4<f32>(lin, 1.0);
    }

    let tn = S.tan_half_fov;
    let px = px_size();
    let P  = view_pos_z(uv, z, tn);
    let Pr = get_view_pos(uv + vec2<f32>( px.x, 0.0), tn);
    let Pl = get_view_pos(uv - vec2<f32>( px.x, 0.0), tn);
    let Pd = get_view_pos(uv + vec2<f32>(0.0,  px.y), tn);
    let Pu = get_view_pos(uv - vec2<f32>(0.0,  px.y), tn);

    let T = normalize(Pr - Pl);
    let B = normalize(Pd - Pu);
    var N = normalize(cross(T, B));
    if dot(N, P) > 0.0 {
        N = -N;
    }

    let curv = max(abs(Pr.z + Pl.z - 2.0 * P.z), abs(Pd.z + Pu.z - 2.0 * P.z)) / max(P.z, 0.01);
    let edge_fade = 1.0 - smoothstep(0.02, 0.08, curv);
    let f = edge_fade * dist_fade * (1.0 - skin);

    if S.debug == 3u {
        return out_g(vec3<f32>(f));
    }
    if S.debug == 0u && f < 0.002 {
        return vec4<f32>(lin, 1.0);
    }

    let screen_h = f32(textureDimensions(scene_tex).y);
    let px_per_unit = screen_h / (2.0 * tn * max(P.z, 0.01));
    let dpx = clamp(S.detail_size * px_per_unit, 1.0, 24.0);
    let d = px * dpx;

    var g: vec2<f32>;
    var cav: f32;

    if S.mode == 1u {
        cav = height_tap(uv);
        g = multi_grad_height(uv, d);
    } else {
        let lum = lum_tap(uv);
        let mean = mean_tap(uv);
        let norm = 1.0 / (mean + 0.08);
        g = multi_grad_lum(uv, d) * norm;
        cav = clamp((lum - mean) * norm, -1.0, 1.0);
    }

    var tilt = (g.x * T + g.y * B) * S.strength;
    let tl = length(tilt);
    if tl > 1.5 {
        tilt *= 1.5 / tl;
    }
    let Np = normalize(N - tilt);

    if S.debug == 6u {
        return out_g(vec3<f32>(dot(Np, T), -dot(Np, B), dot(Np, N)) * 0.5 + 0.5);
    }

    let V = normalize(-P);
    let el = radians(S.light_el);
    let az = radians(S.light_az);
    let Lf = vec3<f32>(cos(el) * sin(az), cos(el) * cos(az), -sin(el));
    let L = normalize(mix(Lf, V, S.torch_blend));

    let nl0 = max(dot(N, L), 0.25);
    let nl1 = max(dot(Np, L), 0.0);
    var shade = (nl1 + 0.15) / (nl0 + 0.15);
    shade = clamp(shade, 0.3, 2.2);
    shade = mix(1.0, shade, f);

    if S.debug == 1u {
        return out_g(vec3<f32>(shade * 0.5));
    }
    if S.debug == 2u {
        return out_g(vec3<f32>(cav * 0.5 + 0.5));
    }

    let cav_mul = 1.0 + cav * S.cavity * 0.6 * f;

    if S.parallax > 0.0 {
        let h = sat(0.5 + cav * 0.5);
        let vt = vec2<f32>(dot(V, T), dot(V, B)) / max(dot(V, N), 0.25);
        var uv2 = uv - vt * (h - 0.5) * S.parallax * px * f;
        uv2 = clamp(uv2, px, vec2<f32>(1.0) - px);
        if abs(depth_z(uv2) - z) > 0.08 * z {
            uv2 = uv;
        }
        col = to_gamma(scene_lin(uv2));
    }

    let dark = sat((-cav - S.mortar_threshold) / max(1.0 - S.mortar_threshold, 0.01));
    let mortar_mul = 1.0 - S.mortar * pow(dark, 0.7) * f;

    col *= shade * cav_mul * mortar_mul;

    if S.spec > 0.0 {
        let H = normalize(L + V);
        let spec = pow(sat(dot(Np, H)), max(S.shininess, 1.0)) * S.spec * f;
        let cl = luma(col);
        col += spec * sat(cl * 2.0);
    }

    return out_g(saturate(col));
}
