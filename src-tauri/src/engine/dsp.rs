//! Small DSP building blocks shared by the synth and effects.

use std::f64::consts::PI;

#[inline]
pub fn midi_to_hz(note: f64) -> f64 {
    440.0 * ((note - 69.0) / 12.0).exp2()
}

#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Coefficient for an exponential approach that covers ~99% of the distance in `secs`.
#[inline]
pub fn exp_coef(secs: f64, sr: f64) -> f32 {
    if secs <= 0.0 {
        0.0
    } else {
        (-4.6 / (secs * sr)).exp() as f32
    }
}

/// Cubic Hermite interpolation; out-of-range reads are zero.
#[inline]
pub fn hermite(buf: &[f32], pos: f64) -> f32 {
    let i = pos.floor() as isize;
    let f = (pos - i as f64) as f32;
    let get = |k: isize| -> f32 {
        if k < 0 || k as usize >= buf.len() {
            0.0
        } else {
            buf[k as usize]
        }
    };
    let xm1 = get(i - 1);
    let x0 = get(i);
    let x1 = get(i + 1);
    let x2 = get(i + 2);
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * f + c2) * f + c1) * f + x0
}

/// PolyBLEP residual for band-limited saw/pulse edges.
#[inline]
pub fn poly_blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// Topology-preserving state variable filter (Cytomic / Simper).
#[derive(Clone, Copy, Default)]
pub struct SvfCoefs {
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
}

impl SvfCoefs {
    pub fn new(cutoff: f64, resonance: f32, sr: f64) -> Self {
        let fc = cutoff.clamp(20.0, sr * 0.45);
        let g = (PI * fc / sr).tan() as f32;
        let k = 2.0 - 1.95 * resonance.clamp(0.0, 1.0);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        SvfCoefs { a1, a2, a3, k }
    }
}

#[derive(Clone, Copy, Default)]
pub struct SvfState {
    ic1: f32,
    ic2: f32,
}

impl SvfState {
    /// Returns (lowpass, bandpass, highpass).
    #[inline]
    pub fn tick(&mut self, c: &SvfCoefs, x: f32) -> (f32, f32, f32) {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        // Guard against denormals / blowups.
        if !self.ic1.is_finite() || !self.ic2.is_finite() {
            self.ic1 = 0.0;
            self.ic2 = 0.0;
        }
        (v2, v1, x - c.k * v1 - v2)
    }
}

/// RBJ cookbook biquad (transposed direct form II).
#[derive(Clone, Copy)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

#[derive(Clone, Copy, Default)]
pub struct BiquadState {
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn norm(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Biquad {
            b0: (b0 / a0) as f32,
            b1: (b1 / a0) as f32,
            b2: (b2 / a0) as f32,
            a1: (a1 / a0) as f32,
            a2: (a2 / a0) as f32,
        }
    }

    pub fn peaking(sr: f64, f: f64, q: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * PI * f.clamp(20.0, sr * 0.45) / sr;
        let alpha = w.sin() / (2.0 * q);
        let c = w.cos();
        Self::norm(1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a)
    }

    pub fn low_shelf(sr: f64, f: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * PI * f / sr;
        let c = w.cos();
        let alpha = w.sin() / 2.0 * 2f64.sqrt();
        let sa = 2.0 * a.sqrt() * alpha;
        Self::norm(
            a * ((a + 1.0) - (a - 1.0) * c + sa),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - sa),
            (a + 1.0) + (a - 1.0) * c + sa,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - sa,
        )
    }

    pub fn high_shelf(sr: f64, f: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * PI * f / sr;
        let c = w.cos();
        let alpha = w.sin() / 2.0 * 2f64.sqrt();
        let sa = 2.0 * a.sqrt() * alpha;
        Self::norm(
            a * ((a + 1.0) + (a - 1.0) * c + sa),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - sa),
            (a + 1.0) - (a - 1.0) * c + sa,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - sa,
        )
    }

    #[inline]
    pub fn tick(&self, s: &mut BiquadState, x: f32) -> f32 {
        let y = self.b0 * x + s.z1;
        s.z1 = self.b1 * x - self.a1 * y + s.z2;
        s.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Cheap xorshift noise in [-1, 1].
#[inline]
pub fn xorshift(state: &mut u32) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    (x as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// Game Boy style LFSR step. Returns the new output bit as ±1.
#[inline]
pub fn lfsr_step(lfsr: &mut u32, short: bool) -> f32 {
    let bit = (*lfsr ^ (*lfsr >> 1)) & 1;
    *lfsr = (*lfsr >> 1) | (bit << 14);
    if short {
        *lfsr = (*lfsr & !(1 << 6)) | (bit << 6);
    }
    if *lfsr & 1 == 0 {
        1.0
    } else {
        -1.0
    }
}

#[inline]
pub fn quantize(x: f32, bits: u8) -> f32 {
    if bits == 0 {
        return x;
    }
    let levels = (1u32 << (bits.min(16) - 1)) as f32;
    (x * levels).round() / levels
}
