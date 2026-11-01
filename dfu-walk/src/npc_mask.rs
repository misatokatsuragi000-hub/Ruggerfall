//! Mask degli NPC per il finto rilievo: una seconda camera che vede solo
//! le entità con `RenderLayers` contenente `NPC_MASK_LAYER`, renderizza
//! su una texture 1/4 di risoluzione, che `relief_post.rs` bind-a come
//! `@binding(7)`. Il fragment shader la legge: dove la mask è "accesa",
//! il pixel è di un NPC e il rilievo viene skippato.

use bevy::prelude::*;
use bevy::render::camera::RenderTarget;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// Layer dedicato agli NPC: la mask camera vede solo questo layer.
/// Il numero è arbitrario (0..=31); usane uno non usato altrove.
pub const NPC_MASK_LAYER: u8 = 2;

/// Divisore di risoluzione della mask (1/4 per lato → 1/16 dei pixel).
/// La mask è binaria e viene ricampionata in bilineare dallo shader del
/// rilievo (`uv` è già a risoluzione piena), quindi 1/4 è più che
/// sufficiente e costa pochissimo.
const MASK_DIV: u32 = 4;

/// Risorsa condivisa con `relief_post.rs`: l'handle della texture di mask.
#[derive(Resource, Clone)]
pub struct NpcMask {
    pub handle: Handle<Image>,
    /// Dimensioni logiche della mask (per eventuali resize futuri).
    pub size: UVec2,
}

#[derive(Component)]
pub struct NpcMaskCamera;

pub struct NpcMaskPlugin;

impl Plugin for NpcMaskPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_npc_mask)
           .add_systems(Update, sync_npc_mask_camera);
    }
}

fn setup_npc_mask(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    // Risoluzione della mask: 1/4 della finestra. Se la finestra cambia
    // dimensione, la mask resta a quella iniziale: è sufficiente, il
    // fragment shader ricampiona in bilineare qualunque sia la dimensione.
    let (w, h) = windows
        .get_single()
        .map(|win| (win.physical_width(), win.physical_height()))
        .unwrap_or((1280, 720));
    let mw = (w / MASK_DIV).max(1);
    let mh = (h / MASK_DIV).max(1);

    // Rgba8Unorm: usiamo il canale `.a` come "presenza NPC".
    // (Potremmo usare R8Unorm con `.r`, ma il materiale degli NPC
    // scrive alpha via `AlphaMode::Mask`, quindi `.a` è gratis.)
    let mask = Image::new(
        Extent3d { width: mw, height: mh, depth_or_array_layers: 1 },
        TextureDimension::D2,
        vec![0u8; (mw * mh * 4) as usize],
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    let handle = images.add(mask);

    // Camera figlia del main camera? No: `sync_npc_mask_camera` la insegue
    // ogni frame, così non serve parentela né con la camera né col Player.
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: -1, // renderizza PRIMA della main camera (order 0)
            target: RenderTarget::Image(handle.clone()),
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
        // Non disegna il cielo, la mask è solo billboard NPC.
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        RenderLayers::layer(NPC_MASK_LAYER),
        NpcMaskCamera,
    ));

    commands.insert_resource(NpcMask { handle, size: UVec2::new(mw, mh) });
    info!("NPC mask: {mw}x{mh} (1/{MASK_DIV} della finestra), layer {}", NPC_MASK_LAYER);
}

fn sync_npc_mask_camera(
    main: Query<&GlobalTransform, (With<Camera3d>, Without<NpcMaskCamera>)>,
    mut mask: Query<&mut Transform, With<NpcMaskCamera>>,
) {
    let Ok(main_gtf) = main.get_single() else { return };
    let Ok(mut mask_tf) = mask.get_single_mut() else { return };
    // Copia posizione e rotazione dalla camera principale. Il mask camera
    // non ha bisogno di projection diversa: la mask viene letta per `uv`
    // della scena principale.
    mask_tf.translation = main_gtf.translation();
    mask_tf.rotation    = main_gtf.rotation();
}