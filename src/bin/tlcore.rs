//! Headless entry point for batch work and end-to-end verification.
//!
//! Usage:
//!   tlcore demo <folder>          create a synthetic test sequence
//!   tlcore analyze <folder>       import / analyze
//!   tlcore hg <folder>            apply the Holy Grail wizard defaults
//!   tlcore deflicker <folder>     run visual deflicker
//!   tlcore transition <folder>    interpolate between keyframes
//!   tlcore xmp <folder>           write XMP sidecars
//!   tlcore render <folder> [out]  render frames and encode with ffmpeg

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use timelapse_assistant::core::analysis::{import_folder, visual_curve_from_thumbs};
use timelapse_assistant::core::deflicker::{compute_deflicker, flicker_score, DeflickerOptions};
use timelapse_assistant::core::holy_grail::{apply_hg, apply_to_images, inputs, HGOptions};
use timelapse_assistant::core::model::Sequence;
use timelapse_assistant::core::render::{render_sequence, RenderEvent};
use timelapse_assistant::core::transition::interpolate;
use timelapse_assistant::core::xmp;

fn main() {
    if let Err(e) = run() {
        eprintln!("错误: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = args
        .split_first()
        .ok_or_else(|| anyhow!("missing command"))?;
    match cmd.as_str() {
        "demo" => demo(PathBuf::from(
            rest.first().ok_or_else(|| anyhow!("folder"))?,
        )),
        "analyze" => analyze(PathBuf::from(
            rest.first().ok_or_else(|| anyhow!("folder"))?,
        )),
        "hg" => hg(PathBuf::from(
            rest.first().ok_or_else(|| anyhow!("folder"))?,
        )),
        "deflicker" => deflicker(PathBuf::from(
            rest.first().ok_or_else(|| anyhow!("folder"))?,
        )),
        "transition" => transition(PathBuf::from(
            rest.first().ok_or_else(|| anyhow!("folder"))?,
        )),
        "keyframes" => keyframes(PathBuf::from(
            rest.first().ok_or_else(|| anyhow!("folder"))?,
        )),
        "hgopt" => hgopt(PathBuf::from(
            rest.first().ok_or_else(|| anyhow!("folder"))?,
        )),
        "report" => report(PathBuf::from(
            rest.first().ok_or_else(|| anyhow!("folder"))?,
        )),
        "xmp" => {
            let folder = PathBuf::from(rest.first().ok_or_else(|| anyhow!("folder"))?);
            let seq = Sequence::load(&folder)?.ok_or_else(|| anyhow!("no project"))?;
            let n = xmp::export_all(&seq)?;
            println!("wrote {n} XMP sidecars");
            Ok(())
        }
        "ffmpeg-status" => ffmpeg_status(),
        "ffmpeg-import" => ffmpeg_import(),
        "ffmpeg-install" => ffmpeg_install(timelapse_assistant::core::ffmpeg::MIRROR_URL),
        "ffmpeg-install-latest" => ffmpeg_install(timelapse_assistant::core::ffmpeg::LATEST_URL),
        "ffmpeg-use" => {
            let version = rest.first().ok_or_else(|| anyhow!("version"))?;
            timelapse_assistant::core::ffmpeg::select(Some(version))?;
            println!("已切换到内置 FFmpeg {version}");
            Ok(())
        }
        "ffmpeg-system" => {
            timelapse_assistant::core::ffmpeg::select(None)?;
            println!("已切换到系统 FFmpeg");
            Ok(())
        }
        "ffmpeg-uninstall" => {
            let version = rest.first().ok_or_else(|| anyhow!("version"))?;
            timelapse_assistant::core::ffmpeg::uninstall(version)?;
            println!("已卸载 FFmpeg {version}");
            Ok(())
        }
        "render" => {
            let folder = PathBuf::from(rest.first().ok_or_else(|| anyhow!("folder"))?);
            let out = rest.get(1).map(PathBuf::from);
            render(folder, out)
        }
        _ => Err(anyhow!("unknown command {cmd}")),
    }
}

fn analyze(folder: PathBuf) -> Result<()> {
    let existing = Sequence::load(&folder)?;
    let seq = import_folder(
        &folder,
        existing,
        Some(&|done, total, name| {
            if done % 10 == 0 || done == total {
                println!("[{done}/{total}] {name}");
            }
        }),
    )?;
    println!(
        "分析完成: {} 张, 平均亮度 {:.3}, 工作流 {:?}",
        seq.images.len(),
        seq.images.iter().map(|i| i.luma).sum::<f32>() / seq.images.len() as f32,
        seq.workflow.stage
    );
    Ok(())
}

fn hg(folder: PathBuf) -> Result<()> {
    let mut seq = Sequence::load(&folder)?.ok_or_else(|| anyhow!("no project"))?;
    let (ev, wb, luma) = inputs(&seq);
    let opts = HGOptions {
        offset: seq.settings.hg_offset,
        stretch: seq.settings.hg_stretch,
        rotate: seq.settings.hg_rotate,
        reduce_initial: seq.settings.hg_reduce_initial,
    };
    let r = apply_hg(&ev, &wb, &luma, opts);
    apply_to_images(&mut seq, &r);
    seq.workflow.hg_wizard = true;
    seq.save()?;
    println!(
        "HG 完成: score={:.5}, 曝光范围 {:.2}..{:.2} EV",
        r.score,
        r.exposure.iter().copied().fold(f32::INFINITY, f32::min),
        r.exposure.iter().copied().fold(f32::NEG_INFINITY, f32::max)
    );
    Ok(())
}

fn deflicker(folder: PathBuf) -> Result<()> {
    let mut seq = Sequence::load(&folder)?.ok_or_else(|| anyhow!("no project"))?;
    let opts = DeflickerOptions::from(&seq.settings);
    let luma = visual_curve_from_thumbs(&seq);
    let base = vec![0.0f32; luma.len()];
    let r = compute_deflicker(&luma, &base, &opts);
    for (im, ev) in seq.images.iter_mut().zip(r.ev.iter()) {
        im.adj.deflicker = *ev;
    }
    seq.workflow.visual_deflicker = true;
    seq.save()?;
    let mean = r.ev.iter().map(|v| v.abs()).sum::<f32>() / r.ev.len().max(1) as f32;
    println!(
        "去闪烁完成: 平均修正 {:.4} EV, 最大 {:.4} EV",
        mean,
        r.ev.iter().fold(0.0f32, |a, b| a.max(b.abs()))
    );
    Ok(())
}

fn keyframes(folder: PathBuf) -> Result<()> {
    let mut seq = Sequence::load(&folder)?.ok_or_else(|| anyhow!("no project"))?;
    let n = seq.images.len();
    if n == 0 {
        return Err(anyhow!("empty sequence"));
    }
    for im in seq.images.iter_mut() {
        im.keyframe = false;
    }
    for i in [0usize, n / 2, n - 1] {
        seq.images[i].keyframe = true;
    }
    seq.workflow.keyframes = 3;
    seq.save()?;
    println!("已标记关键帧: 1 / {} / {}", n / 2 + 1, n);
    Ok(())
}

fn hgopt(folder: PathBuf) -> Result<()> {
    let mut seq = Sequence::load(&folder)?.ok_or_else(|| anyhow!("no project"))?;
    let (ev, wb, luma) = inputs(&seq);
    let base = HGOptions {
        offset: seq.settings.hg_offset,
        stretch: seq.settings.hg_stretch,
        rotate: seq.settings.hg_rotate,
        reduce_initial: seq.settings.hg_reduce_initial,
    };
    let r = timelapse_assistant::core::holy_grail::optimize_hg(&ev, &wb, &luma, base);
    seq.settings.hg_stretch = (r.opts.stretch * 100.0).round() / 100.0;
    seq.settings.hg_rotate = (r.opts.rotate * 100.0).round() / 100.0;
    let res = apply_hg(&ev, &wb, &luma, r.opts);
    apply_to_images(&mut seq, &res);
    seq.workflow.hg_wizard = true;
    seq.save()?;
    println!(
        "HG 优化: stretch={:.2} rotate={:.2} score={:.5}",
        r.opts.stretch, r.opts.rotate, r.score
    );
    Ok(())
}

fn report(folder: PathBuf) -> Result<()> {
    let seq = Sequence::load(&folder)?.ok_or_else(|| anyhow!("no project"))?;
    let before: Vec<f32> = seq.images.iter().map(|i| i.luma).collect();
    let base: Vec<f32> = seq.images.iter().map(|i| i.adj.exposure).collect();
    let opts = DeflickerOptions::from(&seq.settings);
    let r = compute_deflicker(&before, &base, &opts);
    let after = r.after.clone();
    println!("帧数: {}", seq.images.len());
    println!("关键帧: {}", seq.keyframe_indices().len());
    println!(
        "亮度 {:.3}..{:.3}",
        before.iter().copied().fold(f32::INFINITY, f32::min),
        before.iter().copied().fold(f32::NEG_INFINITY, f32::max)
    );
    println!(
        "闪烁指数 去闪烁前 {:.2} / 后 {:.2}",
        flicker_score(&before),
        flicker_score(&after)
    );
    println!(
        "曝光修正 {:.2}..{:.2} EV",
        base.iter().copied().fold(f32::INFINITY, f32::min),
        base.iter().copied().fold(f32::NEG_INFINITY, f32::max)
    );
    Ok(())
}

fn transition(folder: PathBuf) -> Result<()> {
    let mut seq = Sequence::load(&folder)?.ok_or_else(|| anyhow!("no project"))?;
    let keys: Vec<bool> = seq.images.iter().map(|i| i.keyframe).collect();
    if !keys.iter().any(|k| *k) {
        return Err(anyhow!("没有关键帧，请先在界面中标记关键帧"));
    }
    let mode = seq.settings.transition_mode;
    let mut exposures: Vec<f32> = seq.images.iter().map(|i| i.adj.exposure).collect();
    let mut temps: Vec<f32> = seq.images.iter().map(|i| i.adj.temperature).collect();
    let mut tints: Vec<f32> = seq.images.iter().map(|i| i.adj.tint).collect();
    interpolate(&mut exposures, &keys, mode);
    interpolate(&mut temps, &keys, mode);
    interpolate(&mut tints, &keys, mode);
    for (i, im) in seq.images.iter_mut().enumerate() {
        im.adj.exposure = exposures[i];
        im.adj.temperature = temps[i];
        im.adj.tint = tints[i];
    }
    seq.workflow.auto_transition = true;
    seq.save()?;
    println!(
        "自动过渡完成: {} 个关键帧",
        keys.iter().filter(|k| **k).count()
    );
    Ok(())
}

fn ffmpeg_status() -> Result<()> {
    let st = timelapse_assistant::core::ffmpeg::status();
    println!("当前来源: {}", st.source);
    println!("当前版本: {}", st.version.unwrap_or_else(|| "—".into()));
    println!(
        "可执行文件: {}",
        st.path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "—".into())
    );
    println!(
        "组件目录: {}",
        timelapse_assistant::core::ffmpeg::components_root().display()
    );
    let versions = timelapse_assistant::core::ffmpeg::managed_versions();
    println!("已安装内置版本: {}", versions.len());
    for v in versions {
        println!("  {} ({} · {})", v.version, v.source, v.installed_at);
    }
    Ok(())
}

fn ffmpeg_import() -> Result<()> {
    let v = timelapse_assistant::core::ffmpeg::import_system()?;
    println!(
        "已导入系统 FFmpeg {} -> {}",
        v.version,
        timelapse_assistant::core::ffmpeg::components_root()
            .join("ffmpeg")
            .join(&v.dir)
            .display()
    );
    Ok(())
}

fn ffmpeg_install(url: &str) -> Result<()> {
    let url = url.to_string();
    let (tx, rx) = crossbeam_channel::unbounded();
    let handle = std::thread::spawn(move || {
        let (etx, erx) = crossbeam_channel::unbounded();
        let worker_url = url.clone();
        let worker = std::thread::spawn(move || {
            timelapse_assistant::core::ffmpeg::download_and_install(&worker_url, etx)
                .map(|v| v.version)
                .map_err(|e| format!("{e:#}"))
        });
        for ev in erx {
            let _ = tx.send(ev);
        }
        worker.join().unwrap_or_else(|_| Err("安装线程异常".into()))
    });
    for ev in rx {
        use timelapse_assistant::core::ffmpeg::InstallEvent;
        match ev {
            InstallEvent::Stage(s) => println!("{s}"),
            InstallEvent::Progress {
                stage,
                downloaded,
                total,
            } => {
                if let Some(total) = total.filter(|t| *t > 0) {
                    println!(
                        "{stage}: {:.0}% ({}/{})",
                        downloaded as f32 / total as f32 * 100.0,
                        downloaded,
                        total
                    );
                }
            }
            InstallEvent::Finished(_) => {}
        }
    }
    let version = handle
        .join()
        .map_err(|_| anyhow!("install thread panicked"))?
        .map_err(|e| anyhow!(e))?;
    println!("已安装 FFmpeg {version}");
    Ok(())
}

fn render(folder: PathBuf, out: Option<PathBuf>) -> Result<()> {
    let seq = Sequence::load(&folder)?.ok_or_else(|| anyhow!("no project"))?;
    let mut settings = seq.render.clone();
    if let Some(out) = out {
        settings.output_path = Some(out);
    }
    let (tx, rx) = crossbeam_channel::unbounded();
    let cancel = Arc::new(AtomicBool::new(false));
    let seq2 = seq.clone();
    let settings2 = settings.clone();
    let c2 = cancel.clone();
    let handle = std::thread::spawn(move || render_sequence(&seq2, &settings2, c2, tx));
    for msg in rx {
        match msg {
            RenderEvent::Stage(s) => println!("{s}"),
            RenderEvent::Progress { done, total, stage } => {
                if done % 10 == 0 || done == total {
                    println!("{stage}: {done}/{total}");
                }
            }
            RenderEvent::Finished { output, count, .. } => {
                println!("完成: {count} 帧 -> {}", output.display());
            }
            RenderEvent::Failed(e) => eprintln!("失败: {e}"),
        }
    }
    handle
        .join()
        .map_err(|_| anyhow!("render thread panicked"))??;
    Ok(())
}

fn demo(folder: PathBuf) -> Result<()> {
    use image::{ImageBuffer, Rgb};
    std::fs::create_dir_all(&folder)?;
    if std::fs::read_dir(&folder)?.next().is_some() {
        return Err(anyhow!("demo folder is not empty: {}", folder.display()));
    }
    let n = 180usize;
    let (w, h) = (640u32, 360u32);
    for i in 0..n {
        let t = i as f32 / n as f32;
        let flicker = 1.0 + 0.07 * ((i as f32) * 1.7).sin() + 0.03 * ((i as f32) * 0.37).cos();
        let ramp = 0.55 + 0.45 * t;
        let mut img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let horizon = h as f32 * 0.62;
                let sky = if (y as f32) < horizon {
                    1.0 - (y as f32) / horizon
                } else {
                    0.0
                };
                let sun_x = w as f32 * (0.1 + 0.8 * t);
                let sun_y = horizon - 40.0 + 30.0 * (t * 3.0).sin();
                let d = ((x as f32 - sun_x).powi(2) + (y as f32 - sun_y).powi(2)).sqrt();
                let sun = (1.0 - d / 55.0).max(0.0).powi(2);
                let base = 0.12 + 0.55 * sky.powf(0.6);
                let r = (base * 255.0 * ramp * flicker + sun * 240.0).clamp(0.0, 255.0);
                let g = (base * 190.0 * ramp * flicker + sun * 190.0).clamp(0.0, 255.0);
                let b = (base * 160.0 * ramp * flicker * (1.2 - t * 0.4) + sun * 110.0)
                    .clamp(0.0, 255.0);
                img.put_pixel(x, y, Rgb([r as u8, g as u8, b as u8]));
            }
        }
        img.save(folder.join(format!("DEMO_{:04}.JPG", i + 1)))?;
    }
    println!("已生成 {} 帧测试序列: {}", n, folder.display());
    println!("下一步: tlcore analyze \"{}\"", folder.display());
    Ok(())
}
