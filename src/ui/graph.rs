//! Luminance / exposure curve with keyframe markers and playhead.

use gpui::{canvas, point, px, rgb, AnyElement, IntoElement, PathBuilder, Rgba, Styled};

use crate::core::model::Sequence;

const LUMA: u32 = 0x5b9dff;
const EXPOSURE: u32 = 0xe6b45f;
const GRID: u32 = 0x1f252d;
const PLAYHEAD: u32 = 0xe9ecf1;

const PAD_X: f32 = 6.0;
const PAD_TOP: f32 = 17.0;
const PAD_BOTTOM: f32 = 9.0;

fn alpha(hex: u32, a: f32) -> Rgba {
    let mut c = rgb(hex);
    c.a = a;
    c
}

fn normalize(values: &[f32]) -> Vec<f32> {
    let min = values.iter().copied().fold(f32::INFINITY, f32::min);
    let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if !min.is_finite() || !max.is_finite() || (max - min) < 1e-6 {
        return vec![0.5; values.len()];
    }
    values.iter().map(|v| (v - min) / (max - min)).collect()
}

fn dot_polygon(cx: f32, cy: f32, r: f32) -> Vec<gpui::Point<gpui::Pixels>> {
    (0..10)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 10.0;
            point(px(cx + r * a.cos()), px(cy + r * a.sin()))
        })
        .collect()
}

pub fn graph_view(seq: &Sequence, selected: usize) -> AnyElement {
    let n = seq.images.len();
    // Show the brightness the viewer will actually see: the analyzed luma with
    // the current exposure and deflicker corrections applied.
    let luma: Vec<f32> = seq
        .images
        .iter()
        .map(|im| {
            (im.luma.max(1e-6) * im.adj.total_ev().exp2())
                .max(1e-6)
                .log2()
        })
        .collect();
    let exposure: Vec<f32> = seq.images.iter().map(|im| im.adj.exposure).collect();
    let luma_n = normalize(&luma);
    let exp_n = normalize(&exposure);
    let keys: Vec<usize> = (0..n).filter(|&i| seq.images[i].keyframe).collect();
    let playhead_value = luma_n.get(selected).copied();

    let canvas_el = canvas(
        |_, _, _| {},
        move |bounds, _: (), window, _cx| {
            let left = f32::from(bounds.origin.x) + PAD_X;
            let top = f32::from(bounds.origin.y) + PAD_TOP;
            let right = f32::from(bounds.origin.x + bounds.size.width) - PAD_X;
            let bottom = f32::from(bounds.origin.y + bounds.size.height) - PAD_BOTTOM;
            let w = (right - left).max(1.0);
            let h = (bottom - top).max(1.0);
            let fx = |i: usize| {
                if n <= 1 {
                    left + w * 0.5
                } else {
                    left + w * (i as f32 / (n - 1) as f32)
                }
            };
            let fy = |v: f32| top + h * (1.0 - v.clamp(0.0, 1.0));

            // Horizontal reference lines.
            for (g, a) in [(0.0, 0.55), (0.5, 1.0), (1.0, 0.55)] {
                let y = fy(g);
                let mut b = PathBuilder::stroke(px(1.0));
                b.move_to(point(px(left), px(y)));
                b.line_to(point(px(right), px(y)));
                if let Ok(p) = b.build() {
                    window.paint_path(p, alpha(GRID, a));
                }
            }

            // Area under the luma curve.
            if luma_n.len() >= 2 {
                let mut b = PathBuilder::fill();
                b.move_to(point(px(left), px(bottom)));
                for (i, v) in luma_n.iter().enumerate() {
                    b.line_to(point(px(fx(i)), px(fy(*v))));
                }
                b.line_to(point(px(right), px(bottom)));
                b.close();
                if let Ok(p) = b.build() {
                    window.paint_path(p, alpha(LUMA, 0.10));
                }
            }

            // Curves: exposure sits under the primary luma series.
            for (series, color, width, a, dashed) in [
                (&exp_n, EXPOSURE, 1.3, 0.7, true),
                (&luma_n, LUMA, 1.8, 1.0, false),
            ] {
                if series.len() < 2 {
                    continue;
                }
                let mut b = PathBuilder::stroke(px(width));
                if dashed {
                    b = b.dash_array(&[px(4.0), px(3.0)]);
                }
                for (i, v) in series.iter().enumerate() {
                    let p = point(px(fx(i)), px(fy(*v)));
                    if i == 0 {
                        b.move_to(p);
                    } else {
                        b.line_to(p);
                    }
                }
                if let Ok(path) = b.build() {
                    window.paint_path(path, alpha(color, a));
                }
            }

            // Keyframe diamonds along the top edge.
            for &k in &keys {
                let cx = fx(k);
                let cy = f32::from(bounds.origin.y) + 8.0;
                let r = 3.6;
                let mut b = PathBuilder::fill();
                b.add_polygon(
                    &[
                        point(px(cx), px(cy - r)),
                        point(px(cx + r), px(cy)),
                        point(px(cx), px(cy + r)),
                        point(px(cx - r), px(cy)),
                    ],
                    true,
                );
                if let Ok(p) = b.build() {
                    window.paint_path(p, alpha(EXPOSURE, 0.95));
                }
            }

            // Playhead column and marker on the luma curve.
            if n > 0 && selected < n {
                let x = fx(selected);
                let mut b = PathBuilder::stroke(px(1.0));
                b.move_to(point(px(x), px(top)));
                b.line_to(point(px(x), px(bottom)));
                if let Ok(p) = b.build() {
                    window.paint_path(p, alpha(PLAYHEAD, 0.45));
                }
                if let Some(v) = playhead_value {
                    let cy = fy(v);
                    let mut ring = PathBuilder::fill();
                    ring.add_polygon(&dot_polygon(x, cy, 4.4), true);
                    if let Ok(p) = ring.build() {
                        window.paint_path(p, alpha(LUMA, 0.28));
                    }
                    let mut core = PathBuilder::fill();
                    core.add_polygon(&dot_polygon(x, cy, 2.6), true);
                    if let Ok(p) = core.build() {
                        window.paint_path(p, rgb(PLAYHEAD));
                    }
                }
            }
        },
    )
    .size_full();

    canvas_el.into_any_element()
}
