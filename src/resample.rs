//! Linear resampler: samples are consumed at the sink's fixed rate (32768 Hz
//! on the H700) whatever rate the core reports.

#[cfg(any(target_os = "linux", test))]
pub struct Resampler {
    step: f64,
    scaled: f64,
    pos: f64,
    prev: [i16; 2],
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
        Self {
            step,
            scaled: step,
            pos: 0.0,
            prev: [0, 0],
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
        while self.pos < frames as f64 {
            let i = self.pos as usize;
            let frac = self.pos - i as f64;
            for c in 0..2 {
                let a = tap(&self.prev, src, i, c);
                let b = tap(&self.prev, src, i + 1, c);
                out.push((a + (b - a) * frac).round() as i16);
            }
            self.pos += self.scaled;
        }
        self.pos -= frames as f64;
        self.prev = [src[(frames - 1) * 2], src[(frames - 1) * 2 + 1]];
    }
}

#[cfg(any(target_os = "linux", test))]
fn tap(prev: &[i16; 2], src: &[i16], frame: usize, channel: usize) -> f64 {
    if frame == 0 {
        prev[channel] as f64
    } else {
        src[(frame - 1) * 2 + channel] as f64
    }
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

    fn stereo(frames: usize) -> Vec<i16> {
        (0..frames)
            .flat_map(|i| {
                let v = (i % 100) as i16;
                [v, -v]
            })
            .collect()
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

    /// Feed ten seconds of a core's audio in native-rate frame batches at
    /// the NTSC and PAL tick rates; the device-rate output must measure the
    /// same ten seconds (the resampler carries its fractional phase).
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

    #[test]
    fn ten_seconds_stay_real_time_at_native_tick_rates() {
        const DST_HZ: f64 = 32768.0;
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
