//! CPU image development pipeline.
//!
//! The renderer applies exposure, white balance and a compact set of tone
//! controls. LUTs are used for the sRGB transfer function; the
//! per-pixel work happens on linear values so exposure/deflicker corrections are
//! physically additive.

use image::{imageops, DynamicImage, RgbaImage};

use crate::core::model::Adjustments;

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

pub struct ToneTable {
    to_linear: [f32; 256],
    to_srgb: Vec<u8>,
}

impl ToneTable {
    pub fn new() -> Self {
        let mut to_linear = [0.0f32; 256];
        for (i, v) in to_linear.iter_mut().enumerate() {
            *v = srgb_to_linear(i as f32 / 255.0);
        }
        let mut to_srgb = vec![0u8; 4097];
        for (i, v) in to_srgb.iter_mut().enumerate() {
            *v = (linear_to_srgb(i as f32 / 4096.0) * 255.0).round() as u8;
        }
        Self { to_linear, to_srgb }
    }

    #[inline]
    pub fn encode(&self, linear: f32) -> u8 {
        let idx = (linear.clamp(0.0, 1.0) * 4096.0) as usize;
        self.to_srgb[idx.min(4096)]
    }
}

impl Default for ToneTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Resize to fit within `max_dim`, preserving aspect ratio (no upscaling).
pub fn fit(img: DynamicImage, max_dim: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w <= max_dim && h <= max_dim {
        return img;
    }
    let scale = max_dim as f32 / w.max(h) as f32;
    let nw = (w as f32 * scale).round().max(1.0) as u32;
    let nh = (h as f32 * scale).round().max(1.0) as u32;
    img.resize_exact(nw, nh, imageops::FilterType::Triangle)
}

fn tone_curve(v: f32, contrast: f32, blacks: f32, whites: f32) -> f32 {
    let mut x = v;
    if contrast.abs() > 0.01 {
        let c = contrast / 100.0;
        // Symmetric S-curve; c > 0 increases mid-tone contrast without hard clipping.
        let s = x * x * (3.0 - 2.0 * x);
        x = x + (s - x) * c;
    }
    if blacks.abs() > 0.01 {
        let b = blacks / 100.0;
        x += b * 0.18 * (1.0 - x).powi(2);
    }
    if whites.abs() > 0.01 {
        let w = whites / 100.0;
        x += w * 0.18 * x.powi(2);
    }
    x.clamp(0.0, 1.0)
}

/// Apply adjustments to an image. `max_dim` is used for previews/filmstrips.
pub fn develop(
    img: &DynamicImage,
    adj: &Adjustments,
    max_dim: Option<u32>,
    table: &ToneTable,
) -> RgbaImage {
    let source = match max_dim {
        Some(m) => fit(img.clone(), m),
        None => img.clone(),
    };
    let mut out = source.to_rgba8();
    let ev = adj.total_ev();
    let exp_gain = ev.exp2();
    let temp = (adj.temperature / 100.0).clamp(-1.5, 1.5);
    let r_gain = (temp * 0.35).exp2();
    let b_gain = (-temp * 0.35).exp2();
    let g_gain = (-adj.tint / 100.0 * 0.25).exp2();
    let contrast = adj.contrast;
    let highlights = adj.highlights / 100.0;
    let shadows = adj.shadows / 100.0;
    let blacks = adj.blacks;
    let whites = adj.whites;
    let saturation = 1.0 + adj.saturation / 100.0;
    let vibrance = adj.vibrance / 100.0;
    let identity = adj.is_identity();

    for px in out.pixels_mut() {
        let r = table.to_linear[px[0] as usize];
        let g = table.to_linear[px[1] as usize];
        let b = table.to_linear[px[2] as usize];
        if identity {
            continue;
        }

        let mut r = r * exp_gain * r_gain;
        let mut g = g * exp_gain * g_gain;
        let mut b = b * exp_gain * b_gain;

        // Highlight roll-off and shadow lift in linear space.
        if highlights.abs() > 0.01 || shadows.abs() > 0.01 {
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            if highlights.abs() > 0.01 {
                let w = (l / (l + 0.35)).clamp(0.0, 1.0);
                let f = 1.0 + highlights * 0.45 * w;
                r *= f;
                g *= f;
                b *= f;
            }
            if shadows.abs() > 0.01 {
                let w = 1.0 / (1.0 + 6.0 * l);
                let lift = shadows * 0.12 * w;
                r += lift;
                g += lift;
                b += lift;
            }
        }

        // Tone curve in display space.
        let mut sr = linear_to_srgb(r);
        let mut sg = linear_to_srgb(g);
        let mut sb = linear_to_srgb(b);
        if contrast.abs() > 0.01 || blacks.abs() > 0.01 || whites.abs() > 0.01 {
            sr = tone_curve(sr, contrast, blacks, whites);
            sg = tone_curve(sg, contrast, blacks, whites);
            sb = tone_curve(sb, contrast, blacks, whites);
        }

        if (saturation - 1.0).abs() > 0.001 || vibrance.abs() > 0.001 {
            let l = 0.2126 * sr + 0.7152 * sg + 0.0722 * sb;
            let sat = saturation + vibrance * (1.0 - (l - 0.5).abs() * 2.0).max(0.0);
            sr = l + (sr - l) * sat;
            sg = l + (sg - l) * sat;
            sb = l + (sb - l) * sat;
        }

        px[0] = table.encode(srgb_to_linear(sr.clamp(0.0, 1.0)));
        px[1] = table.encode(srgb_to_linear(sg.clamp(0.0, 1.0)));
        px[2] = table.encode(srgb_to_linear(sb.clamp(0.0, 1.0)));
        px[3] = 255;
    }
    out
}

/// Mean luma plus gray-world channel means from a preview (sRGB domain).
pub fn analyze_pixels(img: &RgbaImage) -> (f32, f32, f32) {
    let mut sum_r = 0.0f64;
    let mut sum_g = 0.0f64;
    let mut sum_b = 0.0f64;
    let mut sum_l = 0.0f64;
    let count = img.width().max(1) as f64 * img.height().max(1) as f64;
    for px in img.pixels() {
        let r = px[0] as f64;
        let g = px[1] as f64;
        let b = px[2] as f64;
        sum_r += r;
        sum_g += g;
        sum_b += b;
        sum_l += 0.2126 * r + 0.7152 * g + 0.0722 * b;
    }
    let luma = (sum_l / count / 255.0) as f32;
    let g = (sum_g / count).max(1.0) as f32;
    (
        luma.clamp(1e-4, 1.0),
        (sum_r / count) as f32 / g,
        (sum_b / count) as f32 / g,
    )
}

/// Fast luminance-only sample, used when iterating deflicker previews.
pub fn mean_luma(img: &RgbaImage) -> f32 {
    let mut sum = 0.0f64;
    for px in img.pixels() {
        sum += 0.2126 * px[0] as f64 + 0.7152 * px[1] as f64 + 0.0722 * px[2] as f64;
    }
    (sum / (img.width() * img.height()).max(1) as f64 / 255.0) as f32
}

/// Simple unsharp mask used by the render dialog.
pub fn sharpen(img: &mut RgbaImage, amount: f32) {
    if amount <= 0.0 {
        return;
    }
    let blurred = imageops::blur(img, 1.2);
    let k = amount.clamp(0.0, 2.0);
    for (p, q) in img.pixels_mut().zip(blurred.pixels()) {
        for c in 0..3 {
            let v = p[c] as f32 + (p[c] as f32 - q[c] as f32) * k;
            p[c] = v.clamp(0.0, 255.0) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn srgb_roundtrip() {
        for i in 0..=255 {
            let c = i as f32 / 255.0;
            let back = linear_to_srgb(srgb_to_linear(c));
            assert!((c - back).abs() < 1e-5);
        }
    }

    #[test]
    fn exposure_brightens() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(4, 4, Rgba([100, 100, 100, 255])));
        let t = ToneTable::new();
        let out = develop(
            &img,
            &Adjustments {
                exposure: 1.0,
                ..Adjustments::default()
            },
            None,
            &t,
        );
        assert!(out.get_pixel(0, 0)[0] > 130);
    }

    #[test]
    fn preview_fit_keeps_aspect() {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(4000, 2000));
        let out = fit(img, 800);
        assert_eq!((out.width(), out.height()), (800, 400));
    }
}
