// src/data/dungeon_gen.rs
use super::dungeon::DungeonChunk; // Importa solo DungeonChunk da dungeon.rs
use super::building_gen::{Footprint, BLOCK_SIZE_M}; // Se necessario per le costanti
use std::collections::HashMap;

/// Layout finale di un dungeon pronto per il rendering
#[derive(Debug, Clone)]
pub struct DungeonLayout {
    pub chunks: Vec<DungeonChunk>,
    pub entrance_pos: [f32; 3],
    pub exit_pos: Option<[f32; 3]>,
}

impl DungeonLayout {
    /// Costruisce un layout semplice posizionando i blocchi uno dopo l'altro lungo l'asse Z
    pub fn assemble_linear(chunks: Vec<DungeonChunk>) -> Self {
        let mut positioned_chunks = Vec::new();
        let mut current_z = 0.0;

        for mut chunk in chunks {
            // Posiziona il chunk
            chunk.position_offset = [0.0, 0.0, current_z];

            // Stima la profondità del prossimo blocco (semplificato: usa BLOCK_SIZE_M)
            current_z -= BLOCK_SIZE_M;

            positioned_chunks.push(chunk);
        }

        DungeonLayout {
            chunks: positioned_chunks,
            entrance_pos: [0.0, 0.0, 0.0],
            exit_pos: Some([0.0, 0.0, current_z]),
        }
    }
}
