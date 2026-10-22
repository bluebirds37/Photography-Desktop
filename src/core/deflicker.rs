//! Luminance-based visual deflicker.
//!
//! The visual-preview pass extracts a per-frame luminance curve, smooths it with
//! LOESS and writes the difference back as a correction. The correction is
//! applied on top of the current exposure, so the smoothing sees the already
//! graded sequence. We work in log2 space: luminance is roughly proportional to
//! exposure, so a one-stop exposure error is a factor of two.

use crate::core::loess::{mean_f64, smooth_frames};
use crate::core::model::{DeflickerAccuracy, SequenceSettings};

#[derive(Clone, Debug)]
pub struct DeflickerResult {
    pub ev: Vec<f32>,
    pub before: Vec<f32>,
    pub after: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct DeflickerOptions {
    pub smoothing: f32,
    pub accuracy: DeflickerAccuracy,
    pub passes: u32,
    pub constant: bool,
    pub multipass: bool,
}

impl From<&SequenceSettings> for DeflickerOptions {
    fn from(s: &SequenceSettings) -> Self {
        Self {
            smoothing: s.deflicker_smoothing,
            accuracy: s.deflicker_accuracy,
            passes: s.deflicker_passes,
            constant: s.deflicker_constant,
            multipass: s.deflicker_multipass,
        }
    }
}

/// `luma` is the analyzed relative luminance (0..1), `base_ev` the exposure
/// already applied to those previews (creative + HG). Returns per-frame EV
/// corrections that should be *added* to the exposure.
pub fn compute_deflicker(
    luma: &[f32],
    base_ev: &[f32],
    opts: &DeflickerOptions,
) -> DeflickerResult {
    let n = luma.len().min(base_ev.len());
    if n == 0 {
        return DeflickerResult {
            ev: vec![],
            before: vec![],
            after: vec![],
        };
    }

    let mut logs: Vec<f64> = (0..n)
        .map(|i| {
            (luma[i].max(1e-6) * base_ev[i].exp2())
                .clamp(1e-6, 1.0)
                .log2() as f64
        })
        .collect();
    let before = logs.clone();
    let mut correction = vec![0.0f64; n];

    let passes = if opts.multipass {
        opts.passes.max(1)
    } else {
        1
    };
    let span = opts.smoothing.clamp(0.02, 1.0) as f64;
    let eps = opts.accuracy.epsilon() as f64;
    // Accuracy is a relative luminance difference; translate to log2 space.
    let eps_log = (1.0 + eps).log2();

    for _ in 0..passes {
        let target: Vec<f64> = if opts.constant {
            let m = mean_f64(&logs);
            vec![m; n]
        } else {
            smooth_frames(&logs, span, 0)
        };

        let mut changed = false;
        for i in 0..n {
            let mut delta = target[i] - logs[i];
            if delta.abs() < eps_log {
                delta = 0.0;
            }
            delta = delta.clamp(-2.0, 2.0);
            if delta != 0.0 {
                changed = true;
            }
            correction[i] += delta;
            logs[i] += delta;
        }
        if !changed {
            break;
        }
    }

    let after: Vec<f32> = logs.iter().map(|v| v.exp2() as f32).collect();
    let before: Vec<f32> = before.iter().map(|v| v.exp2() as f32).collect();
    DeflickerResult {
        ev: correction.iter().map(|v| *v as f32).collect(),
        before,
        after,
    }
}

/// Rough flicker metric used in the UI: standard deviation of the high-frequency
/// residual after removing a slow LOESS trend.
pub fn flicker_score(values: &[f32]) -> f32 {
    if values.len() < 4 {
        return 0.0;
    }
    let logs: Vec<f64> = values.iter().map(|v| (v.max(1e-6) as f64).log2()).collect();
    let trend = smooth_frames(&logs, 0.35, 0);
    let residual: Vec<f64> = logs.iter().zip(trend).map(|(a, b)| a - b).collect();
    let m = mean_f64(&residual);
    let var = residual.iter().map(|r| (r - m) * (r - m)).sum::<f64>() / residual.len() as f64;
    (var.sqrt() * 100.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> DeflickerOptions {
        DeflickerOptions {
            smoothing: 0.4,
            accuracy: DeflickerAccuracy::Fine,
            passes: 2,
            constant: false,
            multipass: true,
        }
    }

    #[test]
    fn removes_high_frequency_flicker() {
        let n = 300;
        let luma: Vec<f32> = (0..n)
            .map(|i| {
                let slow = 0.55 + 0.15 * (i as f32 / n as f32);
                let flicker = 1.0 + 0.08 * ((i as f32) * 2.1).sin();
                slow * flicker
            })
            .collect();
        let base = vec![0.0; n];
        let before = flicker_score(&luma);
        let r = compute_deflicker(&luma, &base, &opts());
        let after = flicker_score(&r.after);
        assert!(after < before * 0.3, "before {before}, after {after}");
        // Slow ramp must be preserved.
        assert!(r.after[n - 1] > r.after[0] * 1.15);
    }

    #[test]
    fn constant_mode_flattens() {
        let n = 80;
        let luma: Vec<f32> = (0..n)
            .map(|i| 0.5 + 0.05 * ((i as f32) * 1.7).sin())
            .collect();
        let mut o = opts();
        o.constant = true;
        let r = compute_deflicker(&luma, &vec![0.0; n], &o);
        let min = r.after.iter().copied().fold(f32::INFINITY, f32::min);
        let max = r.after.iter().copied().fold(0.0f32, f32::max);
        assert!((max / min - 1.0).abs() < 0.01, "min {min} max {max}");
    }

    #[test]
    fn honors_existing_exposure() {
        let n = 50;
        let luma = vec![0.5; n];
        let mut base = vec![0.0; n];
        base[10] = 1.0;
        let r = compute_deflicker(&luma, &base, &opts());
        assert!(
            r.ev[10] < -0.5,
            "expected negative correction, got {}",
            r.ev[10]
        );
    }
}
