//! Post-processing "finto rilievo" (porting di DFU_FakeRelief.fx per Bevy 0.15).
//!
//! Usa il colore della scena come mappa d'altezza (chiaro = rilievo, scuro = incavo): ricostruisce le
//! normali dalla depth, le perturba col gradiente della luminanza, rilluminata la superficie e aggiunge
//! cavita', annerimento degli interstizi, parallax a schermo, speculare e filtro pelle. Il codice dello
//! shader e' in `fake_relief.wgsl` (nella stessa cartella di questo file, incluso a compile time).
//!
//! Integrazione, in `main.rs`:
//! ```ignore
//! mod relief_post;
//! // ...
//! .add_plugins(relief_post::FakeReliefPlugin)
//! ```
//! Il plugin aggiunge da solo alle `Camera3d` il componente `FakeRelief`, il `DepthPrepass` (serve la
//! depth) e `Msaa::Off` (la depth multicampione non e' leggibile dallo shader). L'effetto parte dopo il
//! tonemapping e non supporta le camere HDR (le salta).
//!
//! Tasti: F8 attiva/disattiva, F7 cambia modalita' (classico / bump map), F9 scorre le viste di debug.
//! I parametri sono i campi pubblici di `FakeRelief` e si possono cambiare a runtime sulla camera.

use bevy::asset::load_internal_asset;
use bevy::core_pipeline::{
    core_3d::graph::{Core3d, Node3d},
    fullscreen_vertex_shader::fullscreen_shader_vertex_state,
    prepass::{DepthPrepass, ViewPrepassTextures},
};
use bevy::ecs::query::QueryItem;
use bevy::prelude::*;
use bevy::render::{
    camera::ExtractedCamera,
    extract_component::{
        ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin, UniformComponentPlugin,
    },
    render_graph::{NodeRunError, RenderGraphApp, RenderGraphContext, RenderLabel, ViewNode, ViewNodeRunner},
    render_resource::{
        binding_types::{sampler, texture_2d, texture_depth_2d, uniform_buffer},
        *,
    },
    renderer::{RenderContext, RenderDevice},
    texture::{CachedTexture, TextureCache},
    view::ViewTarget,
    Render, RenderApp, RenderSet,
};

const SHADER_HANDLE: Handle<Shader> = Handle::weak_from_u128(0x7C4A_91E2_5D03_4B8F_A2C1_6E09_D3F7_12AB);

/// Divisore di risoluzione della media locale (le texture intermedie sono 1/4 per lato).
const MEAN_DIV: u32 = 4;

/// Parametri dell'effetto (stessi nomi e valori di default di DFU_FakeRelief.fx).
/// I campi `u32` fanno da booleani (0 = no, 1 = si).
#[derive(Component, Clone, Copy, ExtractComponent, ShaderType)]
pub struct FakeRelief {
    /// 0 = classico (gradiente diretto della luminanza), 1 = bump map (mappa altezza).
    pub mode: u32,
    /// Seconda scala del gradiente (dettaglio grossolano).
    pub coarse_scale: u32,
    pub skin_enable: u32,
    /// 0 off, 1 shading, 2 cavita', 3 fattore bordi, 4 maschera pelle, 5 mappa bump, 6 normal map.
    pub debug: u32,
    pub enabled: u32,

    /// Quanto vengono inclinate le normali (negativo = rilievo invertito).
    pub strength: f32,
    /// Larghezza tipica dei dettagli, in unita' di gioco.
    pub detail_size: f32,
    /// Incavi scuri / bordi chiari.
    pub cavity: f32,
    /// Raggio in pixel del filtro che stima la luce locale.
    pub light_scale: f32,
    pub bump_contrast: f32,
    pub bump_smooth: f32,
    /// Annerimento delle zone piu' scure della media locale (fughe fra i mattoni).
    pub mortar: f32,
    pub mortar_threshold: f32,
    /// Spostamento massimo (pixel) della texture in base all'angolo di vista.
    pub parallax: f32,
    pub spec: f32,
    pub shininess: f32,
    /// Direzione della luce: azimut ed elevazione in gradi (spazio camera).
    pub light_az: f32,
    pub light_el: f32,
    /// 0 = luce fissa, 1 = la luce arriva sempre dalla camera.
    pub torch_blend: f32,
    pub skin_hue: f32,
    pub skin_hue_tol: f32,
    pub skin_sat: f32,
    pub skin_sat_tol: f32,
    pub skin_val: f32,
    pub skin_val_tol: f32,
    /// Oltre questa distanza (unita' di gioco) un pixel e' trattato come cielo.
    pub far_plane: f32,
    pub fade_distance: f32,
    /// Letti dalla proiezione della camera: non servono a mano.
    pub near: f32,
    pub tan_half_fov: f32,
    /// Deve coincidere con `MEAN_DIV`.
    pub mean_div: f32,
}

impl Default for FakeRelief {
    fn default() -> Self {
        Self {
            mode: 0,
            coarse_scale: 1,
            skin_enable: 0,
            debug: 0,
            enabled: 1,
            strength: 1.6,
            detail_size: 0.05,
            cavity: 0.8,
            light_scale: 40.0,
            bump_contrast: 1.3,
            bump_smooth: 1.0,
            mortar: 0.6,
            mortar_threshold: 0.15,
            parallax: 3.0,
            spec: 0.0,
            shininess: 24.0,
            light_az: -35.0,
            light_el: 50.0,
            torch_blend: 0.4,
            skin_hue: 20.0,
            skin_hue_tol: 25.0,
            skin_sat: 0.45,
            skin_sat_tol: 0.25,
            skin_val: 0.65,
            skin_val_tol: 0.3,
            far_plane: 1000.0,
            fade_distance: 80.0,
            near: 0.3,
            tan_half_fov: (60.0_f32.to_radians() * 0.5).tan(),
            mean_div: MEAN_DIV as f32,
        }
    }
}

pub struct FakeReliefPlugin;

impl Plugin for FakeReliefPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, SHADER_HANDLE, "fake_relief.wgsl", Shader::from_wgsl);

        app.add_plugins((
            ExtractComponentPlugin::<FakeRelief>::default(),
            UniformComponentPlugin::<FakeRelief>::default(),
        ))
        .add_systems(Update, (attach_to_cameras, update_from_camera, hotkeys).chain());

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .add_systems(Render, prepare_relief_textures.in_set(RenderSet::PrepareResources))
            .add_render_graph_node::<ViewNodeRunner<FakeReliefNode>>(Core3d, FakeReliefLabel)
            .add_render_graph_edges(
                Core3d,
                (Node3d::Tonemapping, FakeReliefLabel, Node3d::EndMainPassPostProcessing),
            );
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app.init_resource::<ReliefPipelines>();
    }
}

// ---------------------------------------------------------------------------
// Lato main world
// ---------------------------------------------------------------------------

/// Ogni `Camera3d` riceve l'effetto, la depth prepass e Msaa spento.
fn attach_to_cameras(mut commands: Commands, cameras: Query<Entity, (With<Camera3d>, Without<FakeRelief>)>) {
    for entity in &cameras {
        commands.entity(entity).insert((FakeRelief::default(), DepthPrepass, Msaa::Off));
    }
}

/// Near plane e FOV vengono dalla proiezione della camera.
fn update_from_camera(mut cameras: Query<(&mut FakeRelief, &Projection)>) {
    for (mut relief, projection) in &mut cameras {
        if let Projection::Perspective(p) = projection {
            let tan_half = (p.fov * 0.5).tan();
            if relief.near != p.near || relief.tan_half_fov != tan_half {
                relief.near = p.near;
                relief.tan_half_fov = tan_half;
            }
        }
    }
}

fn hotkeys(keys: Res<ButtonInput<KeyCode>>, mut cameras: Query<&mut FakeRelief>) {
    for mut relief in &mut cameras {
        if keys.just_pressed(KeyCode::F8) {
            relief.enabled = 1 - relief.enabled;
            info!("Finto rilievo: {}", if relief.enabled == 1 { "attivo" } else { "spento" });
        }
        if keys.just_pressed(KeyCode::F7) {
            relief.mode = 1 - relief.mode;
            info!("Finto rilievo: modalita' {}", if relief.mode == 1 { "bump map" } else { "classica" });
        }
        if keys.just_pressed(KeyCode::F9) {
            relief.debug = (relief.debug + 1) % 7;
            info!("Finto rilievo: vista debug {}", relief.debug);
        }
    }
}

// ---------------------------------------------------------------------------
// Lato render world
// ---------------------------------------------------------------------------

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct FakeReliefLabel;

/// Texture intermedie di una vista: luminanza ridotta, temporanea del blur, media locale e mappa bump.
#[derive(Component)]
struct ReliefTextures {
    a: CachedTexture,
    b: CachedTexture,
    mean: CachedTexture,
    height: CachedTexture,
}

fn prepare_relief_textures(
    mut commands: Commands,
    mut texture_cache: ResMut<TextureCache>,
    render_device: Res<RenderDevice>,
    views: Query<(Entity, &ExtractedCamera), With<FakeRelief>>,
) {
    for (entity, camera) in &views {
        let Some(size) = camera.physical_viewport_size else { continue };
        let (w, h) = (size.x.max(1), size.y.max(1));
        let (low_w, low_h) = ((w + MEAN_DIV - 1) / MEAN_DIV, (h + MEAN_DIV - 1) / MEAN_DIV);

        let mut make = |label: &'static str, width: u32, height: u32| {
            texture_cache.get(
                &render_device,
                TextureDescriptor {
                    label: Some(label),
                    size: Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::R16Float,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let a = make("relief_a", low_w, low_h);
        let b = make("relief_b", low_w, low_h);
        let mean = make("relief_mean", low_w, low_h);
        let height = make("relief_height", w, h);
        commands.entity(entity).insert(ReliefTextures { a, b, mean, height });
    }
}

/// Pipeline e layout di ogni passata. I binding hanno numeri fissi condivisi con lo shader:
/// 0 scena, 1 depth, 2 sampler, 3 sorgente del blur, 4 media, 5 mappa bump, 6 parametri.
/// Ogni passata include solo le texture che legge, cosi' non legge mai quella su cui scrive.
#[derive(Resource)]
struct ReliefPipelines {
    sampler: Sampler,
    layout_down: BindGroupLayout,
    layout_blur: BindGroupLayout,
    layout_height: BindGroupLayout,
    layout_relief: BindGroupLayout,
    down: CachedRenderPipelineId,
    blur_h: CachedRenderPipelineId,
    blur_v: CachedRenderPipelineId,
    height: CachedRenderPipelineId,
    relief: CachedRenderPipelineId,
}

fn queue_pass(
    cache: &PipelineCache,
    label: &'static str,
    layout: &BindGroupLayout,
    entry_point: &'static str,
    format: TextureFormat,
) -> CachedRenderPipelineId {
    cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some(label.into()),
        layout: vec![layout.clone()],
        vertex: fullscreen_shader_vertex_state(),
        fragment: Some(FragmentState {
            shader: SHADER_HANDLE.clone(),
            shader_defs: vec![],
            entry_point: entry_point.into(),
            targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
        }),
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: MultisampleState::default(),
        push_constant_ranges: vec![],
        zero_initialize_workgroup_memory: false,
    })
}

impl FromWorld for ReliefPipelines {
    fn from_world(world: &mut World) -> Self {
        let render_device = world.resource::<RenderDevice>();
        let sampler_obj = render_device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        });

        let float_tex = || texture_2d(TextureSampleType::Float { filterable: true });
        let filtering = || sampler(SamplerBindingType::Filtering);
        let settings = || uniform_buffer::<FakeRelief>(true);

        let layout_down = render_device.create_bind_group_layout(
            "relief_down_layout",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                ((0, float_tex()), (2, filtering()), (6, settings())),
            ),
        );
        let layout_blur = render_device.create_bind_group_layout(
            "relief_blur_layout",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                ((2, filtering()), (3, float_tex()), (6, settings())),
            ),
        );
        let layout_height = render_device.create_bind_group_layout(
            "relief_height_layout",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                ((0, float_tex()), (2, filtering()), (4, float_tex()), (6, settings())),
            ),
        );
        let layout_relief = render_device.create_bind_group_layout(
            "relief_final_layout",
            &BindGroupLayoutEntries::with_indices(
                ShaderStages::FRAGMENT,
                (
                    (0, float_tex()),
                    (1, texture_depth_2d()),
                    (2, filtering()),
                    (4, float_tex()),
                    (5, float_tex()),
                    (6, settings()),
                ),
            ),
        );

        let cache = world.resource::<PipelineCache>();
        let low = TextureFormat::R16Float;
        let down = queue_pass(cache, "relief_down", &layout_down, "fragment_down", low);
        let blur_h = queue_pass(cache, "relief_blur_h", &layout_blur, "fragment_blur_h", low);
        let blur_v = queue_pass(cache, "relief_blur_v", &layout_blur, "fragment_blur_v", low);
        let height = queue_pass(cache, "relief_height", &layout_height, "fragment_height", low);
        let relief = queue_pass(cache, "relief_final", &layout_relief, "fragment_relief", TextureFormat::bevy_default());

        Self {
            sampler: sampler_obj,
            layout_down,
            layout_blur,
            layout_height,
            layout_relief,
            down,
            blur_h,
            blur_v,
            height,
            relief,
        }
    }
}

#[derive(Default)]
struct FakeReliefNode;

fn run_pass(
    render_context: &mut RenderContext,
    label: &'static str,
    pipeline: &RenderPipeline,
    bind_group: &BindGroup,
    target: &TextureView,
    settings_offset: u32,
) {
    let mut pass = render_context.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            ops: Operations::default(),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_render_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[settings_offset]);
    pass.draw(0..3, 0..1);
}

impl ViewNode for FakeReliefNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static FakeRelief,
        &'static DynamicUniformIndex<FakeRelief>,
        &'static ViewPrepassTextures,
        &'static ReliefTextures,
    );

    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        (view_target, settings, settings_index, prepass, textures): QueryItem<Self::ViewQuery>,
        world: &World,
    ) -> Result<(), NodeRunError> {
        if settings.enabled == 0 || view_target.main_texture_format() != TextureFormat::bevy_default() {
            return Ok(());
        }
        let pipelines = world.resource::<ReliefPipelines>();
        let cache = world.resource::<PipelineCache>();
        let (Some(down), Some(blur_h), Some(blur_v), Some(height), Some(relief)) = (
            cache.get_render_pipeline(pipelines.down),
            cache.get_render_pipeline(pipelines.blur_h),
            cache.get_render_pipeline(pipelines.blur_v),
            cache.get_render_pipeline(pipelines.height),
            cache.get_render_pipeline(pipelines.relief),
        ) else {
            return Ok(());
        };
        let Some(settings_binding) = world.resource::<ComponentUniforms<FakeRelief>>().uniforms().binding() else {
            return Ok(());
        };
        let Some(depth_view) = prepass.depth_view() else { return Ok(()) };

        let offset = settings_index.index();
        let post = view_target.post_process_write();
        let device = render_context.render_device();

        let bg_down = device.create_bind_group(
            "relief_down_bind_group",
            &pipelines.layout_down,
            &BindGroupEntries::with_indices(((0, post.source), (2, &pipelines.sampler), (6, settings_binding.clone()))),
        );
        let bg_blur_h = device.create_bind_group(
            "relief_blur_h_bind_group",
            &pipelines.layout_blur,
            &BindGroupEntries::with_indices((
                (2, &pipelines.sampler),
                (3, &textures.a.default_view),
                (6, settings_binding.clone()),
            )),
        );
        let bg_blur_v = device.create_bind_group(
            "relief_blur_v_bind_group",
            &pipelines.layout_blur,
            &BindGroupEntries::with_indices((
                (2, &pipelines.sampler),
                (3, &textures.b.default_view),
                (6, settings_binding.clone()),
            )),
        );
        let bg_height = device.create_bind_group(
            "relief_height_bind_group",
            &pipelines.layout_height,
            &BindGroupEntries::with_indices((
                (0, post.source),
                (2, &pipelines.sampler),
                (4, &textures.mean.default_view),
                (6, settings_binding.clone()),
            )),
        );
        let bg_relief = device.create_bind_group(
            "relief_final_bind_group",
            &pipelines.layout_relief,
            &BindGroupEntries::with_indices((
                (0, post.source),
                (1, depth_view),
                (2, &pipelines.sampler),
                (4, &textures.mean.default_view),
                (5, &textures.height.default_view),
                (6, settings_binding.clone()),
            )),
        );

        // media locale: luminanza 1/4 -> blur orizzontale -> blur verticale
        run_pass(render_context, "relief_down", down, &bg_down, &textures.a.default_view, offset);
        run_pass(render_context, "relief_blur_h", blur_h, &bg_blur_h, &textures.b.default_view, offset);
        run_pass(render_context, "relief_blur_v", blur_v, &bg_blur_v, &textures.mean.default_view, offset);
        // mappa bump, solo se serve
        if settings.mode == 1 || settings.debug == 5 {
            run_pass(render_context, "relief_height", height, &bg_height, &textures.height.default_view, offset);
        }
        // rilievo finale sull'immagine
        run_pass(render_context, "relief_final", relief, &bg_relief, post.destination, offset);
        Ok(())
    }
}
