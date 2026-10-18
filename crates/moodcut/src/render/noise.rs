//! Deterministic value noise and fractal (fBm) noise for procedural effects:
//! fractal noise, turbulent displace and the film-burn transition. Seeded and
//! frame-exact, so renders are reproducible.

#[inline]
fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6b343) ^ (y as u32).wrapping_mul(0xd8163841) ^ (z as u32).wrapping_mul(0xcb1ab31f) ^ seed.wrapping_mul(0x9e3779b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a2d39);
    h ^ (h >> 15)
}

#[inline]
fn lat(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    (hash3(x, y, z, seed) & 0xffff) as f32 / 32767.5 - 1.0
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Smooth 3D value noise in -1..1 (z is time / evolution).
pub fn noise3(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let (x0, y0, z0) = (x.floor(), y.floor(), z.floor());
    let (tx, ty, tz) = (fade(x - x0), fade(y - y0), fade(z - z0));
    let (xi, yi, zi) = (x0 as i32, y0 as i32, z0 as i32);
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let c = |dz: i32| {
        let a = l(lat(xi, yi, zi + dz, seed), lat(xi + 1, yi, zi + dz, seed), tx);
        let b = l(lat(xi, yi + 1, zi + dz, seed), lat(xi + 1, yi + 1, zi + dz, seed), tx);
        l(a, b, ty)
    };
    l(c(0), c(1), tz)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Smooth,
    Turbulent,
    Ridged,
}

/// Fractal noise: `octaves` layers of noise3 at doubling frequency, roughly 0..1.
pub fn fbm(x: f32, y: f32, z: f32, octaves: f32, kind: Kind, seed: u32) -> f32 {
    let n = octaves.clamp(1.0, 10.0);
    let whole = n.floor() as u32;
    let mut sum = 0.0;
    let mut norm = 0.0;
    let mut amp = 1.0;
    let mut f = 1.0;
    for o in 0..whole + 1 {
        // the fractional last octave fades in, so complexity animates smoothly
        let w = if o < whole { 1.0 } else { n - whole as f32 };
        if w <= 0.0 {
            break;
        }
        let v = noise3(x * f, y * f, z + o as f32 * 7.31, seed.wrapping_add(o * 1013));
        let v = match kind {
            Kind::Smooth => v * 0.5 + 0.5,
            Kind::Turbulent => v.abs(),
            Kind::Ridged => 1.0 - v.abs(),
        };
        sum += v * amp * w;
        norm += amp * w;
        amp *= 0.5;
        f *= 2.0;
    }
    if norm > 0.0 {
        sum / norm
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_bounded_and_deterministic() {
        let mut lo: f32 = 1.0;
        let mut hi: f32 = -1.0;
        for i in 0..2000 {
            let v = noise3(i as f32 * 0.37, i as f32 * 0.11, 0.5, 7);
            lo = lo.min(v);
            hi = hi.max(v);
            assert_eq!(v, noise3(i as f32 * 0.37, i as f32 * 0.11, 0.5, 7));
        }
        assert!(lo >= -1.0 && hi <= 1.0 && hi - lo > 1.0, "{lo} {hi}");
        let f = fbm(3.3, 1.2, 0.0, 5.5, Kind::Smooth, 1);
        assert!((0.0..=1.0).contains(&f));
    }
}
