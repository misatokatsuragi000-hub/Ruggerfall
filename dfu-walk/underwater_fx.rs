//! Post-processing di distorsione subacquea.
//!
//! Quando il giocatore è sott'acqua (`UnderwaterState.blend > 0`), il pass
//! distorce le UV dell'immagine renderizzata con due onde sinusoidali
//! sovrapposte, aggiunge un tint blu-verde e un vignettaggio radiale.
//!
//! ## Ordine nel render graph
//!
//! `Tonemapping → FakeReliefLabel → UnderwaterFxLabel → EndMainPassPostProcessing`.
//!
//! La distorsione deve girare **dopo** il finto rilievo: prima si illumina la
//! scena (rilievo), poi si "guarda attraverso l'acqua" (distorsione). Se
//! `underwater_fx` fosse prima, il rilievo rilavorerebbe i pixel già distorti
//! e l'effetto si perderebbe.
//!
//! ## Caustiche
//!
//! Sott'acqua il pass ricostruisce la posizione nel mondo di ogni pixel dalla depth
//! del prepass (`DepthPrepass`, che l'oceano aggiunge alle camere; contiene solo le
//! geometrie opache) e dalla matrice della vista, e aggiunge sulle superfici sommerse
//! il disegno delle caustiche, proiettato lungo la direzione del sole. Con MSAA la
//! depth del prepass e' multisample: ci sono due pipeline (`MULTISAMPLED`). Senza
//! `DepthPrepass` si usa una depth fittizia e le caustiche non compaiono (la
//! distorsione funziona comunque).
//!
//! ## Init delle pipeline in `finish()`
//!
//! `UnderwaterFxPipelines` contiene un `CachedRenderPipelineId` e un
//! `BindGroupLayout`, che richiedono `RenderDevice` e `PipelineCache`. Queste
//! due risorse vengono create da `RenderPlugin::finish()`, che gira **dopo**
//! tutti i `Plugin::build()`. Per questo `init_resource::<UnderwaterFxPipelines>()`
//! è in `finish()`, non in `build()`: se fosse in `build()`, la `FromWorld`
//! cercherebbe `RenderDevice` in un render app dove non esiste ancora e
//! andrebbe in panic. Stessa strategia di `relief_post.rs`.

use bevy::asset::load_internal_asset;
use bevy::core_pipeline::{
    core_3d::graph::{Core3d, Node3d},
    fullscreen_vertex_shader::fullscreen_shader_vertex_state,
    prepass::ViewPrepassTextures,
};
use bevy::ecs::query::QueryItem;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy::render::{
    extract_component::{
        ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
        UniformComponentPlugin,
    },
    render_graph::{
        NodeRunError, RenderGraphApp, RenderGraphContext, RenderLabel, ViewNode, ViewNodeRunner,
    },
    render_resource::{
        binding_types::{
            sampler, texture_2d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer,
        },
        *,
    },
    renderer::{RenderContext, RenderDevice},
    view::{ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
    RenderApp,
};

use crate::data::heightmap::OCEAN_ELEVATION;
use crate::data::terrain_mesh::TERRAIN_HEIGHT_SCALE;
use crate::underwater::{UnderwaterState, UNDERWATER_SUN_FACTOR};

const SHADER_HANDLE: Handle<Shader> =
    Handle::weak_from_u128(0xF1A2_B3C4_D5E6_7890_ABCD_EF01_2345_6789);

/// Livello del mare in metri (coerente con `underwater.rs` e con l'oceano).
const WATER_Y: f32 = OCEAN_ELEVATION * TERRAIN_HEIGHT_SCALE;

/// Intensita' delle caustiche (0 = spente). Si regola anche da `UnderwaterFx` sulla camera.
const CAUSTICS_STRENGTH: f32 = 2.0;

/// Deve coincidere con `UnderwaterFx` in underwater_fx.wgsl (stesso ordine dei campi).
#[derive(Component, Clone, Copy, ExtractComponent, ShaderType)]
pub struct UnderwaterFx {
    pub enabled: u32,
    pub time: f32,
    pub blend: f32,
    pub amplitude: f32,
    pub tint: Vec4,
    pub tint_strength: f32,
    pub vignette_strength: f32,
    /// Intensita' delle caustiche (0 = spente).
    pub caustics_strength: f32,
    /// Quota del livello del mare (m).
    pub water_y: f32,
    /// xyz = direzione verso il sole, w = intensita' (1 = sole pieno).
    pub sun_dir: Vec4,
    pub sun_color: Vec4,
    /// x = start, y = end della nebbia (m).
    pub fog_range: Vec4,
}

impl Default for UnderwaterFx {
    fn default() -> Self {
        Self {
            enabled: 0,
            time: 0.0,
            blend: 0.0,
            amplitude: 0.006,
            tint: Vec4::new(0.55, 0.85, 0.95, 1.0),
            tint_strength: 0.0,
            vignette_strength: 0.0,
            caustics_strength: CAUSTICS_STRENGTH,
            water_y: WATER_Y,
            sun_dir: Vec4::new(0.3, 0.8, 0.4, 0.0),
            sun_color: Vec4::new(1.0, 0.95, 0.85, 1.0),
            fog_range: Vec4::new(0.5, 500.0, 0.0, 0.0),
        }
    }
}

pub struct UnderwaterFxPlugin;

impl Plugin for UnderwaterFxPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, SHADER_HANDLE, "underwater_fx.wgsl", Shader::from_wgsl);

        app.add_plugins((
            ExtractComponentPlugin::<UnderwaterFx>::default(),
            UniformComponentPlugin::<UnderwaterFx>::default(),
        ))
        .add_systems(
            Update,
            (
                attach_to_cameras,
                // Dopo gli effetti subacquei: serve l'illuminance del sole gia' attenuata
                // da `apply_underwater_effects` per poterla riportare al valore di superficie.
                sync_from_state.after(crate::underwater::apply_underwater_effects),
            ),
        );

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .add_render_graph_node::<ViewNodeRunner<UnderwaterFxNode>>(Core3d, UnderwaterFxLabel)
            .add_render_graph_edges(
                Core3d,
                (
                    crate::relief_post::FakeReliefLabel,
                    UnderwaterFxLabel,
                    Node3d::EndMainPassPostProcessing,
                ),
            );
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        // `RenderDevice` e `PipelineCache` sono state create da
        // `RenderPlugin::finish()`, che gira prima di questo `finish()`.
        render_app.init_resource::<UnderwaterFxPipelines>();
    }
}

fn attach_to_cameras(
    mut commands: Commands,
    cameras: Query<Entity, (With<Camera3d>, Without<UnderwaterFx>)>,
) {
    for entity in &cameras {
        commands.entity(entity).insert(UnderwaterFx::default());
    }
}

fn sync_from_state(
    time: Res<Time>,
    state: Res<UnderwaterState>,
    suns: Query<(&GlobalTransform, &DirectionalLight)>,
    mut fx: Query<(&mut UnderwaterFx, Option<&DistanceFog>)>,
) {
    let t = time.elapsed_secs();
    let active = state.blend > 0.001;

    // Sole: la luce piu' intensa. `apply_underwater_effects` ha gia' moltiplicato
    // l'illuminance per UNDERWATER_SUN_FACTOR (via via con il blend): si riporta
    // al valore di superficie, cosi' le caustiche seguono ora del giorno e meteo.
    let sun = suns
        .iter()
        .max_by(|a, b| a.1.illuminance.total_cmp(&b.1.illuminance));
    let (sun_dir, sun_color) = match sun {
        Some((gt, light)) => {
            let to_sun = -*gt.forward();
            let atten = (1.0 - state.blend * (1.0 - UNDERWATER_SUN_FACTOR)).max(0.05);
            let intensity = (light.illuminance / atten / 10_000.0).clamp(0.0, 1.5);
            let c: LinearRgba = light.color.into();
            (
                Vec4::new(to_sun.x, to_sun.y, to_sun.z, intensity),
                Vec4::new(c.red, c.green, c.blue, 1.0),
            )
        }
        None => (Vec4::new(0.3, 0.8, 0.4, 0.0), Vec4::ONE),
    };

    for (mut f, fog) in &mut fx {
        f.enabled = active as u32;
        f.time = t;
        f.blend = state.blend;
        f.sun_dir = sun_dir;
        f.sun_color = sun_color;
        if let Some(DistanceFog { falloff: FogFalloff::Linear { start, end }, .. }) = fog {
            f.fog_range = Vec4::new(*start, *end, 0.0, 0.0);
        }
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct UnderwaterFxLabel;

#[derive(Resource)]
struct UnderwaterFxPipelines {
    sampler: Sampler,
    layout: BindGroupLayout,
    pipeline: CachedRenderPipelineId,
    /// Variante per la depth del prepass multisample (MSAA attivo).
    layout_ms: BindGroupLayout,
    pipeline_ms: CachedRenderPipelineId,
    /// Depth fittizia 1x1 (valore 0 = "cielo"), usata se la camera non ha `DepthPrepass`.
    dummy_depth: TextureView,
}

impl FromWorld for UnderwaterFxPipelines {
    fn from_world(world: &mut World) -> Self {
        let render_device = world.resource::<RenderDevice>();
        // Rinomino: `sampler` collide con `binding_types::sampler(...)`.
        let sampler_obj = render_device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        });

        let make_layout = |label: &'static str, multisampled: bool| {
            render_device.create_bind_group_layout(
                label,
                &BindGroupLayoutEntries::with_indices(
                    ShaderStages::FRAGMENT,
                    (
                        (0, texture_2d(TextureSampleType::Float { filterable: true })),
                        (1, sampler(SamplerBindingType::Filtering)),
                        (2, uniform_buffer::<UnderwaterFx>(true)),
                        (
                            3,
                            if multisampled {
                                texture_depth_2d_multisampled()
                            } else {
                                texture_depth_2d()
                            },
                        ),
                        (4, uniform_buffer::<ViewUniform>(true)),
                    ),
                ),
            )
        };
        let layout = make_layout("underwater_fx_layout", false);
        let layout_ms = make_layout("underwater_fx_layout_ms", true);

        let dummy_texture = render_device.create_texture(&TextureDescriptor {
            label: Some("underwater_fx_dummy_depth"),
            size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Depth32Float,
            usage: TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let dummy_depth = dummy_texture.create_view(&TextureViewDescriptor::default());

        let cache = world.resource::<PipelineCache>();
        let queue = |label: &'static str, layout: &BindGroupLayout, defs: Vec<ShaderDefVal>| {
            cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some(label.into()),
                layout: vec![layout.clone()],
                vertex: fullscreen_shader_vertex_state(),
                fragment: Some(FragmentState {
                    shader: SHADER_HANDLE.clone(),
                    shader_defs: defs,
                    entry_point: "fragment".into(),
                    targets: vec![Some(ColorTargetState {
                        format: TextureFormat::bevy_default(),
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    })],
                }),
                primitive: PrimitiveState::default(),
                depth_stencil: None,
                multisample: MultisampleState::default(),
                push_constant_ranges: vec![],
                zero_initialize_workgroup_memory: false,
            })
        };
        let pipeline = queue("underwater_fx_pipeline", &layout, vec![]);
        let pipeline_ms = queue("underwater_fx_pipeline_ms", &layout_ms, vec!["MULTISAMPLED".into()]);

        Self {
            sampler: sampler_obj,
            layout,
            pipeline,
            layout_ms,
            pipeline_ms,
            dummy_depth,
        }
    }
}

#[derive(Default)]
struct UnderwaterFxNode;

impl ViewNode for UnderwaterFxNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static UnderwaterFx,
        &'static DynamicUniformIndex<UnderwaterFx>,
        &'static ViewUniformOffset,
        Option<&'static ViewPrepassTextures>,
    );

    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        (view_target, fx, fx_index, view_offset, prepass): QueryItem<Self::ViewQuery>,
        world: &World,
    ) -> Result<(), NodeRunError> {
        if fx.enabled == 0 || view_target.main_texture_format() != TextureFormat::bevy_default() {
            return Ok(());
        }

        let pipelines = world.resource::<UnderwaterFxPipelines>();

        // Depth del prepass della camera (o quella fittizia) e se e' multisample.
        let (depth_view, multisampled) = match prepass.and_then(|p| p.depth.as_ref()) {
            Some(d) => (&d.texture.default_view, d.texture.texture.sample_count() > 1),
            None => (&pipelines.dummy_depth, false),
        };
        let (layout, pipeline_id) = if multisampled {
            (&pipelines.layout_ms, pipelines.pipeline_ms)
        } else {
            (&pipelines.layout, pipelines.pipeline)
        };

        let Some(pipeline) = world.resource::<PipelineCache>().get_render_pipeline(pipeline_id)
        else {
            return Ok(());
        };
        let Some(uniform_binding) = world
            .resource::<ComponentUniforms<UnderwaterFx>>()
            .uniforms()
            .binding()
        else {
            return Ok(());
        };
        let Some(view_binding) = world.resource::<ViewUniforms>().uniforms.binding() else {
            return Ok(());
        };

        let post = view_target.post_process_write();
        let device = render_context.render_device();

        let bind_group = device.create_bind_group(
            "underwater_fx_bind_group",
            layout,
            &BindGroupEntries::with_indices((
                (0, post.source),
                (1, &pipelines.sampler),
                (2, uniform_binding),
                (3, depth_view),
                (4, view_binding),
            )),
        );

        let mut pass = render_context.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("underwater_fx_pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post.destination,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_render_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[fx_index.index(), view_offset.offset]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}
