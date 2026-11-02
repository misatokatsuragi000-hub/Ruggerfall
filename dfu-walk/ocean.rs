//! Oceano FFT (Tessendorf / GodotOceanWaves) per Bevy 0.15.
//!
//! Pipeline, tutta su GPU (nodo del render graph prima del main pass):
//!   1. `ocean_spectrum.wgsl`: spettro TMA/JONSWAP evoluto nel tempo -> 4 campi complessi
//!      per cascata in `fft_a`.
//!   2. `stockham.wgsl` (horizontal): FFT inversa sulle righe fft_a -> fft_b
//!   3. `stockham.wgsl` (vertical):   FFT inversa sulle colonne fft_b -> fft_a
//!   4. `ocean_unpack.wgsl`: scrive displacement, pendenze e schiuma persistente
//!      nelle texture usate da `ocean_render.wgsl`.

use bevy::core_pipeline::core_3d::graph::{Core3d, Node3d};
use bevy::pbr::{Material, MaterialPlugin, NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;
use bevy::render::{
    extract_resource::{ExtractResource, ExtractResourcePlugin},
    render_asset::{RenderAssetUsages, RenderAssets},
    render_graph::{self, RenderGraphApp, RenderLabel},
    render_resource::{
        binding_types::{
            storage_buffer_read_only_sized, storage_buffer_sized, texture_storage_2d_array,
            uniform_buffer,
        },
        *,
    },
    renderer::{RenderContext, RenderDevice, RenderQueue},
    texture::GpuImage,
    view::NoFrustumCulling,
    Render, RenderApp, RenderSet,
};

// =============================================================================
// Parametri (da qui si regola l'aspetto del mare)
// =============================================================================

pub const OCEAN_SIZE: u32 = 256;
pub const NUM_CASCADES: u32 = 3;

/// Lato di ogni cascata in metri (devono dividere 819,2).
pub const TILE_LENGTHS: [f32; 3] = [409.6, 102.4, 25.6];
/// Bande di numeri d'onda (rad/m) di ogni cascata: [k_min, k_max).
pub const K_BANDS: [f32; 4] = [0.0, 1.6, 6.3, 25.0];

/// Vento (m/s) e fetch (m): determinano altezza e lunghezza delle onde.
pub const WIND_SPEED: f32 = 12.0;
pub const FETCH: f32 = 150_000.0;
/// Profondita' dell'acqua (m) per la correzione TMA.
pub const WATER_DEPTH: f32 = 200.0;
/// Direzione di propagazione (rad, nel piano x-z).
pub const WIND_ANGLE: f32 = 0.4;
/// 0 = onde in tutte le direzioni, 1 = molto direzionali.
pub const SPREAD: f32 = 0.8;
/// Allungamento delle onde (swell), 0..1.
pub const SWELL: f32 = 0.3;
/// Dettaglio delle onde corte, 0..1 (1 = nessuna soppressione).
pub const DETAIL: f32 = 0.97;
/// Choppiness (spostamento orizzontale).
pub const CHOPPINESS: f32 = 0.7;
/// Schiuma: soglia dello Jacobiano, velocita' di crescita e di decadimento.
pub const FOAM_THRESHOLD: f32 = 0.85;
pub const FOAM_GROW: f32 = 6.0;
pub const FOAM_DECAY: f32 = 0.7;
/// Moltiplicatore della schiuma in fase di rendering.
pub const FOAM_GAIN: f32 = 1.5;

/// Piano dell'oceano: lato (m) e suddivisioni (maglia di 8 m).
const PLANE_SIZE: f32 = 4096.0;
const PLANE_SUBDIVISIONS: u32 = 511;
const PLANE_SPACING: f32 = PLANE_SIZE / (PLANE_SUBDIVISIONS + 1) as f32;

// =============================================================================
// Materiale di rendering
// =============================================================================

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct OceanMaterial {
    #[uniform(100)]
    pub params: OceanParams,
    #[texture(101, dimension = "2d_array")]
    #[sampler(102)]
    pub displacement: Handle<Image>,
    #[texture(103, dimension = "2d_array")]
    pub normal: Handle<Image>,
    #[texture(104, dimension = "2d_array", sample_type = "float", filterable = false)]
    pub foam: Handle<Image>,
}

#[derive(Clone, Debug, ShaderType, Reflect)]
pub struct OceanParams {
    pub map_scales: [Vec4; 4],
    pub water_color: Vec4,
    pub scatter_color: Vec4,
    pub foam_color: Vec4,
    pub sky_zenith: Vec4,
    pub sky_horizon: Vec4,
    pub sun_dir: Vec4,
    pub sun_color: Vec4,
    pub fog: Vec4,
    pub num_cascades: u32,
    pub choppiness: f32,
    pub roughness: f32,
    pub foam_gain: f32,
}

impl Default for OceanParams {
    fn default() -> Self {
        let mut map_scales = [Vec4::ZERO; 4];
        for (i, len) in TILE_LENGTHS.iter().enumerate() {
            map_scales[i] = Vec4::new(1.0 / len, 1.0 / len, 1.0, 1.0);
        }
        Self {
            map_scales,
            water_color: Vec4::new(0.004, 0.045, 0.09, 1.0),
            scatter_color: Vec4::new(0.02, 0.18, 0.17, 1.0),
            foam_color: Vec4::new(0.95, 0.97, 1.0, 1.0),
            sky_zenith: Vec4::new(0.20, 0.38, 0.75, 1.0),
            sky_horizon: Vec4::new(0.60, 0.72, 0.88, 1.0),
            sun_dir: Vec4::new(0.3, 0.8, 0.4, 1.0),
            sun_color: Vec4::new(1.0, 0.95, 0.85, 1.0),
            fog: Vec4::ZERO,
            num_cascades: NUM_CASCADES,
            choppiness: CHOPPINESS,
            roughness: 0.1,
            foam_gain: FOAM_GAIN,
        }
    }
}

impl Material for OceanMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/ocean_render.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/ocean_render.wgsl".into()
    }
    // Se la camera richiede un prepass, usiamo lo stesso vertex shader per muovere i vertici
    fn prepass_vertex_shader() -> ShaderRef {
        "shaders/ocean_render.wgsl".into()
    }

    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline<Self>,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::render::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

// =============================================================================
// Risorse main world
// =============================================================================

#[derive(Resource, Clone, ExtractResource)]
pub struct OceanAssets {
    pub displacement: Handle<Image>,
    pub normal: Handle<Image>,
    pub foam: Handle<Image>,
}

#[derive(Component)]
pub struct OceanPlane;

// =============================================================================
// Risorse render world
// =============================================================================

#[derive(Resource)]
struct OceanGpuBuffers {
    fft_a: Buffer,
    fft_b: Buffer,
    spectrum_uniform: Buffer,
    unpack_uniform: Buffer,
}

#[derive(Resource)]
struct OceanPipelines {
    spectrum_layout: BindGroupLayout,
    spectrum: CachedComputePipelineId,
    fft_layout: BindGroupLayout,
    fft_h: CachedComputePipelineId,
    fft_v: CachedComputePipelineId,
    unpack_layout: BindGroupLayout,
    unpack: CachedComputePipelineId,
}

#[derive(Resource)]
struct OceanBindGroups {
    spectrum: BindGroup,
    fft_h: BindGroup,
    fft_v: BindGroup,
    unpack: BindGroup,
}

#[derive(Resource)]
struct OceanShaderHandles {
    spectrum: Handle<Shader>,
    stockham: Handle<Shader>,
    unpack: Handle<Shader>,
}

#[derive(Resource, Default)]
struct OceanTime {
    elapsed: f32,
}

// =============================================================================
// Uniform
// =============================================================================

#[derive(Clone, Debug, ShaderType)]
pub struct SpectrumParams {
    pub cascades: [Vec4; 4],
    pub alpha: f32,
    pub peak_frequency: f32,
    pub wind_speed: f32,
    pub angle: f32,
    pub depth: f32,
    pub swell: f32,
    pub detail: f32,
    pub spread: f32,
    pub time: f32,
    pub num_cascades: u32,
}

#[derive(Clone, Debug, ShaderType)]
pub struct UnpackParams {
    pub choppiness: f32,
    pub foam_threshold: f32,
    pub foam_grow: f32,
    pub foam_decay: f32,
    pub delta_time: f32,
    pub num_cascades: u32,
    pub _pad0: f32,
    pub _pad1: f32,
}

fn spectrum_params(time: f32) -> SpectrumParams {
    const G: f32 = 9.81;
    let mut cascades = [Vec4::ZERO; 4];
    for i in 0..NUM_CASCADES as usize {
        cascades[i] = Vec4::new(TILE_LENGTHS[i], K_BANDS[i], K_BANDS[i + 1], 0.0);
    }
    SpectrumParams {
        cascades,
        alpha: 0.076 * (WIND_SPEED * WIND_SPEED / (FETCH * G)).powf(0.22),
        peak_frequency: 22.0 * (G * G / (WIND_SPEED * FETCH)).powf(1.0 / 3.0),
        wind_speed: WIND_SPEED,
        angle: WIND_ANGLE,
        depth: WATER_DEPTH,
        swell: SWELL,
        detail: DETAIL,
        spread: SPREAD,
        time,
        num_cascades: NUM_CASCADES,
    }
}

// =============================================================================
// Nodo del render graph
// =============================================================================

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
pub struct OceanComputeLabel;

struct OceanComputeNode;

impl FromWorld for OceanComputeNode {
    fn from_world(_world: &mut World) -> Self {
        Self
    }
}

impl render_graph::Node for OceanComputeNode {
    fn run(
        &self,
        _graph: &mut render_graph::RenderGraphContext,
        render_context: &mut RenderContext,
        world: &World,
    ) -> Result<(), render_graph::NodeRunError> {
        let Some(pipelines) = world.get_resource::<OceanPipelines>() else {
            return Ok(());
        };
        let Some(bind_groups) = world.get_resource::<OceanBindGroups>() else {
            return Ok(());
        };
        let cache = world.resource::<PipelineCache>();
        let (Some(spectrum), Some(fft_h), Some(fft_v), Some(unpack)) = (
            cache.get_compute_pipeline(pipelines.spectrum),
                                                                        cache.get_compute_pipeline(pipelines.fft_h),
                                                                        cache.get_compute_pipeline(pipelines.fft_v),
                                                                        cache.get_compute_pipeline(pipelines.unpack),
        ) else {
            return Ok(());
        };

        let groups = OCEAN_SIZE / 16;
        let encoder = render_context.command_encoder();

        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("ocean_spectrum"),
                                                      timestamp_writes: None,
            });
            pass.set_pipeline(spectrum);
            pass.set_bind_group(0, &bind_groups.spectrum, &[]);
            pass.dispatch_workgroups(groups, groups, NUM_CASCADES);
        }
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("ocean_fft_horizontal"),
                                                      timestamp_writes: None,
            });
            pass.set_pipeline(fft_h);
            pass.set_bind_group(0, &bind_groups.fft_h, &[]);
            pass.dispatch_workgroups(OCEAN_SIZE * 4 * NUM_CASCADES, 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("ocean_fft_vertical"),
                                                      timestamp_writes: None,
            });
            pass.set_pipeline(fft_v);
            pass.set_bind_group(0, &bind_groups.fft_v, &[]);
            pass.dispatch_workgroups(OCEAN_SIZE * 4 * NUM_CASCADES, 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("ocean_unpack"),
                                                      timestamp_writes: None,
            });
            pass.set_pipeline(unpack);
            pass.set_bind_group(0, &bind_groups.unpack, &[]);
            pass.dispatch_workgroups(groups, groups, NUM_CASCADES);
        }
        Ok(())
    }
}

// =============================================================================
// Plugin
// =============================================================================

pub struct OceanPlugin;

impl Plugin for OceanPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            // Disabilitiamo sia prepass che shadow map sul plugin di Bevy 0.15
            MaterialPlugin::<OceanMaterial> {
                prepass_enabled: false,
                shadows_enabled: false,
                ..default()
            },
            ExtractResourcePlugin::<OceanAssets>::default(),
        ))
        .add_systems(Startup, (setup_ocean_textures, setup_ocean_plane).chain())
        .add_systems(Update, (update_ocean_plane, update_ocean_lighting));

        let render_app = app.sub_app_mut(RenderApp);
        render_app
        .init_resource::<OceanTime>()
        .add_systems(
            Render,
            (ensure_ocean_gpu_buffers, prepare_ocean_pipelines).in_set(RenderSet::Prepare),
        )
        .add_systems(Render, write_ocean_uniforms.in_set(RenderSet::PrepareResources))
        .add_systems(
            Render,
            prepare_ocean_bind_groups.in_set(RenderSet::PrepareBindGroups),
        );
    }

    fn finish(&self, app: &mut App) {
        let (spectrum, stockham, unpack) = {
            let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
            (
                shaders.add(Shader::from_wgsl(
                    include_str!("ocean_spectrum.wgsl"),
                                              "ocean_spectrum.wgsl",
                )),
             shaders.add(Shader::from_wgsl(include_str!("stockham.wgsl"), "stockham.wgsl")),
             shaders.add(Shader::from_wgsl(
                 include_str!("ocean_unpack.wgsl"),
                                           "ocean_unpack.wgsl",
             )),
            )
        };

        let render_app = app.sub_app_mut(RenderApp);
        render_app.insert_resource(OceanShaderHandles { spectrum, stockham, unpack });
        render_app
        .add_render_graph_node::<OceanComputeNode>(Core3d, OceanComputeLabel)
        .add_render_graph_edge(Core3d, OceanComputeLabel, Node3d::StartMainPass);
    }
}

// =============================================================================
// Setup main world
// =============================================================================

fn make_ocean_texture(
    images: &mut Assets<Image>,
    label: &'static str,
    format: TextureFormat,
        bytes_per_texel: u32,
) -> Handle<Image> {
    let mut img = Image::new(
        Extent3d {
            width: OCEAN_SIZE,
            height: OCEAN_SIZE,
            depth_or_array_layers: NUM_CASCADES,
        },
        TextureDimension::D2,
        vec![0u8; (OCEAN_SIZE * OCEAN_SIZE * NUM_CASCADES * bytes_per_texel) as usize],
                             format,
                                 RenderAssetUsages::RENDER_WORLD,
    );
    img.texture_descriptor.usage = TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING;
    img.texture_descriptor.label = Some(label);
    img.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
                                       ..default()
    });
    img.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
        address_mode_u: bevy::image::ImageAddressMode::Repeat,
        address_mode_v: bevy::image::ImageAddressMode::Repeat,
        address_mode_w: bevy::image::ImageAddressMode::Repeat,
        mag_filter: bevy::image::ImageFilterMode::Linear,
        min_filter: bevy::image::ImageFilterMode::Linear,
        mipmap_filter: bevy::image::ImageFilterMode::Linear,
        ..default()
    });
    images.add(img)
}

fn setup_ocean_textures(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(OceanAssets {
        displacement: make_ocean_texture(&mut images, "ocean_displacement", TextureFormat::Rgba16Float, 8),
                             normal: make_ocean_texture(&mut images, "ocean_normal", TextureFormat::Rgba16Float, 8),
                             foam: make_ocean_texture(&mut images, "ocean_foam", TextureFormat::R32Float, 4),
    });
}

fn setup_ocean_plane(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<OceanMaterial>>,
    assets: Res<OceanAssets>,
) {
    let mesh = meshes.add(
        Plane3d::default()
        .mesh()
        .size(PLANE_SIZE, PLANE_SIZE)
        .subdivisions(PLANE_SUBDIVISIONS),
    );
    let material = materials.add(OceanMaterial {
        params: OceanParams::default(),
                                 displacement: assets.displacement.clone(),
                                 normal: assets.normal.clone(),
                                 foam: assets.foam.clone(),
    });

    commands.spawn((
        Mesh3d(mesh),
                    MeshMaterial3d(material),
                    Transform::from_xyz(
                        0.0,
                        crate::data::heightmap::OCEAN_ELEVATION * crate::data::terrain_mesh::TERRAIN_HEIGHT_SCALE,
                        0.0,
                    ),
                    OceanPlane,
                    NotShadowCaster,
                    NotShadowReceiver,
                    NoFrustumCulling,
    ));
}

pub fn update_ocean_plane(
    player: Query<&Transform, With<crate::player::Player>>,
    mut ocean: Query<&mut Transform, (With<OceanPlane>, Without<crate::player::Player>)>,
) {
    let Ok(pt) = player.get_single() else {
        return;
    };
    let Ok(mut ot) = ocean.get_single_mut() else {
        return;
    };
    ot.translation.x = (pt.translation.x / PLANE_SPACING).round() * PLANE_SPACING;
    ot.translation.z = (pt.translation.z / PLANE_SPACING).round() * PLANE_SPACING;
    ot.translation.y =
    crate::data::heightmap::OCEAN_ELEVATION * crate::data::terrain_mesh::TERRAIN_HEIGHT_SCALE;
}

fn update_ocean_lighting(
    suns: Query<(&GlobalTransform, &DirectionalLight)>,
                         ocean: Query<&MeshMaterial3d<OceanMaterial>, With<OceanPlane>>,
                         mut materials: ResMut<Assets<OceanMaterial>>,
) {
    let Ok(handle) = ocean.get_single() else {
        return;
    };
    let Some((gt, light)) = suns
    .iter()
    .max_by(|a, b| a.1.illuminance.total_cmp(&b.1.illuminance))
    else {
        return;
    };
    let forward: Vec3 = *gt.forward();
    let to_sun = -forward;
    let color: LinearRgba = light.color.into();
    let sun_dir = Vec4::new(to_sun.x, to_sun.y, to_sun.z, (light.illuminance / 10_000.0).clamp(0.0, 3.0));
    let sun_color = Vec4::new(color.red, color.green, color.blue, 1.0);

    let Some(current) = materials.get(&handle.0) else {
        return;
    };
    if current.params.sun_dir == sun_dir && current.params.sun_color == sun_color {
        return;
    }
    if let Some(m) = materials.get_mut(&handle.0) {
        m.params.sun_dir = sun_dir;
        m.params.sun_color = sun_color;
    }
}

// =============================================================================
// Render world: buffer GPU
// =============================================================================

fn ensure_ocean_gpu_buffers(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    existing: Option<Res<OceanGpuBuffers>>,
) {
    if existing.is_some() {
        return;
    }

    let fft_size = (OCEAN_SIZE * OCEAN_SIZE) as u64 * 8 * 4 * NUM_CASCADES as u64;
    let make_storage = |label: &'static str| {
        render_device.create_buffer(&BufferDescriptor {
            label: Some(label),
                                    size: fft_size,
                                    usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
                                    mapped_at_creation: false,
        })
    };
    let make_uniform = |label: &'static str, size: u64| {
        render_device.create_buffer(&BufferDescriptor {
            label: Some(label),
                                    size: size.next_multiple_of(16),
                                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                                    mapped_at_creation: false,
        })
    };

    commands.insert_resource(OceanGpuBuffers {
        fft_a: make_storage("ocean_fft_a"),
                             fft_b: make_storage("ocean_fft_b"),
                             spectrum_uniform: make_uniform("ocean_spectrum_uniform", SpectrumParams::min_size().get()),
                             unpack_uniform: make_uniform("ocean_unpack_uniform", UnpackParams::min_size().get()),
    });
}

// =============================================================================
// Render world: scrittura uniform
// =============================================================================

fn write_ocean_uniforms(
    mut ocean_time: ResMut<OceanTime>,
    time: Res<Time>,
    gpu: Option<Res<OceanGpuBuffers>>,
    render_queue: Res<RenderQueue>,
) {
    let Some(gpu) = gpu else { return };
    ocean_time.elapsed += time.delta_secs();

    use bevy::render::render_resource::encase::UniformBuffer;

    let mut sbuf = UniformBuffer::new(Vec::new());
    if sbuf.write(&spectrum_params(ocean_time.elapsed)).is_ok() {
        render_queue.write_buffer(&gpu.spectrum_uniform, 0, sbuf.as_ref());
    }

    let unpack = UnpackParams {
        choppiness: CHOPPINESS,
        foam_threshold: FOAM_THRESHOLD,
        foam_grow: FOAM_GROW,
        foam_decay: FOAM_DECAY,
        delta_time: time.delta_secs(),
        num_cascades: NUM_CASCADES,
        _pad0: 0.0,
        _pad1: 0.0,
    };
    let mut ubuf = UniformBuffer::new(Vec::new());
    if ubuf.write(&unpack).is_ok() {
        render_queue.write_buffer(&gpu.unpack_uniform, 0, ubuf.as_ref());
    }
}

// =============================================================================
// Render world: pipeline
// =============================================================================

fn prepare_ocean_pipelines(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    shader_handles: Res<OceanShaderHandles>,
    existing: Option<Res<OceanPipelines>>,
) {
    if existing.is_some() {
        return;
    }

    let spectrum_layout = render_device.create_bind_group_layout(
        "ocean_spectrum_layout",
        &[
            uniform_buffer::<SpectrumParams>(false).build(0, ShaderStages::COMPUTE),
                                                                 storage_buffer_sized(false, None).build(1, ShaderStages::COMPUTE),
        ],
    );
    let fft_layout = render_device.create_bind_group_layout(
        "ocean_fft_layout",
        &[
            storage_buffer_read_only_sized(false, None).build(0, ShaderStages::COMPUTE),
                                                            storage_buffer_sized(false, None).build(1, ShaderStages::COMPUTE),
        ],
    );
    let unpack_layout = render_device.create_bind_group_layout(
        "ocean_unpack_layout",
        &[
            uniform_buffer::<UnpackParams>(false).build(0, ShaderStages::COMPUTE),
                                                               storage_buffer_read_only_sized(false, None).build(1, ShaderStages::COMPUTE),
                                                               texture_storage_2d_array(TextureFormat::Rgba16Float, StorageTextureAccess::WriteOnly)
                                                               .build(2, ShaderStages::COMPUTE),
                                                               texture_storage_2d_array(TextureFormat::Rgba16Float, StorageTextureAccess::WriteOnly)
                                                               .build(3, ShaderStages::COMPUTE),
                                                               texture_storage_2d_array(TextureFormat::R32Float, StorageTextureAccess::ReadWrite)
                                                               .build(4, ShaderStages::COMPUTE),
        ],
    );

    let queue = |label: &'static str, layout: &BindGroupLayout, shader: &Handle<Shader>, entry: &'static str| {
        pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(label.into()),
                                              layout: vec![layout.clone()],
                                              push_constant_ranges: vec![],
                                              shader: shader.clone(),
                                              shader_defs: vec![],
                                              entry_point: entry.into(),
                                              zero_initialize_workgroup_memory: false,
        })
    };

    let spectrum = queue("ocean_spectrum_pipeline", &spectrum_layout, &shader_handles.spectrum, "main");
    let fft_h = queue("ocean_fft_h_pipeline", &fft_layout, &shader_handles.stockham, "horizontal");
    let fft_v = queue("ocean_fft_v_pipeline", &fft_layout, &shader_handles.stockham, "vertical");
    let unpack = queue("ocean_unpack_pipeline", &unpack_layout, &shader_handles.unpack, "main");

    commands.insert_resource(OceanPipelines {
        spectrum_layout,
        spectrum,
        fft_layout,
        fft_h,
        fft_v,
        unpack_layout,
        unpack,
    });
}

// =============================================================================
// Render world: bind group
// =============================================================================

fn prepare_ocean_bind_groups(
    mut commands: Commands,
    assets: Option<Res<OceanAssets>>,
    gpu: Option<Res<OceanGpuBuffers>>,
    pipelines: Option<Res<OceanPipelines>>,
    render_device: Res<RenderDevice>,
    images: Res<RenderAssets<GpuImage>>,
    existing: Option<Res<OceanBindGroups>>,
) {
    if existing.is_some() {
        return;
    }
    let (Some(assets), Some(gpu), Some(pipelines)) = (assets, gpu, pipelines) else {
        return;
    };
    let (Some(disp), Some(norm), Some(foam)) = (
        images.get(&assets.displacement),
                                                images.get(&assets.normal),
                                                images.get(&assets.foam),
    ) else {
        return;
    };

    let spectrum = render_device.create_bind_group(
        "ocean_spectrum_bind_group",
        &pipelines.spectrum_layout,
        &[
            BindGroupEntry { binding: 0, resource: gpu.spectrum_uniform.as_entire_binding() },
                                                   BindGroupEntry { binding: 1, resource: gpu.fft_a.as_entire_binding() },
        ],
    );
    let fft_h = render_device.create_bind_group(
        "ocean_fft_h_bind_group",
        &pipelines.fft_layout,
        &[
            BindGroupEntry { binding: 0, resource: gpu.fft_a.as_entire_binding() },
                                                BindGroupEntry { binding: 1, resource: gpu.fft_b.as_entire_binding() },
        ],
    );
    let fft_v = render_device.create_bind_group(
        "ocean_fft_v_bind_group",
        &pipelines.fft_layout,
        &[
            BindGroupEntry { binding: 0, resource: gpu.fft_b.as_entire_binding() },
                                                BindGroupEntry { binding: 1, resource: gpu.fft_a.as_entire_binding() },
        ],
    );
    let unpack = render_device.create_bind_group(
        "ocean_unpack_bind_group",
        &pipelines.unpack_layout,
        &[
            BindGroupEntry { binding: 0, resource: gpu.unpack_uniform.as_entire_binding() },
                                                 BindGroupEntry { binding: 1, resource: gpu.fft_a.as_entire_binding() },
                                                 BindGroupEntry { binding: 2, resource: BindingResource::TextureView(&disp.texture_view) },
                                                 BindGroupEntry { binding: 3, resource: BindingResource::TextureView(&norm.texture_view) },
                                                 BindGroupEntry { binding: 4, resource: BindingResource::TextureView(&foam.texture_view) },
        ],
    );

    commands.insert_resource(OceanBindGroups { spectrum, fft_h, fft_v, unpack });
}
