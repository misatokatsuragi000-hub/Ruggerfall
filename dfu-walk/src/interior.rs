//! Interni degli edifici: con E vicino a una Taverna, una Gilda o un Negozio si entra.
//! Con E vicino alla porta (o Backspace ovunque) si esce.
//! Riconosce la vera porta d'ingresso/uscita ed esce con certezza premendo E entro 3.5 metri.
use crate::dialog::DialogState;
use crate::mod_textures::ModTextures;
use crate::player::{Player, EYE_HEIGHT};
use crate::tiles::{climate_of, next_rand};
use crate::wall_textures::WallTextures;
use crate::world::*;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use crate::data::blocks::RmbBlock;
use crate::data::bsa::BsaArchive;
use crate::data::building_gen::DF_UNIT;
use crate::data::city_map::{collect_city_map, MapBuilding, PoiKind};
use crate::data::find_file_ci;
use crate::data::interior::{build_interior, InteriorScene};
use crate::data::mesh_builder::{Submesh, WorldMesh};
use crate::data::terrain_mesh::MAP_PIXEL_SIZE;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

const INTERIOR_Y: f32 = 6000.0;
const ENTER_DISTANCE: f32 = 4.5;
// Raggio di uscita generoso: 3.5 metri
const EXIT_DISTANCE: f32 = 3.5;
const WALK_SPEED: f32 = 4.0;
const RUN_MULTIPLIER: f32 = 2.0;
const GRAVITY: f32 = 20.0;
const JUMP_SPEED: f32 = 5.5;
const RADIUS: f32 = 0.3;
const STEP_UP: f32 = 0.6;
const SNAP_DOWN: f32 = 0.5;
const DOOR_REACH: f32 = 2.2;
const DOOR_OPEN_SECONDS: f32 = 1.5;
const DOOR_OPEN_SIGN: f32 = 1.0;

// =============================================================================
// FLATS.CFG: mappa (texture_archive, texture_record) del flat -> indice volto
// =============================================================================
#[derive(Clone, Copy, Debug)]
struct FlatInfo {
    face: usize,
}

#[derive(Default)]
pub struct FlatsFaceMap {
    entries: HashMap<(u16, u8), Vec<FlatInfo>>,
}

impl FlatsFaceMap {
    fn load(data: &GameData) -> Self {
        let mut map = Self::default();
        let Some(path) = find_file_ci(&data.dir, "FLATS.CFG") else {
            info!("FLATS.CFG non trovato: i volti interni useranno la generazione procedurale");
            return map;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            warn!("FLATS.CFG: impossibile leggere {}", path.display());
            return map;
        };
        let mut valid = 0usize;
        let mut skipped = 0usize;
        let mut lines = text.lines().peekable();
        while let Some(line) = lines.next() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if tokens.len() >= 6
                && tokens[0].parse::<u16>().is_ok()
                && tokens[2].parse::<u32>().is_ok()
                && tokens[3].parse::<usize>().is_ok()
                {
                    let archive = tokens[0].parse::<u16>().unwrap();
                    let record = tokens[1].parse::<u8>().unwrap();
                    let face = tokens[3].parse::<usize>().unwrap();
                    if archive <= 999 {
                        map.entries.entry((archive, record)).or_default().push(FlatInfo { face });
                        valid += 1;
                    } else {
                        skipped += 1;
                    }
                } else if tokens.len() == 2
                    && tokens[0].parse::<u16>().is_ok()
                    && tokens[1].parse::<u8>().is_ok()
                    {
                        let archive = tokens[0].parse::<u16>().unwrap();
                        let record = tokens[1].parse::<u8>().unwrap();
                        for _ in 0..4 {
                            if lines.next().is_none() { break; }
                        }
                        if let Some(face_line) = lines.next() {
                            if let Ok(face) = face_line.trim().parse::<usize>() {
                                if archive <= 999 {
                                    map.entries.entry((archive, record)).or_default().push(FlatInfo { face });
                                    valid += 1;
                                } else {
                                    skipped += 1;
                                }
                            } else {
                                skipped += 1;
                            }
                        }
                    } else {
                        skipped += 1;
                    }
        }
        if valid == 0 {
            warn!("FLATS.CFG: nessuna voce valida dopo il parsing. I volti interni useranno la generazione procedurale.");
        } else {
            info!("FLATS.CFG: {} voci valide, {} scartate, {} flat distinti mappati", valid, skipped, map.entries.len());
        }
        map
    }

    fn face_for(&self, archive: u16, record: u8) -> Option<usize> {
        let entries = self.entries.get(&(archive, record))?;
        entries.first().map(|e| e.face)
    }
}

struct Active {
    root: Entity,
    origin: Vec3,
    scene: InteriorScene,
    saved_translation: Vec3,
    saved_feet_y: f32,
    saved_yaw: f32,
    saved_origin: (i32, i32),
    kind: PoiKind,
    /// Identita' dell'edificio (map pixel + sorgente): serve a ritrovare i
    /// compagni in attesa quando si rientra nello stesso edificio.
    building: String,
}

/// Stato dell'interno attivo + tracking degli NPC già reclutati.
///
/// `recruited` contiene le chiavi `(flat_archive, flat_record)` degli NPC interni
/// che sono stati convertiti in follower: al re-spawn dell'interno questi vengono
/// saltati, così non compaiono due copie.
#[derive(Resource, Default)]
pub struct InteriorState {
    active: Option<Active>,
    pub recruited: std::collections::HashSet<(u16, u8)>,
    /// Nomi degli NPC custom reclutati: i custom non hanno un flat reale
    /// (archivio/record sono 0), quindi si distinguono per nome. Vengono
    /// spawnati in OGNI interno, per cui senza questo set ricomparirebbero
    /// accanto al loro follower a ogni ingresso.
    pub recruited_custom: std::collections::HashSet<String>,
}

impl InteriorState {
    pub fn is_inside(&self) -> bool {
        self.active.is_some()
    }

    /// Identita' dell'edificio attivo (stessa stringa a ogni ingresso nello stesso edificio).
    pub fn building(&self) -> Option<String> {
        self.active.as_ref().map(|a| a.building.clone())
    }

    /// Origine (mondo) dell'interno attivo: cambia a ogni ingresso.
    pub fn origin(&self) -> Option<Vec3> {
        self.active.as_ref().map(|a| a.origin)
    }

    pub fn kind(&self) -> Option<PoiKind> {
        self.active.as_ref().map(|a| a.kind)
    }

    /// Segna un NPC interno come reclutato: non verrà più ri-spawnato all'interno.
    pub fn mark_recruited(&mut self, archive: u16, record: u8) {
        self.recruited.insert((archive, record));
    }

    /// True se l'NPC (identificato da archivio+record del flat) è già stato reclutato.
    pub fn is_recruited(&self, archive: u16, record: u8) -> bool {
        self.recruited.contains(&(archive, record))
    }

    /// Segna un NPC custom (per nome) come reclutato.
    pub fn mark_recruited_custom(&mut self, name: &str) {
        self.recruited_custom.insert(name.to_string());
    }

    /// True se l'NPC custom con questo nome e' gia' stato reclutato.
    pub fn is_recruited_custom(&self, name: &str) -> bool {
        self.recruited_custom.contains(name)
    }
}

pub fn outside(state: Res<InteriorState>) -> bool {
    !state.is_inside()
}

#[derive(Component)]
pub struct FlatBillboard;

/// NPC interno (billboard). `size` è la dimensione reale in metri della mesh
/// `Rectangle::new(w, h)` con cui è stato creato il billboard: serve al follower
/// per preservare l'aspect ratio quando lo si converte in `Follower`.
#[derive(Component)]
pub struct InteriorNpc {
    pub face_record: usize,
    pub flat_archive: u16,
    pub flat_record: u8,
    /// Dimensione del billboard in metri `(w, h)`.
    pub size: Vec2,
}

/// Componente per identificare un NPC custom caricato da file esterno
#[derive(Component)]
pub struct CustomNpcName {
    pub name: String,
}

/// Dati per un NPC custom caricato dalla cartella textures
#[derive(Clone, Debug)]
pub struct CustomNpcData {
    pub name: String,
    pub texture_stem: String, // Nome file senza estensione (es. "npc_bartolo")
    /// Descrizione del personaggio (riga `description="..."` del .txt), usata
    /// al posto del prompt generico nella generazione dei dialoghi.
    pub description: Option<String>,
    /// Voce TTS imposta (riga `voice = af_sarah` del .txt), es. `af_sarah`.
    pub voice: Option<String>,
}

/// Legge il `.txt` di un NPC custom.
/// - 1a riga non vuota: nome dell'NPC;
/// - una riga successiva `description="A brave and kind warrior..."`: descrizione
///   (le virgolette sono facoltative). Il campo e' opzionale.
pub fn parse_custom_txt(content: &str) -> (String, Option<String>, Option<String>) {
    let mut lines = content.trim_start_matches('\u{feff}').lines();
    let name = lines
        .by_ref()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string();
    let mut description = None;
    let mut voice = None;
    for line in lines {
        let line = line.trim();
        let (key, rest) = if let Some(r) = line.strip_prefix("description") {
            ("description", r)
        } else if let Some(r) = line.strip_prefix("voice") {
            ("voice", r)
        } else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix('=') else { continue };
        let value = value.trim();
        let value = value.strip_prefix('"').unwrap_or(value);
        let value = value.strip_suffix('"').unwrap_or(value).trim();
        if value.is_empty() {
            continue;
        }
        match key {
            "description" if description.is_none() => description = Some(value.to_string()),
            // miscele: "af_bella,af_sky" o "af_bella:0.7,af_sky:0.3"; gli spazi si tolgono
            "voice" if voice.is_none() => {
                voice = Some(value.split_whitespace().collect::<String>())
            }
            _ => {}
        }
    }
    (name, description, voice)
}

/// Risorsa che cachea gli NPC custom all'avvio
#[derive(Resource, Default)]
pub struct CustomNpcCache {
    pub npcs: Vec<CustomNpcData>,
}

impl CustomNpcCache {
    pub fn load(textures_dir: &std::path::Path) -> Self {
        let mut npcs = Vec::new();
        if let Ok(entries) = fs::read_dir(textures_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                // Il .txt (nome dell'NPC) ha lo stesso stem dell'immagine: va escluso,
                // altrimenti ogni NPC viene caricato due volte (immagine + txt) e
                // compaiono due copie identiche nello stesso punto.
                let is_txt = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map_or(false, |e| e.eq_ignore_ascii_case("txt"));
                if is_txt {
                    continue;
                }
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    // Stesso stem con estensioni diverse (png/jpg): una sola copia.
                    if npcs.iter().any(|n: &CustomNpcData| n.texture_stem == stem) {
                        continue;
                    }
                    if stem.starts_with("npc_") {
                        // Controlla se esiste il file .txt associato
                        let txt_path = path.with_extension("txt");
                        if txt_path.exists() {
                            if let Ok(name) = fs::read_to_string(&txt_path) {
                                let (trimmed_name, description, voice) = parse_custom_txt(&name);
                                if !trimmed_name.is_empty() {
                                    npcs.push(CustomNpcData {
                                        name: trimmed_name,
                                        texture_stem: stem.to_string(),
                                        description,
                                        voice,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        info!("Caricati {} NPC custom dalla cartella textures", npcs.len());
        Self { npcs }
    }
}

#[derive(Component)]
pub struct RoomDoor {
    index: usize,
    closed_y: f32,
    progress: f32,
}

fn distance_to_building(b: &MapBuilding, px: f32, pz: f32) -> f32 {
    let (s, c) = b.rot_deg.to_radians().sin_cos();
    let (dx, dz) = (px - b.cx, pz - b.cz);
    let lx = dx * c - dz * s;
    let lz = dx * s + dz * c;
    let ox = (lx.abs() - b.half_w).max(0.0);
    let oz = (lz.abs() - b.half_d).max(0.0);
    ox.hypot(oz)
}

fn submesh_to_bevy(mesh: &WorldMesh, sm: &Submesh, size: (f32, f32)) -> Mesh {
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::with_capacity(sm.indices.len());
    for &i in &sm.indices {
        let new = *remap.entry(i).or_insert_with(|| {
            let k = positions.len() as u32;
            positions.push(mesh.vertices[i as usize]);
            normals.push(mesh.normals[i as usize]);
            let uv = mesh.uvs[i as usize];
            uvs.push([uv[0] / size.0, uv[1] / size.1]);
            k
        });
        indices.push(new);
    }
    let mut out = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    out.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    out.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    out.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    out.insert_indices(Indices::U32(indices));
    out
}

fn debug_uv_scale(mesh: &WorldMesh, sm: &Submesh, size: (f32, f32)) {
    let mut ratios = Vec::new();
    for t in sm.indices.chunks_exact(3) {
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let (ia, ib) = (t[a] as usize, t[b] as usize);
            let (pa, pb) = (mesh.vertices[ia], mesh.vertices[ib]);
            let d = ((pa[0] - pb[0]).powi(2) + (pa[1] - pb[1]).powi(2) + (pa[2] - pb[2]).powi(2)).sqrt();
            let (ua, ub) = (mesh.uvs[ia], mesh.uvs[ib]);
            let (du, dv) = ((ua[0] - ub[0]) / size.0, (ua[1] - ub[1]) / size.1);
            let duv = (du * du + dv * dv).sqrt();
            if d > 0.05 && duv > 1e-6 {
                ratios.push(d / duv);
            }
        }
    }
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if let Some(m) = ratios.get(ratios.len() / 2) {
        println!(
            "UV dbg {}/{} size {:?}: {} spigoli, metri per ripetizione (mediana) = {:.4}",
                 sm.texture_archive, sm.texture_record, size, ratios.len(), m
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn interior_enter_exit(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    data: Res<GameData>,
    origin: Res<Origin>,
    mut state: ResMut<InteriorState>,
    mut clear: ResMut<ClearColor>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut wall_textures: ResMut<WallTextures>,
    mut mods: ResMut<ModTextures>,
    mut player: Query<(&mut Player, &mut Transform)>,
                           mut flats_face_map: Local<Option<FlatsFaceMap>>,
                           custom_npcs: Res<CustomNpcCache>, // Aggiunto parametro per gli NPC custom
) {
    let leave = keys.just_pressed(KeyCode::Backspace);
    let act = keys.just_pressed(KeyCode::KeyE);
    if !leave && !act {
        return;
    }
    let Ok((mut p, mut tf)) = player.get_single_mut() else {
        return;
    };

    // ---- USCITA DALL'INTERNO ----
    if state.active.is_some() {
        let near_exit = {
            let a = state.active.as_ref().unwrap();
            let local = tf.translation - a.origin;
            // Esci se sei vicino alla porta d'uscita OPPURE vicino al punto in cui sei spawnato all'entrata
            let d_spawn = (local.x - a.scene.spawn[0]).hypot(local.z - a.scene.spawn[2]);
            let d_exit = a.scene.exit.map_or(f32::MAX, |e| {
                (local.x - e[0]).hypot(local.z - e[2])
            });
            d_spawn < EXIT_DISTANCE || d_exit < EXIT_DISTANCE
        };
        if leave || near_exit {
            let a = state.active.take().unwrap();
            let (de, dn) = (origin.east - a.saved_origin.0, origin.north - a.saved_origin.1);
            let mut pos = a.saved_translation;
            pos.x -= de as f32 * MAP_PIXEL_SIZE;
            pos.z += dn as f32 * MAP_PIXEL_SIZE;
            p.feet_y = a.saved_feet_y;
            p.vel_y = 0.0;
            p.on_ground = true;
            p.yaw = a.saved_yaw + std::f32::consts::PI;
            tf.translation = pos;
            commands.entity(a.root).despawn_recursive();
            clear.0 = Color::srgb(0.55, 0.72, 0.92);
            println!("Sei uscito dall'edificio.");
        }
        return;
    }

    // ---- INGRESSO NELL'EDIFICIO ----
    if !act {
        return;
    }
    if flats_face_map.is_none() {
        *flats_face_map = Some(FlatsFaceMap::load(&data));
    }
    let (mx, my) = map_pixel_of(&origin, tf.translation);
    let Some(loc) = data.locations.get(&(mx, my)) else {
        println!("Qui non c'e' nessuna citta'.");
        return;
    };
    let (Ok(blocks), Ok(arch)) = (
        BsaArchive::parse(&data.blocks_buf),
                                  BsaArchive::parse(&data.arch3d_buf),
    ) else {
        println!("Interni: impossibile leggere BLOCKS.BSA o ARCH3D.BSA.");
        return;
    };
    let map = collect_city_map(loc, &blocks, &arch);
    let tile = tile_translation(&origin, mx, my);
    let (px, pz) = (tf.translation.x - tile.x, tf.translation.z - tile.z);
    let mut best: Option<(f32, (usize, usize, u8), PoiKind)> = None;
    let (mut places, mut nearest) = (0usize, f32::MAX);
    for b in &map.buildings {
        let Some(kind) = b.kind.filter(|k| matches!(k, PoiKind::Tavern | PoiKind::Shop | PoiKind::Guild)) else {
            continue;
        };
        let Some(src) = b.source else { continue; };
        let d = distance_to_building(b, px, pz);
        places += 1;
        nearest = nearest.min(d);
        if d <= ENTER_DISTANCE && best.map_or(true, |(bd, _, _)| d < bd) {
            best = Some((d, src, kind));
        }
    }
    let Some((_, (bx, by, sub), kind)) = best else {
        if places == 0 {
            println!(
                "Interni: nessuna taverna, gilda o negozio trovato in {} (map pixel {mx},{my}).",
                     loc.name
            );
        } else {
            println!(
                "Interni: {} edifici accessibili in {}, il piu' vicino a {:.1} m (serve entro {ENTER_DISTANCE} m).",
                     places, loc.name, nearest
            );
        }
        return;
    };
    let Some(name) = loc.block_names.get(by * loc.width + bx) else {
        println!("Interni: blocco ({bx},{by}) fuori dalla location.");
        return;
    };
    let Some(raw) = blocks.get_by_name(name) else {
        println!("Interni: il blocco {name} non esiste in BLOCKS.BSA.");
        return;
    };
    let Ok(rmb) = RmbBlock::parse(raw) else {
        println!("Interni: il blocco {name} non si legge.");
        return;
    };
    let Some(scene) = build_interior(&rmb, sub as usize, &arch) else {
        println!("L'interno di {name} (sotto-record {sub}) non ha modelli 3D.");
        return;
    };
    let origin_world = Vec3::new(tf.translation.x, INTERIOR_Y, tf.translation.z);
    let climate = climate_of(&data, mx, my);
    let face_lookup = flats_face_map.as_ref().expect("inizializzata sopra");
    let root = commands
    .spawn((Transform::from_translation(origin_world), Visibility::default()))
    .with_children(|parent| {
        // ---- MURI ----
        for sm in &scene.mesh.submeshes {
            if sm.indices.is_empty() {
                continue;
            }
            let size = wall_textures.original_size(sm.texture_archive, sm.texture_record, &data);
            debug_uv_scale(&scene.mesh, sm, size);
            let handle = wall_textures.material(
                sm.texture_archive,
                sm.texture_record,
                climate,
                &data,
                &mut mods,
                &mut images,
                &mut materials,
            );
            if let Some(m) = materials.get_mut(&handle) {
                m.double_sided = true;
                m.cull_mode = None;
            }
            let mut bevy_mesh = submesh_to_bevy(&scene.mesh, sm, size);
            wall_textures.prepare_mesh(&handle, &mut bevy_mesh);
            parent.spawn((
                Mesh3d(meshes.add(bevy_mesh)),
                          MeshMaterial3d(handle),
                          Transform::IDENTITY,
            ));
        }
        // ---- PORTE ----
        for (i, d) in scene.doors.iter().enumerate() {
            let closed = -d.yaw_deg.to_radians();
            parent
            .spawn((
                Transform {
                    translation: Vec3::from(d.pos),
                    rotation: Quat::from_rotation_y(closed),
                    ..default()
                },
                Visibility::default(),
                    RoomDoor {
                        index: i,
                        closed_y: closed,
                        progress: 0.0,
                    },
            ))
            .with_children(|door| {
                for sm in &d.mesh.submeshes {
                    if sm.indices.is_empty() {
                        continue;
                    }
                    let size = wall_textures.original_size(
                        sm.texture_archive,
                        sm.texture_record,
                        &data,
                    );
                    let handle = wall_textures.material(
                        sm.texture_archive,
                        sm.texture_record,
                        climate,
                        &data,
                        &mut mods,
                        &mut images,
                        &mut materials,
                    );
                    if let Some(m) = materials.get_mut(&handle) {
                        m.double_sided = true;
                        m.cull_mode = None;
                    }
                    let mut bevy_mesh = submesh_to_bevy(&d.mesh, sm, size);
                    wall_textures.prepare_mesh(&handle, &mut bevy_mesh);
                    door.spawn((
                        Mesh3d(meshes.add(bevy_mesh)),
                                MeshMaterial3d(handle),
                                Transform::IDENTITY,
                    ));
                }
            });
        }
        // Flat (arredi statici: tavoli, barili, quadri, luci) con supporto HD mod
        for f in &scene.flats {
            let Some((material, w_px, h_px)) = wall_textures.flat_material(
                f.archive,
                f.record,
                &data,
                &mut mods,
                &mut images,
                &mut materials,
            ) else {
                continue;
            };
            let (w, h) = (w_px * DF_UNIT, h_px * DF_UNIT);
            parent.spawn((
                Mesh3d(meshes.add(Rectangle::new(w, h))),
                          MeshMaterial3d(material),
                          Transform::from_xyz(f.pos[0], f.pos[1] + h * 0.5, f.pos[2]),
                          FlatBillboard,
            ));
        }
        // People (NPC - oste, clienti, mercanti, membri di gilda) con supporto HD mod e dialogo.
        for (i, f) in scene.people.iter().enumerate() {
            // Salta gli NPC già reclutati: al re-spawn non devono ricomparire.
            if state.is_recruited(f.archive, f.record) {
                continue;
            }
            let Some((material, w_px, h_px)) = wall_textures.flat_material(
                f.archive,
                f.record,
                &data,
                &mut mods,
                &mut images,
                &mut materials,
            ) else {
                continue;
            };
            let (w, h) = (w_px * DF_UNIT, h_px * DF_UNIT);
            let face_record = face_lookup
            .face_for(f.archive, f.record)
            .unwrap_or_else(|| {
                ((f.archive as usize * 37) + (f.record as usize * 13) + (i * 7)) % 500
            });
            parent.spawn((
                Mesh3d(meshes.add(Rectangle::new(w, h))),
                          MeshMaterial3d(material),
                          Transform::from_xyz(f.pos[0], f.pos[1] + h * 0.5, f.pos[2]),
                          FlatBillboard,
                          InteriorNpc {
                              face_record,
                              flat_archive: f.archive,
                              flat_record: f.record,
                              size: Vec2::new(w, h),
                          },
            ));
        }

        // --- SPAWN NPC CUSTOM DA FILE TEXTURE ---
        for custom_npc in &custom_npcs.npcs {
            // Salta i custom gia' reclutati: ora seguono il giocatore come follower.
            if state.is_recruited_custom(&custom_npc.name) {
                continue;
            }
            // Usa ModTextures per recuperare l'handle dell'immagine
            // ModTextures gestisce automaticamente il caching e il decoding
            if let Some(image_handle) = mods.image_by_name(&custom_npc.texture_stem, &mut images) {

                // Recupera le dimensioni reali dell'immagine per calcolare l'aspect ratio
                let size_px = mods.image_size(&image_handle)
                .or_else(|| images.get(&image_handle).map(|i| i.size_f32()))
                .unwrap_or(Vec2::new(64.0, 128.0)); // Default fallback

                // Scala arbitraria per convertire pixel in metri nel mondo di gioco
                // DF_UNIT è circa 0.04 metri per pixel nei flat originali
                let scale_factor = DF_UNIT * 0.074; // Un po' più grandi dei flat standard
                let w = size_px.x * scale_factor;
                let h = size_px.y * scale_factor;

                let material = materials.add(StandardMaterial {
                    base_color_texture: Some(image_handle),
                                             alpha_mode: AlphaMode::Mask(0.5),
                                             cull_mode: None,
                                             double_sided: true,
                                             perceptual_roughness: 0.9,
                                             reflectance: 0.1,
                                             ..default()
                });

                // Posizione random vicina allo spawn usando next_rand
                let mut rng_seed = custom_npc.texture_stem.len() as u64;
                let spawn_x = scene.spawn[0] + (next_rand(&mut rng_seed) - 0.5) * 4.0;
                let spawn_z = scene.spawn[2] + (next_rand(&mut rng_seed) - 0.5) * 4.0;

                parent.spawn((
                    Mesh3d(meshes.add(Rectangle::new(w, h))),
                              MeshMaterial3d(material),
                              Transform::from_xyz(spawn_x, scene.spawn[1] + h * 0.5, spawn_z),
                              FlatBillboard,
                              // Usiamo valori dummy per InteriorNpc dato che è un custom
                              InteriorNpc {
                                  face_record: 0,
                                  flat_archive: 0,
                                  flat_record: 0,
                                  size: Vec2::new(w, h),
                              },
                              CustomNpcName {
                                  name: custom_npc.name.clone(),
                              },
                ));
            }
        }

        let cx = (scene.bounds_min[0] + scene.bounds_max[0]) * 0.5;
        let cz = (scene.bounds_min[2] + scene.bounds_max[2]) * 0.5;
        parent.spawn((
            PointLight {
                color: Color::srgb(1.0, 0.86, 0.62),
                      intensity: 600_000.0,
                      range: 40.0,
                      shadows_enabled: false,
                      ..default()
            },
            Transform::from_xyz(cx, scene.spawn[1] + 2.3, cz),
        ));
    })
    .id();
    let spawn = Vec3::from(scene.spawn);
    let exit = scene.exit.map(Vec3::from).unwrap_or(spawn);
    // Orienta la visuale in avanti voltando le spalle alla porta d'uscita
    let look_dir = if scene.exit.is_some() {
        Vec3::new(spawn.x - exit.x, 0.0, spawn.z - exit.z)
    } else {
        let cx = (scene.bounds_min[0] + scene.bounds_max[0]) * 0.5;
        let cz = (scene.bounds_min[2] + scene.bounds_max[2]) * 0.5;
        Vec3::new(cx - spawn.x, 0.0, cz - spawn.z)
    };
    let saved = Active {
        root,
        origin: origin_world,
        saved_translation: tf.translation,
        saved_feet_y: p.feet_y,
        saved_yaw: p.yaw,
        saved_origin: (origin.east, origin.north),
        kind,
        building: format!("{mx},{my}:{bx},{by},{sub}"),
        scene,
    };
    p.feet_y = origin_world.y + spawn.y;
    p.vel_y = 0.0;
    p.on_ground = true;
    p.fly = false;
    if look_dir.length_squared() > 0.01 {
        let norm = look_dir.normalize();
        p.yaw = (-norm.x).atan2(-norm.z);
    }
    tf.translation = Vec3::new(
        origin_world.x + spawn.x,
        p.feet_y + EYE_HEIGHT,
        origin_world.z + spawn.z,
    );
    clear.0 = Color::BLACK;
    state.active = Some(saved);
    let what = match kind {
        PoiKind::Shop => "negozio",
        PoiKind::Guild => "gilda",
        _ => "taverna",
    };
    println!("Sei entrato in {name} ({what}, sotto-record {sub}). E vicino alla porta o Backspace per uscire.");
}

pub fn interior_move(
    dialog: Res<DialogState>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<InteriorState>,
    mut player: Query<(&mut Player, &mut Transform)>,
) {
    if dialog.open {
        return;
    }
    let Some(active) = state.active.as_ref() else {
        return;
    };
    let Ok((mut p, mut tf)) = player.get_single_mut() else {
        return;
    };
    let dt = time.delta_secs().min(0.05);
    let (scene, o) = (&active.scene, active.origin);
    let forward = Vec3::new(-p.yaw.sin(), 0.0, -p.yaw.cos());
    let right = Vec3::new(p.yaw.cos(), 0.0, -p.yaw.sin());
    let axis = |pos: KeyCode, neg: KeyCode| {
        keys.pressed(pos) as i32 as f32 - keys.pressed(neg) as i32 as f32
    };
    let wish = (forward * axis(KeyCode::KeyW, KeyCode::KeyS)
    + right * axis(KeyCode::KeyD, KeyCode::KeyA))
    .normalize_or_zero();
    let running = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let speed = WALK_SPEED * if running { RUN_MULTIPLIER } else { 1.0 };
    let total = speed * dt;
    let steps = (total / 0.1).ceil().max(1.0) as i32;
    let step = wish * (total / steps as f32);
    let mut pos = tf.translation;
    if step != Vec3::ZERO {
        for _ in 0..steps {
            let feet = p.feet_y - o.y;
            for cand in [
                step,
                Vec3::new(step.x, 0.0, 0.0),
                Vec3::new(0.0, 0.0, step.z),
            ] {
                if cand == Vec3::ZERO {
                    continue;
                }
                let n = pos + cand;
                if !scene.blocked(n.x - o.x, n.z - o.z, feet, RADIUS) {
                    pos = n;
                    break;
                }
            }
        }
    }
    let (lx, lz) = (pos.x - o.x, pos.z - o.z);
    let feet_local = p.feet_y - o.y;
    let floor = scene.floor_at(lx, lz, feet_local, STEP_UP);
    if keys.just_pressed(KeyCode::Space) && p.on_ground {
        p.vel_y = JUMP_SPEED;
        p.on_ground = false;
    }
    match floor {
        Some(f) if p.vel_y <= 0.0 && feet_local - f <= SNAP_DOWN => {
            p.feet_y = o.y + f;
            p.vel_y = 0.0;
            p.on_ground = true;
        }
        _ => {
            p.vel_y -= GRAVITY * dt;
            p.feet_y += p.vel_y * dt;
            p.on_ground = false;
            if let Some(f) = floor {
                if p.feet_y - o.y <= f {
                    p.feet_y = o.y + f;
                    p.vel_y = 0.0;
                    p.on_ground = true;
                }
            }
        }
    }
    if p.feet_y - o.y < scene.bounds_min[1] - 5.0 {
        pos = Vec3::new(o.x + scene.spawn[0], 0.0, o.z + scene.spawn[2]);
        p.feet_y = o.y + scene.spawn[1];
        p.vel_y = 0.0;
        p.on_ground = true;
    }
    pos.y = p.feet_y + EYE_HEIGHT;
    tf.translation = pos;
}

pub fn face_flats(
    player: Query<&Player>,
    mut flats: Query<&mut Transform, (With<FlatBillboard>, Without<crate::tiles::NpcWalker>)>,
) {
    let Ok(p) = player.get_single() else { return };
    let rot = Quat::from_rotation_y(p.yaw);
    for mut tf in &mut flats {
        tf.rotation = rot;
    }
}

pub fn interior_toggle_doors(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<InteriorState>,
    player: Query<(&Player, &Transform)>,
) {
    if !keys.just_pressed(KeyCode::KeyE) {
        return;
    }
    let Some(active) = state.active.as_mut() else {
        return;
    };
    let Ok((p, tf)) = player.get_single() else { return };
    let o = active.origin;
    let (lx, lz) = (tf.translation.x - o.x, tf.translation.z - o.z);
    let feet = p.feet_y - o.y;
    if let Some(i) = active.scene.toggle_nearest_door(lx, lz, feet, DOOR_REACH) {
        let open = active.scene.door_open[i];
        println!("Porta {i} {}.", if open { "aperta" } else { "chiusa" });
    }
}

pub fn animate_doors(
    time: Res<Time>,
    state: Res<InteriorState>,
    mut doors: Query<(&mut RoomDoor, &mut Transform)>,
) {
    let Some(active) = state.active.as_ref() else {
        return;
    };
    let step = time.delta_secs() / DOOR_OPEN_SECONDS;
    for (mut d, mut tf) in &mut doors {
        let open = active.scene.door_open.get(d.index).copied().unwrap_or(false);
        d.progress = if open {
            (d.progress + step).min(1.0)
        } else {
            (d.progress - step).max(0.0)
        };
        let eased = d.progress * d.progress * (3.0 - 2.0 * d.progress);
        tf.rotation =
        Quat::from_rotation_y(d.closed_y - DOOR_OPEN_SIGN * eased * std::f32::consts::FRAC_PI_2);
    }
}
