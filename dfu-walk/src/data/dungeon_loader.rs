// src/data/dungeon_loader.rs (Bozza concettuale basata sul tuo codice)
use crate::data::blocks::RmbBlock;
use crate::data::bsa::BsaArchive;
use crate::data::arch3d::Arch3dModel;
use crate::data::mesh_builder::WorldMesh;
use std::collections::HashMap;

pub struct DungeonMesh {
    pub mesh: WorldMesh,
    pub colliders: Vec<crate::tiles::WallSegment>, // O usa la tua struct Colliders
}

pub fn load_dungeon_block(
    block_name: &str,
    blocks_bsa: &BsaArchive,
    arch3d_bsa: &BsaArchive,
) -> Option<DungeonMesh> {
    let raw = blocks_bsa.get_by_name(block_name)?;
    let rmb = RmbBlock::parse(raw).ok()?;

    let mut world_mesh = WorldMesh::new();
    let mut model_cache: HashMap<u32, Arch3dModel> = HashMap::new();

    // Nei dungeon, gli oggetti 3D sono già posizionati nello spazio locale del blocco.
    // Spesso non hanno un "subrecord" di edificio, sono diretti.
    for obj in &rmb.objects_3d {
        if let Some(model) = model_cache.entry(obj.model_id).or_insert_with(|| {
            arch3d_bsa.get_by_id(obj.model_id)
            .and_then(|raw| Arch3dModel::parse(obj.model_id, raw).ok())
        }) {
            // Conversione coordinate DF -> Metri Bevy
            // Nota: Nei dungeon, le coordinate sono spesso assolute rispetto al centro del blocco
            let x = obj.x as f32 * 0.0254;
            let y = -obj.y as f32 * 0.0254; // Y invertito in DF
            let z = obj.z as f32 * 0.0254;

            world_mesh.add_instance(model, x, y, z, obj.rotation_y_deg);
        }
    }

    Some(DungeonMesh {
        mesh: world_mesh,
         colliders: vec![], // Qui dovresti estrarre le collisioni dai triangoli
    })
}
