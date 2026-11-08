//! Assemblatore della geometria 3D, collisioni e porte apribili di un dungeon RDB.
//!
//! ## Riconoscimento delle porte (porting di `RDBLayout.IsActionDoor` di DFU)
//!
//! In Daggerfall una porta è un modello RDB la cui `Description` nel
//! `ModelReferenceList` è una di: `"DOR"`, `"DDR"`, `"NEW"`, `"CAV"`. Il modello
//! `72100` (redBrick) ha `"DOR"` ma è solo decorativo e va escluso.
//!
//! ## Angolo di apertura
//!
//! DFU usa `OpenAngle = -90°` fisso, perché i modelli delle porte sono orientati
//! in modo che la cerniera stia all'origine e il pannello si estenda sempre dallo
//! stesso lato. Nel nostro codice la mesh locale ha i vertici originali, quindi
//! per le porte con cerniera "dall'altra parte" (tipiche delle doppie ante `DDR`)
//! il verso va invertito. `compute_open_angle` guarda il baricentro del modello
//! rispetto alla cerniera e restituisce `-90°` o `+90°` di conseguenza.
//!
//! ## Marker di uscita (modello 70300) — parità con DFU
//!
//! In DFU (`RDBLayout.AddModels`) il modello `70300` ("Entrance/Exit - Standalone")
//! è riconosciuto **in qualunque blocco**, non solo in quello di partenza. È una
//! **porta visibile** (non un marker invisibile), ma senza collider: in DFU viene
//! aggiunto con `ignoreCollider: true` e non contribuisce alla mesh statica.
//!
//! Nel nostro codice:
//!
//! - il modello `70300` viene aggiunto a `BuiltDungeon::visible_only`, che viene
//!   renderizzato come la mesh principale ma **non** processato per `floors`/`walls`;
//! - `exit_marker` salva la posizione, ma la sua Y può essere una sentinella
//!   (`y_raw = -1` → `oy ≈ 0`): in quel caso allineiamo `exit_pos` alla Y della
//!   porta più vicina in XZ (che ha la Y reale del piano in cui il RDB ha modellato
//!   il passaggio), così il toggle E funziona correttamente.
//!
//! ## Collisione dei muri obliqui (fix gallerie a volta)
//!
//! Il vecchio `tri_dist_xz` proiettava il triangolo in XZ e restituiva `0.0`
//! quando il punto cadeva dentro la proiezione, **ignorando la quota `y`**. Per
//! un muro verticale è corretto (la proiezione XZ coincide col muro). Per un
//! muro obliquo (gallerie a volta, tetti spioventi, rampe ripide) la proiezione
//! XZ copre una regione molto più grande del triangolo reale, e il giocatore al
//! centro della galleria veniva classificato "dentro la proiezione" e bloccato.
//!
//! `tri_dist_xz` ora prende anche `feet_y` e calcola la distanza dal punto ai
//! **tagli orizzontali** del triangolo alle quote `feet_y + 0.45`, `+ 1.1`,
//! `+ 1.8` (la banda del corpo). Per i muri verticali i tagli coincidono con la
//! proiezione XZ → comportamento invariato. Per i muri obliqui il taglio è più
//! corto e non blocca più i punti lontani dalla parte di muro che il corpo può
//! effettivamente toccare. Stessa tecnica di `WallTri::slice_at_y` in
//! `tiles.rs::probe`.
//!
//! ## Nemici (porting fedele di `RDBLayout.AddFixedEnemies` / `AddRandomEnemies`)
//!
//! Negli RDB i veri nemici usano i record 15 (mostro casuale) e 16 (mostro fisso)
//! dell'archivio 199. Ecco cosa fa DFU **esattamente** per ognuno:
//!
//! **Record 16 — nemico fisso** (`AddFixedRDBEnemy`):
//!  - `typeValue = FactionOrMobileId & 0xFF`;
//!  - se `typeValue == 99` → **ignorato** (DFU commenta: "ignore known invalid types");
//!  - altrimenti → cast diretto `(MobileTypes)typeValue`. I byte alti sono
//!    "garbage data" per i marker RDB (commento esplicito in DFU), quindi
//!    **non** si filtra per `faction < 256`: si prende sempre `& 0xFF`.
//!
//! **Record 15 — nemico casuale** (`AddRandomRDBEnemyClassic`):
//!  - `slot = Flags`; se `slot == 0` DFU randomizza 1..6;
//!  - il tipo viene scelto da una tabella precalcolata (256 nemici per il
//!    dungeon) usando `slot` come indice;
//!  - il nemico appare **nella posizione del marker**, non altrove.
//!
//! **Nessun filtro per "prima stanza"** esiste in DFU. La sala del trono del
//! Palazzo Reale è pulita perché il suo RDB non contiene marker `record 15`
//! o `record 16` con MobileID valido: se il tuo port ne spawna, sono marker
//! che DFU scarterebbe (MobileID = 99 o specie inesistente), oppure il RDB
//! della sala del trono non ha proprio marker di spawn.

use std::collections::HashMap;
use std::sync::Arc;
use bevy::prelude::{info, Quat, Vec3};
use super::arch3d::Arch3dModel;
use super::bsa::BsaArchive;
use super::dungeon_layout::DungeonData;
use super::mesh_builder::WorldMesh;
use super::rdb::{RdbBlock, RdbResource, RDB_SIDE_METERS};

pub type Tri = [[f32; 3]; 3];

/// Modello "redBrick door": ha `desc == "DOR"` ma non è una porta apribile
/// (fonte: `RDBLayout.cs` di DFU).
const RED_BRICK_DOOR_MODEL_ID: u32 = 72100;

/// Modello "Entrance/Exit - Standalone" ("the exit door" in DFU). È una porta
/// **visibile** ma senza collider: in DFU viene aggiunto con
/// `ignoreCollider: true` e non contribuisce alla mesh statica.
const EXIT_DOOR_MODEL_ID: u32 = 70300;

/// Altezza massima di un ostacolo che il giocatore può superare camminando.
/// Tenere allineato a `STEP_UP` in `dungeon.rs`.
pub const STEP_HEIGHT: f32 = 0.8;

/// Una superficie è **pavimento** (camminabile) solo se `ny > FLOOR_MIN_NY`
/// (pendenza < 60°). Una superficie è **muro** se `|ny| <= FLOOR_MIN_NY`.
/// Prima i muri erano solo `|ny| <= 0.15` e i pavimenti `ny > 0.15`: le facce
/// con pendenza intermedia (pareti inclinate delle grotte, rampe ripide)
/// finivano SOLO tra i pavimenti, quindi non bloccavano e si potevano scalare
/// a gradini da `STEP_HEIGHT`, attraversando di fatto la parete.
const FLOOR_MIN_NY: f32 = 0.17;

/// **Pareti a strapiombo** (tipo caverna): facce rivolte verso il basso, con
/// `OVERHANG_MIN_NY < ny < -FLOOR_MIN_NY`, cioè quelle che si inclinano sopra
/// la testa del giocatore. Prima venivano scartate del tutto (né pavimento né
/// muro) e si passava attraverso la parete; contarle come muri pieni faceva
/// invece incastrare il giocatore contro la parte alta che sporge.
/// Ora collidono solo nella parte bassa: dalle ginocchia fino a
/// `OVERHANG_SOLID_H` metri sopra i piedi. La parte sopra viene ignorata.
const OVERHANG_SOLID_H: f32 = 1.0;
/// Le facce quasi orizzontali (soffitti, `ny` molto negativo) non sono pareti.
const OVERHANG_MIN_NY: f32 = -0.9;

/// Un modello è considerato porta solo se è "a forma di porta": sottile e
/// non più grande di una porta/doppia anta. Le descrizioni `NEW`/`CAV` sono
/// molto generiche: se un pezzo strutturale (stanza, passaggio) viene preso
/// per porta, sparisce dalla mesh statica e quindi dalle collisioni (muri e
/// pavimento) e viene pure fatto ruotare quando "lo apri".
const DOOR_MAX_THICKNESS: f32 = 0.8;
const DOOR_MAX_WIDTH: f32 = 5.0;
const DOOR_MAX_HEIGHT: f32 = 5.0;

/// Valore di `FactionOrMobileId & 0xFF` che DFU ignora esplicitamente
/// (`AddFixedRDBEnemy`: "ignore known invalid types").
const INVALID_FIXED_MOBILE_ID: u8 = 99;

/// Riconosce una porta in stile DFU: descrizione del ModelReferenceList.
fn is_dungeon_door(desc: &str, model_id: u32) -> bool {
    if model_id == RED_BRICK_DOOR_MODEL_ID {
        return false;
    }
    matches!(desc, "DOR" | "DDR" | "NEW" | "CAV")
}

/// Calcola l'angolo di apertura in base a dove si trova il pannello rispetto
/// alla cerniera (= origine del modello).
fn compute_open_angle(m: &Arch3dModel) -> f32 {
    if m.vertices.is_empty() {
        return -90.0;
    }
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    let mut sum = Vec3::ZERO;
    for v in &m.vertices {
        let p = Vec3::from(*v);
        sum += p;
        for k in 0..3 {
            mn[k] = mn[k].min(p[k]);
            mx[k] = mx[k].max(p[k]);
        }
    }
    let centroid = sum / m.vertices.len() as f32;
    let (dx, dz) = (mx[0] - mn[0], mx[2] - mn[2]);

    if dx >= dz {
        if centroid.x.abs() > 0.05 {
            if centroid.x > 0.0 { -90.0 } else { 90.0 }
        } else {
            -90.0
        }
    } else {
        if centroid.z.abs() > 0.05 {
            if centroid.z > 0.0 { -90.0 } else { 90.0 }
        } else {
            -90.0
        }
    }
}

fn door_dims(m: &Arch3dModel) -> [f32; 3] {
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for v in &m.vertices {
        for k in 0..3 {
            mn[k] = mn[k].min(v[k]);
            mx[k] = mx[k].max(v[k]);
        }
    }
    if m.vertices.is_empty() { return [0.0; 3]; }
    [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]]
}

fn is_door_shaped(m: &Arch3dModel) -> bool {
    let d = door_dims(m);
    let thin = d[0].min(d[2]);
    let wide = d[0].max(d[2]);
    thin <= DOOR_MAX_THICKNESS && wide <= DOOR_MAX_WIDTH && d[1] <= DOOR_MAX_HEIGHT
}

/// Forma di collisione di una porta a livello di pannello chiuso: un
/// segmento in XZ (asse lungo del pannello, già ruotato di `closed_yaw` e
/// traslato alla cerniera) con uno spessore, più l'intervallo verticale.
#[derive(Debug, Clone, Copy)]
pub struct DoorShape {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub half_thick: f32,
    pub y_min: f32,
    pub y_max: f32,
}

impl DoorShape {
    /// Distanza dal bordo del pannello (negativa se dentro).
    fn dist(&self, x: f32, z: f32) -> f32 {
        dist_segment_xz(x, z, [self.a[0], 0.0, self.a[1]], [self.b[0], 0.0, self.b[1]])
        - self.half_thick
    }

    /// True se `(x, z)` sta nel varco della porta (tra gli stipiti e a
    /// poca distanza dal piano del pannello). Serve a "bucare" i muri
    /// vicini alla porta SOLO nel passaggio e SOLO a porta aperta.
    fn in_passage(&self, x: f32, z: f32) -> bool {
        let (abx, abz) = (self.b[0] - self.a[0], self.b[1] - self.a[1]);
        let len = (abx * abx + abz * abz).sqrt().max(1e-4);
        let (ux, uz) = (abx / len, abz / len);
        let (px, pz) = (x - self.a[0], z - self.a[1]);
        let along = px * ux + pz * uz;
        let perp = (px * -uz + pz * ux).abs();
        along >= -0.2 && along <= len + 0.2 && perp < 1.2
    }
}

/// Calcola la forma della porta dai vertici locali del modello.
fn door_shape(m: &Arch3dModel, pos: [f32; 3], yaw_deg: f32) -> DoorShape {
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for v in &m.vertices {
        for k in 0..3 {
            mn[k] = mn[k].min(v[k]);
            mx[k] = mx[k].max(v[k]);
        }
    }
    if m.vertices.is_empty() {
        mn = [0.0; 3];
        mx = [0.0, 2.4, 0.0];
    }
    let (dx, dz) = (mx[0] - mn[0], mx[2] - mn[2]);
    let (la, lb, thin) = if dx >= dz {
        let mz = (mn[2] + mx[2]) * 0.5;
        (Vec3::new(mn[0], 0.0, mz), Vec3::new(mx[0], 0.0, mz), dz)
    } else {
        let mxx = (mn[0] + mx[0]) * 0.5;
        (Vec3::new(mxx, 0.0, mn[2]), Vec3::new(mxx, 0.0, mx[2]), dx)
    };
    let q = Quat::from_rotation_y(yaw_deg.to_radians());
    let (wa, wb) = (q.mul_vec3(la), q.mul_vec3(lb));
    DoorShape {
        a: [pos[0] + wa.x, pos[2] + wa.z],
        b: [pos[0] + wb.x, pos[2] + wb.z],
        half_thick: (thin * 0.5).max(0.05),
        y_min: pos[1] + mn[1],
        y_max: pos[1] + mx[1],
    }
}

/// Distanza XZ dal punto al triangolo **proiettato**, limitata alla banda
/// verticale del corpo del giocatore.
///
/// Il triangolo viene tagliato ai piani orizzontali `y = feet_y + 0.45`,
/// `y = feet_y + 1.1`, `y = feet_y + 1.8`. Per ogni taglio si ottiene un
/// segmento in XZ; la distanza dal punto al triangolo è la minima distanza
/// dai tre segmenti.
///
/// **Perché non la proiezione XZ del triangolo intero**: per un muro
/// verticale i due metodi coincidono (la proiezione XZ è il segmento che
/// taglia il piano, e i tre tagli sono tutti uguali alla proiezione). Per un
/// muro obliquo (gallerie a volta, tetti spioventi, rampe) la proiezione XZ
/// è molto più grande del triangolo reale e copre punti che il corpo non
/// può toccare a nessuna quota. Il giocatore al centro di una galleria a
/// volta veniva classificato "dentro la proiezione" e bloccato. Vedi
/// `tests::oblique_wall_does_not_block_when_player_is_clear`.
///
/// Il taglio può non esistere per un dato y (triangolo tutto sopra o tutto
/// sotto la quota): in quel caso contribuisce con `f32::MAX` e non
/// interferisce con gli altri.
fn tri_dist_xz(t: &Tri, x: f32, z: f32, feet_y: f32) -> f32 {
    let lo = feet_y + 0.45;
    let hi = feet_y + 1.8;
    let mid = (lo + hi) * 0.5;
    let mut best = f32::MAX;
    for y in [lo, mid, hi] {
        if let Some((a, b)) = tri_slice_at_y(t, y) {
            let d = dist_segment_xz(x, z, a, b);
            if d < best {
                best = d;
            }
        }
    }
    best
}

/// Intersezione del triangolo con il piano orizzontale `y = y_const`.
/// Restituisce il segmento in cui il piano taglia il triangolo (in 3D), o
/// `None` se il piano non lo attraversa. Stessa tecnica di
/// `WallTri::slice_at_y` in `tiles.rs`.
fn tri_slice_at_y(t: &Tri, y_const: f32) -> Option<([f32; 3], [f32; 3])> {
    let v = [t[0], t[1], t[2]];
    let mut pts: [[f32; 3]; 2] = [[0.0; 3]; 2];
    let mut n = 0;
    for k in 0..3 {
        let p = v[k];
        let q = v[(k + 1) % 3];
        let dp = p[1] - y_const;
        let dq = q[1] - y_const;
        if (dp < 0.0) != (dq < 0.0) && n < 2 {
            let s = dp / (dp - dq);
            pts[n] = [
                p[0] + (q[0] - p[0]) * s,
                y_const,
                p[2] + (q[2] - p[2]) * s,
            ];
            n += 1;
        }
    }
    (n == 2).then_some((pts[0], pts[1]))
}

#[inline]
fn tri_in_band(t: &Tri, feet_y: f32) -> bool {
    let (lo, hi) = (feet_y + 0.45, feet_y + 1.8);
    let ymin = t[0][1].min(t[1][1]).min(t[2][1]);
    let ymax = t[0][1].max(t[1][1]).max(t[2][1]);
    if ymax <= feet_y + STEP_HEIGHT { return false; }
    if ymax < lo || ymin > hi { return false; }
    true
}

/// Come `tri_in_band`, ma per le pareti a strapiombo: conta solo la parte
/// bassa, fino a `OVERHANG_SOLID_H` sopra i piedi.
#[inline]
fn overhang_in_band(t: &Tri, feet_y: f32) -> bool {
    let (lo, hi) = (feet_y + 0.45, feet_y + OVERHANG_SOLID_H);
    let ymin = t[0][1].min(t[1][1]).min(t[2][1]);
    let ymax = t[0][1].max(t[1][1]).max(t[2][1]);
    if ymax <= feet_y + STEP_HEIGHT { return false; }
    if ymax < lo || ymin > hi { return false; }
    true
}

/// Come `tri_dist_xz`, ma taglia lo strapiombo solo a quote basse
/// (`feet + 0.45 .. feet + OVERHANG_SOLID_H`): la parte alta che sporge sopra
/// il giocatore non entra nella distanza.
fn overhang_dist_xz(t: &Tri, x: f32, z: f32, feet_y: f32) -> f32 {
    let lo = feet_y + 0.45;
    let hi = feet_y + OVERHANG_SOLID_H;
    let mid = (lo + hi) * 0.5;
    let mut best = f32::MAX;
    for y in [lo, mid, hi] {
        if let Some((a, b)) = tri_slice_at_y(t, y) {
            let d = dist_segment_xz(x, z, a, b);
            if d < best {
                best = d;
            }
        }
    }
    best
}

/// Nucleo condiviso (BuiltDungeon + DungeonCollision): True se il cerchio
/// `(x, z, radius)` tocca un muro, un muro-di-porta chiusa o un pannello chiuso.
#[allow(clippy::too_many_arguments)]
fn blocked_impl(
    walls: &[Tri],
    overhangs: &[Tri],
    door_walls: &[(usize, Tri)],
                shapes: &[DoorShape],
                open: &[bool],
                x: f32,
                z: f32,
                feet_y: f32,
                radius: f32,
) -> bool {
    for t in walls {
        if tri_in_band(t, feet_y) && tri_dist_xz(t, x, z, feet_y) < radius {
            return true;
        }
    }
    for t in overhangs {
        if overhang_in_band(t, feet_y) && overhang_dist_xz(t, x, z, feet_y) < radius {
            return true;
        }
    }
    for (di, t) in door_walls {
        // Porta aperta e giocatore nel varco: il muro vicino alla porta non
        // blocca. Fuori dal varco (o a porta chiusa) il muro resta pieno.
        if open.get(*di).copied().unwrap_or(false)
            && shapes.get(*di).map_or(false, |s| s.in_passage(x, z))
            {
                continue;
            }
            if tri_in_band(t, feet_y) && tri_dist_xz(t, x, z, feet_y) < radius {
                return true;
            }
    }
    let (lo, hi) = (feet_y + 0.45, feet_y + 1.8);
    for (s, &is_open) in shapes.iter().zip(open) {
        if is_open { continue; }
        if s.y_max <= lo || s.y_min >= hi { continue; }
        if s.dist(x, z) < radius {
            return true;
        }
    }
    false
}

/// Come `blocked_impl` ma restituisce la distanza minima (limitata a `cap`).
/// Serve a `dungeon_move` per "sbloccarsi" quando il giocatore è già
/// penetrato in un muro, SENZA concedere un raggio di collisione ridotto.
#[allow(clippy::too_many_arguments)]
fn clearance_impl(
    walls: &[Tri],
    overhangs: &[Tri],
    door_walls: &[(usize, Tri)],
                  shapes: &[DoorShape],
                  open: &[bool],
                  x: f32,
                  z: f32,
                  feet_y: f32,
                  cap: f32,
) -> f32 {
    let mut best = cap;
    for t in walls {
        if tri_in_band(t, feet_y) {
            best = best.min(tri_dist_xz(t, x, z, feet_y));
        }
    }
    for t in overhangs {
        if overhang_in_band(t, feet_y) {
            best = best.min(overhang_dist_xz(t, x, z, feet_y));
        }
    }
    for (di, t) in door_walls {
        if open.get(*di).copied().unwrap_or(false)
            && shapes.get(*di).map_or(false, |s| s.in_passage(x, z))
            {
                continue;
            }
            if tri_in_band(t, feet_y) {
                best = best.min(tri_dist_xz(t, x, z, feet_y));
            }
    }
    let (lo, hi) = (feet_y + 0.45, feet_y + 1.8);
    for (s, &is_open) in shapes.iter().zip(open) {
        if is_open { continue; }
        if s.y_max <= lo || s.y_min >= hi { continue; }
        best = best.min(s.dist(x, z).max(0.0));
    }
    best
}

/// Bounds verticali locali in metri (già scalati da `Arch3dModel::parse`).
fn local_y_range(m: &Arch3dModel) -> (f32, f32) {
    let mut mn = f32::MAX;
    let mut mx = f32::MIN;
    for v in &m.vertices {
        mn = mn.min(v[1]);
        mx = mx.max(v[1]);
    }
    if mn > mx { (0.0, 2.4) } else { (mn, mx) }
}

#[derive(Debug, Clone)]
pub struct DungeonDoor {
    pub model_id: u32,
    /// Mesh in spazio locale (cerniera = origine del modello).
    pub mesh: WorldMesh,
    /// Cerniera in coordinate mondo del dungeon.
    pub pos: [f32; 3],
    /// Rotazione a porta chiusa (gradi).
    pub closed_yaw: f32,
    /// Centro **reale del pannello** in coordinate mondo (non la cerniera!),
    /// usato per il toggle E e per assegnare i muri vicini alla porta.
    pub center: [f32; 3],
    /// Angolo di apertura in gradi: `-90°` di default DFU, `+90°` per le porte
    /// con cerniera "dall'altra parte".
    pub open_angle_deg: f32,
    /// Segmento XZ del pannello chiuso (mondo), spessore e quote verticali.
    pub seg_a: [f32; 2],
    pub seg_b: [f32; 2],
    pub half_thick: f32,
    pub y_min: f32,
    pub y_max: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnemySpawnKind {
    Random,
    Fixed(Option<u8>),
}

/// Nemico da generare a runtime: posizione locale del dungeon + MobileID del
/// mostro. La posizione è nello stesso sistema delle città: `x` est, `y` su,
/// `z` = -nord (come tutto il resto del progetto).
#[derive(Debug, Clone)]
pub struct DungeonEnemySpawn {
    pub pos: [f32; 3],
    pub kind: EnemySpawnKind,
    pub mobile_id: u8,
}

/// Mappa MobileID → (archivio, record) della texture del mostro.
fn mobile_texture(mobile_id: u8) -> Option<(u16, u8)> {
    let entry = match mobile_id {
        // Mostri comuni (0..=42)
        0 => (257, 0),   // Rat
        1 => (259, 0),   // Giant Rat
        2 => (260, 0),   // Imp
        3 => (261, 0),   // Spriggan
        4 => (262, 0),   // Giant Bat
        5 => (263, 0),   // Gargoyle
        6 => (265, 0),   // Ice Atronach
        7 => (267, 0),   // Orc
        8 => (268, 0),   // Skeletal Warrior
        9 => (271, 0),   // Ghost
        10 => (275, 0),  // Mummy
        11 => (276, 0),  // Giant Scorpion
        12 => (265, 0),  // Spider
        13 => (271, 0),  // Wraith
        14 => (265, 0),  // Fire Atronach
        15 => (267, 0),  // Zombie
        16 => (259, 0),  // Centaur
        17 => (262, 0),  // Harpy
        18 => (271, 0),  // Lich
        19 => (268, 0),  // Werewolf
        20 => (259, 0),  // Nymph
        21 => (267, 0),  // Slaughterfish
        22 => (267, 0),  // Orc Sergeant
        23 => (265, 0),  // Giant Spider
        24 => (265, 0),  // Iron Atronach
        25 => (263, 0),  // Giant
        26 => (271, 0),  // Daedra Lord
        27 => (262, 0),  // Dragonling
        // Umanoidi (128..=146): usano le persone
        128..=146 => (334, mobile_id.saturating_sub(128)),
        _ => return None,
    };
    Some(entry)
}

/// Espone la tabella al resto del crate.
pub fn mobile_texture_public(mobile_id: u8) -> Option<(u16, u8)> {
    mobile_texture(mobile_id)
}

// -----------------------------------------------------------------------------
// Struttura collisioni condivisa tra Dungeon e IA Nemici
// -----------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct DungeonCollision {
    pub floors: Vec<Tri>,
    pub walls: Vec<Tri>,
    /// Pareti a strapiombo: collidono solo nel primo metro sopra i piedi.
    pub overhangs: Vec<Tri>,
    /// Centri delle porte chiuse (mantenuto per compatibilità).
    pub closed_doors: Vec<[f32; 3]>,
    pub door_walls: Vec<(usize, Tri)>,
    pub door_shapes: Vec<DoorShape>,
    pub door_open: Vec<bool>,
}

impl DungeonCollision {
    pub fn floor_at(&self, x: f32, z: f32, ref_y: f32, step: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        for t in &self.floors {
            if let Some(y) = plane_y(t, x, z) {
                if y <= ref_y + step && best.map_or(true, |b| y > b) {
                    best = Some(y);
                }
            }
        }
        best
    }

    pub fn blocked(&self, x: f32, z: f32, feet_y: f32, radius: f32) -> bool {
        blocked_impl(
            &self.walls, &self.overhangs, &self.door_walls, &self.door_shapes, &self.door_open,
            x, z, feet_y, radius,
        )
    }

    pub fn clearance(&self, x: f32, z: f32, feet_y: f32, cap: f32) -> f32 {
        clearance_impl(
            &self.walls, &self.overhangs, &self.door_walls, &self.door_shapes, &self.door_open,
            x, z, feet_y, cap,
        )
    }

    /// Controlla se la linea di vista è libera da muri/ostacoli per l'IA nemica.
    pub fn line_clear(&self, from: [f32; 3], to: [f32; 3], _radius: f32) -> bool {
        let p = Vec3::from(from);
        let q = Vec3::from(to);
        let dir = q - p;
        let len = dir.length();
        if len < 1e-4 {
            return true;
        }
        let ray_dir = dir / len;

        for t in &self.walls {
            let v0 = Vec3::from(t[0]);
            let v1 = Vec3::from(t[1]);
            let v2 = Vec3::from(t[2]);

            let e1 = v1 - v0;
            let e2 = v2 - v0;
            let h = ray_dir.cross(e2);
            let a = e1.dot(h);
            if a.abs() < 1e-7 {
                continue;
            }
            let f = 1.0 / a;
            let s = p - v0;
            let u = f * s.dot(h);
            if !(0.0..=1.0).contains(&u) {
                continue;
            }
            let q_vec = s.cross(e1);
            let v = f * ray_dir.dot(q_vec);
            if v < 0.0 || u + v > 1.0 {
                continue;
            }
            let dist = f * e2.dot(q_vec);
            if dist > 0.1 && dist < len - 0.1 {
                return false;
            }
        }
        true
    }
}

pub struct BuiltDungeon {
    pub mesh: WorldMesh,
    /// Mesh dei modelli visibili ma senza collider (la porta d'uscita 70300).
    /// In DFU è `AddStandaloneModel(..., ignoreCollider: true)`. Va
    /// renderizzata in `dungeon.rs` come la mesh principale, ma i suoi
    /// triangoli NON devono essere processati per `floors`/`walls`.
    pub visible_only: WorldMesh,
    pub spawn_pos: [f32; 3],
    pub exit_pos: [f32; 3],
    /// True se `exit_pos` viene da un marker 70300 vero (il RDB ha
    /// un'uscita esplicita). False se è il fallback a `spawn_pos`: in quel
    /// caso il tasto E **non** deve uscire dal dungeon (solo Backspace),
    /// perché altrimenti aprire una porta vicino allo spawn teletrasporta
    /// il player fuori (bug del palazzo reale).
    pub has_exit_marker: bool,
    pub lights: Vec<([f32; 3], f32)>,
    pub flats: Vec<([f32; 3], u16, u8)>,
    pub doors: Vec<DungeonDoor>,
    pub door_open: Vec<bool>,
    pub floors: Vec<Tri>,
    pub walls: Vec<Tri>,
    /// Pareti a strapiombo (facce rivolte in basso, tipo caverna): collidono
    /// solo nella parte bassa, vedi `OVERHANG_SOLID_H`.
    pub overhangs: Vec<Tri>,
    /// Muri vicini a una porta, con indice della porta. Non vengono più
    /// cancellati: restano pieni e si "aprono" solo nel varco, a porta aperta.
    pub door_walls: Vec<(usize, Tri)>,
    /// Forma di collisione del pannello di ogni porta (parallela a `doors`).
    pub door_shapes: Vec<DoorShape>,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// Nemici individuati dai flat editor 199 record 15 e 16. Nessun filtro
    /// per "prima stanza" (DFU non ce l'ha): solo il filtro `MobileID == 99`
    /// di `AddFixedRDBEnemy`.
    pub enemies: Vec<DungeonEnemySpawn>,
    /// Alias identico ad enemies per compatibilità con dungeon_enemies.rs.
    pub enemy_spawns: Vec<DungeonEnemySpawn>,
}

fn plane_y(t: &Tri, x: f32, z: f32) -> Option<f32> {
    let (a, b, c) = (t[0], t[1], t[2]);
    let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
    if d.abs() < 1e-9 { return None; }
    let l1 = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / d;
    let l2 = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / d;
    let l3 = 1.0 - l1 - l2;
    const EPS: f32 = -1e-4;
    if l1 < EPS || l2 < EPS || l3 < EPS { return None; }
    Some(l1 * a[1] + l2 * b[1] + l3 * c[1])
}

fn dist_segment_xz(x: f32, z: f32, a: [f32; 3], b: [f32; 3]) -> f32 {
    let (abx, abz) = (b[0] - a[0], b[2] - a[2]);
    let (apx, apz) = (x - a[0], z - a[2]);
    let len2 = abx * abx + abz * abz;
    let t = if len2 < 1e-9 { 0.0 } else { ((apx * abx + apz * abz) / len2).clamp(0.0, 1.0) };
    let (cx, cz) = (a[0] + abx * t, a[2] + abz * t);
    ((x - cx).powi(2) + (z - cz).powi(2)).sqrt()
}

impl BuiltDungeon {
    pub fn collision(&self) -> Arc<DungeonCollision> {
        let closed_doors = self.doors
        .iter()
        .zip(&self.door_open)
        .filter(|(_, &open)| !open)
        .map(|(d, _)| d.center)
        .collect();
        Arc::new(DungeonCollision {
            floors: self.floors.clone(),
                 walls: self.walls.clone(),
                 overhangs: self.overhangs.clone(),
                 closed_doors,
                 door_walls: self.door_walls.clone(),
                 door_shapes: self.door_shapes.clone(),
                 door_open: self.door_open.clone(),
        })
    }

    pub fn floor_at(&self, x: f32, z: f32, ref_y: f32, step: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        for t in &self.floors {
            if let Some(y) = plane_y(t, x, z) {
                if y <= ref_y + step && best.map_or(true, |b| y > b) {
                    best = Some(y);
                }
            }
        }
        best
    }

    pub fn blocked(&self, x: f32, z: f32, feet_y: f32, radius: f32) -> bool {
        blocked_impl(
            &self.walls, &self.overhangs, &self.door_walls, &self.door_shapes, &self.door_open,
            x, z, feet_y, radius,
        )
    }

    /// Distanza minima (<= `cap`) da muri/porte chiuse. Vedi `clearance_impl`.
    pub fn clearance(&self, x: f32, z: f32, feet_y: f32, cap: f32) -> f32 {
        clearance_impl(
            &self.walls, &self.overhangs, &self.door_walls, &self.door_shapes, &self.door_open,
            x, z, feet_y, cap,
        )
    }

    pub fn toggle_nearest_door(
        &mut self,
        x: f32,
        z: f32,
        feet_y: f32,
        reach: f32,
    ) -> Option<usize> {
        let mut best: Option<(f32, usize)> = None;
        for (i, d) in self.doors.iter().enumerate() {
            if (d.center[1] - feet_y).abs() > 2.5 { continue; }
            let dist = (d.center[0] - x).hypot(d.center[2] - z);
            if dist <= reach && best.map_or(true, |(b, _)| dist < b) {
                best = Some((dist, i));
            }
        }
        let (_, i) = best?;

        // Nuovo stato della porta "bersaglio".
        let new_state = !self.door_open[i];
        self.door_open[i] = new_state;

        // Apri/chiudi insieme a tutte le porte vicine (doppia anta DDR,
        // duplicati dell'RDB, o porte troppo vicine per essere distinte).
        // 1.5 m è sufficiente per coprire due pannelli della stessa porta e
        // piccoli disallineamenti tra duplicati, senza toccare porte adiacenti
        // (che stanno almeno a 2 m).
        const COMPANION_DIST_SQ: f32 = 1.5 * 1.5;
        let target_center = self.doors[i].center;
        for j in 0..self.doors.len() {
            if j == i { continue; }
            let d = &self.doors[j];
            let dx = d.center[0] - target_center[0];
            let dy = d.center[1] - target_center[1];
            let dz = d.center[2] - target_center[2];
            if dx * dx + dy * dy + dz * dz < COMPANION_DIST_SQ {
                self.door_open[j] = new_state;
            }
        }

        Some(i)
    }
}

pub fn build_dungeon(
    dungeon: &DungeonData,
    blocks_bsa: &BsaArchive,
    arch3d_bsa: &BsaArchive,
) -> Option<BuiltDungeon> {
    let mut mesh = WorldMesh::new();
    // NUOVO: modelli visibili senza collider (in DFU: standalone models).
    // Ospita la porta d'uscita 70300, che deve essere vista ma non bloccare.
    let mut visible_only = WorldMesh::new();
    let mut model_cache: HashMap<u32, Option<Arch3dModel>> = HashMap::new();
    let mut lights = Vec::new();
    let mut flats = Vec::new();
    let mut enemies: Vec<DungeonEnemySpawn> = Vec::new();
    let mut doors = Vec::new();

    let mut enter_marker: Option<[f32; 3]> = None;
    let mut exit_marker: Option<[f32; 3]> = None;

    let mut desc_counts: HashMap<String, (usize, u32)> = HashMap::new();

    // Contatori diagnostici (per log finale).
    let mut flat199_fixed_ok: usize = 0;
    let mut flat199_fixed_invalid: usize = 0;
    let mut flat199_random: usize = 0;

    for b_info in &dungeon.blocks {
        let Some(raw_rdb) = blocks_bsa.get_by_name(&b_info.block_name) else { continue };
        let Ok(rdb) = RdbBlock::parse(&b_info.block_name, raw_rdb) else { continue };

        let bx_offset = b_info.x as f32 * RDB_SIDE_METERS;
        let bz_offset = b_info.z as f32 * RDB_SIDE_METERS;

        for obj in &rdb.objects {
            let ox = bx_offset + (obj.x as f32 * 0.025);
            let oy = -(obj.y as f32 * 0.025);
            let oz = bz_offset + (obj.z as f32 * 0.025);

            match &obj.resource {
                RdbResource::Model(m_res) => {
                    let model_id = m_res.model_id;
                    if model_id == 0 { continue; }

                    // --- Marker di uscita (modello 70300) -----------------
                    //
                    // Parità con DFU (`RDBLayout.AddModels`):
                    //  - il modello 70300 viene riconosciuto in QUALSIASI
                    //    blocco, non solo in quello di partenza;
                    //  - viene aggiunto a `visible_only` (porta visibile ma
                    //    senza collider), NON alla mesh statica che genera
                    //    `floors`/`walls`.
                    if model_id == EXIT_DOOR_MODEL_ID {
                        if exit_marker.is_none() {
                            exit_marker = Some([ox, oy, oz]);
                        }
                        // Porta visibile senza collider: aggiungila a
                        // `visible_only`, così viene renderizzata ma non
                        // blocca il passaggio.
                        let model = model_cache.entry(model_id).or_insert_with(|| {
                            arch3d_bsa
                            .get_by_id(model_id)
                            .and_then(|r| Arch3dModel::parse(model_id, r).ok())
                        });
                        if let Some(m) = model.as_ref() {
                            let rot = Quat::from_rotation_z(m_res.z_rot_deg.to_radians())
                            * Quat::from_rotation_x(m_res.x_rot_deg.to_radians())
                            * Quat::from_rotation_y(m_res.y_rot_deg.to_radians());
                            add_dungeon_instance(&mut visible_only, m, [ox, oy, oz], rot);
                        }
                        continue;
                    }

                    let desc = rdb
                    .model_refs
                    .get(m_res.model_index as usize)
                    .map(|r| r.description.as_str())
                    .unwrap_or("");
                    let entry = desc_counts
                    .entry(desc.to_string())
                    .or_insert((0, model_id));
                    entry.0 += 1;

                    let model = model_cache.entry(model_id).or_insert_with(|| {
                        arch3d_bsa
                        .get_by_id(model_id)
                        .and_then(|r| Arch3dModel::parse(model_id, r).ok())
                    });
                    let Some(m) = model.as_ref() else { continue };

                    let rot = Quat::from_rotation_z(m_res.z_rot_deg.to_radians())
                    * Quat::from_rotation_x(m_res.x_rot_deg.to_radians())
                    * Quat::from_rotation_y(m_res.y_rot_deg.to_radians());

                    let named_door = is_dungeon_door(desc, model_id);
                    let door_like = named_door && is_door_shaped(m);
                    if named_door {
                        let d = door_dims(m);
                        info!(
                            "Dungeon: modello {} desc={:?} dims=({:.2},{:.2},{:.2}) yaw={:.0} -> {}",
                              model_id, desc, d[0], d[1], d[2], m_res.y_rot_deg,
                              if door_like { "PORTA" } else { "NON porta (troppo grande): geometria statica" }
                        );
                    }

                    if door_like {
                        let open_angle = compute_open_angle(m);
                        let (mn_y, mx_y) = local_y_range(m);
                        let center_y = oy + (mn_y + mx_y) * 0.5;
                        let shape = door_shape(m, [ox, oy, oz], m_res.y_rot_deg);
                        let panel_center = [
                            (shape.a[0] + shape.b[0]) * 0.5,
                            center_y,
                            (shape.a[1] + shape.b[1]) * 0.5,
                        ];

                        let mut door_mesh = WorldMesh::new();
                        door_mesh.add_instance(m, 0.0, 0.0, 0.0, 0.0);

                        doors.push(DungeonDoor {
                            model_id,
                            mesh: door_mesh,
                            pos: [ox, oy, oz],
                            closed_yaw: m_res.y_rot_deg,
                            center: panel_center,
                            open_angle_deg: open_angle,
                            seg_a: shape.a,
                            seg_b: shape.b,
                            half_thick: shape.half_thick,
                            y_min: shape.y_min,
                            y_max: shape.y_max,
                        });
                    } else {
                        add_dungeon_instance(&mut mesh, m, [ox, oy, oz], rot);
                    }
                }
                RdbResource::Flat(f_res) => {
                    if f_res.archive == 199 {
                        // Marker Enter (8) e Start (10) nella stanza iniziale.
                        if (f_res.record == 8 || f_res.record == 10) && b_info.is_starting_block {
                            if enter_marker.is_none() || f_res.record == 8 {
                                enter_marker = Some([ox, oy, oz]);
                            }
                        }

                        // Nemici: parità esatta con `RDBLayout` di DFU.
                        //
                        // Record 16 (`AddFixedRDBEnemy`):
                        //  - `typeValue = FactionOrMobileId & 0xFF` (i byte alti
                        //    sono "garbage data" secondo il commento in DFU);
                        //  - se `typeValue == 99` → ignorato (invalid enum);
                        //  - altrimenti il marker è un nemico fisso con quel
                        //    MobileID. Nessun filtro di posizione/stanza.
                        //
                        // Record 15 (`AddRandomRDBEnemyClassic`):
                        //  - il marker è solo un "punto di spawn";
                        //  - `slot = Flags`, se 0 → random 1..6;
                        //  - il tipo è scelto a runtime dalla tabella; il
                        //    nemico appare comunque nella posizione del marker.
                        //
                        // Nessun filtro per "prima stanza" esiste in DFU.
                        if f_res.record == 16 {
                            let mobile_id = (f_res.faction_or_mobile_id & 0xFF) as u8;
                            if mobile_id == INVALID_FIXED_MOBILE_ID {
                                flat199_fixed_invalid += 1;
                            } else {
                                flat199_fixed_ok += 1;
                                enemies.push(DungeonEnemySpawn {
                                    pos: [ox, oy, oz],
                                    kind: EnemySpawnKind::Fixed(Some(mobile_id)),
                                             mobile_id,
                                });
                            }
                        } else if f_res.record == 15 {
                            // Il byte basso di `faction_or_mobile_id` in DFU
                            // non è usato per i random: viene scelto a runtime
                            // dalla tabella in base a `Flags`. Il tuo
                            // `dungeon_enemies.rs` fa già la scelta. Passiamo
                            // `mobile_id = 0` per indicare "random".
                            flat199_random += 1;
                            enemies.push(DungeonEnemySpawn {
                                pos: [ox, oy, oz],
                                kind: EnemySpawnKind::Random,
                                mobile_id: 0,
                            });
                        }
                    } else {
                        flats.push(([ox, oy, oz], f_res.archive, f_res.record));
                    }
                }
                RdbResource::Light(l_res) => {
                    let radius_m = (l_res.radius as f32 * 0.025).max(7.0);
                    lights.push(([ox, oy, oz], radius_m));
                }
                RdbResource::None => {}
            }
        }
    }

    if !enemies.is_empty() || flat199_fixed_invalid > 0 {
        info!(
            "Dungeon: {} nemici fissi validi, {} fissi con MobileID=99 (ignorati), {} random; {} spawn totali",
              flat199_fixed_ok,
              flat199_fixed_invalid,
              flat199_random,
              enemies.len()
        );
    }

    if mesh.vertices.is_empty() && doors.is_empty() && visible_only.vertices.is_empty() {
        return None;
    }

    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for v in &mesh.vertices {
        for k in 0..3 {
            mn[k] = mn[k].min(v[k]);
            mx[k] = mx[k].max(v[k]);
        }
    }

    let mut floors = Vec::new();
    let mut walls = Vec::new();
    let mut overhangs: Vec<Tri> = Vec::new();
    for sm in &mesh.submeshes {
        for ch in sm.indices.chunks_exact(3) {
            let t: Tri = [
                mesh.vertices[ch[0] as usize],
                mesh.vertices[ch[1] as usize],
                mesh.vertices[ch[2] as usize],
            ];
            let e1 = [t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]];
            let e2 = [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]];
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len < 1e-6 { continue; }
            let ny = n[1] / len;
            if ny > FLOOR_MIN_NY {
                floors.push(t);
            } else if ny.abs() <= FLOOR_MIN_NY {
                walls.push(t);
            } else if ny > OVERHANG_MIN_NY {
                // ny in (OVERHANG_MIN_NY, -FLOOR_MIN_NY): parete a strapiombo.
                overhangs.push(t);
            }
        }
    }

    // Muri vicini a una porta: NON si cancellano più (prima `walls.retain`
    // eliminava ogni triangolo con baricentro entro 1.2 m dalla CERNIERA, e un
    // triangolo di muro è grande diversi metri: ne uscivano buchi enormi, anche
    // a porta chiusa). Ora vengono assegnati alla porta e smettono di bloccare
    // solo se la porta è aperta E il giocatore è nel varco.
    let door_shapes: Vec<DoorShape> = doors
    .iter()
    .map(|d: &DungeonDoor| DoorShape {
        a: d.seg_a, b: d.seg_b, half_thick: d.half_thick, y_min: d.y_min, y_max: d.y_max,
    })
    .collect();
    let mut door_walls: Vec<(usize, Tri)> = Vec::new();
    if !doors.is_empty() {
        const DOOR_CLEAR_RADIUS: f32 = 1.2;
        const DOOR_CLEAR_DOWN: f32 = 0.7;
        const DOOR_CLEAR_UP: f32 = 0.6;

        let mut kept = Vec::with_capacity(walls.len());
        for t in walls.drain(..) {
            let cx = (t[0][0] + t[1][0] + t[2][0]) / 3.0;
            let cy = (t[0][1] + t[1][1] + t[2][1]) / 3.0;
            let cz = (t[0][2] + t[1][2] + t[2][2]) / 3.0;
            let mut owner: Option<(f32, usize)> = None;
            for (i, d) in doors.iter().enumerate() {
                let dx = cx - d.center[0];
                let dz = cz - d.center[2];
                let dist = (dx * dx + dz * dz).sqrt();
                if dist < DOOR_CLEAR_RADIUS
                    && cy >= d.center[1] - DOOR_CLEAR_DOWN
                    && cy <= d.center[1] + DOOR_CLEAR_UP
                    && owner.map_or(true, |(b, _)| dist < b)
                    {
                        owner = Some((dist, i));
                    }
            }
            match owner {
                Some((_, i)) => door_walls.push((i, t)),
                None => kept.push(t),
            }
        }
        walls = kept;
    }

    let spawn_pos = enter_marker.unwrap_or([
        (mn[0] + mx[0]) * 0.5,
                                           mn[1] + 1.0,
                                           (mn[2] + mx[2]) * 0.5,
    ]);

    // **Nessun filtro finale**: DFU non ha filtri per "prima stanza", "blocco
    // di spawn" o distanza. Tutti i nemici trovati sono validi e vengono
    // restituiti al chiamante. Se nella sala del trono non vuoi nemici, la
    // causa è che il RDB del palazzo non ne contiene: se il tuo port ne
    // spawna, sono marker che DFU scarterebbe (MobileID=99 o specie
    // inesistente) — gestito qui sopra con `INVALID_FIXED_MOBILE_ID` e, a
    // valle, in `dungeon_enemies.rs` con `species_index()`.

    // -----------------------------------------------------------------------
    // Allineamento della Y del marker 70300
    // -----------------------------------------------------------------------
    //
    // Il marker 70300 ha spesso `y_raw = -1` (una sentinella che significa
    // "appoggiami al pavimento"): la sua Y non è affidabile. La porta vera
    // è la `5500x` costruita dal RDB, che ha la Y del piano in cui è modellata.
    //
    // Se il marker è "a terra" (`|oy| < 1.0`) ma la porta più vicina in XZ ha
    // una Y molto diversa (> 3 m), usiamo la Y della porta: altrimenti il
    // toggle E confronta `d.center[1]` (Y della porta) con `p.feet_y -
    // origin.y` (Y del giocatore) e non trova mai la porta.
    if let Some([mx, my, mz]) = exit_marker {
        if my.abs() < 1.0 {
            let near_door = doors
            .iter()
            .filter(|d| {
                let dx = d.center[0] - mx;
                let dz = d.center[2] - mz;
                (dx * dx + dz * dz).sqrt() < 3.0
            })
            .min_by(|a, b| {
                let da = (a.center[0] - mx).powi(2) + (a.center[2] - mz).powi(2);
                let db = (b.center[0] - mx).powi(2) + (b.center[2] - mz).powi(2);
                da.total_cmp(&db)
            });
            if let Some(door) = near_door {
                let dy = door.center[1] - my;
                if dy.abs() > 3.0 {
                    info!(
                        "FIX: marker 70300 a y={:.1} (sentinella), porta vicina a y={:.1} \
(dy={:.1}). Allineo exit_pos alla Y della porta.",
                          my, door.center[1], dy
                    );
                    exit_marker = Some([mx, door.center[1], mz]);
                }
            }
        }
    }

    let has_exit_marker = exit_marker.is_some();
    let exit_pos = exit_marker.unwrap_or(spawn_pos);
    let door_count = doors.len();
    let enemy_spawns = enemies.clone();

    if has_exit_marker {
        info!(
            "Dungeon: marker di uscita (70300) trovato a ({:.1}, {:.1}, {:.1})",
              exit_pos[0], exit_pos[1], exit_pos[2]
        );
    } else {
        info!(
            "Dungeon: nessun marker di uscita (70300) nel RDB, uso lo spawn come fallback"
        );
    }

    Some(BuiltDungeon {
        mesh,
         visible_only,
         spawn_pos,
         exit_pos,
         has_exit_marker,
         lights,
         flats,
         doors,
         door_open: vec![false; door_count],
         floors,
         walls,
         overhangs,
         door_walls,
         door_shapes,
         bounds_min: mn,
         bounds_max: mx,
         enemies,
         enemy_spawns,
    })
}

/// Distanza (m) con cui si separano due facce coincidenti.
const COPLANAR_PUSH: f32 = 0.005;
/// Due facce sono "coincidenti" se i piani distano meno di così.
const COPLANAR_PLANE_EPS: f32 = 0.003;

/// Arazzi, quadri e simili in DF sono spesso **due facce nello stesso
/// identico piano**, una per lato (es. giglio davanti, drago dietro), pensate
/// per il backface culling. Qui i materiali sono `double_sided` senza culling,
/// quindi entrambe le facce vengono disegnate e il depth buffer sceglie quale
/// mostrare in base all'angolo di vista (z-fighting): da qui "a seconda
/// dell'angolazione vedi giglio o drago".
///
/// Per ogni coppia di facce coincidenti si calcola uno spostamento
/// deterministico lungo la normale:
///  - normali opposte: ciascuna si sposta verso il PROPRIO lato, così da
///    ogni lato è sempre più vicina la faccia giusta;
///  - stessa normale (sovrapposizione/decal): la faccia successiva
///    nell'elenco vince, spostata in avanti.
/// Quanto (m) si rimpiccioliscono i poligoni verso il centro prima del test di sovrapposizione:
/// due facce che si toccano solo lungo un lato (o con meno di 1 cm di sovrapposizione) non
/// contano come sovrapposte.
const COPLANAR_OVERLAP_SHRINK: f32 = 0.01;

/// Due poligoni (coplanari, normale `n`) si sovrappongono davvero in area? Test degli assi
/// separatori sui poligoni proiettati nel piano, assumendoli convessi (le facce DF lo sono).
///
/// Prima `coplanar_face_offsets` usava solo il riquadro (AABB) delle facce: due facce
/// adiacenti nello stesso piano, che condividono soltanto un lato, hanno gli AABB
/// sovrapposti, quindi una delle due veniva spinta di 5 mm fuori dal piano: un gradino
/// fra pannelli vicini, che da vicino o di striscio apre una fessura e un "disallineamento".
fn polys_overlap(a: &[Vec3], b: &[Vec3], n: Vec3) -> bool {
    if a.len() < 3 || b.len() < 3 {
        return false;
    }
    let t = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let u = n.cross(t).normalize_or_zero();
    if u == Vec3::ZERO {
        return false;
    }
    let v = n.cross(u);
    let project = |pts: &[Vec3]| -> Vec<[f32; 2]> {
        let c = pts.iter().copied().sum::<Vec3>() / pts.len() as f32;
        pts.iter()
            .map(|&p| {
                let d = p - c;
                let len = d.length();
                let q = if len > 1e-6 {
                    c + d * (1.0 - COPLANAR_OVERLAP_SHRINK.min(len * 0.5) / len)
                } else {
                    p
                };
                [q.dot(u), q.dot(v)]
            })
            .collect()
    };
    let (pa, pb) = (project(a), project(b));
    let range = |pts: &[[f32; 2]], axis: [f32; 2], len: f32| -> (f32, f32) {
        let (mut mn, mut mx) = (f32::MAX, f32::MIN);
        for s in pts {
            let d = (s[0] * axis[0] + s[1] * axis[1]) / len;
            mn = mn.min(d);
            mx = mx.max(d);
        }
        (mn, mx)
    };
    for poly in [&pa, &pb] {
        for i in 0..poly.len() {
            let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
            let axis = [-(q[1] - p[1]), q[0] - p[0]];
            let len = (axis[0] * axis[0] + axis[1] * axis[1]).sqrt();
            if len < 1e-9 {
                continue;
            }
            let (amn, amx) = range(&pa, axis, len);
            let (bmn, bmx) = range(&pb, axis, len);
            if amx <= bmn || bmx <= amn {
                return false; // asse separatore: non si sovrappongono
            }
        }
    }
    true
}

fn coplanar_face_offsets(model: &Arch3dModel, wv: &[[f32; 3]]) -> Vec<Vec3> {
    struct Info { n: Vec3, d: f32, mn: Vec3, mx: Vec3, ok: bool, pts: Vec<Vec3> }

    let nf = model.faces.len();
    let mut offs = vec![Vec3::ZERO; nf];
    if nf < 2 || nf > 400 {
        return offs;
    }

    let infos: Vec<Info> = model
    .faces
    .iter()
    .map(|f| {
        let mut mn = Vec3::splat(f32::MAX);
        let mut mx = Vec3::splat(f32::MIN);
        let mut pts: Vec<Vec3> = Vec::new();
        for &i in &f.vertex_indices {
            if let Some(v) = wv.get(i as usize) {
                let p = Vec3::from(*v);
                mn = mn.min(p);
                mx = mx.max(p);
                pts.push(p);
            }
        }
        // Stessa convenzione di `add_dungeon_instance`: normale del
        // triangolo (p0, p2, p1).
        let mut n = Vec3::ZERO;
        if pts.len() >= 3 {
            for k in 1..pts.len() - 1 {
                let c = (pts[k + 1] - pts[0]).cross(pts[k] - pts[0]);
                if c.length() > 1e-6 {
                    n = c.normalize();
                    break;
                }
            }
        }
        let d = if pts.is_empty() { 0.0 } else { n.dot(pts[0]) };
        Info { n, d, mn, mx, ok: n != Vec3::ZERO, pts }
    })
    .collect();

    let tol = Vec3::splat(0.02);
    for i in 0..nf {
        if !infos[i].ok { continue; }
        for j in (i + 1)..nf {
            if !infos[j].ok { continue; }
            let (a, b) = (&infos[i], &infos[j]);
            let dot = a.n.dot(b.n);
            if dot.abs() < 0.999 { continue; }
            let plane_dist = if dot > 0.0 { (a.d - b.d).abs() } else { (a.d + b.d).abs() };
            if plane_dist > COPLANAR_PLANE_EPS { continue; }
            if !((a.mn - tol).cmple(b.mx).all() && (b.mn - tol).cmple(a.mx).all()) {
                continue;
            }
            // Gli AABB si toccano anche per facce adiacenti: serve una sovrapposizione vera.
            if !polys_overlap(&a.pts, &b.pts, a.n) {
                continue;
            }
            if dot < 0.0 {
                offs[i] += a.n * COPLANAR_PUSH;
                offs[j] += b.n * COPLANAR_PUSH;
            } else {
                offs[j] += b.n * COPLANAR_PUSH;
            }
        }
    }
    offs
}

/// Di quanto (m) si allargano nel piano i triangoli per chiudere le fessure fra modelli
/// adiacenti. DF costruisce le stanze con modelli separati e dopo rotazione e conversione
/// in float i bordi non combaciano (e ci sono giunzioni a T): dalla fessura si vede lo
/// sfondo, una linea sottile nera lungo il bordo di un triangolo. Ogni lato si sposta in
/// fuori di `SEAM_GROW`, quindi si chiudono fessure fino a 2 x `SEAM_GROW`; la striscia di
/// sovrapposizione e' coplanare e gli UV proseguono linearmente, quindi non si vede.
const SEAM_GROW: f32 = 0.03;
/// Facce con `|ny|` almeno cosi' = pavimenti, soffitti, rampe; sotto = muri.
const SEAM_MIN_NY: f32 = 0.5;
/// Pavimenti, soffitti e rampe: triangoli piu' sottili di cosi' (raggio del cerchio
/// inscritto, m) non si toccano.
const SEAM_MIN_INRADIUS: f32 = 0.03;
/// Muri: si allargano solo i triangoli grandi (pareti, colonne), non gli oggetti piccoli
/// (sedie, gambe di tavolo, torce), che altrimenti diventerebbero visibilmente piu' spessi.
const SEAM_WALL_MIN_INRADIUS: f32 = 0.2;
/// Tetto del fattore di scala: un triangolo piccolo non cresce a dismisura.
const SEAM_MAX_SCALE: f32 = 1.3;

/// Allarga il triangolo nel suo piano: ogni lato si sposta in fuori di `grow` m (scala
/// attorno all'incentro). Gli UV seguono la posizione (estrapolazione lineare), cosi' la
/// texture non si deforma.
fn grow_triangle(
    p: [[f32; 3]; 3],
    uv: [[f32; 2]; 3],
    grow: f32,
) -> ([[f32; 3]; 3], [[f32; 2]; 3]) {
    let (a, b, c) = (Vec3::from(p[0]), Vec3::from(p[1]), Vec3::from(p[2]));
    let n = (b - a).cross(c - a);
    let area2 = n.length();
    if grow <= 0.0 || area2 < 1e-9 {
        return (p, uv);
    }
    let min_r = if (n.y / area2).abs() >= SEAM_MIN_NY {
        SEAM_MIN_INRADIUS
    } else {
        SEAM_WALL_MIN_INRADIUS
    };
    // lati opposti ai vertici 0, 1, 2
    let (la, lb, lc) = ((b - c).length(), (c - a).length(), (a - b).length());
    let perim = la + lb + lc;
    let r = area2 / perim; // raggio del cerchio inscritto
    if r < min_r {
        return (p, uv);
    }
    let s = (1.0 + grow / r).min(SEAM_MAX_SCALE);
    let w = [la / perim, lb / perim, lc / perim]; // pesi dell'incentro
    let mut out_p = p;
    let mut out_uv = uv;
    for i in 0..3 {
        // vertice allargato = incentro + (vertice - incentro) * s, cioe' pesi baricentrici
        // (1 - s) * w + s * e_i rispetto al triangolo originale.
        let mut lam = [(1.0 - s) * w[0], (1.0 - s) * w[1], (1.0 - s) * w[2]];
        lam[i] += s;
        out_p[i] = [
            lam[0] * p[0][0] + lam[1] * p[1][0] + lam[2] * p[2][0],
            lam[0] * p[0][1] + lam[1] * p[1][1] + lam[2] * p[2][1],
            lam[0] * p[0][2] + lam[1] * p[1][2] + lam[2] * p[2][2],
        ];
        out_uv[i] = [
            lam[0] * uv[0][0] + lam[1] * uv[1][0] + lam[2] * uv[2][0],
            lam[0] * uv[0][1] + lam[1] * uv[1][1] + lam[2] * uv[2][1],
        ];
    }
    (out_p, out_uv)
}

fn add_dungeon_instance(
    world_mesh: &mut WorldMesh,
    model: &Arch3dModel,
    pos: [f32; 3],
    rot: Quat,
) {
    let p_offset = Vec3::from(pos);
    let world_verts: Vec<[f32; 3]> = model
    .vertices
    .iter()
    .map(|v| (rot.mul_vec3(Vec3::from(*v)) + p_offset).to_array())
    .collect();

    let mut grouped: HashMap<(u16, u8), Vec<u32>> = HashMap::new();
    let face_offsets = coplanar_face_offsets(model, &world_verts);

    for (fi, face) in model.faces.iter().enumerate() {
        let off = face_offsets[fi].to_array();
        let n_pts = face.vertex_indices.len();
        if n_pts < 3 { continue; }

        let key = (face.texture_archive, face.texture_record);
        let indices = grouped.entry(key).or_default();

        for k in 1..n_pts - 1 {
            let idx0 = face.vertex_indices[0] as usize;
            let idx1 = face.vertex_indices[k] as usize;
            let idx2 = face.vertex_indices[k + 1] as usize;

            if idx0 >= world_verts.len() || idx1 >= world_verts.len() || idx2 >= world_verts.len() {
                continue;
            }

            let add = |p: [f32; 3]| [p[0] + off[0], p[1] + off[1], p[2] + off[2]];
            let p0 = add(world_verts[idx0]);
            let p1 = add(world_verts[idx1]);
            let p2 = add(world_verts[idx2]);

            let v01 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
            let v02 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
            let nx = v01[1] * v02[2] - v01[2] * v02[1];
            let ny = v01[2] * v02[0] - v01[0] * v02[2];
            let nz = v01[0] * v02[1] - v01[1] * v02[0];
            let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-5);
            let normal = [nx / len, ny / len, nz / len];

            let uv0 = face.uvs.first().copied().unwrap_or([0.0, 0.0]);
            let uv1 = face.uvs.get(k).copied().unwrap_or([1.0, 0.0]);
            let uv2 = face.uvs.get(k + 1).copied().unwrap_or([0.0, 1.0]);

            // Chiude le fessure fra modelli adiacenti (pavimenti, soffitti, rampe, muri grandi).
            let ([p0, p1, p2], [uv0, uv1, uv2]) =
                grow_triangle([p0, p1, p2], [uv0, uv1, uv2], SEAM_GROW);

            let start_idx = world_mesh.vertices.len() as u32;

            world_mesh.vertices.push(p0);
            world_mesh.normals.push(normal);
            world_mesh.uvs.push(uv0);

            world_mesh.vertices.push(p2);
            world_mesh.normals.push(normal);
            world_mesh.uvs.push(uv2);

            world_mesh.vertices.push(p1);
            world_mesh.normals.push(normal);
            world_mesh.uvs.push(uv1);

            indices.push(start_idx);
            indices.push(start_idx + 1);
            indices.push(start_idx + 2);
        }
    }

    for ((archive, record), idxs) in grouped {
        if let Some(existing) = world_mesh
            .submeshes
            .iter_mut()
            .find(|s| s.texture_archive == archive && s.texture_record == record)
            {
                existing.indices.extend(idxs);
            } else {
                world_mesh.submeshes.push(super::mesh_builder::Submesh {
                    texture_archive: archive,
                    texture_record: record,
                    indices: idxs,
                });
            }
    }
}

// =============================================================================
// Test
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Parete obliqua in XZ (non verticale): triangolo `[-2,0,-2], [2,0,2], [-2,2,2]`.
    ///
    /// La sua proiezione XZ è il triangolo `(-2,-2), (2,2), (-2,2)`, che
    /// **copre l'origine (0,0)** (il punto sta sul bordo `y=x`). Il vecchio
    /// `tri_dist_xz` restituiva `0.0` per l'origine e bloccava il giocatore.
    /// Il nuovo codice taglia a `y = 0.45`, `1.1`, `1.8`: a ciascuna di
    /// queste quote il triangolo si è già ristretto verso `(-2, 2)` e il
    /// punto (0,0) è ben lontano dal taglio.
    ///
    /// Nella galleria del dungeon iniziale, le pareti a volta hanno
    /// esattamente questa forma: la proiezione XZ copre tutta la sezione
    /// trasversale, ma a quota del corpo le pareti sono solo ai lati.
    #[test]
    fn oblique_wall_does_not_block_when_player_is_clear() {
        let crossed: Tri = [[-2.0, 0.0, -2.0], [2.0, 0.0, 2.0], [-2.0, 2.0, 2.0]];
        let feet = 0.0;

        // Il giocatore al centro (0,0) NON deve essere bloccato.
        assert!(
            !blocked_impl(&[crossed], &[], &[], &[], &[], 0.0, 0.0, feet, 0.35),
                "il giocatore al centro non deve essere bloccato da una parete obliqua che lo sovrasta",
        );

        // Ma un punto **a contatto con la parte di muro che il corpo può
        // toccare** deve bloccarlo. A y=1.1 il taglio è il segmento da
        // (-2, 0.2) a (-0.2, 2); il punto (-1.5, 1.1) dista ~0.28 da esso.
        assert!(
            blocked_impl(&[crossed], &[], &[], &[], &[], -1.5, 1.1, feet, 0.35),
                "il giocatore a contatto con la parete obliqua deve essere bloccato",
        );

        // Controllo: la funzione `tri_dist_xz` da sola è coerente.
        // Al centro: distanza > radius.
        assert!(tri_dist_xz(&crossed, 0.0, 0.0, feet) > 0.35);
        // Vicino al taglio: distanza < radius.
        assert!(tri_dist_xz(&crossed, -1.5, 1.1, feet) < 0.35);
    }

    /// Un muro verticale si comporta come prima: a qualunque quota il taglio
    /// coincide con la proiezione XZ e la distanza è la stessa.
    #[test]
    fn vertical_wall_behaviour_is_unchanged() {
        let vertical: Tri = [[1.0, 0.0, 0.0], [1.0, 0.0, 2.0], [1.0, 2.0, 1.0]];
        let feet = 0.0;

        // A 0.7 m dal muro: bloccato.
        assert!(blocked_impl(&[vertical], &[], &[], &[], &[], 0.7, 1.0, feet, 0.35));
        // A -0.5 m: libero.
        assert!(!blocked_impl(&[vertical], &[], &[], &[], &[], -0.5, 1.0, feet, 0.35));
        // A 0.0: a 1 m dal muro, libero.
        assert!(!blocked_impl(&[vertical], &[], &[], &[], &[], 0.0, 1.0, feet, 0.35));
    }

    /// Un muro basso (tutto sotto `STEP_HEIGHT`) non blocca, come prima.
    #[test]
    fn low_wall_does_not_block() {
        let low: Tri = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 0.4, 0.0]];
        assert!(!blocked_impl(&[low], &[], &[], &[], &[], 2.0, 0.0, 0.0, 0.35));
    }

    /// Parete a strapiombo (si inclina sopra la testa del giocatore): il piede
    /// è in x=-1, la cima sporge fino a x=+1. Il giocatore sotto la cima NON
    /// deve essere bloccato dalla parte alta, ma contro la parte bassa si'.
    #[test]
    fn overhang_blocks_only_lower_part() {
        // Piano inclinato lungo z: base a x=-1 (y=0), cima a x=+1 (y=3).
        let ov: Tri = [[-1.0, 0.0, -3.0], [-1.0, 0.0, 3.0], [1.0, 3.0, 0.0]];
        let feet = 0.0;

        // Sotto la parte alta (x=0.8): le quote > 1 m sono ignorate; tra y=0.45
        // e y=1.0 la parete sta tra x=-0.7 e x=-0.33, quindi a 0.8 e' libero.
        assert!(!blocked_impl(&[], &[ov], &[], &[], &[], 0.8, 0.0, feet, 0.35));
        // Contro la parte bassa (x=-0.5, a ~0.02 m dal taglio a y=0.73): bloccato.
        assert!(blocked_impl(&[], &[ov], &[], &[], &[], -0.5, 0.0, feet, 0.35));
        // Lontano dalla parete: libero.
        assert!(!blocked_impl(&[], &[ov], &[], &[], &[], -2.5, 0.0, feet, 0.35));
    }

    /// Un pavimento si allarga di `SEAM_GROW` per lato; gli UV seguono la posizione.
    #[test]
    fn seam_growth_moves_edges_out_and_keeps_uvs_linear() {
        let p = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 0.0, 4.0]];
        let uv = [[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]]; // uv = (x, z)
        let (g, guv) = grow_triangle(p, uv, 0.03);
        for i in 0..3 {
            assert_eq!(g[i][1], 0.0, "il piano non cambia");
            assert!((guv[i][0] - g[i][0]).abs() < 1e-4, "uv.x segue x");
            assert!((guv[i][1] - g[i][2]).abs() < 1e-4, "uv.y segue z");
        }
        let min_x = g.iter().map(|v| v[0]).fold(f32::MAX, f32::min);
        let min_z = g.iter().map(|v| v[2]).fold(f32::MAX, f32::min);
        assert!((min_x + 0.03).abs() < 1e-3, "lato x=0 spostato di 3 cm: {min_x}");
        assert!((min_z + 0.03).abs() < 1e-3, "lato z=0 spostato di 3 cm: {min_z}");
    }

    /// Due pavimenti con una fessura di 4 cm dopo l'allargamento si sovrappongono.
    #[test]
    fn seam_growth_closes_a_gap() {
        let left = [[-4.0, 0.0, 0.0], [-0.02, 0.0, 0.0], [-4.0, 0.0, 4.0]];
        let right = [[0.02, 0.0, 0.0], [4.0, 0.0, 0.0], [0.02, 0.0, 4.0]];
        let uv = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let (l, _) = grow_triangle(left, uv, SEAM_GROW);
        let (r, _) = grow_triangle(right, uv, SEAM_GROW);
        let left_edge = l.iter().map(|v| v[0]).fold(f32::MIN, f32::max);
        let right_edge = r.iter().map(|v| v[0]).fold(f32::MAX, f32::min);
        assert!(left_edge >= right_edge, "{left_edge} vs {right_edge}");
    }

    /// Un muro grande si allarga nel suo piano (qui z = 0: la z non cambia).
    #[test]
    fn seam_growth_expands_big_walls_in_plane() {
        let wall = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 4.0, 0.0]];
        let uv = [[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]];
        let (g, guv) = grow_triangle(wall, uv, SEAM_GROW);
        for i in 0..3 {
            assert_eq!(g[i][2], 0.0, "il piano non cambia");
            assert!((guv[i][0] - g[i][0]).abs() < 1e-4);
            assert!((guv[i][1] - g[i][1]).abs() < 1e-4);
        }
        let min_x = g.iter().map(|v| v[0]).fold(f32::MAX, f32::min);
        let min_y = g.iter().map(|v| v[1]).fold(f32::MAX, f32::min);
        assert!((min_x + 0.03).abs() < 1e-3 && (min_y + 0.03).abs() < 1e-3);
    }

    /// Gli oggetti piccoli (facce verticali sottili) non si toccano.
    #[test]
    fn seam_growth_leaves_small_walls_alone() {
        let small = [[0.0, 0.0, 0.0], [0.3, 0.0, 0.0], [0.0, 0.3, 0.0]];
        let uv = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let (g, guv) = grow_triangle(small, uv, SEAM_GROW);
        assert_eq!(g, small);
        assert_eq!(guv, uv);
    }

    #[test]
    fn adjacent_coplanar_faces_do_not_overlap() {
        let sq = |x0: f32, z0: f32, s: f32| {
            [
                Vec3::new(x0, 0.0, z0),
                Vec3::new(x0 + s, 0.0, z0),
                Vec3::new(x0 + s, 0.0, z0 + s),
                Vec3::new(x0, 0.0, z0 + s),
            ]
        };
        let n = Vec3::Y;
        // si toccano lungo un lato: niente spinta
        assert!(!polys_overlap(&sq(0.0, 0.0, 1.0), &sq(1.0, 0.0, 1.0), n));
        // separati
        assert!(!polys_overlap(&sq(0.0, 0.0, 1.0), &sq(1.5, 0.0, 1.0), n));
        // sovrapposti a meta'
        assert!(polys_overlap(&sq(0.0, 0.0, 1.0), &sq(0.5, 0.0, 1.0), n));
        // un decal dentro un pannello grande
        assert!(polys_overlap(&sq(0.0, 0.0, 2.0), &sq(0.5, 0.5, 0.4), n));
        // sovrapposizione di soli 5 mm: trattata come contatto
        assert!(!polys_overlap(&sq(0.0, 0.0, 1.0), &sq(0.995, 0.0, 1.0), n));
    }
}
