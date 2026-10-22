//! Frame rendering and ffmpeg encoding.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::Sender;
use image::{imageops, DynamicImage, RgbaImage};
use rayon::prelude::*;

use crate::core::analysis::decode_full;
use crate::core::image_pipeline::{develop, sharpen, ToneTable};
use crate::core::model::{OutputCodec, OutputFormat, RateControl, RenderSettings, Sequence};

#[derive(Clone, Debug)]
pub enum RenderEvent {
    Stage(String),
    Progress {
        done: usize,
        total: usize,
        stage: String,
    },
    Finished {
        output: PathBuf,
        frames: Option<PathBuf>,
        count: usize,
    },
    Failed(String),
}

pub use crate::core::ffmpeg::find_ffmpeg;

pub fn fit_to_canvas(img: &DynamicImage, width: u32, height: u32) -> RgbaImage {
    let (w, h) = (img.width().max(1), img.height().max(1));
    let scale = (width as f32 / w as f32).max(height as f32 / h as f32);
    let nw = (w as f32 * scale).round().max(1.0) as u32;
    let nh = (h as f32 * scale).round().max(1.0) as u32;
    let resized = img
        .resize_exact(nw, nh, imageops::FilterType::Lanczos3)
        .to_rgba8();
    let x = (nw.saturating_sub(width)) / 2;
    let y = (nh.saturating_sub(height)) / 2;
    imageops::crop_imm(&resized, x, y, width, height).to_image()
}

fn develop_frame(
    seq: &Sequence,
    index: usize,
    table: &ToneTable,
    settings: &RenderSettings,
) -> Result<RgbaImage> {
    let im = &seq.images[index];
    let path = im.path(&seq.folder);
    let src = decode_full(&path)?;
    let mut adj = im.adj.clone();
    if !settings.deflicker {
        adj.deflicker = 0.0;
    }
    let mut out = develop(&src, &adj, None, table);
    out = fit_to_canvas(
        &DynamicImage::ImageRgba8(out),
        settings.width,
        settings.height,
    );
    if settings.sharpen > 0.0 {
        sharpen(&mut out, settings.sharpen);
    }
    Ok(out)
}

/// Render a single frame for the interactive preview (fast path).
pub fn preview_frame(
    seq: &Sequence,
    index: usize,
    max_dim: u32,
    table: &ToneTable,
) -> Result<RgbaImage> {
    let im = seq
        .images
        .get(index)
        .ok_or_else(|| anyhow!("frame {index} out of range"))?;
    let src = crate::core::analysis::decode_preview(&im.path(&seq.folder), max_dim)?;
    Ok(develop(&src, &im.adj, Some(max_dim), table))
}

fn output_path(seq: &Sequence, settings: &RenderSettings) -> PathBuf {
    if let Some(p) = &settings.output_path {
        return p.clone();
    }
    let ext = match settings.format {
        OutputFormat::Mp4 => "mp4",
        OutputFormat::Mov => "mov",
    };
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    seq.folder.join(format!("{}_{}.{}", seq.name, stamp, ext))
}

fn frame_dir(seq: &Sequence) -> PathBuf {
    seq.folder
        .join(crate::core::model::PROJECT_DIR)
        .join("render")
        .join("frames")
}

pub fn render_sequence(
    seq: &Sequence,
    settings: &RenderSettings,
    cancel: Arc<AtomicBool>,
    tx: Sender<RenderEvent>,
) -> Result<()> {
    let total = seq.images.len();
    if total == 0 {
        return Err(anyhow!("empty sequence"));
    }
    let out = output_path(seq, settings);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dir = frame_dir(seq);
    // Only touch our own work directory.
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::create_dir_all(&dir)?;

    if settings.write_xmp {
        let _ = tx.send(RenderEvent::Stage("写入 XMP".into()));
        crate::core::xmp::export_all(seq)?;
    }
    let _ = tx.send(RenderEvent::Stage("渲染帧".into()));
    let table = ToneTable::new();
    let done = AtomicUsize::new(0);
    let blur = settings.motion_blur;
    let blur_frames = settings.motion_blur_frames.clamp(2, 9) as usize;

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads((num_cpus::get() / 2).clamp(2, 4))
        .build()
        .context("render thread pool")?;
    let result: Result<()> = pool.install(|| {
        (0..total).into_par_iter().try_for_each(|i| {
            if cancel.load(Ordering::Relaxed) {
                return Err(anyhow!("cancelled"));
            }
            let frame = if blur {
                let mut acc: Option<RgbaImage> = None;
                let mut weight_sum = 0.0f32;
                let half = blur_frames / 2;
                for k in 0..blur_frames {
                    let src_idx = i.saturating_add(k).saturating_sub(half).min(total - 1);
                    if src_idx == i && k != half {
                        continue;
                    }
                    let w = if src_idx == i { 2.0 } else { 1.0 };
                    let f = develop_frame(seq, src_idx, &table, settings)?;
                    match &mut acc {
                        None => {
                            acc = Some(f);
                            weight_sum = w;
                        }
                        Some(a) => {
                            let new_sum = weight_sum + w;
                            for (p, q) in a.pixels_mut().zip(f.pixels()) {
                                for c in 0..3 {
                                    p[c] = ((p[c] as f32 * weight_sum + q[c] as f32 * w) / new_sum)
                                        .clamp(0.0, 255.0)
                                        as u8;
                                }
                            }
                            weight_sum = new_sum;
                        }
                    }
                }
                acc.unwrap_or(develop_frame(seq, i, &table, settings)?)
            } else {
                develop_frame(seq, i, &table, settings)?
            };
            let path = dir.join(format!("frame_{:06}.png", i + 1));
            frame.save(&path)?;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 5 == 0 || n == total {
                let _ = tx.send(RenderEvent::Progress {
                    done: n,
                    total,
                    stage: "渲染帧".into(),
                });
            }
            Ok(())
        })
    });

    if let Err(e) = result {
        if e.to_string() == "cancelled" {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(anyhow!("已取消"));
        }
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }

    let frames_keep = if settings.export_frames {
        let target = seq
            .folder
            .join(crate::core::model::PROJECT_DIR)
            .join("exported_frames");
        let _ = std::fs::remove_dir_all(&target);
        std::fs::create_dir_all(&target)?;
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            std::fs::copy(entry.path(), target.join(entry.file_name()))?;
        }
        Some(target)
    } else {
        None
    };

    let frame_count = total;
    if settings.export_frames && settings.format == OutputFormat::Mp4 {
        // Still encode; Export frames is an additional output.
    }
    let _ = tx.send(RenderEvent::Stage("编码视频".into()));
    let ffmpeg = find_ffmpeg().ok_or_else(|| anyhow!("未找到 ffmpeg，请安装并加入 PATH"))?;
    if let Err(e) = encode(&ffmpeg, &dir, settings, &out, cancel.clone(), tx.clone()) {
        if !settings.export_frames {
            let _ = std::fs::remove_dir_all(&dir);
        }
        return Err(e);
    }

    if !settings.export_frames {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let _ = tx.send(RenderEvent::Finished {
        output: out,
        frames: frames_keep,
        count: frame_count,
    });
    Ok(())
}

/// Build codec-level ffmpeg arguments. Kept separate so rate control can be
/// unit tested without launching ffmpeg.
pub(crate) fn video_args(settings: &RenderSettings) -> Vec<String> {
    let mut args = Vec::new();
    match settings.codec {
        OutputCodec::H264 => {
            args.extend(["-c:v".into(), "libx264".into()]);
            args.extend(["-preset".into(), settings.encoder_preset.x264().into()]);
            args.extend(rate_args(settings));
            args.extend(["-pix_fmt".into(), "yuv420p".into()]);
            args.extend(["-movflags".into(), "+faststart".into()]);
        }
        OutputCodec::H265 => {
            args.extend(["-c:v".into(), "libx265".into()]);
            args.extend(["-preset".into(), settings.encoder_preset.x264().into()]);
            args.extend(rate_args(settings));
            args.extend(["-pix_fmt".into(), "yuv420p10le".into()]);
            args.extend(["-tag:v".into(), "hvc1".into()]);
        }
        OutputCodec::ProRes => {
            args.extend([
                "-c:v".into(),
                "prores_ks".into(),
                "-profile:v".into(),
                "3".into(),
                "-pix_fmt".into(),
                "yuv422p10le".into(),
            ]);
        }
    }
    args
}

fn rate_args(settings: &RenderSettings) -> Vec<String> {
    match settings.rate_control {
        RateControl::Crf => vec!["-crf".into(), settings.quality.to_string()],
        RateControl::Bitrate => {
            let mbps = settings.bitrate_mbps.clamp(0.5, 200.0);
            let max = mbps * 1.25;
            let buf = mbps * 2.0;
            vec![
                "-b:v".into(),
                format!("{mbps:.0}M"),
                "-maxrate".into(),
                format!("{max:.0}M"),
                "-bufsize".into(),
                format!("{buf:.0}M"),
            ]
        }
    }
}

fn encode(
    ffmpeg: &Path,
    dir: &Path,
    settings: &RenderSettings,
    out: &Path,
    cancel: Arc<AtomicBool>,
    tx: Sender<RenderEvent>,
) -> Result<()> {
    let pattern = dir.join("frame_%06d.png");
    let mut cmd = Command::new(ffmpeg);
    cmd.arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-framerate")
        .arg(format!("{}", settings.fps))
        .arg("-i")
        .arg(&pattern);
    for arg in video_args(settings) {
        cmd.arg(arg);
    }
    cmd.arg("-an").arg(out);
    let _ = tx.send(RenderEvent::Stage(format!("ffmpeg: {}", ffmpeg.display())));
    let mut child = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("start {}", ffmpeg.display()))?;

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            return Err(anyhow!("已取消"));
        }
        match child.try_wait()? {
            Some(status) => {
                if status.success() {
                    return Ok(());
                }
                let mut err = String::new();
                if let Some(mut s) = child.stderr.take() {
                    use std::io::Read;
                    let _ = s.read_to_string(&mut err);
                }
                return Err(anyhow!("ffmpeg 失败: {err}"));
            }
            None => std::thread::sleep(std::time::Duration::from_millis(120)),
        }
    }
}

pub fn default_output(seq: &Sequence) -> PathBuf {
    output_path(seq, &seq.render)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn bitrate_mode_uses_target_bitrate() {
        let s = RenderSettings {
            codec: OutputCodec::H264,
            rate_control: RateControl::Bitrate,
            bitrate_mbps: 24.0,
            ..RenderSettings::default()
        };
        let args = video_args(&s);
        assert!(args.windows(2).any(|w| w[0] == "-b:v" && w[1] == "24M"));
        assert!(!args.iter().any(|a| a == "-crf"));
    }

    #[test]
    fn crf_mode_uses_quality() {
        let s = RenderSettings {
            codec: OutputCodec::H265,
            rate_control: RateControl::Crf,
            quality: 18,
            ..RenderSettings::default()
        };
        let args = video_args(&s);
        assert!(args.windows(2).any(|w| w[0] == "-crf" && w[1] == "18"));
        assert!(!args.iter().any(|a| a == "-b:v"));
    }

    #[test]
    fn prores_ignores_bitrate() {
        let s = RenderSettings {
            codec: OutputCodec::ProRes,
            rate_control: RateControl::Bitrate,
            bitrate_mbps: 50.0,
            ..RenderSettings::default()
        };
        let args = video_args(&s);
        assert!(args.iter().any(|a| a == "prores_ks"));
        assert!(!args.iter().any(|a| a == "-b:v"));
    }

    #[test]
    fn canvas_fill_crops_center() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(100, 50, Rgba([1, 2, 3, 255])));
        let out = fit_to_canvas(&img, 50, 50);
        assert_eq!((out.width(), out.height()), (50, 50));
    }
}
