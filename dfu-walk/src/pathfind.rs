//! Ricerca di percorso a griglia (A*) per i dungeon, senza dipendenze da Bevy.
//!
//! La geometria del dungeon non e' una griglia: e' una mesh. Qui la si "interroga" tramite
//! una funzione `probe(x, z, y_indicativa) -> Option<y_pavimento>` (nel gioco usa
//! `BuiltDungeon::floor_at` e `blocked`): `Some(y)` se in quel punto si puo' stare in piedi.
//!
//! - Griglia 2.5D: lo stato e' (cella x, cella z, livello), con livelli di `LEVEL_H` metri,
//!   cosi' piani sovrapposti (scale, balconi) non si confondono.
//! - 8 direzioni; le diagonali si fanno solo se i due lati sono liberi (niente tagli d'angolo).
//! - A "passi": `Search::step(budget, probe)` espande al massimo `budget` nodi e restituisce
//!   `Searching`, `Found(percorso)` o `Failed`. In un dungeon grande basta chiamarlo ogni
//!   frame con qualche migliaio di nodi: nessun blocco.
//! - Ogni risposta di `probe` e' in cache (cella + livello), quindi ogni cella si interroga
//!   una volta sola anche se raggiunta da 8 vicini.
//!
//! Il percorso e' una polilinea di celle; `resample` la ridisegna a punti equidistanti.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

/// Altezza di un "livello" per distinguere piani sovrapposti.
const LEVEL_H: f32 = 2.0;
const DIRS: [(i32, i32); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];

type Key = (i32, i32, i32);

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Searching,
    /// Percorso dal punto di partenza all'obiettivo, `[x, y, z]`.
    Found(Vec<[f32; 3]>),
    Failed,
}

#[derive(Clone, Copy)]
struct Open {
    f: f32,
    key: Key,
}

impl PartialEq for Open {
    fn eq(&self, o: &Self) -> bool {
        self.f == o.f && self.key == o.key
    }
}
impl Eq for Open {}
impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Open {
    // BinaryHeap e' un max-heap: invertiamo per estrarre la `f` minore
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.partial_cmp(&self.f).unwrap_or(Ordering::Equal).then_with(|| self.key.cmp(&o.key))
    }
}

fn level(y: f32) -> i32 {
    (y / LEVEL_H).round() as i32
}

pub fn dist3(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Lunghezza di una polilinea.
pub fn path_length(path: &[[f32; 3]]) -> f32 {
    path.windows(2).map(|w| dist3(w[0], w[1])).sum()
}

/// Interroga `probe` con la quota arrotondata a 0.5 m e ne memorizza la risposta.
fn probe_q(
    cache: &mut HashMap<Key, Option<f32>>,
    cell: f32,
    ix: i32,
    iz: i32,
    hint: f32,
    probe: &mut dyn FnMut(f32, f32, f32) -> Option<f32>,
) -> Option<f32> {
    let hq = (hint * 2.0).round() / 2.0;
    let key = (ix, iz, (hint * 2.0).round() as i32);
    if let Some(v) = cache.get(&key) {
        return *v;
    }
    let v = probe(ix as f32 * cell, iz as f32 * cell, hq);
    cache.insert(key, v);
    v
}

/// Insieme delle celle calpestabili raggiungibili da un punto (vedi `flood`).
pub struct Reach {
    cell: f32,
    /// Per ogni colonna (x, z): le quote dei pavimenti raggiunti (piani sovrapposti).
    cols: HashMap<(i32, i32), Vec<f32>>,
    truncated: bool,
}

impl Reach {
    /// Numero di celle raggiunte.
    pub fn len(&self) -> usize {
        self.cols.values().map(Vec::len).sum()
    }

    /// True se si e' fermata al tetto `max_cells` (l'insieme e' incompleto).
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// C'e' una cella raggiunta entro `radius` m in orizzontale e `max_dy` m in verticale da `p`?
    pub fn contains_near(&self, p: [f32; 3], radius: f32, max_dy: f32) -> bool {
        let r = (radius / self.cell).ceil() as i32;
        let (cx, cz) = ((p[0] / self.cell).round() as i32, (p[2] / self.cell).round() as i32);
        for ix in cx - r..=cx + r {
            for iz in cz - r..=cz + r {
                let (x, z) = (ix as f32 * self.cell, iz as f32 * self.cell);
                if (x - p[0]).hypot(z - p[2]) > radius {
                    continue;
                }
                if let Some(ys) = self.cols.get(&(ix, iz)) {
                    if ys.iter().any(|y| (y - p[1]).abs() <= max_dy) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Tutte le celle raggiungibili a piedi da `start` (visita in ampiezza, stesse regole di
/// `Search`: 8 direzioni, niente tagli d'angolo, piani separati per quota). Serve a sapere
/// se un punto sta in una zona collegata all'ingresso, senza cercare un percorso preciso.
/// `max_cells`: tetto di celle visitate (poi `truncated()` e' vero).
pub fn flood(
    start: [f32; 3],
    cell: f32,
    max_cells: usize,
    probe: &mut dyn FnMut(f32, f32, f32) -> Option<f32>,
) -> Reach {
    let cell = cell.max(0.1);
    let mut cache: HashMap<Key, Option<f32>> = HashMap::new();
    let mut seen: HashSet<Key> = HashSet::new();
    let mut cols: HashMap<(i32, i32), Vec<f32>> = HashMap::new();
    let mut queue: VecDeque<(i32, i32, f32)> = VecDeque::new();
    let (sx, sz) = ((start[0] / cell).round() as i32, (start[2] / cell).round() as i32);
    seen.insert((sx, sz, level(start[1])));
    cols.entry((sx, sz)).or_default().push(start[1]);
    queue.push_back((sx, sz, start[1]));
    let mut truncated = false;

    while let Some((ix, iz, y)) = queue.pop_front() {
        if seen.len() >= max_cells {
            truncated = true;
            break;
        }
        for (dx, dz) in DIRS {
            let (nx, nz) = (ix + dx, iz + dz);
            let Some(fy) = probe_q(&mut cache, cell, nx, nz, y, probe) else { continue };
            if dx != 0 && dz != 0
                && (probe_q(&mut cache, cell, ix + dx, iz, y, probe).is_none()
                    || probe_q(&mut cache, cell, ix, iz + dz, y, probe).is_none())
            {
                continue;
            }
            if !seen.insert((nx, nz, level(fy))) {
                continue;
            }
            cols.entry((nx, nz)).or_default().push(fy);
            queue.push_back((nx, nz, fy));
        }
    }
    Reach { cell, cols, truncated }
}

pub struct Search {
    cell: f32,
    goal: [f32; 3],
    goal_radius: f32,
    max_expanded: usize,
    expanded: usize,
    open: BinaryHeap<Open>,
    g: HashMap<Key, f32>,
    pos: HashMap<Key, [f32; 3]>,
    parent: HashMap<Key, Key>,
    closed: HashSet<Key>,
    cache: HashMap<Key, Option<f32>>,
    done: Option<Status>,
    /// Il nodo espanso piu' vicino all'obiettivo in orizzontale: (distanza, posizione).
    closest: Option<(f32, [f32; 3])>,
    hit_limit: bool,
}

impl Search {
    /// `cell`: lato della cella (m). `goal_radius`: distanza orizzontale a cui si considera
    /// raggiunto l'obiettivo. `max_expanded`: tetto di nodi prima di rinunciare.
    pub fn new(start: [f32; 3], goal: [f32; 3], cell: f32, goal_radius: f32, max_expanded: usize) -> Search {
        let cell = cell.max(0.1);
        let start_key: Key = ((start[0] / cell).round() as i32, (start[2] / cell).round() as i32, level(start[1]));
        let mut s = Search {
            cell,
            goal,
            goal_radius,
            max_expanded,
            expanded: 0,
            open: BinaryHeap::new(),
            g: HashMap::new(),
            pos: HashMap::new(),
            parent: HashMap::new(),
            closed: HashSet::new(),
            cache: HashMap::new(),
            done: None,
            closest: None,
            hit_limit: false,
        };
        s.g.insert(start_key, 0.0);
        s.pos.insert(start_key, start);
        s.open.push(Open { f: (start[0] - goal[0]).hypot(start[2] - goal[2]), key: start_key });
        s
    }

    pub fn expanded(&self) -> usize {
        self.expanded
    }

    /// Punto raggiunto piu' vicino all'obiettivo (posizione, distanza orizzontale m).
    /// Utile quando la ricerca fallisce: dice dove si ferma il mondo calpestabile.
    pub fn closest(&self) -> Option<([f32; 3], f32)> {
        self.closest.map(|(d, p)| (p, d))
    }

    /// True se la ricerca e' stata interrotta dal tetto `max_expanded` (e non perche'
    /// ha esaurito tutte le celle raggiungibili).
    pub fn hit_limit(&self) -> bool {
        self.hit_limit
    }

    fn finish(&mut self, status: Status) -> Status {
        self.done = Some(status.clone());
        status
    }

    fn reached(&self, p: [f32; 3]) -> bool {
        (p[0] - self.goal[0]).hypot(p[2] - self.goal[2]) <= self.goal_radius && (p[1] - self.goal[1]).abs() <= 3.0
    }

    fn probe_cached(
        &mut self,
        ix: i32,
        iz: i32,
        hint: f32,
        probe: &mut dyn FnMut(f32, f32, f32) -> Option<f32>,
    ) -> Option<f32> {
        // La risposta dipende dalla quota di partenza (scalino massimo, salto massimo):
        // la si arrotonda a 0.5 m e si interroga `probe` con quel valore, cosi' una
        // risposta in cache vale per tutte le quote che finiscono nello stesso intervallo.
        probe_q(&mut self.cache, self.cell, ix, iz, hint, probe)
    }

    fn reconstruct(&self, key: Key) -> Vec<[f32; 3]> {
        let mut k = key;
        let mut rev = vec![self.pos[&k]];
        while let Some(&p) = self.parent.get(&k) {
            rev.push(self.pos[&p]);
            k = p;
        }
        rev.reverse();
        if rev.last().map_or(true, |l| *l != self.goal) {
            rev.push(self.goal);
        }
        rev
    }

    /// Espande al massimo `budget` nodi.
    pub fn step(&mut self, budget: usize, probe: &mut dyn FnMut(f32, f32, f32) -> Option<f32>) -> Status {
        if let Some(d) = &self.done {
            return d.clone();
        }
        for _ in 0..budget {
            let Some(cur) = self.open.pop() else { return self.finish(Status::Failed) };
            if !self.closed.insert(cur.key) {
                continue; // voce vecchia: il nodo e' gia' stato espanso con un costo migliore
            }
            self.expanded += 1;
            let pos = self.pos[&cur.key];
            let g_cur = self.g[&cur.key];
            let dh = (pos[0] - self.goal[0]).hypot(pos[2] - self.goal[2]);
            if self.closest.map_or(true, |(d, _)| dh < d) {
                self.closest = Some((dh, pos));
            }
            if self.reached(pos) {
                let path = self.reconstruct(cur.key);
                return self.finish(Status::Found(path));
            }
            if self.expanded >= self.max_expanded {
                self.hit_limit = true;
                return self.finish(Status::Failed);
            }

            let (ix, iz) = (cur.key.0, cur.key.1);
            for (dx, dz) in DIRS {
                let (nx, nz) = (ix + dx, iz + dz);
                let Some(fy) = self.probe_cached(nx, nz, pos[1], probe) else { continue };
                if dx != 0 && dz != 0 {
                    // niente tagli d'angolo: servono liberi anche i due lati
                    if self.probe_cached(ix + dx, iz, pos[1], probe).is_none()
                        || self.probe_cached(ix, iz + dz, pos[1], probe).is_none()
                    {
                        continue;
                    }
                }
                let nkey: Key = (nx, nz, level(fy));
                if self.closed.contains(&nkey) {
                    continue;
                }
                let base = if dx != 0 && dz != 0 { std::f32::consts::SQRT_2 } else { 1.0 };
                let cost = base * self.cell + 0.5 * (fy - pos[1]).abs();
                let tentative = g_cur + cost;
                let better = self.g.get(&nkey).map_or(true, |&old| tentative < old - 1e-6);
                if better {
                    let np = [nx as f32 * self.cell, fy, nz as f32 * self.cell];
                    let h = (np[0] - self.goal[0]).hypot(np[2] - self.goal[2]);
                    self.g.insert(nkey, tentative);
                    self.pos.insert(nkey, np);
                    self.parent.insert(nkey, cur.key);
                    self.open.push(Open { f: tentative + h, key: nkey });
                }
            }
        }
        Status::Searching
    }
}

/// Ridisegna una polilinea a punti equidistanti (`spacing` m), escluso il primo punto
/// (dove sta il giocatore) e incluso l'ultimo se rimane un tratto apprezzabile.
pub fn resample(path: &[[f32; 3]], spacing: f32) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    if path.len() < 2 || spacing <= 0.0 {
        return out;
    }
    let mut carry = 0.0f32; // distanza percorsa dall'ultimo punto emesso
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        let seg = dist3(a, b);
        if seg <= 1e-6 {
            continue;
        }
        let mut t = spacing - carry; // distanza, lungo questo segmento, del prossimo punto
        while t <= seg + 1e-4 {
            let f = (t / seg).min(1.0);
            out.push([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f]);
            t += spacing;
        }
        carry = seg - (t - spacing);
    }
    if carry > spacing * 0.4 {
        if let Some(last) = path.last() {
            out.push(*last);
        }
    }
    out
}

// =============================================================================
// Test
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    fn run(start: [f32; 3], goal: [f32; 3], budget: usize, max: usize, probe: &mut dyn FnMut(f32, f32, f32) -> Option<f32>) -> Status {
        let mut s = Search::new(start, goal, 1.0, 1.0, max);
        loop {
            match s.step(budget, probe) {
                Status::Searching => continue,
                done => return done,
            }
        }
    }

    #[test]
    fn straight_line_in_open_field() {
        let mut open = |_x: f32, _z: f32, _y: f32| Some(0.0);
        let Status::Found(p) = run([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], 1000, 50_000, &mut open) else {
            panic!("doveva trovare il percorso")
        };
        assert_eq!(p.first().copied(), Some([0.0, 0.0, 0.0]));
        assert_eq!(p.last().copied(), Some([10.0, 0.0, 0.0]));
        let len = path_length(&p);
        assert!((10.0..=11.5).contains(&len), "lunghezza {len}");
    }

    #[test]
    fn walks_around_a_wall() {
        // muro pieno su x = 5 per |z| <= 5
        let mut probe = |x: f32, z: f32, _y: f32| {
            if x.round() == 5.0 && z.abs() <= 5.0 { None } else { Some(0.0) }
        };
        let Status::Found(p) = run([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], 1000, 100_000, &mut probe) else {
            panic!("doveva trovare il giro")
        };
        assert!(path_length(&p) > 14.0, "deve aggirare il muro, non attraversarlo");
        for q in &p[1..p.len() - 1] {
            assert!(!(q[0].round() == 5.0 && q[2].abs() <= 5.0), "il percorso attraversa il muro in {q:?}");
        }
    }

    #[test]
    fn unreachable_goal_fails() {
        // l'obiettivo e' chiuso in un anello di muri a distanza 2
        let mut probe = |x: f32, z: f32, _y: f32| {
            let d = (x - 10.0).abs().max(z.abs());
            if d.round() == 2.0 { None } else { Some(0.0) }
        };
        let r = run([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], 500, 3_000, &mut probe);
        assert_eq!(r, Status::Failed);
    }

    #[test]
    fn failed_search_reports_closest_point_and_limit() {
        // il mondo calpestabile finisce a x = 4: l'obiettivo (x = 10) e' irraggiungibile
        let mut probe = |x: f32, _z: f32, _y: f32| if x.round() >= 5.0 { None } else { Some(0.0) };
        let mut s = Search::new([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], 1.0, 1.0, 5_000);
        let r = loop {
            match s.step(500, &mut probe) {
                Status::Searching => continue,
                done => break done,
            }
        };
        assert_eq!(r, Status::Failed);
        assert!(s.hit_limit(), "area illimitata: deve fermarsi sul tetto");
        let (p, d) = s.closest().expect("almeno la partenza");
        assert!((p[0] - 4.0).abs() < 1e-3, "{p:?}");
        assert!((d - 6.0).abs() < 1e-3, "{d}");
    }

    #[test]
    fn flood_reaches_only_the_connected_region() {
        // due zone separate da un muro pieno su x = 5
        let mut probe = |x: f32, z: f32, _y: f32| {
            if x.round() == 5.0 || x.abs() > 9.0 || z.abs() > 4.0 { None } else { Some(0.0) }
        };
        let r = flood([0.0, 0.0, 0.0], 1.0, 100_000, &mut probe);
        assert!(r.contains_near([3.0, 0.0, 0.0], 1.0, 1.0));
        assert!(!r.contains_near([8.0, 0.0, 0.0], 1.0, 1.0), "stanza isolata");
        assert!(!r.truncated());
        assert!(r.len() > 20);
    }

    #[test]
    fn flood_keeps_levels_apart() {
        // stesso (x, z) ma 12 m piu' in alto: non e' raggiungibile da sotto
        let mut probe = |x: f32, z: f32, _y: f32| if x.abs() <= 3.0 && z.abs() <= 3.0 { Some(0.0) } else { None };
        let r = flood([0.0, 0.0, 0.0], 1.0, 100_000, &mut probe);
        assert!(r.contains_near([0.0, 0.0, 0.0], 1.0, 2.0));
        assert!(!r.contains_near([0.0, 12.0, 0.0], 1.0, 2.0));
    }

    #[test]
    fn flood_stops_at_the_cap() {
        let mut probe = |_x: f32, _z: f32, _y: f32| Some(0.0);
        let r = flood([0.0, 0.0, 0.0], 1.0, 200, &mut probe);
        assert!(r.truncated());
    }

    #[test]
    fn stepping_in_small_budgets_gives_the_same_path() {
        let mk = || {
            |x: f32, z: f32, _y: f32| if x.round() == 3.0 && z.abs() <= 2.0 { None } else { Some(0.0) }
        };
        let (mut a, mut b) = (mk(), mk());
        let big = run([0.0, 0.0, 0.0], [8.0, 0.0, 1.0], 100_000, 100_000, &mut a);
        let small = run([0.0, 0.0, 0.0], [8.0, 0.0, 1.0], 3, 100_000, &mut b);
        assert_eq!(big, small);
    }

    #[test]
    fn two_levels_do_not_mix() {
        // sotto (y=0) c'e' un corridoio libero, sopra (y=4) il pavimento e' pieno di buchi:
        // chi parte sopra non deve "cadere" nel livello sotto.
        let mut probe = |x: f32, _z: f32, y: f32| {
            if y > 2.0 {
                if x.round() >= 3.0 { None } else { Some(4.0) }
            } else {
                Some(0.0)
            }
        };
        let r = run([0.0, 4.0, 0.0], [10.0, 4.0, 0.0], 1000, 5_000, &mut probe);
        assert_eq!(r, Status::Failed);
    }

    #[test]
    fn resample_is_evenly_spaced() {
        let line = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]];
        let pts = resample(&line, 2.0);
        assert_eq!(pts.len(), 5);
        assert!((pts[0][0] - 2.0).abs() < 1e-3);
        assert!((pts[4][0] - 10.0).abs() < 1e-3);
    }

    #[test]
    fn resample_carries_over_corners() {
        // L: 3 m in x, poi 3 m in z; spaziatura 2 -> punti a 2, 4 (lungo la L), e il finale
        let l = [[0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [3.0, 0.0, 3.0]];
        let pts = resample(&l, 2.0);
        assert!((pts[0][0] - 2.0).abs() < 1e-3 && pts[0][2].abs() < 1e-3);
        assert!((pts[1][0] - 3.0).abs() < 1e-3 && (pts[1][2] - 1.0).abs() < 1e-3);
        let last = pts.last().copied().unwrap();
        assert!((last[0] - 3.0).abs() < 1e-3 && (last[2] - 3.0).abs() < 1e-3);
    }

    #[test]
    fn degenerate_inputs_are_empty() {
        assert!(resample(&[], 2.0).is_empty());
        assert!(resample(&[[0.0; 3]], 2.0).is_empty());
        assert!(resample(&[[0.0; 3], [1.0, 0.0, 0.0]], 0.0).is_empty());
    }
}
