//! Windowed-sinc resampler: samples are consumed at the sink's fixed rate
//! (48 kHz on the H700) whatever rate the core reports.

#[cfg(any(target_os = "linux", test))]
const TAPS: usize = 48;
#[cfg(any(target_os = "linux", test))]
const HALF: usize = TAPS / 2;
#[cfg(any(target_os = "linux", test))]
const PHASES: usize = 256;
/// Passband edge as a fraction of the destination Nyquist.
#[cfg(any(target_os = "linux", test))]
const CUTOFF: f64 = 22_000.0 / 24_000.0;
#[cfg(any(target_os = "linux", test))]
const BETA: f64 = 7.0;

#[cfg(any(target_os = "linux", test))]
pub struct Resampler {
    step: f64,
    scaled: f64,
    pos: f64,
    kernel: Vec<f32>,
    history: Vec<f32>,
}

#[cfg(any(target_os = "linux", test))]
impl Resampler {
    pub fn new(src_hz: f64, dst_hz: f64) -> Self {
        let usable = |hz: f64| hz.is_finite() && hz > 0.0;
        let step = if usable(src_hz) && usable(dst_hz) {
            src_hz / dst_hz
        } else {
            1.0
        };
        // Downsampling needs the stopband below the destination Nyquist;
        // upsampling only needs to suppress images, so keep the top of the
        // source band.
        let cutoff = if step > 1.0 { CUTOFF / step } else { 0.98 };
        Self {
            step,
            scaled: step,
            pos: HALF as f64,
            kernel: build_kernel(cutoff),
            history: vec![0.0; TAPS * 2],
        }
    }

    /// Gentle rate control against the sink's fill level: a `ratio` above
    /// one consumes the source faster, below one slower.
    pub fn set_ratio(&mut self, ratio: f64) {
        if ratio > 0.0 {
            self.scaled = self.step / ratio;
        }
    }

    /// Consume a stereo batch from the core and emit the device-rate batch.
    pub fn process(&mut self, src: &[i16], out: &mut Vec<i16>) {
        out.clear();
        let frames = src.len() / 2;
        if frames == 0 {
            return;
        }
        self.history
            .extend(src[..frames * 2].iter().map(|&s| f32::from(s)));
        let frames = self.history.len() / 2;
        while self.pos + (HALF as f64) < frames as f64 {
            let i = self.pos as usize;
            let at = (self.pos - i as f64) * PHASES as f64;
            let phase = at as usize;
            let blend = (at - phase as f64) as f32;
            let a = &self.kernel[phase * TAPS..(phase + 1) * TAPS];
            let b = &self.kernel[(phase + 1) * TAPS..(phase + 2) * TAPS];
            let window = &self.history[(i + 1 - HALF) * 2..(i + 1 + HALF) * 2];
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for t in 0..TAPS {
                let w = a[t] + (b[t] - a[t]) * blend;
                l += window[t * 2] * w;
                r += window[t * 2 + 1] * w;
            }
            out.push(l.round().clamp(-32768.0, 32767.0) as i16);
            out.push(r.round().clamp(-32768.0, 32767.0) as i16);
            self.pos += self.scaled;
        }
        let drop = (self.pos as usize + 1).saturating_sub(HALF);
        if drop > 0 {
            self.history.drain(..drop * 2);
            self.pos -= drop as f64;
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn build_kernel(cutoff: f64) -> Vec<f32> {
    let mut kernel = vec![0.0f32; (PHASES + 1) * TAPS];
    let i0_beta = bessel_i0(BETA);
    let mut taps = [0.0f64; TAPS];
    for phase in 0..=PHASES {
        let frac = phase as f64 / PHASES as f64;
        let mut sum = 0.0;
        for (t, tap) in taps.iter_mut().enumerate() {
            let x = t as f64 + 1.0 - HALF as f64 - frac;
            let ratio = x / HALF as f64;
            let window = if ratio.abs() >= 1.0 {
                0.0
            } else {
                bessel_i0(BETA * (1.0 - ratio * ratio).sqrt()) / i0_beta
            };
            *tap = cutoff * sinc(cutoff * x) * window;
            sum += *tap;
        }
        let row = &mut kernel[phase * TAPS..(phase + 1) * TAPS];
        for (dst, tap) in row.iter_mut().zip(taps.iter()) {
            *dst = (tap / sum) as f32;
        }
    }
    kernel
}

#[cfg(any(target_os = "linux", test))]
fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

#[cfg(any(target_os = "linux", test))]
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..32 {
        term *= (x / (2.0 * k as f64)).powi(2);
        sum += term;
    }
    sum
}

/// Fade out the last `ms` milliseconds of an interleaved stereo buffer, so a
/// one-shot track that ends on a loud sample cannot click.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn fade_out(samples: &mut [i16], rate: u32, ms: u32) {
    let frames = samples.len() / 2;
    let fade = ((rate as u64 * ms as u64) / 1000) as usize;
    if fade == 0 || frames == 0 {
        return;
    }
    let fade = fade.min(frames);
    for i in 0..fade {
        let gain = if fade > 1 {
            (fade - 1 - i) as f32 / (fade - 1) as f32
        } else {
            0.0
        };
        let idx = (frames - fade + i) * 2;
        samples[idx] = (samples[idx] as f32 * gain) as i16;
        samples[idx + 1] = (samples[idx + 1] as f32 * gain) as i16;
    }
}

#[cfg(test)]
mod tests {
    use super::{fade_out, Resampler};
    use std::f64::consts::PI;

    fn stereo(frames: usize) -> Vec<i16> {
        (0..frames)
            .flat_map(|i| {
                let v = (i % 100) as i16;
                [v, -v]
            })
            .collect()
    }

    fn tone(frames: usize, rate: f64, hz: f64, amp: f64) -> Vec<i16> {
        (0..frames)
            .flat_map(|i| {
                let v = (amp * (2.0 * PI * hz * i as f64 / rate).sin()).round() as i16;
                [v, v]
            })
            .collect()
    }

    fn tone_amp(samples: &[i16], rate: f64, hz: f64) -> f64 {
        let n = samples.len() / 2;
        if n < 4 {
            return 0.0;
        }
        let start = n / 4;
        let end = n * 3 / 4;
        let len = (end - start) as f64;
        let (mut re, mut im, mut wsum) = (0.0, 0.0, 0.0);
        for i in start..end {
            let w = 0.5 - 0.5 * (2.0 * PI * (i - start) as f64 / (len - 1.0)).cos();
            let t = i as f64 / rate;
            let s = samples[i * 2] as f64;
            re += s * w * (2.0 * PI * hz * t).cos();
            im -= s * w * (2.0 * PI * hz * t).sin();
            wsum += w;
        }
        2.0 * (re * re + im * im).sqrt() / wsum
    }

    #[test]
    fn ratio_scales_output_count() {
        let mut fast = Resampler::new(48000.0, 48000.0);
        fast.set_ratio(2.0);
        let mut out = Vec::new();
        fast.process(&stereo(64), &mut out);
        assert_eq!(out.len() / 2, 128);

        let mut slow = Resampler::new(48000.0, 48000.0);
        slow.set_ratio(0.5);
        let mut out = Vec::new();
        slow.process(&stereo(64), &mut out);
        assert_eq!(out.len() / 2, 32);
    }

    #[test]
    fn dc_gain_is_unity() {
        let mut r = Resampler::new(44100.0, 48000.0);
        let mut out = Vec::new();
        r.process(&vec![1000i16; 4800 * 2], &mut out);
        let mid = &out[out.len() / 4..out.len() * 3 / 4];
        for &v in mid {
            assert!((i32::from(v) - 1000).abs() <= 2, "{v}");
        }
    }

    #[test]
    fn aliases_are_filtered() {
        let rate = 65536.0;
        let src = tone(rate as usize / 4, rate, 20_000.0, 16_383.0);

        let mut r = Resampler::new(rate, 32768.0);
        let mut out = Vec::new();
        r.process(&src, &mut out);
        let alias = tone_amp(&out, 32768.0, 12_768.0);
        assert!(alias < 16_383.0 * 0.02, "alias {alias}");

        let mut r = Resampler::new(rate, 48000.0);
        let mut out = Vec::new();
        r.process(&src, &mut out);
        let pass = tone_amp(&out, 48000.0, 20_000.0);
        assert!(pass > 16_383.0 * 0.8, "passband {pass}");
    }

    #[test]
    fn chunking_matches_one_shot() {
        let src = tone(3000, 44100.0, 1000.0, 10_000.0);
        let mut one = Vec::new();
        Resampler::new(44100.0, 48000.0).process(&src, &mut one);

        let mut chunks = Vec::new();
        let mut r = Resampler::new(44100.0, 48000.0);
        let mut out = Vec::new();
        for chunk in src.chunks(1097 * 2) {
            r.process(chunk, &mut out);
            chunks.extend_from_slice(&out);
        }
        let n = one.len().min(chunks.len());
        assert!(n > 0);
        for i in 0..n {
            assert!(
                (i32::from(one[i]) - i32::from(chunks[i])).abs() <= 1,
                "sample {i}"
            );
        }
        assert!(chunks.len() + 100 >= one.len(), "chunked output truncated");
    }

    #[test]
    fn fade_out_ends_at_silence() {
        let mut samples = vec![10_000i16; 200];
        fade_out(&mut samples, 1000, 10);
        assert_eq!(samples[0], 10_000, "outside the fade");
        assert_eq!(samples[180], 10_000, "first faded frame keeps full gain");
        assert!(samples[182] < 10_000 && samples[182] > 8_000);
        assert_eq!(samples[198], 0, "last frame");
        assert_eq!(samples[199], 0);
    }

    /// Feed ten seconds of a core's audio in native-rate frame batches at
    /// the NTSC and PAL tick rates; the device-rate output must measure the
    /// same ten seconds (the resampler carries its fractional phase).
    #[test]
    fn ten_seconds_stay_real_time_at_native_tick_rates() {
        const DST_HZ: f64 = 48000.0;
        for fps in [59.7275_f64, 60.0988] {
            let src_hz = 44100.0;
            let mut r = Resampler::new(src_hz, DST_HZ);
            let total = (src_hz * 10.0) as usize;
            let mut sent = 0usize;
            let mut acc = 0.0f64;
            let mut produced = 0usize;
            let mut out = Vec::new();
            while sent < total {
                acc += src_hz / fps;
                let mut n = acc.floor() as usize;
                acc -= n as f64;
                n = n.min(total - sent);
                sent += n;
                r.process(&stereo(n), &mut out);
                produced += out.len() / 2;
            }
            let secs = produced as f64 / DST_HZ;
            assert!(
                (secs - 10.0).abs() <= 0.005,
                "{fps} fps produced {secs} s of audio"
            );
        }
    }
}
