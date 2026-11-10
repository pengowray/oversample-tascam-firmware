//! The pitch shifter's two transforms: a Hann-windowed 1024-point real FFT
//! and the inverse of a 256-point real FFT.
//!
//! Both use a 2^n-point complex FFT done two radix-2 stages at a time
//! (radix 2², He and Torkelson, 1996): each pass is a radix-4 butterfly, with
//! 3 twiddle multiplies for every 4 values instead of the 4 that two radix-2
//! stages take, and half as many passes over the data. Its input or output is
//! in bit-reversed order. The forward transform writes its input in that order
//! as it reads the frame, and the inverse leaves its output in that order for
//! the overlap-add to read, so neither needs a pass to reorder. The twiddles
//! come from [`PHASORS`], which the shifter also uses for its rotations. On a PC
//! without SIMD the forward transform took 2.8 ms per second of audio, against
//! 3.6 ms for microfft's `rfft_1024` and its windowing.

use microfft::Complex32;

use super::cos_sin_turns;

/// Entries per turn in [`PHASORS`].
pub(super) const PHASOR_STEPS: usize = 1024;

/// e^(2πi n / PHASOR_STEPS) for n in 0..=PHASOR_STEPS: the shifter's rotations
/// and both transforms' twiddles.
pub(super) static PHASORS: [Complex32; PHASOR_STEPS + 1] = {
    let mut t = [Complex32::new(0.0, 0.0); PHASOR_STEPS + 1];
    let mut n = 0;
    while n <= PHASOR_STEPS {
        let (c, s) = cos_sin_turns(n as f32 / PHASOR_STEPS as f32);
        t[n] = Complex32::new(c, s);
        n += 1;
    }
    t
};

/// e^(-2πi n / PHASOR_STEPS).
#[inline(always)]
fn twiddle(n: usize) -> Complex32 {
    let p = PHASORS[n];
    Complex32::new(p.re, -p.im)
}

/// `n` with its lowest `bits` bits reversed.
#[inline(always)]
fn bit_reverse(n: usize, bits: u32) -> usize {
    ((n as u32).reverse_bits() >> (32 - bits)) as usize
}

/// `i × z`.
#[inline(always)]
fn times_i(z: Complex32) -> Complex32 {
    Complex32::new(-z.im, z.re)
}

/// Samples in a forward frame.
const N: usize = 1024;
/// Points of the forward transform's complex FFT.
const M: usize = N / 2;

/// The spectrum of the frame `input[start..]` then `input[..start]` (a
/// circular buffer; `start` even), under a periodic Hann window, in
/// microfft's packed layout: bins 0 to 511, with the Nyquist bin in the
/// imaginary part of bin 0. The result is 8 times the transform of the
/// windowed frame, which saves 3 multiplies a bin; [`super::SYNTHESIS`] takes
/// the 8 back.
///
/// The window is applied in the spectrum: a periodic Hann window multiplies
/// each bin by 1/2 and subtracts 1/4 of each neighbour.
pub(super) fn windowed_rfft_1024(input: &[f32; N], start: usize, out: &mut [Complex32; M]) {
    debug_assert!(start.is_multiple_of(2));
    // Even samples as real parts, odd as imaginary: z[n] = x[2n] + i x[2n+1].
    // The first radix-2 stage pairs z[r] with z[r + M/2], and its outputs go to
    // bit-reversed positions 2m and 2m + 1, where r is m bit-reversed.
    for m in 0..M / 2 {
        let r = bit_reverse(m, 8);
        let i = (start + 2 * r) % N;
        let j = (i + N / 2) % N;
        let a = Complex32::new(input[i], input[i + 1]);
        let b = Complex32::new(input[j], input[j + 1]);
        out[2 * m] = a + b;
        out[2 * m + 1] = a - b;
    }
    // Two stages a pass, on blocks of 8, 32, 128 and 512.
    dit_pass::<2, 8, M>(out);
    dit_pass::<8, 32, M>(out);
    dit_pass::<32, 128, M>(out);
    dit_pass::<128, 512, M>(out);

    // The real spectrum from z's: X[k] = E[k] + W^k O[k], where E and O are
    // the spectra of the even and odd samples and W = e^(-2πi/N). Bins k and
    // M - k come from z's at k and M - k. These are twice E, O and X.
    let both = |out: &[Complex32; M], k: usize| {
        let (a, b) = (out[k], out[M - k]);
        let e = Complex32::new(a.re + b.re, a.im - b.im);
        let o = Complex32::new(a.im + b.im, b.re - a.re);
        let wo = o * twiddle(k);
        (e + wo, Complex32::new(e.re - wo.re, wo.im - e.im))
    };
    let z0 = out[0];
    let (dc, nyquist) = (2.0 * (z0.re + z0.im), 2.0 * (z0.re - z0.im));
    let mid = Complex32::new(2.0 * out[M / 2].re, -2.0 * out[M / 2].im);

    // In the same pass, the Hann window: 4 × (X[k] - (X[k - 1] + X[k + 1]) / 2)
    // from twice X, which is 8 times the windowed bin. Bin k - 1 and bin
    // M + 1 - k are done once bins k and M - k are known, in places already
    // read. X[-1] is X[1]'s conjugate and X[M] is Nyquist.
    let (first, last) = both(out, 1);
    out[0] = Complex32::new(2.0 * (dc - first.re), 2.0 * (nyquist - last.re));
    let (mut lo2, mut lo1) = (Complex32::new(dc, 0.0), first);
    let (mut hi2, mut hi1) = (Complex32::new(nyquist, 0.0), last);
    for k in 2..M / 2 {
        let (lo, hi) = both(out, k);
        out[k - 1] = lo1 * 2.0 - (lo2 + lo);
        out[M + 1 - k] = hi1 * 2.0 - (hi2 + hi);
        (lo2, lo1, hi2, hi1) = (lo1, lo, hi1, hi);
    }
    out[M / 2 - 1] = lo1 * 2.0 - (lo2 + mid);
    out[M / 2] = mid * 2.0 - (lo1 + hi1);
    out[M / 2 + 1] = hi1 * 2.0 - (hi2 + mid);
}

/// Two decimation-in-time radix-2 stages on blocks of `B` = 4 × `Q` values,
/// in place: the stage pairing values `Q` apart, then the one pairing values
/// `2Q` apart.
#[inline(always)]
fn dit_pass<const Q: usize, const B: usize, const L: usize>(x: &mut [Complex32; L]) {
    const { assert!(B == 4 * Q && L.is_multiple_of(B)) };
    let blocks = x.as_chunks_mut::<B>().0;
    let stride = PHASOR_STEPS / B;
    for j in 0..Q {
        // W^j, W^2j and W^3j, for W = e^(-2πi / B).
        let (w1, w2, w3) = (twiddle(j * stride), twiddle(2 * j * stride), twiddle(3 * j * stride));
        for x in blocks.iter_mut() {
            let x0 = x[j];
            let x1 = x[j + Q] * w2;
            let x2 = x[j + 2 * Q] * w1;
            let x3 = x[j + 3 * Q] * w3;
            let (s01, d01) = (x0 + x1, x0 - x1);
            let (s23, d23) = (x2 + x3, x2 - x3);
            x[j] = s01 + s23;
            x[j + Q] = d01 - times_i(d23);
            x[j + 2 * Q] = s01 - s23;
            x[j + 3 * Q] = d01 + times_i(d23);
        }
    }
}

/// Two decimation-in-frequency radix-2 stages on blocks of `B` = 4 × `Q`
/// values, in place: the stage pairing values `2Q` apart, then the one pairing
/// values `Q` apart.
#[inline(always)]
fn dif_pass<const Q: usize, const B: usize, const L: usize>(x: &mut [Complex32; L]) {
    const { assert!(B == 4 * Q && L.is_multiple_of(B)) };
    let blocks = x.as_chunks_mut::<B>().0;
    let stride = PHASOR_STEPS / B;
    for j in 0..Q {
        let (w1, w2, w3) = (twiddle(j * stride), twiddle(2 * j * stride), twiddle(3 * j * stride));
        for x in blocks.iter_mut() {
            let (x0, x1, x2, x3) = (x[j], x[j + Q], x[j + 2 * Q], x[j + 3 * Q]);
            let (s02, d02) = (x0 + x2, x0 - x2);
            let (s13, d13) = (x1 + x3, x1 - x3);
            x[j] = s02 + s13;
            x[j + Q] = (s02 - s13) * w2;
            x[j + 2 * Q] = (d02 - times_i(d13)) * w1;
            x[j + 3 * Q] = (d02 + times_i(d13)) * w3;
        }
    }
}

/// Bins of the inverse transform's real spectrum, and points of its complex
/// FFT.
pub(super) const INV: usize = 128;

/// Inverse of a 256-point real FFT whose bins 0 to 127 are `spec` (bin 0 is
/// DC and the Nyquist bin 0, so it is ignored), each first multiplied by
/// `gain`. `spec` is overwritten.
///
/// Call `f(k, z)` for k in 0..128, in no particular order: frame sample `2k`
/// is `z.re / 256` and sample `2k + 1` is `-z.im / 256`.
///
/// The even and odd samples of the frame are packed as one complex signal
/// `z[m] = x[2m] + i x[2m+1]`, whose spectrum is rebuilt from the real
/// spectrum. The forward FFT of its conjugate gives the conjugate of the
/// inverse.
pub(super) fn inverse_rfft_256(spec: &mut [Complex32; INV], gain: &[f32; INV], mut f: impl FnMut(usize, Complex32)) {
    spec[0] = Complex32::new(0.0, 0.0);
    for k in 1..=INV / 2 {
        let a = spec[k] * gain[k];
        let b = if k == INV / 2 { a } else { spec[INV - k] * gain[INV - k] };
        // Twice the spectra of the even and odd samples: e = a + conj(b),
        // o = (a - conj(b)) × e^(2πik / 256).
        let e = Complex32::new(a.re + b.re, a.im - b.im);
        let o = Complex32::new(a.re - b.re, a.im + b.im) * PHASORS[k * (PHASOR_STEPS / (2 * INV))];
        // Conjugates of z's spectrum, e + i o, at k and INV - k.
        spec[k] = Complex32::new(e.re - o.im, -e.im - o.re);
        spec[INV - k] = Complex32::new(e.re + o.im, e.im - o.re);
    }
    // Blocks of 128, 32 and 8, then the last stage as the output is read: its
    // outputs at positions 2m and 2m + 1 are bins m and m + 64 bit-reversed.
    dif_pass::<32, 128, INV>(spec);
    dif_pass::<8, 32, INV>(spec);
    dif_pass::<2, 8, INV>(spec);
    for m in 0..INV / 2 {
        let (u, v) = (spec[2 * m], spec[2 * m + 1]);
        let k = bit_reverse(m, 6);
        f(k, u + v);
        f(k + INV / 2, u - v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::noise;

    #[test]
    fn forward_matches_microfft_with_a_hann_window() {
        let x = noise(1.0, N, 3);
        for start in [0, 256, 768] {
            let mut ring = [0.0f32; N];
            for (n, &v) in x.iter().enumerate() {
                ring[(start + n) % N] = v;
            }
            let mut ours = [Complex32::new(0.0, 0.0); M];
            windowed_rfft_1024(&ring, start, &mut ours);
            let mut buf: [f32; N] = core::array::from_fn(|n| {
                let s = libm::sinf(core::f32::consts::PI * n as f32 / N as f32);
                x[n] * s * s
            });
            let theirs = microfft::real::rfft_1024(&mut buf);
            let scale = theirs.iter().map(|c| c.norm_sqr()).fold(0.0f32, f32::max).sqrt();
            for (k, (a, b)) in ours.iter().zip(theirs.iter()).enumerate() {
                let d = (*a * 0.125 - *b).norm_sqr().sqrt() / scale;
                assert!(d < 2e-6, "start {start}, bin {k}: {a} / 8 vs {b}");
            }
        }
    }

    #[test]
    fn inverse_undoes_the_forward_fft() {
        let x = noise(1.0, 2 * INV, 7);
        let mut buf: [f32; 2 * INV] = x.clone().try_into().unwrap();
        let mut spec = *microfft::real::rfft_256(&mut buf);
        // The inverse ignores DC and Nyquist.
        spec[0] = Complex32::new(0.0, 0.0);
        let mean = x.iter().sum::<f32>() / x.len() as f32;
        let alternating =
            x.iter().enumerate().map(|(n, v)| if n % 2 == 0 { *v } else { -*v }).sum::<f32>() / x.len() as f32;
        let mut seen = [false; INV];
        inverse_rfft_256(&mut spec, &[1.0; INV], |k, z| {
            seen[k] = true;
            let even = z.re / (2 * INV) as f32;
            let odd = -z.im / (2 * INV) as f32;
            let (want_even, want_odd) = (x[2 * k] - mean - alternating, x[2 * k + 1] - mean + alternating);
            assert!((even - want_even).abs() < 1e-5, "sample {}: {even} vs {want_even}", 2 * k);
            assert!((odd - want_odd).abs() < 1e-5, "sample {}: {odd} vs {want_odd}", 2 * k + 1);
        });
        assert!(seen.iter().all(|&s| s));
    }
}
