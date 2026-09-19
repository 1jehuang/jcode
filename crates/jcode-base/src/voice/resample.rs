//! Streaming windowed-sinc low-pass resampling. Filter history crosses callback boundaries.
use super::VoiceError;
use std::collections::VecDeque;

pub(super) struct Resampler {
    history: VecDeque<f32>,
    base: i64,
    input: i64,
    next: f64,
    step: f64,
    radius: i64,
    cutoff: f64,
}
impl Resampler {
    pub fn new(rate: u32) -> Result<Self, VoiceError> {
        if !(8000..=192000).contains(&rate) {
            return Err(VoiceError::MicrophoneUnavailable);
        }
        let step = rate as f64 / 16000.0;
        let radius = (24.0 * step.max(1.0)).ceil() as i64;
        Ok(Self {
            history: VecDeque::new(),
            base: 0,
            input: 0,
            next: 0.0,
            step,
            radius,
            cutoff: 0.45 / step.max(1.0),
        })
    }
    pub fn push(&mut self, sample: f32, out: &mut Vec<i16>) {
        self.history.push_back(if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        });
        self.input += 1;
        while self.next + (self.radius as f64) < self.input as f64 {
            self.emit(out);
        }
        let keep = (self.next.floor() as i64 - self.radius).max(0);
        while self.base < keep && !self.history.is_empty() {
            self.history.pop_front();
            self.base += 1;
        }
    }
    fn emit(&mut self, out: &mut Vec<i16>) {
        let center = self.next.floor() as i64;
        let mut value = 0.0;
        let mut weight = 0.0;
        for index in center - self.radius..=center + self.radius {
            let x = index as f64 - self.next;
            if x.abs() > self.radius as f64 {
                continue;
            }
            let z = 2.0 * self.cutoff * x;
            let sinc = if z.abs() < 1e-12 {
                1.0
            } else {
                (std::f64::consts::PI * z).sin() / (std::f64::consts::PI * z)
            };
            let window = 0.42
                + 0.5 * (std::f64::consts::PI * x / self.radius as f64).cos()
                + 0.08 * (2.0 * std::f64::consts::PI * x / self.radius as f64).cos();
            let w = 2.0 * self.cutoff * sinc * window;
            weight += w;
            if index >= self.base && index < self.input {
                value += self.history[(index - self.base) as usize] as f64 * w;
            }
        }
        out.push(((value / weight).clamp(-1.0, 1.0) * 32767.0).round() as i16);
        self.next += self.step;
    }
    /// Zero-pad only the filter lookahead, retaining the original audio duration.
    pub fn finish(&mut self, out: &mut Vec<i16>) {
        while self.next < self.input as f64 {
            self.emit(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tone(rate: u32, hz: f64) -> Vec<i16> {
        let mut r = Resampler::new(rate).unwrap();
        let mut out = Vec::new();
        for i in 0..rate {
            r.push(
                (0.5 * (2.0 * std::f64::consts::PI * hz * i as f64 / rate as f64).sin()) as f32,
                &mut out,
            );
        }
        r.finish(&mut out);
        out
    }
    fn rms(x: &[i16]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }
    #[test]
    fn duration_and_antialias_at_native_rates() {
        for rate in [8000, 16000, 44100, 48000, 96000, 192000] {
            let pass = tone(rate, 1000.0);
            assert!((pass.len() as isize - 16000).abs() <= 1);
            assert!(rms(&pass[100..15900]) > 11000.0);
            if rate > 16000 {
                let stop = tone(rate, 12000.0);
                assert!(rms(&stop[100..15900]) < 30.0, "rate {rate}");
            }
        }
    }
    #[test]
    fn finite_bounded_and_chunk_independent() {
        let mut r = Resampler::new(48000).unwrap();
        let mut a = Vec::new();
        for _ in 0..1000 {
            r.push(f32::NAN, &mut a);
        }
        r.finish(&mut a);
        assert!(a.iter().all(|s| *s == 0));
        assert_eq!(a.len(), 334);
        assert!(r.history.len() < 600);
    }
}
