//! Robust LOESS smoothing.
//!
//! Fits a local linear regression with tricube weights over a moving window of
//! `span * n` samples. Visual deflicker uses zero robustness iterations; the
//! generic implementation also provides an optional bisquare reweighting loop.

pub fn mean(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().copied().map(f64::from).sum::<f64>() as f32 / values.len() as f32
}

pub fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    if v.len() % 2 == 0 {
        (v[mid - 1] + v[mid]) * 0.5
    } else {
        v[mid]
    }
}

fn tricube(u: f64) -> f64 {
    if u >= 1.0 {
        0.0
    } else {
        let a = 1.0 - u * u * u;
        a * a * a
    }
}

/// Local linear regression.
///
/// * `x` must be sorted ascending (frame indices or time stamps).
/// * `span` is the fraction of samples used by each local fit (0..=1).
/// * `robustness_iters` is the number of bisquare reweighting passes.
pub fn loess(x: &[f64], y: &[f64], span: f64, robustness_iters: usize, eps: f64) -> Vec<f64> {
    assert_eq!(x.len(), y.len(), "x and y must have the same length");
    let n = x.len();
    if n == 0 {
        return vec![];
    }
    if n <= 2 {
        return y.to_vec();
    }
    let span = span.clamp(0.0, 1.0);
    let q = ((span * n as f64).round() as usize).clamp(2, n);

    let mut robustness = vec![1.0f64; n];
    let mut fitted = vec![0.0f64; n];

    for iter in 0..=robustness_iters {
        for i in 0..n {
            // Grow a window of q nearest samples around i (ties go left).
            let mut left = i;
            let mut right = i;
            while right - left + 1 < q {
                if left == 0 {
                    right += 1;
                } else if right + 1 >= n {
                    left -= 1;
                } else if x[i] - x[left - 1] <= x[right + 1] - x[i] {
                    left -= 1;
                } else {
                    right += 1;
                }
            }

            let bandwidth = (x[i] - x[left]).max(x[right] - x[i]).max(1e-12);
            let mut sw = 0.0;
            let mut swx = 0.0;
            let mut swy = 0.0;
            let mut swxx = 0.0;
            let mut swxy = 0.0;
            for j in left..=right {
                let dx = x[j] - x[i];
                let u = (dx / bandwidth).abs();
                let w = tricube(u) * robustness[j];
                if w <= 0.0 {
                    continue;
                }
                sw += w;
                swx += w * dx;
                swy += w * y[j];
                swxx += w * dx * dx;
                swxy += w * dx * y[j];
            }
            if sw <= 1e-15 {
                fitted[i] = y[i];
                continue;
            }
            let denom = swxx - swx * swx / sw;
            let slope = if denom.abs() < 1e-15 {
                0.0
            } else {
                (swxy - swx * swy / sw) / denom
            };
            let intercept = (swy - slope * swx) / sw;
            fitted[i] = intercept;
        }

        if iter == robustness_iters {
            break;
        }

        let residuals: Vec<f64> = (0..n).map(|i| (y[i] - fitted[i]).abs()).collect();
        let med = median(&residuals);
        if med < eps {
            break;
        }
        let six_med = 6.0 * med;
        for i in 0..n {
            let r = residuals[i] / six_med;
            robustness[i] = if r >= 1.0 {
                0.0
            } else {
                let a = 1.0 - r * r;
                a * a
            };
        }
    }

    // Preserve constant inputs exactly (avoids float dust in flat sequences).
    if y.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-12) {
        return vec![y[0]; n];
    }
    fitted
}

/// Replace every value with mean + offset, clamped to 0..1.
pub fn flatten_mean(values: &[f64], offset: f64) -> Vec<f64> {
    let m = (mean_f64(values) + offset).clamp(0.0, 1.0);
    vec![m; values.len()]
}

/// Straight line from the first to the last value.
pub fn linear_endpoints(values: &[f64]) -> Vec<f64> {
    let n = values.len();
    if n == 0 {
        return vec![];
    }
    if n == 1 {
        return vec![values[0]];
    }
    let a = values[0];
    let b = values[n - 1];
    (0..n)
        .map(|i| a + (b - a) * i as f64 / (n - 1) as f64)
        .collect()
}

pub fn mean_f64(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

/// Convenience wrapper for smoothing a curve sampled at frame indices.
pub fn smooth_frames(values: &[f64], span: f64, robustness_iters: usize) -> Vec<f64> {
    let x: Vec<f64> = (0..values.len()).map(|i| i as f64).collect();
    loess(&x, values, span, robustness_iters, 1e-12)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_curve_stays_flat() {
        let y = vec![0.4; 120];
        let s = smooth_frames(&y, 0.3, 0);
        for v in s {
            assert!((v - 0.4).abs() < 1e-9);
        }
    }

    #[test]
    fn linear_curve_is_reproduced() {
        let y: Vec<f64> = (0..200).map(|i| 0.2 + 0.6 * i as f64 / 199.0).collect();
        let s = smooth_frames(&y, 0.35, 0);
        for (a, b) in y.iter().zip(s.iter()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn spike_is_attenuated() {
        let mut y = vec![0.5; 200];
        for v in y.iter_mut().skip(95).take(10) {
            *v = 0.65;
        }
        let s = smooth_frames(&y, 0.3, 0);
        assert!(s[100] < 0.56, "spike not smoothed: {}", s[100]);
        assert!((s[10] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn median_even_and_odd() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), 2.5);
    }
}
