//! Rumore di Perlin 2D nell'intervallo [0, 1].
//!
//! ATTENZIONE: DFU usa `Mathf.PerlinNoise` di Unity, il cui codice non è pubblico.
//! Questa è l'implementazione classica ("improved noise" di Ken Perlin, tabella di
//! permutazione di riferimento, valore 0.5 sui punti interi) e ha lo stesso
//! carattere, ma non è garantito che coincida bit a bit con quella di Unity.
//! Nel campionatore delle altezze serve solo per il microrilievo (al massimo
//! 2,5 unità su 1539), quindi la differenza resta piccola; la funzione si può
//! sostituire passando un'altra `NoiseFn`.

const PERM: [u8; 256] = [
    151, 160, 137, 91, 90, 15, 131, 13, 201, 95, 96, 53, 194, 233, 7, 225, 140, 36, 103, 30, 69,
    142, 8, 99, 37, 240, 21, 10, 23, 190, 6, 148, 247, 120, 234, 75, 0, 26, 197, 62, 94, 252, 219,
    203, 117, 35, 11, 32, 57, 177, 33, 88, 237, 149, 56, 87, 174, 20, 125, 136, 171, 168, 68, 175,
    74, 165, 71, 134, 139, 48, 27, 166, 77, 146, 158, 231, 83, 111, 229, 122, 60, 211, 133, 230,
    220, 105, 92, 41, 55, 46, 245, 40, 244, 102, 143, 54, 65, 25, 63, 161, 1, 216, 80, 73, 209, 76,
    132, 187, 208, 89, 18, 169, 200, 196, 135, 130, 116, 188, 159, 86, 164, 100, 109, 198, 173,
    186, 3, 64, 52, 217, 226, 250, 124, 123, 5, 202, 38, 147, 118, 126, 255, 82, 85, 212, 207, 206,
    59, 227, 47, 16, 58, 17, 182, 189, 28, 42, 223, 183, 170, 213, 119, 248, 152, 2, 44, 154, 163,
    70, 221, 153, 101, 155, 167, 43, 172, 9, 129, 22, 39, 253, 19, 98, 108, 110, 79, 113, 224, 232,
    178, 185, 112, 104, 218, 246, 97, 228, 251, 34, 242, 193, 238, 210, 144, 12, 191, 179, 162,
    241, 81, 51, 145, 235, 249, 14, 239, 107, 49, 192, 214, 31, 181, 199, 106, 157, 184, 84, 204,
    176, 115, 121, 50, 45, 127, 4, 150, 254, 138, 236, 205, 93, 222, 114, 67, 29, 24, 72, 243, 141,
    128, 195, 78, 66, 215, 61, 156, 180,
];

fn p(i: usize) -> usize {
    PERM[i & 255] as usize
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}

fn grad(hash: usize, x: f32, y: f32) -> f32 {
    // Gradienti del rumore migliorato di Perlin con z = 0.
    let h = hash & 15;
    let u = if h < 8 { x } else { y };
    let v = if h < 4 {
        y
    } else if h == 12 || h == 14 {
        x
    } else {
        0.0
    };
    (if h & 1 == 0 { u } else { -u }) + (if h & 2 == 0 { v } else { -v })
}

/// Rumore 2D in [0, 1]; vale 0.5 sui punti a coordinate intere.
pub fn perlin_noise(x: f32, y: f32) -> f32 {
    let xf = x.floor();
    let yf = y.floor();
    let xi = (xf as i64 & 255) as usize;
    let yi = (yf as i64 & 255) as usize;
    let (x, y) = (x - xf, y - yf);
    let (u, v) = (fade(x), fade(y));

    let a = p(xi) + yi;
    let b = p(xi + 1) + yi;
    let n = lerp(
        v,
        lerp(u, grad(p(a), x, y), grad(p(b), x - 1.0, y)),
        lerp(u, grad(p(a + 1), x, y - 1.0), grad(p(b + 1), x - 1.0, y - 1.0)),
    );
    (n + 1.0) * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permutation_table_is_a_permutation() {
        let mut seen = [false; 256];
        for &v in PERM.iter() {
            assert!(!seen[v as usize], "valore duplicato {v}");
            seen[v as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn lattice_points_are_one_half() {
        for (x, y) in [(0.0, 0.0), (3.0, 7.0), (-2.0, 5.0), (255.0, 256.0)] {
            assert!((perlin_noise(x, y) - 0.5).abs() < 1e-6, "({x}, {y})");
        }
    }

    #[test]
    fn stays_in_unit_range_and_is_continuous() {
        let mut prev = perlin_noise(0.0, 0.0);
        let (mut lo, mut hi) = (1.0f32, 0.0f32);
        for i in 1..4000 {
            let t = i as f32 * 0.01;
            let n = perlin_noise(t, t * 0.7 + 3.3);
            assert!((0.0..=1.0).contains(&n), "{n} fuori da [0,1]");
            assert!((n - prev).abs() < 0.1, "salto a t={t}");
            lo = lo.min(n);
            hi = hi.max(n);
            prev = n;
        }
        assert!(hi - lo > 0.3, "il rumore deve variare");
    }

    #[test]
    fn repeats_every_256() {
        let a = perlin_noise(10.3, 20.7);
        let b = perlin_noise(10.3 + 256.0, 20.7 + 256.0);
        assert!((a - b).abs() < 1e-3);
    }
}
