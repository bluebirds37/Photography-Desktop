//! Keyframe interpolation for adjustment columns.
//!
//! Every adjustment column is treated independently. Values between two
//! keyframes are interpolated; outside the outermost keyframes the nearest
//! value is held. Smooth mode uses a cubic smoothstep so exposure and colour
//! ramps don't kick at the keyframe boundaries.

use crate::core::model::{AdjustColumn, ImageEntry, TransitionMode};

fn ease(u: f32, mode: TransitionMode) -> f32 {
    match mode {
        TransitionMode::Linear => u,
        TransitionMode::Smooth => u * u * (3.0 - 2.0 * u),
    }
}

/// Interpolate one column. `values` is modified in place; keyframe values are
/// preserved exactly.
pub fn interpolate(values: &mut [f32], keyframes: &[bool], mode: TransitionMode) {
    let n = values.len().min(keyframes.len());
    if n == 0 {
        return;
    }
    let keys: Vec<usize> = (0..n).filter(|&i| keyframes[i]).collect();
    if keys.is_empty() {
        return;
    }
    if keys.len() == 1 {
        let v = values[keys[0]];
        values[..n].fill(v);
        return;
    }
    let first = keys[0];
    let last = *keys.last().unwrap();
    let first_val = values[first];
    values[..first].fill(first_val);
    let last_val = values[last];
    values[last + 1..n].fill(last_val);

    for pair in keys.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let va = values[a];
        let vb = values[b];
        let span = (b - a) as f32;
        for i in a + 1..b {
            let u = (i - a) as f32 / span;
            values[i] = va + (vb - va) * ease(u, mode);
        }
    }
}

/// Interpolate every creative adjustment column between keyframes. Deflicker
/// corrections are left untouched because they are per-frame measurements.
pub fn interpolate_adjustments(
    images: &mut [ImageEntry],
    keyframes: &[bool],
    mode: TransitionMode,
) {
    let columns = [
        AdjustColumn::Exposure,
        AdjustColumn::Temperature,
        AdjustColumn::Tint,
        AdjustColumn::Contrast,
        AdjustColumn::Highlights,
        AdjustColumn::Shadows,
        AdjustColumn::Whites,
        AdjustColumn::Blacks,
        AdjustColumn::Saturation,
        AdjustColumn::Vibrance,
    ];
    for col in columns {
        let mut values: Vec<f32> = images.iter().map(|im| col.get(&im.adj)).collect();
        interpolate(&mut values, keyframes, mode);
        for (im, v) in images.iter_mut().zip(values) {
            col.set(&mut im.adj, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_outside_keyframes() {
        let mut v = vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
        let k = vec![false, true, false, false, true, false];
        interpolate(&mut v, &k, TransitionMode::Linear);
        assert_eq!(v, vec![1.0, 1.0, 2.0, 3.0, 4.0, 4.0]);
    }

    #[test]
    fn smooth_mode_hits_keyframes() {
        let mut v = vec![0.0; 11];
        v[0] = 0.0;
        v[10] = 10.0;
        let mut k = vec![false; 11];
        k[0] = true;
        k[10] = true;
        interpolate(&mut v, &k, TransitionMode::Smooth);
        assert_eq!(v[0], 0.0);
        assert_eq!(v[10], 10.0);
        assert!((v[5] - 5.0).abs() < 1e-5);
        assert!(v[2] < 2.0, "smoothstep eases in");
    }
}
