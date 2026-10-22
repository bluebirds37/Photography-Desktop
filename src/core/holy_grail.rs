//! Holy-Grail wizard: compensate the exposure / white-balance steps made while
//! shooting a day-to-night sequence.
//!
//! A per-frame exposure curve is built from the camera settings, smoothed by the
//! initial-smoothing control, then combined with a creative ramp built from
//! three sliders: offset, stretch and rotate. The Optimize action performs a
//! coarse-to-fine search over stretch then rotate, minimising the residual of
//! the resulting curve. Search ranges:
//! stretch ±1.5 in 0.02 steps, rotate ±4.0 in 0.04 steps.

use crate::core::loess::smooth_frames;

#[derive(Clone, Copy, Debug)]
pub struct HGOptions {
    pub offset: f32,
    pub stretch: f32,
    pub rotate: f32,
    /// Smoothing span applied to the raw exposure curve.
    pub reduce_initial: f32,
}

impl Default for HGOptions {
    fn default() -> Self {
        Self {
            offset: 0.0,
            stretch: 0.0,
            rotate: 0.0,
            reduce_initial: 0.5,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HGResult {
    pub exposure: Vec<f32>,
    pub temperature: Vec<f32>,
    pub tint: Vec<f32>,
    /// Normalised camera-exposure trend used to build the compensation.
    pub trend: Vec<f32>,
    pub score: f32,
}

/// Fill missing samples by linear interpolation, falling back to `fallback`.
fn fill_series(values: &[Option<f32>], fallback: &[f32]) -> Vec<f32> {
    let n = values.len();
    let mut out = vec![0.0f32; n];
    let known: Vec<usize> = (0..n).filter(|&i| values[i].is_some()).collect();
    if known.is_empty() {
        return fallback.to_vec();
    }
    for i in 0..n {
        if let Some(v) = values[i] {
            out[i] = v;
        }
    }
    let mut k = 0;
    for i in 0..n {
        if values[i].is_some() {
            continue;
        }
        while k + 1 < known.len() && known[k + 1] < i {
            k += 1;
        }
        let left = known[k];
        let right = if k + 1 < known.len() {
            known[k + 1]
        } else {
            left
        };
        out[i] = if right == left {
            out[left]
        } else {
            let u = (i - left) as f32 / (right - left) as f32;
            out[left] + (out[right] - out[left]) * u
        };
    }
    out
}

fn roughness(values: &[f32]) -> f32 {
    if values.len() < 3 {
        return 0.0;
    }
    let mut s = 0.0f32;
    for w in values.windows(3) {
        s += (w[2] - 2.0 * w[1] + w[0]).abs();
    }
    s / values.len() as f32
}

/// Build the exposure and white-balance curves for a sequence.
///
/// `camera_ev` is the exposure setting (aperture/shutter/ISO) in EV; `luma` is
/// the analyzed image brightness and is used when EXIF is missing. `wb` is a
/// gray-world warmth proxy and is used to remove abrupt WB jumps while leaving
/// the slow sunset colour drift intact.
pub fn apply_hg(
    camera_ev: &[Option<f32>],
    wb: &[Option<f32>],
    luma: &[f32],
    opts: HGOptions,
) -> HGResult {
    let n = luma.len();
    if n == 0 {
        return HGResult {
            exposure: vec![],
            temperature: vec![],
            tint: vec![],
            trend: vec![],
            score: 0.0,
        };
    }
    let fallback: Vec<f32> = luma.iter().map(|v| v.max(1e-6).log2()).collect();
    let ev_raw: Vec<Option<f32>> = camera_ev.to_vec();
    let ev_filled = fill_series(&ev_raw, &fallback);
    let mean_ev =
        crate::core::loess::mean_f64(&ev_filled.iter().map(|v| *v as f64).collect::<Vec<_>>())
            as f32;
    let trend: Vec<f32> = ev_filled.iter().map(|v| v - mean_ev).collect();

    let mut compensation = Vec::with_capacity(n);
    for i in 0..n {
        let t = if n > 1 {
            i as f32 / (n - 1) as f32
        } else {
            0.5
        };
        let ramp = (t - 0.5) * opts.rotate + opts.offset;
        compensation.push(-trend[i] * (1.0 + opts.stretch) + ramp);
    }

    let span = opts.reduce_initial.clamp(0.0, 1.0) as f64;
    let compensation64: Vec<f64> = compensation.iter().map(|v| *v as f64).collect();
    let smoothed = if span > 0.0 {
        smooth_frames(&compensation64, span.max(0.02), 0)
    } else {
        compensation64
    };
    let exposure: Vec<f32> = smoothed
        .iter()
        .map(|v| (*v as f32).clamp(-5.0, 5.0))
        .collect();

    // White balance: remove rapid jumps, keep the long sunset drift.
    let wb_filled = fill_series(wb, &vec![0.0; n]);
    let wb_trend: Vec<f64> = wb_filled.iter().map(|v| *v as f64).collect();
    let wb_slow = smooth_frames(&wb_trend, 0.45, 0);
    let mut temperature = vec![0.0f32; n];
    for i in 0..n {
        let jump = wb_trend[i] - wb_slow[i];
        let t = if n > 1 {
            i as f32 / (n - 1) as f32
        } else {
            0.5
        };
        temperature[i] = (-jump * (0.35 + opts.stretch * 0.15) as f64
            + (t - 0.5) as f64 * opts.rotate as f64 * 2.0)
            .clamp(-100.0, 100.0) as f32;
    }
    let tint = vec![0.0f32; n];

    let adjusted_log: Vec<f32> = (0..n)
        .map(|i| (luma[i].max(1e-6) * (exposure[i]).exp2()).max(1e-6).log2())
        .collect();
    let score = roughness(&adjusted_log)
        + 0.02 * exposure.iter().map(|v| v.abs()).sum::<f32>()
        + 0.001 * temperature.iter().map(|v| v.abs()).sum::<f32>();

    HGResult {
        exposure,
        temperature,
        tint,
        trend,
        score,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct HGSearchResult {
    pub opts: HGOptions,
    pub score: f32,
}

/// Search stretch, then rotate, for the Optimize action.
pub fn optimize_hg(
    camera_ev: &[Option<f32>],
    wb: &[Option<f32>],
    luma: &[f32],
    base: HGOptions,
) -> HGSearchResult {
    // The search is O(n * span * evaluations); subsample long sequences so the
    // Optimize button stays interactive. The chosen parameters are applied to
    // the full sequence afterwards.
    let n = luma.len();
    let stride = if n > 600 { (n / 600).max(1) } else { 1 };
    let ev_s: Vec<Option<f32>> = camera_ev.iter().step_by(stride).cloned().collect();
    let wb_s: Vec<Option<f32>> = wb.iter().step_by(stride).cloned().collect();
    let luma_s: Vec<f32> = luma.iter().step_by(stride).copied().collect();

    let mut best = HGSearchResult {
        opts: base,
        score: f32::INFINITY,
    };
    // Stretch: ±1.5 in 0.02 steps (151 evaluations).
    for step in 0..=150 {
        let stretch = step as f32 / 50.0 - 1.5;
        let opts = HGOptions { stretch, ..base };
        let r = apply_hg(&ev_s, &wb_s, &luma_s, opts);
        if r.score < best.score {
            best = HGSearchResult {
                opts,
                score: r.score,
            };
        }
    }
    // Rotate: ±4.0 in 0.04 steps (201 evaluations).
    let base = best.opts;
    for step in 0..=200 {
        let rotate = step as f32 / 25.0 - 4.0;
        let opts = HGOptions { rotate, ..base };
        let r = apply_hg(&ev_s, &wb_s, &luma_s, opts);
        if r.score < best.score {
            best = HGSearchResult {
                opts,
                score: r.score,
            };
        }
    }
    best
}

/// Apply an HG result to an image list. WB values use the same UI units as the
/// adjustment panel.
pub fn apply_to_images(seq: &mut crate::core::model::Sequence, result: &HGResult) {
    for (i, im) in seq.images.iter_mut().enumerate() {
        if let Some(v) = result.exposure.get(i) {
            im.adj.exposure = *v;
        }
        if let Some(v) = result.temperature.get(i) {
            im.adj.temperature = *v;
        }
        if let Some(v) = result.tint.get(i) {
            im.adj.tint = *v;
        }
    }
}

/// Read-only inputs from a sequence.
pub fn inputs(
    seq: &crate::core::model::Sequence,
) -> (Vec<Option<f32>>, Vec<Option<f32>>, Vec<f32>) {
    let ev = seq.images.iter().map(|im| im.camera_ev).collect();
    let wb = seq
        .images
        .iter()
        .map(|im| {
            if im.luma > 0.0 {
                Some(im.wb_proxy())
            } else {
                None
            }
        })
        .collect();
    let luma = seq.images.iter().map(|im| im.luma).collect();
    (ev, wb, luma)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compensates_exposure_steps() {
        let n = 200;
        let mut ev: Vec<Option<f32>> = Vec::new();
        for i in 0..n {
            ev.push(Some(if i < 100 { 10.0 } else { 11.0 }));
        }
        let luma: Vec<f32> = ev
            .iter()
            .map(|e| (0.4f32 * e.unwrap().exp2()).clamp(0.1, 1.0))
            .collect();
        let r = apply_hg(&ev, &vec![None; n], &luma, HGOptions::default());
        assert!(
            r.exposure[50] > r.exposure[150],
            "compensation should drop when camera EV rises"
        );
        assert!((r.exposure[150] - r.exposure[50] + 1.0).abs() < 0.2);
    }

    #[test]
    fn offset_shifts_curve() {
        let n = 40;
        let ev = vec![Some(9.0); n];
        let luma = vec![0.5; n];
        let a = apply_hg(&ev, &vec![None; n], &luma, HGOptions::default());
        let b = apply_hg(
            &ev,
            &vec![None; n],
            &luma,
            HGOptions {
                offset: 1.0,
                ..HGOptions::default()
            },
        );
        assert!((b.exposure[0] - a.exposure[0] - 1.0).abs() < 0.05);
    }

    #[test]
    fn optimizer_returns_valid_ranges() {
        let n = 100;
        let ev: Vec<Option<f32>> = (0..n)
            .map(|i| Some(9.0 + (i as f32 / n as f32) * 2.0))
            .collect();
        let luma: Vec<f32> = ev
            .iter()
            .map(|e| (e.unwrap() * 0.05).clamp(0.1, 0.9))
            .collect();
        let r = optimize_hg(&ev, &vec![None; n], &luma, HGOptions::default());
        assert!(r.opts.stretch >= -1.5 && r.opts.stretch <= 1.5);
        assert!(r.opts.rotate >= -4.0 && r.opts.rotate <= 4.0);
    }
}
