//! Porting di DFU_FakeRelief.fx a Bevy 0.15 come ViewNode multi-pass.
//! 5 pass: Luminance → BlurH → BlurV → Height → Relief.
use bevy::{
    color::LinearRgba,
    core_pipeline::core_3d::graph::{Core3d, Node3d},
    ecs::query::QueryItem,
    prelude::*,
    render::{
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_graph::{self, RenderGraphApp, RenderLabel, ViewNode, ViewNodeRunner},
        render_resource::*,
        renderer::{RenderContext, RenderDevice},
        texture::{CachedTexture, TextureCache},
        view::{window::ExtractedWindows, ViewDepthTexture, ViewTarget},
        Render, RenderApp, RenderSet,
    },
};

// ============================================================================
// Settings (app-world + render-world)
// ============================================================================
#[derive(Resource, Clone)]
pub struct FakeReliefSettings {
    pub enabled: bool,
    pub strength: f32,
    pub detail_size: f32,
    pub coarse_scale: bool,
    pub cavity: f32,
    pub light_scale: f32,
    pub mortar: f32,
    pub mortar_threshold: f32,
    pub parallax: f32,
    pub spec: f32,
    pub shininess: f32,
    pub light_az: f32,
    pub light_el: f32,
    pub torch_blend: f32,
    pub fov: f32,
    pub far_plane: f32,
    pub near_plane: f32,
    pub fade_distance: f32,
    pub bump_contrast: f32,
    pub bump_smooth: f32,
    pub debug: u32,
    pub mode: u32,
    pub skin_enable: bool,
    pub skin_hue: f32,
    pub skin_hue_tol: f32,
    pub skin_sat: f32,
    pub skin_sat_tol: f32,
    pub skin_val: f32,
    pub skin_val_tol: f32,
}

impl Default for FakeReliefSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            strength: 2.2,
            detail_size: 0.05,
            coarse_scale: true,
            cavity: 1.0,
            light_scale: 40.0,
            mortar: 0.8,
            mortar_threshold: 0.15,
            parallax: 3.5,
            spec: 0.15,
            shininess: 24.0,
            light_az: -35.0,
            light_el: 50.0,
            torch_blend: 0.4,
            fov: 70.0,
            far_plane: 60_000.0,
            near_plane: 0.3,
            fade_distance: 120.0,
            bump_contrast: 1.3,
            bump_smooth: 1.0,
            debug: 0,
            mode: 0,
            skin_enable: false,
            skin_hue: 20.0,
            skin_hue_tol: 25.0,
            skin_sat: 0.45,
            skin_sat_tol: 0.25,
            skin_val: 0.65,
            skin_val_tol: 0.3,
        }
    }
}

impl ExtractResource for FakeReliefSettings {
    type Source = Self;
    fn extract_resource(source: &Self::Source) -> Self {
        source.clone()
    }
}

/// Deve combaciare byte-per-byte con `struct Settings` nello shader WGSL.
#[derive(Clone, Copy, ShaderType)]
struct FakeReliefUniform {
    strength: f32,
    detail_size: f32,
    coarse_scale: u32,
    cavity: f32,
    light_scale: f32,
    mortar: f32,
    mortar_threshold: f32,
    parallax: f32,
    spec: f32,
    shininess: f32,
    light_az: f32,
    light_el: f32,
    torch_blend: f32,
    fov: f32,
    far_plane: f32,
    near_plane: f32,
    fade_distance: f32,
    bump_contrast: f32,
    bump_smooth: f32,
    skin_enable: u32,
    skin_hue: f32,
    skin_hue_tol: f32,
    skin_sat: f32,
    skin_sat_tol: f32,
    skin_val: f32,
    skin_val_tol: f32,
    mode: u32,
    debug: u32,
    screen_size: Vec2,
    _pad0: Vec2,
}

// ============================================================================
// Render-world resources
// ============================================================================
#[derive(Resource)]
struct FakeReliefPipelines {
    lum_layout: BindGroupLayout,
    blur_layout: BindGroupLayout,
    height_layout: BindGroupLayout,
    relief_layout: BindGroupLayout,
    sampler_linear: Sampler,
    sampler_clamp: Sampler,
    lum: CachedRenderPipelineId,
    blur_h: CachedRenderPipelineId,
    blur_v: CachedRenderPipelineId,
    height: CachedRenderPipelineId,
    relief: CachedRenderPipelineId,
}

impl FromWorld for FakeReliefPipelines {
    fn from_world(world: &mut World) -> Self {
        let render_device = world.resource::<RenderDevice>();
        let pipeline_cache = world.resource::<PipelineCache>();
        let asset_server = world.resource::<AssetServer>();
        let shader = asset_server.load("shaders/fake_relief.wgsl");

        let make_layout = |extra: &[(u32, bool, bool)]| {
            let mut entries = vec![BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }];
            for &(binding, is_depth, is_sampler) in extra {
                let ty = if is_depth {
                    BindingType::Texture {
                        sample_type: TextureSampleType::Depth,
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    }
                } else if is_sampler {
                    BindingType::Sampler(SamplerBindingType::Filtering)
                } else {
                    BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    }
                };
                entries.push(BindGroupLayoutEntry {
                    binding,
                    visibility: ShaderStages::FRAGMENT,
                    ty,
                    count: None,
                });
            }
            render_device.create_bind_group_layout("fr_layout", &entries)
        };

        let lum_layout = make_layout(&[(1, false, false), (2, false, true)]);
        let blur_layout = make_layout(&[(1, false, false), (2, false, true)]);
        let height_layout = make_layout(&[
            (1, false, false),
                                        (2, false, true),
                                        (3, false, false),
                                        (4, false, true),
        ]);

        // DEVE COMBACIARE CON LO SHADER WGSL PASS_RELIEF
        let relief_layout = make_layout(&[
            (1, false, false), // color_tex
                                        (2, false, true),  // color_smp
                                        (3, false, false), // mean_tex
                                        (4, false, true),  // mean_smp
                                        (5, false, false), // height_tex
                                        (6, false, true),  // height_smp
                                        (7, true, false),  // depth_tex
        ]);

        let sampler_linear = render_device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: FilterMode::Linear,
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            ..default()
        });
        let sampler_clamp = sampler_linear.clone();

        let make_pipeline = |layout: &BindGroupLayout,
        entry: &'static str,
        defs: Vec<ShaderDefVal>,
        fmt: TextureFormat| {
            pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some(entry.into()),
                                                 layout: vec![layout.clone()],
                                                 push_constant_ranges: Vec::new(),
                                                 vertex: VertexState {
                                                     shader: shader.clone(),
                                                 entry_point: "vs_main".into(),
                                                 shader_defs: defs.clone(),
                                                 buffers: vec![],
                                                 },
                                                 primitive: PrimitiveState::default(),
                                                 depth_stencil: None,
                                                 multisample: MultisampleState::default(),
                                                 fragment: Some(FragmentState {
                                                     shader: shader.clone(),
                                                                entry_point: entry.into(),
                                                                shader_defs: defs,
                                                                targets: vec![Some(ColorTargetState {
                                                                    format: fmt,
                                                                        blend: None,
                                                                        write_mask: ColorWrites::ALL,
                                                                })],
                                                 }),
                                                 zero_initialize_workgroup_memory: false,
            })
        };

        let lum = make_pipeline(
            &lum_layout,
            "fs_main",
            vec!["PASS_LUMINANCE".into()],
                                TextureFormat::R16Float,
        );
        let blur_h = make_pipeline(
            &blur_layout,
            "fs_blur_h",
            vec!["PASS_BLUR".into()],
                                   TextureFormat::R16Float,
        );
        let blur_v = make_pipeline(
            &blur_layout,
            "fs_blur_v",
            vec!["PASS_BLUR".into()],
                                   TextureFormat::R16Float,
        );
        let height = make_pipeline(
            &height_layout,
            "fs_main",
            vec!["PASS_HEIGHT".into()],
                                   TextureFormat::R16Float,
        );
        let relief = make_pipeline(
            &relief_layout,
            "fs_main",
            vec!["PASS_RELIEF".into()],
                                   TextureFormat::bevy_default(),
        );

        Self {
            lum_layout,
            blur_layout,
            height_layout,
            relief_layout,
            sampler_linear,
            sampler_clamp,
            lum,
            blur_h,
            blur_v,
            height,
            relief,
        }
    }
}

#[derive(Resource)]
struct FakeReliefTextures {
    lum: CachedTexture,
    tmp: CachedTexture,
    mean: CachedTexture,
    height: CachedTexture,
}

#[derive(Resource, Default)]
struct FakeReliefUniformBuf(Option<UniformBuffer<FakeReliefUniform>>);

#[derive(Debug, PartialEq, Eq, Clone, Hash, RenderLabel)]
struct FakeReliefLabel;

// ============================================================================
// Node
// ============================================================================
#[derive(Default)]
struct FakeReliefNode;

impl ViewNode for FakeReliefNode {
    type ViewQuery = (&'static ViewTarget, &'static ViewDepthTexture);

    fn run<'w>(
        &self,
        _graph: &mut render_graph::RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        (view_target, depth): QueryItem<'w, Self::ViewQuery>,
               world: &'w World,
    ) -> Result<(), render_graph::NodeRunError> {
        let settings = world.resource::<FakeReliefSettings>();
        if !settings.enabled {
            return Ok(());
        }

        let pipelines = world.resource::<FakeReliefPipelines>();
        let textures = world.resource::<FakeReliefTextures>();
        let uniform_buf = world.resource::<FakeReliefUniformBuf>();
        let Some(uniform_buf) = &uniform_buf.0 else {
            return Ok(());
        };

        let pipeline_cache = world.resource::<PipelineCache>();
        let color_view = view_target.main_texture_view();
        let depth_view = depth.view();

        let Some(lum_p) = pipeline_cache.get_render_pipeline(pipelines.lum) else {
            return Ok(());
        };
        let Some(blur_h_p) = pipeline_cache.get_render_pipeline(pipelines.blur_h) else {
            return Ok(());
        };
        let Some(blur_v_p) = pipeline_cache.get_render_pipeline(pipelines.blur_v) else {
            return Ok(());
        };
        let Some(height_p) = pipeline_cache.get_render_pipeline(pipelines.height) else {
            return Ok(());
        };
        let Some(relief_p) = pipeline_cache.get_render_pipeline(pipelines.relief) else {
            return Ok(());
        };

        // --- Pass 1: Luminance ---
        {
            let bg = render_context.render_device().create_bind_group(
                "fr_lum_bg",
                &pipelines.lum_layout,
                &BindGroupEntries::sequential((
                    uniform_buf,
                    color_view,
                    &pipelines.sampler_linear,
                )),
            );
            let mut pass = render_context.begin_tracked_render_pass(RenderPassDescriptor {
                label: Some("fr_lum"),
                                                                    color_attachments: &[Some(RenderPassColorAttachment {
                                                                        view: &textures.lum.default_view,
                                                                        resolve_target: None,
                                                                        ops: Operations {
                                                                            load: LoadOp::Clear(LinearRgba::BLACK.into()),
                                                                                              store: StoreOp::Store,
                                                                        },
                                                                    })],
                                                                    depth_stencil_attachment: None,
                                                                    timestamp_writes: None,
                                                                    occlusion_query_set: None,
            });
            pass.set_render_pipeline(lum_p);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }

        // --- Pass 2: BlurH (lum -> tmp) ---
        {
            let bg = render_context.render_device().create_bind_group(
                "fr_blur_h_bg",
                &pipelines.blur_layout,
                &BindGroupEntries::sequential((
                    uniform_buf,
                    &textures.lum.default_view,
                    &pipelines.sampler_clamp,
                )),
            );
            let mut pass = render_context.begin_tracked_render_pass(RenderPassDescriptor {
                label: Some("fr_blur_h"),
                                                                    color_attachments: &[Some(RenderPassColorAttachment {
                                                                        view: &textures.tmp.default_view,
                                                                        resolve_target: None,
                                                                        ops: Operations {
                                                                            load: LoadOp::Clear(LinearRgba::BLACK.into()),
                                                                                              store: StoreOp::Store,
                                                                        },
                                                                    })],
                                                                    depth_stencil_attachment: None,
                                                                    timestamp_writes: None,
                                                                    occlusion_query_set: None,
            });
            pass.set_render_pipeline(blur_h_p);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }

        // --- Pass 3: BlurV (tmp -> mean) ---
        {
            let bg = render_context.render_device().create_bind_group(
                "fr_blur_v_bg",
                &pipelines.blur_layout,
                &BindGroupEntries::sequential((
                    uniform_buf,
                    &textures.tmp.default_view,
                    &pipelines.sampler_clamp,
                )),
            );
            let mut pass = render_context.begin_tracked_render_pass(RenderPassDescriptor {
                label: Some("fr_blur_v"),
                                                                    color_attachments: &[Some(RenderPassColorAttachment {
                                                                        view: &textures.mean.default_view,
                                                                        resolve_target: None,
                                                                        ops: Operations {
                                                                            load: LoadOp::Clear(LinearRgba::BLACK.into()),
                                                                                              store: StoreOp::Store,
                                                                        },
                                                                    })],
                                                                    depth_stencil_attachment: None,
                                                                    timestamp_writes: None,
                                                                    occlusion_query_set: None,
            });
            pass.set_render_pipeline(blur_v_p);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }

        // --- Pass 4: Height (lum + mean -> height) ---
        {
            let bg = render_context.render_device().create_bind_group(
                "fr_height_bg",
                &pipelines.height_layout,
                &BindGroupEntries::sequential((
                    uniform_buf,
                    &textures.lum.default_view,
                    &pipelines.sampler_linear,
                    &textures.mean.default_view,
                    &pipelines.sampler_clamp,
                )),
            );
            let mut pass = render_context.begin_tracked_render_pass(RenderPassDescriptor {
                label: Some("fr_height"),
                                                                    color_attachments: &[Some(RenderPassColorAttachment {
                                                                        view: &textures.height.default_view,
                                                                        resolve_target: None,
                                                                        ops: Operations {
                                                                            load: LoadOp::Clear(LinearRgba::BLACK.into()),
                                                                                              store: StoreOp::Store,
                                                                        },
                                                                    })],
                                                                    depth_stencil_attachment: None,
                                                                    timestamp_writes: None,
                                                                    occlusion_query_set: None,
            });
            pass.set_render_pipeline(height_p);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }

        // --- Pass 5: Relief (finale) ---
        let post = view_target.post_process_write();
        {
            let bg = render_context.render_device().create_bind_group(
                "fr_relief_bg",
                &pipelines.relief_layout,
                &BindGroupEntries::sequential((
                    uniform_buf,
                    post.source,                  // binding 1: color_tex
                    &pipelines.sampler_clamp,     // binding 2: color_smp
                    &textures.mean.default_view,  // binding 3: mean_tex
                    &pipelines.sampler_clamp,     // binding 4: mean_smp
                    &textures.height.default_view,// binding 5: height_tex
                    &pipelines.sampler_clamp,     // binding 6: height_smp
                    depth_view,                   // binding 7: depth_tex
                )),
            );
            let mut pass = render_context.begin_tracked_render_pass(RenderPassDescriptor {
                label: Some("fr_relief"),
                                                                    color_attachments: &[Some(RenderPassColorAttachment {
                                                                        view: post.destination,
                                                                        resolve_target: None,
                                                                        ops: Operations {
                                                                            load: LoadOp::Clear(LinearRgba::BLACK.into()),
                                                                                              store: StoreOp::Store,
                                                                        },
                                                                    })],
                                                                    depth_stencil_attachment: None,
                                                                    timestamp_writes: None,
                                                                    occlusion_query_set: None,
            });
            pass.set_render_pipeline(relief_p);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }

        Ok(())
    }
}

// ============================================================================
// Systems (prepare)
// ============================================================================
fn window_size(windows: &ExtractedWindows) -> (u32, u32) {
    windows
    .primary
    .and_then(|e| windows.windows.get(&e))
    .map(|w| (w.physical_width, w.physical_height))
    .unwrap_or((1920, 1080))
}

fn prepare_textures(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    mut texture_cache: ResMut<TextureCache>,
    windows: Res<ExtractedWindows>,
) {
    let (w, h) = window_size(&windows);
    let make = |cache: &mut TextureCache, label: &'static str| {
        cache.get(
            &render_device,
            TextureDescriptor {
                label: Some(label),
                  size: Extent3d {
                      width: w.max(1),
                  height: h.max(1),
                  depth_or_array_layers: 1,
                  },
                  mip_level_count: 1,
                  sample_count: 1,
                  dimension: TextureDimension::D2,
                  format: TextureFormat::R16Float,
                      usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                      view_formats: &[],
            },
        )
    };
    let lum = make(&mut texture_cache, "fr_lum");
    let tmp = make(&mut texture_cache, "fr_tmp");
    let mean = make(&mut texture_cache, "fr_mean");
    let height = make(&mut texture_cache, "fr_height");
    commands.insert_resource(FakeReliefTextures {
        lum,
        tmp,
        mean,
        height,
    });
}

fn prepare_uniform(
    mut commands: Commands,
    settings: Res<FakeReliefSettings>,
    windows: Res<ExtractedWindows>,
) {
    let (w, h) = window_size(&windows);
    let uniform = FakeReliefUniform {
        strength: settings.strength,
        detail_size: settings.detail_size,
        coarse_scale: settings.coarse_scale as u32,
        cavity: settings.cavity,
        light_scale: settings.light_scale,
        mortar: settings.mortar,
        mortar_threshold: settings.mortar_threshold,
        parallax: settings.parallax,
        spec: settings.spec,
        shininess: settings.shininess,
        light_az: settings.light_az,
        light_el: settings.light_el,
        torch_blend: settings.torch_blend,
        fov: settings.fov,
        far_plane: settings.far_plane,
        near_plane: settings.near_plane,
        fade_distance: settings.fade_distance,
        bump_contrast: settings.bump_contrast,
        bump_smooth: settings.bump_smooth,
        skin_enable: settings.skin_enable as u32,
        skin_hue: settings.skin_hue,
        skin_hue_tol: settings.skin_hue_tol,
        skin_sat: settings.skin_sat,
        skin_sat_tol: settings.skin_sat_tol,
        skin_val: settings.skin_val,
        skin_val_tol: settings.skin_val_tol,
        mode: settings.mode,
        debug: settings.debug,
        screen_size: Vec2::new(w as f32, h as f32),
        _pad0: Vec2::ZERO,
    };
    let buf = UniformBuffer::from(uniform);
    commands.insert_resource(FakeReliefUniformBuf(Some(buf)));
}

fn handle_input(keys: Res<ButtonInput<KeyCode>>, mut settings: ResMut<FakeReliefSettings>) {
    if keys.just_pressed(KeyCode::KeyK) {
        settings.enabled = !settings.enabled;
        println!(
            "FakeRelief: {}",
            if settings.enabled { "ATTIVO" } else { "DISATTIVO" }
        );
    }
    if keys.just_pressed(KeyCode::KeyJ) {
        settings.debug = match settings.debug {
            0 => 6,
            6 => 1,
            1 => 2,
            2 => 5,
            _ => 0,
        };
        let label = match settings.debug {
            0 => "Grafica normale",
            1 => "Debug: Shading",
            2 => "Debug: Cavità",
            5 => "Debug: Bump Map",
            6 => "Debug: Normal Map",
            _ => "Altro",
        };
        println!("FakeRelief Vista: [{}] {}", settings.debug, label);
    }
}

// ============================================================================
// Plugin
// ============================================================================
pub struct FakeReliefPlugin;

impl Plugin for FakeReliefPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FakeReliefSettings>()
        .add_plugins(ExtractResourcePlugin::<FakeReliefSettings>::default())
        .add_systems(Update, handle_input);

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
        .init_resource::<FakeReliefUniformBuf>()
        .add_systems(
            Render,
            (prepare_textures, prepare_uniform)
            .chain()
            .in_set(RenderSet::PrepareResources),
        )
        .add_render_graph_node::<ViewNodeRunner<FakeReliefNode>>(Core3d, FakeReliefLabel)
        .add_render_graph_edges(
            Core3d,
            (
                Node3d::Tonemapping,
             FakeReliefLabel,
             Node3d::EndMainPassPostProcessing,
            ),
        );
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        // Pipeline create UNA SOLA VOLTA all'avvio, non ogni frame!
        render_app.init_resource::<FakeReliefPipelines>();
    }
}
