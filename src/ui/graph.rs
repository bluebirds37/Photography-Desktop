//! Luminance / exposure curve with keyframe markers and playhead.

use gpui::{canvas, point, px, rgb, AnyElement, IntoElement, PathBuilder, Rgba, Styled};

use crate::core::model::Sequence;

const CYAN: u32 = 0x58c7d8;
const AMBER: u32 = 0xe8b05c;
const GRID: u32 = 0x1e252d;
const PLAYHEAD: u32 = 0xf2f5f8;

fn alpha(hex: u32, a: f32) -> Rgba {
    let mut c = rgb(hex);
    c.a = a;
    c
}

fn normalize(values: &[f32]) -> (Vec<f32>, f32, f32) {
    let min = values.iter().copied().fold(f32::INFINITY, f32::min);
    let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if !min.is_finite() || !max.is_finite() || (max - min) < 1e-6 {
        return (vec![0.5; values.len()], 0.0, 1.0);
    }
    (
        values.iter().map(|v| (v - min) / (max - min)).collect(),
        min,
        max,
    )
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
    let (luma_n, _, _) = normalize(&luma);
    let (exp_n, _, _) = normalize(&exposure);
    let keys: Vec<usize> = (0..n).filter(|&i| seq.images[i].keyframe).collect();

    let canvas_el = canvas(
        |_, _, _| {},
        move |bounds, _: (), window, _cx| {
            let w = f32::from(bounds.size.width).max(1.0);
            let h = f32::from(bounds.size.height).max(1.0);
            let ox = bounds.origin.x;
            let oy = bounds.origin.y;
            let x_at = |i: usize| {
                if n <= 1 {
                    ox + px(w * 0.5)
                } else {
                    ox + px(w * (i as f32 / (n - 1) as f32))
                }
            };
            let y_at = |v: f32| oy + px(h * (1.0 - v.clamp(0.0, 1.0)));

            // Horizontal grid.
            for g in 0..=4 {
                let y = oy + px(h * g as f32 / 4.0);
                let mut b = PathBuilder::stroke(px(1.0));
                b.move_to(point(bounds.origin.x, y));
                b.line_to(point(bounds.origin.x + bounds.size.width, y));
                if let Ok(p) = b.build() {
                    window.paint_path(p, rgb(GRID));
                }
            }
            // Vertical keyframe ticks.
            for &k in &keys {
                let x = x_at(k);
                let mut b = PathBuilder::stroke(px(1.0));
                b.move_to(point(x, oy));
                b.line_to(point(x, oy + px(h * 0.18)));
                if let Ok(p) = b.build() {
                    window.paint_path(p, alpha(AMBER, 0.75));
                }
            }
            // Curves.
            for (series, color) in [(&luma_n, CYAN), (&exp_n, AMBER)] {
                if series.len() < 2 {
                    continue;
                }
                let mut b = PathBuilder::stroke(px(1.6));
                for (i, v) in series.iter().enumerate() {
                    let p = point(x_at(i), y_at(*v));
                    if i == 0 {
                        b.move_to(p);
                    } else {
                        b.line_to(p);
                    }
                }
                if let Ok(path) = b.build() {
                    window.paint_path(path, rgb(color));
                }
            }
            // Playhead.
            if n > 0 && selected < n {
                let x = x_at(selected);
                let mut b = PathBuilder::stroke(px(1.0));
                b.move_to(point(x, oy));
                b.line_to(point(x, oy + bounds.size.height));
                if let Ok(p) = b.build() {
                    window.paint_path(p, alpha(PLAYHEAD, 0.9));
                }
            }
        },
    )
    .size_full();

    canvas_el.into_any_element()
}
