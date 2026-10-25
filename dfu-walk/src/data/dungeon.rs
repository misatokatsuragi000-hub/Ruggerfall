use super::bsa::BsaArchive;
use super::blocks::RmbBlock;


/// Identifica se un blocco RMB appartiene a un dungeon.
pub fn is_dungeon_block(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    // Prefissi tipici dei dungeon in Daggerfall
    upper.starts_with("DUNG") || 
    upper.starts_with("CRYPT") || 
    upper.starts_with("CAVE") || 
    upper.starts_with("RUIN") ||
    upper.starts_with("GRAV") // Graveyard interiors
}

/// Struttura dati per un singolo chunk di dungeon
#[derive(Debug, Clone)]
pub struct DungeonChunk {
    pub block_name: String,
    pub rmb: RmbBlock,
    pub position_offset: [f32; 3], // Offset relativo all'origine del dungeon
}

/// Estrae i blocchi dungeon da una location o da un archivio specifico.
/// Nota: I dungeon non sono sempre nelle DFLocation standard come le città, 
/// ma possono essere referenziati direttamente dai map pixel o generati proceduralmente.
pub fn load_dungeon_blocks(
    blocks_bsa: &BsaArchive,
    block_names: &[String],
) -> Vec<DungeonChunk> {
    let mut chunks = Vec::new();
    
    for name in block_names {
        if is_dungeon_block(name) {
            if let Some(raw) = blocks_bsa.get_by_name(name) {
                if let Ok(rmb) = RmbBlock::parse(raw) {
                    chunks.push(DungeonChunk {
                        block_name: name.clone(),
                        rmb,
                        position_offset: [0.0, 0.0, 0.0], // Da calcolare in base al layout
                    });
                }
            }
        }
    }
    chunks
}
