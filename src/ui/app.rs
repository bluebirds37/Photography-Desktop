use std::borrow::Cow;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Sender};
use gpui::prelude::*;
use gpui::*;
use image::{DynamicImage, RgbaImage};

use crate::core::analysis::{import_folder, thumb_abs_path, visual_curve_from_thumbs};
use crate::core::deflicker::{compute_deflicker, flicker_score, DeflickerOptions};
use crate::core::ffmpeg::{self, FfmpegStatus, InstallEvent, ManagedVersion};
use crate::core::holy_grail::{apply_hg, apply_to_images, inputs, optimize_hg, HGOptions};
use crate::core::image_pipeline::ToneTable;
use crate::core::model::{
    AdjustColumn, DeflickerAccuracy, EncoderPreset, ImageEntry, OutputCodec, OutputFormat,
    RateControl, RenderSettings, Sequence, TransitionMode,
};
use crate::core::render::preview_frame;
use crate::core::render::{default_output, render_sequence, RenderEvent};
use crate::core::transition::interpolate_adjustments;
use crate::core::xmp;

// Surfaces: a near-black studio shell with two raised levels for chrome and cards.
const CANVAS_DEEP: u32 = 0x07080a;
const CANVAS: u32 = 0x0a0c0f;
const PANEL: u32 = 0x101318;
const SURFACE: u32 = 0x161a20;
const CONTROL: u32 = 0x20252d;
const CONTROL_HOVER: u32 = 0x282e37;
const TRACK: u32 = 0x262c34;

// Lines.
const BORDER: u32 = 0x232830;
const BORDER_STRONG: u32 = 0x333a45;

// Text hierarchy, all >= 4.5:1 on PANEL.
const TEXT: u32 = 0xe9ecf1;
const TEXT_SECONDARY: u32 = 0xaab3bf;
const TEXT_MUTED: u32 = 0x808a97;
const TEXT_FAINT: u32 = 0x6f7985;

// Accent and semantics.
const ACCENT: u32 = 0x5b9dff;
const ACCENT_HOVER: u32 = 0x7db4ff;
const ACCENT_SOFT: u32 = 0x162539;
const ACCENT_LINE: u32 = 0x2c4c78;
const SELECTION: u32 = 0x1b2d45;
const ON_ACCENT: u32 = 0x07101a;
const WARNING: u32 = 0xe6b45f;
const WARNING_SOFT: u32 = 0x2a2114;
const SUCCESS: u32 = 0x5bd39a;
const SUCCESS_SOFT: u32 = 0x142820;
const DANGER: u32 = 0xef7070;
const DANGER_SOFT: u32 = 0x2e1719;

macro_rules! children {
    ($($x:expr),* $(,)?) => {
        vec![$($x.into_any_element()),*]
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Adjust,
    Tools,
    Render,
}

enum Msg {
    PickFolder,
    Folder(Option<PathBuf>),
    Imported(Result<Sequence, String>),
    Preview {
        generation: u64,
        index: usize,
        bytes: Vec<u8>,
    },
    Thumbs(Vec<Option<Vec<u8>>>),
    Progress {
        done: usize,
        total: usize,
        label: String,
    },
    Render(RenderEvent),
    RenderFailed(String),
    Toast(String),
    OutputPath(PathBuf),
    DeflickerDone {
        seq: Box<Sequence>,
        before: f32,
        after: f32,
    },
    Ffmpeg(InstallEvent),
}

pub struct TimelapseApp {
    seq: Option<Sequence>,
    selected: usize,
    start_frame: Option<usize>,
    preview: Option<Arc<Image>>,
    thumbs: Vec<Option<Arc<Image>>>,
    preview_generation: u64,
    playing: bool,
    last_play: Instant,
    tab: Tab,
    settings_open: bool,
    status: String,
    toast: Option<(String, Instant)>,
    busy: bool,
    progress: Option<(usize, usize, String)>,
    render_running: bool,
    cancel: Option<Arc<AtomicBool>>,
    tx: Sender<Msg>,
    dirty: bool,
    last_save: Instant,
    auto_keyframe: bool,
    deflicker_result: Option<(f32, f32)>,
    last_render: Option<PathBuf>,
    advanced_export: bool,
    ffmpeg_status: FfmpegStatus,
    ffmpeg_versions: Vec<ManagedVersion>,
    ffmpeg_install: Option<(String, f32)>,
    ffmpeg_busy: bool,
}

impl TimelapseApp {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (tx, rx) = unbounded();
        let mut app = Self {
            seq: None,
            selected: 0,
            start_frame: None,
            preview: None,
            thumbs: Vec::new(),
            preview_generation: 0,
            playing: false,
            last_play: Instant::now(),
            tab: Tab::Adjust,
            settings_open: false,
            status: "打开一个图像文件夹，开始延时摄影后期".into(),
            toast: None,
            busy: false,
            progress: None,
            render_running: false,
            cancel: None,
            tx: tx.clone(),
            dirty: false,
            last_save: Instant::now(),
            auto_keyframe: true,
            deflicker_result: None,
            last_render: None,
            advanced_export: true,
            ffmpeg_status: ffmpeg::status(),
            ffmpeg_versions: ffmpeg::managed_versions(),
            ffmpeg_install: None,
            ffmpeg_busy: false,
        };
        app.start_loop(cx, rx);
        app.refresh_ffmpeg();
        if ffmpeg::should_auto_install() {
            app.spawn_install(|tx| ffmpeg::auto_install(tx), cx);
        }
        let args: Vec<String> = std::env::args().skip(1).collect();
        for (i, arg) in args.iter().enumerate() {
            match arg.as_str() {
                "--tab" => {
                    app.tab = match args.get(i + 1).map(|s| s.as_str()) {
                        Some("tools") => Tab::Tools,
                        Some("render") => Tab::Render,
                        Some("settings") => {
                            app.settings_open = true;
                            Tab::Adjust
                        }
                        _ => Tab::Adjust,
                    };
                }
                "--frame" => {
                    if let Some(n) = args.get(i + 1).and_then(|s| s.parse::<usize>().ok()) {
                        app.start_frame = Some(n.saturating_sub(1));
                    }
                }
                other if !other.starts_with("--") => {
                    let path = PathBuf::from(other);
                    if path.is_dir() {
                        app.import(path, cx);
                    }
                }
                _ => {}
            }
        }
        app
    }

    fn start_loop(&mut self, cx: &mut Context<Self>, rx: Receiver<Msg>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(70))
                .await;
            let mut messages = Vec::new();
            while let Ok(m) = rx.try_recv() {
                messages.push(m);
            }
            let alive = cx
                .update(|cx| {
                    let _ = this.update(cx, |this, cx| {
                        for m in messages {
                            this.handle(m, cx);
                        }
                        this.tick();
                        cx.notify();
                    });
                })
                .is_ok();
            if !alive {
                break;
            }
        })
        .detach();
    }

    fn tick(&mut self) {
        if let Some((_, at)) = &self.toast {
            if at.elapsed() > Duration::from_secs(4) {
                self.toast = None;
            }
        }
        if self.playing {
            let fps = self
                .seq
                .as_ref()
                .map(|s| s.render.fps)
                .unwrap_or(25.0)
                .max(1.0);
            let interval = Duration::from_secs_f32(1.0 / fps);
            if self.last_play.elapsed() >= interval {
                self.last_play = Instant::now();
                if let Some(seq) = &self.seq {
                    if !seq.images.is_empty() {
                        self.selected = (self.selected + 1) % seq.images.len();
                    }
                }
            }
        }
        if self.dirty && self.last_save.elapsed() > Duration::from_secs(2) && !self.busy {
            self.save_now();
        }
    }

    fn handle(&mut self, msg: Msg, cx: &mut Context<Self>) {
        match msg {
            Msg::PickFolder => {
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let picked = rfd::FileDialog::new()
                        .set_title("选择延时摄影序列文件夹")
                        .pick_folder();
                    let _ = tx.send(Msg::Folder(picked));
                });
            }
            Msg::Folder(Some(folder)) => self.import(folder, cx),
            Msg::Folder(None) => {}
            Msg::Imported(Ok(mut seq)) => {
                self.busy = false;
                self.progress = None;
                self.deflicker_result = None;
                seq.sort_images();
                self.status = format!("已载入 {} 张图像", seq.images.len());
                let n = seq.images.len();
                self.selected = self
                    .start_frame
                    .take()
                    .unwrap_or(0)
                    .min(n.saturating_sub(1));
                self.thumbs.clear();
                let paths: Vec<Option<PathBuf>> = seq
                    .images
                    .iter()
                    .map(|im| im.thumb.as_ref().map(|t| thumb_abs_path(&seq.folder, t)))
                    .collect();
                self.seq = Some(seq);
                self.dirty = false;
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let bytes = paths
                        .into_iter()
                        .map(|p| p.and_then(|p| std::fs::read(p).ok()))
                        .collect();
                    let _ = tx.send(Msg::Thumbs(bytes));
                });
                self.request_preview(cx);
            }
            Msg::Imported(Err(e)) => {
                self.busy = false;
                self.progress = None;
                self.toast = Some((format!("载入失败: {e}"), Instant::now()));
            }
            Msg::Preview {
                generation,
                index,
                bytes,
            } => {
                if generation == self.preview_generation && index == self.selected {
                    self.preview = Some(Arc::new(Image::from_bytes(ImageFormat::Jpeg, bytes)));
                }
            }
            Msg::Thumbs(bytes) => {
                self.thumbs = bytes
                    .into_iter()
                    .map(|b| b.map(|b| Arc::new(Image::from_bytes(ImageFormat::Jpeg, b))))
                    .collect();
            }
            Msg::Progress { done, total, label } => {
                self.busy = done < total;
                self.progress = Some((done, total, label.clone()));
                self.status = format!("{label}  {done}/{total}");
            }
            Msg::Render(ev) => match ev {
                RenderEvent::Stage(s) => {
                    self.status = s.clone();
                    self.progress = Some((0, 1, s));
                }
                RenderEvent::Progress { done, total, stage } => {
                    self.status = format!("{stage}  {done}/{total}");
                    self.progress = Some((done, total, stage));
                }
                RenderEvent::Finished { output, count, .. } => {
                    self.render_running = false;
                    self.busy = false;
                    self.progress = None;
                    self.last_render = Some(output.clone());
                    self.status = format!("渲染完成：{count} 帧");
                    if let Some(seq) = &mut self.seq {
                        seq.workflow.stage = crate::core::model::WorkflowStage::Finished;
                        self.dirty = true;
                    }
                    self.toast = Some((format!("已输出 {}", output.display()), Instant::now()));
                }
                RenderEvent::Failed(e) => {
                    self.render_running = false;
                    self.busy = false;
                    self.progress = None;
                    self.toast = Some((format!("渲染失败: {e}"), Instant::now()));
                }
            },
            Msg::RenderFailed(e) => {
                self.render_running = false;
                self.busy = false;
                self.progress = None;
                self.toast = Some((format!("渲染失败: {e}"), Instant::now()));
            }
            Msg::Toast(t) => self.toast = Some((t, Instant::now())),
            Msg::OutputPath(path) => {
                if let Some(seq) = &mut self.seq {
                    seq.render.output_path = Some(path);
                }
                self.dirty = true;
                self.toast = Some(("已设置输出路径".into(), Instant::now()));
            }
            Msg::Ffmpeg(ev) => match ev {
                InstallEvent::Stage(stage) => {
                    self.ffmpeg_install = Some((stage.clone(), 0.0));
                    self.status = stage;
                }
                InstallEvent::Progress {
                    stage,
                    downloaded,
                    total,
                } => {
                    let pct = total
                        .filter(|t| *t > 0)
                        .map(|t| downloaded as f32 / t as f32)
                        .unwrap_or(0.0);
                    self.ffmpeg_install = Some((format!("{stage} {:.0}%", pct * 100.0), pct));
                }
                InstallEvent::Finished(result) => {
                    self.ffmpeg_busy = false;
                    self.ffmpeg_install = None;
                    self.refresh_ffmpeg();
                    match result {
                        Ok(v) => {
                            self.status = format!("FFmpeg {v} 已安装");
                            self.toast = Some((format!("FFmpeg {v} 已就绪"), Instant::now()));
                        }
                        Err(e) => {
                            self.refresh_ffmpeg();
                            if self.ffmpeg_status.path.is_some() {
                                self.status = "已使用系统 FFmpeg".into();
                                self.toast =
                                    Some(("已使用系统 FFmpeg".into(), Instant::now()));
                            } else {
                                self.status = "FFmpeg 安装失败".into();
                                self.toast =
                                    Some((format!("FFmpeg 安装失败: {e}"), Instant::now()));
                            }
                        }
                    }
                }
            },
            Msg::DeflickerDone { seq, before, after } => {
                self.seq = Some(*seq);
                self.deflicker_result = Some((before, after));
                self.busy = false;
                self.dirty = true;
                self.status = format!("去闪烁完成：闪烁指数 {before:.2} → {after:.2}");
                self.request_preview(cx);
            }
        }
    }

    fn import(&mut self, folder: PathBuf, _cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = format!("正在分析 {}", folder.display());
        let tx = self.tx.clone();
        let existing = Sequence::load(&folder).ok().flatten();
        std::thread::spawn(move || {
            let result = import_folder(
                &folder,
                existing,
                Some(&|done, total, name| {
                    let _ = tx.send(Msg::Progress {
                        done,
                        total,
                        label: name.to_string(),
                    });
                }),
            )
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Imported(result));
        });
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(seq) = &self.seq {
            if index >= seq.images.len() {
                return;
            }
        }
        self.selected = index;
        if !self.playing {
            self.request_preview(cx);
        }
        cx.notify();
    }

    fn request_preview(&mut self, _cx: &mut Context<Self>) {
        let Some(seq) = self.seq.clone() else {
            return;
        };
        let index = self.selected;
        if index >= seq.images.len() {
            return;
        }
        self.preview_generation = self.preview_generation.wrapping_add(1);
        let generation = self.preview_generation;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let table = ToneTable::new();
            match preview_frame(&seq, index, 1400, &table) {
                Ok(img) => {
                    if let Some(bytes) = encode_jpeg(&img, 92) {
                        let _ = tx.send(Msg::Preview {
                            generation,
                            index,
                            bytes,
                        });
                    }
                }
                Err(e) => {
                    let _ = tx.send(Msg::Toast(format!("预览失败: {e:#}")));
                }
            }
        });
    }

    fn save_now(&mut self) {
        let Some(seq) = &mut self.seq else { return };
        match seq.save() {
            Ok(()) => {
                self.dirty = false;
                self.last_save = Instant::now();
            }
            Err(e) => {
                self.toast = Some((format!("保存失败: {e:#}"), Instant::now()));
            }
        }
    }

    fn set_adjustment(&mut self, col: AdjustColumn, value: f32, cx: &mut Context<Self>) {
        let auto_key = self.auto_keyframe;
        let Some(seq) = &mut self.seq else { return };
        let Some(im) = seq.images.get_mut(self.selected) else {
            return;
        };
        col.set(&mut im.adj, value);
        if auto_key {
            im.keyframe = true;
        }
        self.dirty = true;
        self.request_preview(cx);
        cx.notify();
    }

    fn toggle_keyframe(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = &mut self.seq else { return };
        let Some(im) = seq.images.get_mut(self.selected) else {
            return;
        };
        im.keyframe = !im.keyframe;
        let on = im.keyframe;
        self.dirty = true;
        self.status = if on {
            "已标记关键帧".into()
        } else {
            "已取消关键帧".into()
        };
        cx.notify();
    }

    fn mark_end_keyframes(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = &mut self.seq else { return };
        let n = seq.images.len();
        if n == 0 {
            return;
        }
        seq.images[0].keyframe = true;
        seq.images[n - 1].keyframe = true;
        self.dirty = true;
        self.status = "已标记首尾关键帧".into();
        cx.notify();
    }

    fn apply_hg(&mut self, optimize: bool, cx: &mut Context<Self>) {
        let Some(mut seq) = self.seq.clone() else {
            return;
        };
        let (ev, wb, luma) = inputs(&seq);
        let base = HGOptions {
            offset: seq.settings.hg_offset,
            stretch: seq.settings.hg_stretch,
            rotate: seq.settings.hg_rotate,
            reduce_initial: seq.settings.hg_reduce_initial,
        };
        let opts = if optimize {
            let r = optimize_hg(&ev, &wb, &luma, base);
            seq.settings.hg_stretch = (r.opts.stretch * 100.0).round() / 100.0;
            seq.settings.hg_rotate = (r.opts.rotate * 100.0).round() / 100.0;
            self.status = format!(
                "自动优化完成：拉伸 {:.2}，旋转 {:.2}",
                seq.settings.hg_stretch, seq.settings.hg_rotate
            );
            r.opts
        } else {
            base
        };
        let result = apply_hg(&ev, &wb, &luma, opts);
        apply_to_images(&mut seq, &result);
        seq.workflow.hg_wizard = true;
        seq.settings.hg_offset = opts.offset;
        seq.settings.hg_stretch = opts.stretch;
        seq.settings.hg_rotate = opts.rotate;
        self.seq = Some(seq);
        self.dirty = true;
        self.request_preview(cx);
        cx.notify();
    }

    fn apply_deflicker(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(seq) = self.seq.clone() else { return };
        self.busy = true;
        self.status = "正在渲染视觉预览并分析闪烁…".into();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let opts = DeflickerOptions::from(&seq.settings);
            let luma = visual_curve_from_thumbs(&seq);
            let before = flicker_score(&luma);
            let base = vec![0.0f32; luma.len()];
            let r = compute_deflicker(&luma, &base, &opts);
            let after = flicker_score(&r.after);
            let mut seq = seq;
            for (im, ev) in seq.images.iter_mut().zip(r.ev.iter()) {
                im.adj.deflicker = *ev;
            }
            seq.workflow.visual_deflicker = true;
            seq.workflow.deflicker_passes = seq.settings.deflicker_passes;
            let _ = tx.send(Msg::DeflickerDone {
                seq: Box::new(seq),
                before,
                after,
            });
        });
        cx.notify();
    }

    fn clear_deflicker(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = &mut self.seq else { return };
        for im in &mut seq.images {
            im.adj.deflicker = 0.0;
        }
        seq.workflow.visual_deflicker = false;
        self.deflicker_result = None;
        self.dirty = true;
        self.request_preview(cx);
        self.status = "已清除去闪烁修正".into();
        cx.notify();
    }

    fn auto_transition(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = &mut self.seq else { return };
        let keys: Vec<bool> = seq.images.iter().map(|i| i.keyframe).collect();
        if keys.iter().filter(|k| **k).count() < 2 {
            self.toast = Some(("至少需要两个关键帧".into(), Instant::now()));
            cx.notify();
            return;
        }
        let mode = seq.settings.transition_mode;
        interpolate_adjustments(&mut seq.images, &keys, mode);
        seq.workflow.auto_transition = true;
        self.dirty = true;
        self.request_preview(cx);
        self.status = format!(
            "自动过渡完成（{}）",
            if mode == TransitionMode::Smooth {
                "平滑"
            } else {
                "线性"
            }
        );
        cx.notify();
    }

    fn export_xmp(&mut self, cx: &mut Context<Self>) {
        let Some(seq) = self.seq.clone() else { return };
        match xmp::export_all(&seq) {
            Ok(n) => {
                self.status = format!("已写入 {n} 个 XMP 边车文件");
                self.toast = Some((format!("导出了 {n} 个 XMP"), Instant::now()));
            }
            Err(e) => self.toast = Some((format!("XMP 导出失败: {e:#}"), Instant::now())),
        }
        cx.notify();
    }

    fn start_render(&mut self, cx: &mut Context<Self>) {
        if self.render_running {
            return;
        }
        if ffmpeg::find_ffmpeg().is_none() {
            self.settings_open = true;
            self.toast = Some((
                "尚未安装 FFmpeg，请先在设置中安装或导入".into(),
                Instant::now(),
            ));
            cx.notify();
            return;
        }
        self.save_now();
        let Some(seq) = self.seq.clone() else { return };
        let settings = seq.render.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.render_running = true;
        self.busy = true;
        self.status = "准备渲染…".into();
        let tx = self.tx.clone();
        let (rtx, rrx) = unbounded::<RenderEvent>();
        let forward_tx = tx.clone();
        std::thread::spawn(move || {
            for ev in rrx {
                let _ = forward_tx.send(Msg::Render(ev));
            }
        });
        std::thread::spawn(move || {
            if let Err(e) = render_sequence(&seq, &settings, cancel, rtx) {
                let _ = tx.send(Msg::RenderFailed(format!("{e:#}")));
            }
        });
        cx.notify();
    }

    fn cancel_render(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = &self.cancel {
            c.store(true, Ordering::Relaxed);
            self.status = "正在取消…".into();
            cx.notify();
        }
    }

    fn pick_output(&mut self) {
        let seq = self.seq.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            if let Some(dir) = rfd::FileDialog::new()
                .set_title("选择输出文件夹")
                .pick_folder()
            {
                if let Some(seq) = seq {
                    let name = default_output(&seq)
                        .file_name()
                        .map(|s| s.to_owned())
                        .unwrap_or_default();
                    let path = dir.join(name);
                    let _ = tx.send(Msg::OutputPath(path));
                }
            }
        });
    }

    fn open_output(&mut self) {
        let Some(path) = &self.last_render else {
            return;
        };
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
        #[cfg(windows)]
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
        #[cfg(all(not(windows), not(target_os = "macos")))]
        let _ = std::process::Command::new("xdg-open")
            .arg(path.parent().unwrap_or(path))
            .spawn();
    }
}

fn encode_jpeg(img: &RgbaImage, quality: u8) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    {
        let mut cursor = Cursor::new(&mut bytes);
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality);
        if enc
            .encode_image(&DynamicImage::ImageRgba8(img.clone()))
            .is_err()
        {
            return None;
        }
    }
    Some(bytes)
}

fn label(s: impl Into<SharedString>, size: f32, color: u32) -> Div {
    div()
        .text_size(px(size))
        .text_color(rgb(color))
        .child(s.into())
}

fn button(id: &'static str, text: &'static str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(32.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .rounded(px(6.0))
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .bg(rgb(if primary { ACCENT } else { SURFACE }))
        .text_color(rgb(if primary { ON_ACCENT } else { TEXT_SECONDARY }))
        .border_1()
        .border_color(rgb(if primary { ACCENT } else { BORDER_STRONG }))
        .hover(move |s| {
            if primary {
                s.bg(rgb(ACCENT_HOVER)).border_color(rgb(ACCENT_HOVER))
            } else {
                s.bg(rgb(CONTROL_HOVER)).text_color(rgb(TEXT))
            }
        })
        .child(text)
}

fn icon_button(
    id: &'static str,
    icon_path: &'static str,
    text: &'static str,
    primary: bool,
) -> Stateful<Div> {
    button(id, text, primary)
        .child(crate::ui::icons::icon(
            icon_path,
            13.0,
            if primary { ON_ACCENT } else { TEXT_SECONDARY },
        ))
}

fn small_button(id: &'static str, text: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(26.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(4.0))
        .rounded(px(5.0))
        .cursor_pointer()
        .text_size(px(10.5))
        .text_color(rgb(TEXT_MUTED))
        .border_1()
        .border_color(rgb(BORDER))
        .hover(|s| s.bg(rgb(CONTROL_HOVER)).text_color(rgb(TEXT)))
        .child(text)
}

fn window_button(
    id: &'static str,
    text: &'static str,
    area: WindowControlArea,
    close: bool,
) -> Stateful<Div> {
    div()
        .window_control_area(area)
        .id(id)
        .w(px(46.0))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(rgb(TEXT_MUTED))
        .hover(move |s| {
            if close {
                s.bg(rgb(0xc42b1c)).text_color(rgb(0xffffff))
            } else {
                s.bg(rgb(CONTROL_HOVER)).text_color(rgb(TEXT))
            }
        })
        .child(text)
}

fn settings_button(id: &'static str, text: &'static str, primary: bool) -> Stateful<Div> {
    button(id, text, primary).h(px(34.0)).px(px(14.0))
}

fn dot(color: u32) -> Div {
    div().size(px(6.0)).flex_none().rounded_full().bg(rgb(color))
}

fn switch(on: bool, color: u32) -> Div {
    div()
        .w(px(28.0))
        .h(px(16.0))
        .flex_none()
        .rounded_full()
        .p(px(2.0))
        .bg(rgb(if on { color } else { TRACK }))
        .flex()
        .child(
            div()
                .size(px(12.0))
                .rounded_full()
                .bg(rgb(if on { 0xf4f7fa } else { TEXT_MUTED }))
                .when(on, |el| el.ml(px(12.0))),
        )
}

fn workflow_step(text: impl Into<SharedString>, active: bool, done: bool) -> Div {
    let (dot_color, text_color) = if done {
        (SUCCESS, TEXT_SECONDARY)
    } else if active {
        (ACCENT, TEXT)
    } else {
        (TEXT_FAINT, TEXT_MUTED)
    };
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(dot(dot_color))
        .child(label(text, 10.0, text_color))
}

fn divider_v() -> Div {
    div().w(px(1.0)).h(px(12.0)).flex_none().bg(rgb(BORDER))
}

impl TimelapseApp {
    fn title_bar(&self) -> Div {
        let on_mac = cfg!(target_os = "macos");
        let seq_name = self
            .seq
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let drag = div()
            .flex_1()
            .h_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px_3()
            .when(on_mac, |el| el.pl(px(78.0)))
            .window_control_area(WindowControlArea::Drag)
            .child(label("延时摄影助手", 10.5, TEXT_FAINT))
            .when(!seq_name.is_empty(), |el| {
                el.child(div().size(px(3.0)).rounded_full().bg(rgb(TEXT_FAINT)))
                    .child(label(seq_name, 10.5, TEXT_MUTED))
            });
        div()
            .h(px(34.0))
            .flex_none()
            .flex()
            .items_center()
            .bg(rgb(CANVAS))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(drag)
            .when(!on_mac, |el| {
                el.child(window_button("win-min", "–", WindowControlArea::Min, false))
                    .child(window_button("win-max", "□", WindowControlArea::Max, false))
                    .child(window_button(
                        "win-close",
                        "✕",
                        WindowControlArea::Close,
                        true,
                    ))
            })
    }

    fn top_bar(&self, cx: &mut Context<Self>) -> Div {
        let image_count = self.seq.as_ref().map(|s| s.images.len()).unwrap_or(0);
        let keyframes = self
            .seq
            .as_ref()
            .map(|s| s.images.iter().filter(|i| i.keyframe).count())
            .unwrap_or(0);
        let wf = self
            .seq
            .as_ref()
            .map(|s| s.workflow.clone())
            .unwrap_or_default();
        let loaded = image_count > 0;
        div()
            .h(px(58.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .px_4()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .size(px(30.0))
                    .flex_none()
                    .rounded(px(8.0))
                    .bg(rgb(ACCENT_SOFT))
                    .border_1()
                    .border_color(rgb(ACCENT_LINE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(ACCENT))
                    .text_size(px(11.5))
                    .font_weight(FontWeight::BOLD)
                    .child("TL"),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(label("延时摄影助手", 14.0, TEXT).font_weight(FontWeight::SEMIBOLD))
                    .child(label(
                        "TIMELAPSE POST · RUST / GPUI",
                        8.5,
                        TEXT_FAINT,
                    )),
            )
            .child(div().w(px(10.0)))
            .child(icon_button("open-folder", crate::ui::icons::FOLDER, "打开文件夹", !loaded).on_click(
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.tx.send(Msg::PickFolder).ok();
                    cx.notify();
                }),
            ))
            .when(loaded, |el| {
                el.child(icon_button("save-project", crate::ui::icons::SAVE, "保存", false).on_click(
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.save_now();
                        cx.notify();
                    }),
                ))
                .child(icon_button("refresh-seq", crate::ui::icons::REFRESH, "重新扫描", false).on_click(
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Some(seq) = &this.seq {
                            let folder = seq.folder.clone();
                            this.import(folder, cx);
                        }
                    }),
                ))
            })
            .child(div().flex_1())
            .when(loaded, |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(14.0))
                        .child(label("流程", 9.0, TEXT_FAINT))
                        .child(workflow_step("初始化", true, true))
                        .child(workflow_step(
                            "关键帧",
                            keyframes > 0,
                            keyframes > 0,
                        ))
                        .child(workflow_step("HG 向导", wf.hg_wizard, wf.hg_wizard))
                        .child(workflow_step(
                            "自动过渡",
                            wf.auto_transition,
                            wf.auto_transition,
                        ))
                        .child(workflow_step(
                            "去闪烁",
                            wf.visual_deflicker,
                            wf.visual_deflicker,
                        ))
                        .child(workflow_step(
                            "渲染",
                            wf.stage == crate::core::model::WorkflowStage::Finished,
                            wf.stage == crate::core::model::WorkflowStage::Finished,
                        )),
                )
            })
            .child({
                let (text, color, bg, border) = if self.ffmpeg_busy {
                    ("FFmpeg 安装中".to_string(), WARNING, WARNING_SOFT, alpha(WARNING, 0.25))
                } else if let Some(v) = &self.ffmpeg_status.version {
                    (format!("FFmpeg {v}"), SUCCESS, SUCCESS_SOFT, alpha(SUCCESS, 0.22))
                } else if self.ffmpeg_status.source == "system" {
                    ("FFmpeg 系统".to_string(), SUCCESS, SUCCESS_SOFT, alpha(SUCCESS, 0.22))
                } else {
                    ("需要 FFmpeg".to_string(), DANGER, DANGER_SOFT, alpha(DANGER, 0.25))
                };
                div()
                    .id("ffmpeg-status")
                    .h(px(26.0))
                    .px(px(8.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .bg(rgb(bg))
                    .border_1()
                    .border_color(border)
                    .hover(|s| s.opacity(0.85))
                    .child(dot(color))
                    .child(label(text, 10.0, color))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.settings_open = true;
                        cx.notify();
                    }))
            })
            .when(loaded, |el| {
                el.child(
                    icon_button("render-top", crate::ui::icons::FILM, "渲染视频", true).on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.start_render(cx);
                        }),
                    ),
                )
            })
            .child(divider_v())
            .child(
                div()
                    .id("open-settings")
                    .size(px(30.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(if self.settings_open { ACCENT_LINE } else { BORDER }))
                    .bg(rgb(if self.settings_open { ACCENT_SOFT } else { SURFACE }))
                    .hover(|s| s.bg(rgb(CONTROL_HOVER)).border_color(rgb(BORDER_STRONG)))
                    .child(crate::ui::icons::icon(
                        crate::ui::icons::SETTINGS,
                        15.0,
                        if self.settings_open { ACCENT } else { TEXT_SECONDARY },
                    ))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.settings_open = !this.settings_open;
                        cx.notify();
                    })),
            )
    }

    fn ffmpeg_ok(&self) -> bool {
        self.ffmpeg_status.path.is_some()
    }

    fn refresh_ffmpeg(&mut self) {
        self.ffmpeg_versions = ffmpeg::managed_versions();
        self.ffmpeg_status = ffmpeg::status();
    }

    fn spawn_install(
        &mut self,
        task: impl FnOnce(crossbeam_channel::Sender<InstallEvent>) + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.ffmpeg_busy {
            return;
        }
        self.ffmpeg_busy = true;
        self.ffmpeg_install = Some(("准备组件".into(), 0.0));
        self.status = "正在准备 FFmpeg 组件…".into();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let (etx, erx) = unbounded();
            let worker = std::thread::spawn(move || task(etx));
            for ev in erx {
                let _ = tx.send(Msg::Ffmpeg(ev));
            }
            if worker.join().is_err() {
                let _ = tx.send(Msg::Ffmpeg(InstallEvent::Finished(Err(
                    "安装线程异常".into()
                ))));
            }
        });
        cx.notify();
    }

    fn ff_import_button(
        &self,
        id: &'static str,
        primary: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        settings_button(id, "导入系统 FFmpeg", primary)
            .flex_1()
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.spawn_install(
                    |tx| {
                        let r = ffmpeg::import_system()
                            .map(|v| v.version)
                            .map_err(|e| format!("{e:#}"));
                        let _ = tx.send(InstallEvent::Finished(r));
                    },
                    cx,
                );
            }))
    }

    fn ff_open_dir_button(&self, id: &'static str, cx: &mut Context<Self>) -> Stateful<Div> {
        settings_button(id, "打开组件目录", false)
            .flex_1()
            .on_click(cx.listener(|this, _: &ClickEvent, _, _| {
                let opener = if cfg!(target_os = "macos") {
                    "open"
                } else {
                    "explorer"
                };
                let _ = std::process::Command::new(opener)
                    .arg(ffmpeg::components_root())
                    .spawn();
                this.status = "已打开组件目录".into();
            }))
    }

    fn settings_cards(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let status = &self.ffmpeg_status;
        let source = match status.source.as_str() {
            "managed" => "内置组件",
            "system" => "系统 FFmpeg",
            "env" => "环境变量",
            _ => "未安装",
        };
        let version = status.version.clone().unwrap_or_else(|| "—".into());
        let path = status
            .path
            .as_ref()
            .map(|p| short_path(p, 30))
            .unwrap_or_else(|| "—".into());
        let progress = self.ffmpeg_install.as_ref().map(|(stage, pct)| {
            let pct = pct.clamp(0.0, 1.0);
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(label(stage.clone(), 10.0, ACCENT))
                        .child(div().flex_1())
                        .child(label(format!("{:.0}%", pct * 100.0), 10.0, TEXT_FAINT)),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(6.0))
                        .rounded_full()
                        .overflow_hidden()
                        .bg(rgb(CONTROL))
                        .child(
                            div()
                                .h_full()
                                .w(relative(pct))
                                .rounded_full()
                                .bg(rgb(ACCENT)),
                        ),
                )
        });

        let mut versions = div().flex().flex_col().gap(px(4.0));
        if self.ffmpeg_versions.is_empty() {
            versions = versions.child(label("尚未安装内置版本", 10.0, TEXT_MUTED));
        }
        for v in &self.ffmpeg_versions {
            let is_selected =
                status.source == "managed" && status.version.as_deref() == Some(v.version.as_str());
            let use_version = v.version.clone();
            let del_version = v.version.clone();
            let src = match v.source.as_str() {
                "system" => "系统导入",
                "zip" => "ZIP",
                "download" => "下载",
                other => other,
            };
            versions = versions.child(
                div()
                    .id(SharedString::from(format!("ffver-{}", v.version)))
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(rgb(if is_selected { ACCENT_LINE } else { BORDER }))
                    .bg(rgb(if is_selected { ACCENT_SOFT } else { SURFACE }))
                    .child(dot(if is_selected { ACCENT } else { TEXT_FAINT }))
                    .child(label(
                        v.version.clone(),
                        10.5,
                        if is_selected { ACCENT } else { TEXT_SECONDARY },
                    ))
                    .child(label(src.to_string(), 9.0, TEXT_FAINT))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id(SharedString::from(format!("ffuse-{}", v.version)))
                            .h(px(22.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(10.0))
                            .bg(rgb(if is_selected { SELECTION } else { CONTROL }))
                            .text_color(rgb(if is_selected { ACCENT } else { TEXT_SECONDARY }))
                            .hover(|s| s.bg(rgb(CONTROL_HOVER)).text_color(rgb(TEXT)))
                            .child(if is_selected { "使用中" } else { "使用" })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Err(e) = ffmpeg::select(Some(&use_version)) {
                                    this.toast = Some((format!("切换失败: {e:#}"), Instant::now()));
                                }
                                this.refresh_ffmpeg();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("ffdel-{}", v.version)))
                            .h(px(22.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .text_size(px(10.0))
                            .text_color(rgb(TEXT_FAINT))
                            .hover(|s| s.bg(rgb(DANGER_SOFT)).text_color(rgb(DANGER)))
                            .child("卸载")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Err(e) = ffmpeg::uninstall(&del_version) {
                                    this.toast = Some((format!("卸载失败: {e:#}"), Instant::now()));
                                }
                                this.refresh_ffmpeg();
                                cx.notify();
                            })),
                    ),
            );
        }

        vec![
            section_card(
                "视频组件 · FFmpeg",
                children![
                    div()
                        .h(px(34.0))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(px(6.0))
                        .bg(rgb(if status.path.is_some() {
                            SUCCESS_SOFT
                        } else {
                            WARNING_SOFT
                        }))
                        .child(dot(if status.path.is_some() {
                            SUCCESS
                        } else {
                            WARNING
                        }))
                        .child(label(
                            if status.path.is_some() { "已就绪" } else { "未安装" },
                            10.5,
                            if status.path.is_some() {
                                SUCCESS
                            } else {
                                WARNING
                            },
                        ))
                        .child(div().flex_1())
                        .child(label(version.clone(), 10.0, TEXT_MUTED)),
                    info_row("当前版本", version),
                    info_row("来源", source),
                    info_row("可执行文件", path),
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .when_some(progress, |el, p| el.child(div().pb_1().child(p)))
                        .when(!self.ffmpeg_busy, |el| {
                            el.child(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .when(ffmpeg::DOWNLOADABLE, |el| {
                                        el.child(
                                            settings_button("ff-mirror", "快速安装（镜像）", true)
                                                .w_full()
                                                .on_click(cx.listener(
                                                    |this, _: &ClickEvent, _, cx| {
                                                        this.spawn_install(
                                                            |tx| {
                                                                let r = ffmpeg::download_and_install(
                                                                    ffmpeg::MIRROR_URL,
                                                                    tx.clone(),
                                                                )
                                                                .map(|v| v.version)
                                                                .map_err(|e| format!("{e:#}"));
                                                                let _ = tx.send(
                                                                    InstallEvent::Finished(r),
                                                                );
                                                            },
                                                            cx,
                                                        );
                                                    },
                                                )),
                                        )
                                    })
                                    .when(!ffmpeg::DOWNLOADABLE, |el| {
                                        el.child(self.ff_import_button("ff-import-main", true, cx))
                                    })
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .gap_2()
                                            .child(
                                                settings_button("ff-zip", "从压缩包安装", false)
                                                    .flex_1()
                                                    .on_click(cx.listener(
                                                        |this, _: &ClickEvent, _, cx| {
                                                            this.spawn_install(
                                                                |tx| {
                                                                    let picked = rfd::FileDialog::new()
                                                                        .set_title("选择 FFmpeg 压缩包")
                                                                        .add_filter(
                                                                            "压缩包",
                                                                            &["zip", "tgz", "gz"],
                                                                        )
                                                                        .pick_file();
                                                                    let r = match picked {
                                                                        Some(path) => ffmpeg::install_from_archive(
                                                                            &path,
                                                                            "zip",
                                                                            Some(tx.clone()),
                                                                        )
                                                                        .map(|v| v.version)
                                                                        .map_err(|e| format!("{e:#}")),
                                                                        None => Err("已取消".into()),
                                                                    };
                                                                    let _ = tx.send(
                                                                        InstallEvent::Finished(r),
                                                                    );
                                                                },
                                                                cx,
                                                            );
                                                        },
                                                    )),
                                            )
                                            .child(self.ff_open_dir_button("ff-open", cx)),
                                    )
                                    .when(ffmpeg::DOWNLOADABLE, |el| {
                                        el.child(
                                            div()
                                                .w_full()
                                                .flex()
                                                .gap_2()
                                                .child(
                                                    settings_button("ff-download", "下载最新版", false)
                                                        .flex_1()
                                                        .on_click(cx.listener(
                                                            |this, _: &ClickEvent, _, cx| {
                                                                this.spawn_install(
                                                                    |tx| {
                                                                        let r = ffmpeg::download_and_install(
                                                                            ffmpeg::LATEST_URL,
                                                                            tx.clone(),
                                                                        )
                                                                        .map(|v| v.version)
                                                                        .map_err(|e| format!("{e:#}"));
                                                                        let _ = tx.send(
                                                                            InstallEvent::Finished(r),
                                                                        );
                                                                    },
                                                                    cx,
                                                                );
                                                            },
                                                        )),
                                                )
                                                .child(self.ff_import_button("ff-import", false, cx)),
                                        )
                                    })
                                    .when(!ffmpeg::DOWNLOADABLE, |el| {
                                        el.child(label(
                                            "当前平台使用系统 FFmpeg；也可导入对应平台的压缩包。",
                                            9.5,
                                            TEXT_FAINT,
                                        ))
                                    }),
                            )
                        }),
                ],
            )
            .into_any_element(),
            section_card("已安装版本", children![versions]).into_any_element(),
            section_card(
                "使用系统 FFmpeg",
                children![
                    label(
                        "清除内置版本选择，渲染时回退到 PATH 中的 ffmpeg。",
                        10.0,
                        TEXT_MUTED
                    ),
                    settings_button("ff-system", "使用系统 FFmpeg", false).w_full().on_click(cx.listener(
                        |this, _: &ClickEvent, _, cx| {
                            if let Err(e) = ffmpeg::select(None) {
                                this.toast = Some((format!("切换失败: {e:#}"), Instant::now()));
                            }
                            this.refresh_ffmpeg();
                            cx.notify();
                        },
                    )),
                ],
            )
            .into_any_element(),
            section_card(
                "组件路径",
                children![
                    info_row("组件根目录", short_path(&ffmpeg::components_root(), 30)),
                    info_row("FFmpeg 目录", short_path(&ffmpeg::ffmpeg_root(), 30)),
                    label(
                        "首次运行会自动导入系统 FFmpeg；也可手动从压缩包导入或切换版本。",
                        10.0,
                        TEXT_MUTED
                    ),
                ],
            )
            .into_any_element(),
        ]
    }

    fn settings_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut cards = self.settings_cards(cx).into_iter();
        let component = cards.next();
        let side: Vec<AnyElement> = cards.collect();

        div()
            .id("settings-backdrop")
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .w_full()
            .h_full()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(alpha(0x05070a, 0.62))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.settings_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .id("settings-dialog")
                    .w(px(712.0))
                    .max_h(px(748.0))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .rounded(px(14.0))
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(BORDER_STRONG))
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.stop_propagation()))
                    .child(
                        div()
                            .h(px(60.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(11.0))
                            .px_5()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .size(px(28.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(7.0))
                                    .bg(rgb(ACCENT_SOFT))
                                    .border_1()
                                    .border_color(rgb(ACCENT_LINE))
                                    .child(crate::ui::icons::icon(
                                        crate::ui::icons::SETTINGS,
                                        14.0,
                                        ACCENT,
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(1.0))
                                    .child(
                                        label("设置", 14.0, TEXT)
                                            .font_weight(FontWeight::SEMIBOLD),
                                    )
                                    .child(label("应用偏好与 FFmpeg 组件管理", 10.0, TEXT_MUTED)),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id("settings-close")
                                    .size(px(26.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(6.0))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(CONTROL_HOVER)))
                                    .child(crate::ui::icons::icon(
                                        crate::ui::icons::CLOSE,
                                        13.0,
                                        TEXT_MUTED,
                                    ))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.settings_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-scroll")
                            .flex_1()
                            .min_h(px(0.0))
                            .overflow_y_scroll()
                            .p_4()
                            .flex()
                            .items_start()
                            .gap_3()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .when_some(component, |el, card| el.child(card)),
                            )
                            .child(
                                div()
                                    .w(px(322.0))
                                    .flex_none()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .children(side),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn left_panel(&self, cx: &mut Context<Self>) -> Div {
        let (n, name, folder, keys) = match &self.seq {
            Some(s) => (
                s.images.len(),
                s.name.clone(),
                s.folder.display().to_string(),
                s.images.iter().filter(|i| i.keyframe).count(),
            ),
            None => (0, "未载入序列".to_string(), String::new(), 0),
        };
        let entity = cx.entity();
        let list = if n > 0 {
            uniform_list(
                "image-list",
                n,
                cx.processor(
                    move |this: &mut TimelapseApp,
                          range: std::ops::Range<usize>,
                          _window: &mut Window,
                          _cx: &mut Context<TimelapseApp>| {
                        let mut rows = Vec::with_capacity(range.len());
                        let entity = entity.clone();
                        if let Some(seq) = &this.seq {
                            for ix in range {
                                let im = &seq.images[ix];
                                let selected = ix == this.selected;
                                let thumb = this.thumbs.get(ix).and_then(|t| t.clone());
                                let mut row = div()
                                    .id(("row", ix))
                                    .relative()
                                    .h(px(56.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.0))
                                    .pl(px(10.0))
                                    .pr(px(8.0))
                                    .rounded(px(6.0))
                                    .cursor_pointer()
                                    .bg(rgb(if selected { SELECTION } else { PANEL }))
                                    .hover(move |s| {
                                        if selected {
                                            s
                                        } else {
                                            s.bg(rgb(SURFACE))
                                        }
                                    })
                                    .when(selected, |el| {
                                        el.child(
                                            div()
                                                .absolute()
                                                .left(px(0.0))
                                                .top(px(12.0))
                                                .w(px(2.0))
                                                .h(px(32.0))
                                                .rounded_full()
                                                .bg(rgb(ACCENT)),
                                        )
                                    })
                                    .child(
                                        div()
                                            .w(px(60.0))
                                            .h(px(40.0))
                                            .flex_none()
                                            .rounded(px(4.0))
                                            .bg(rgb(CANVAS_DEEP))
                                            .border_1()
                                            .border_color(rgb(if selected {
                                                ACCENT_LINE
                                            } else {
                                                BORDER
                                            }))
                                            .overflow_hidden()
                                            .when_some(thumb, |el, image| {
                                                el.child(
                                                    img(ImageSource::Image(image))
                                                        .w_full()
                                                        .h_full()
                                                        .object_fit(ObjectFit::Cover),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .flex()
                                            .flex_col()
                                            .gap(px(3.0))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(5.0))
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w(px(0.0))
                                                            .truncate()
                                                            .child(label(
                                                                im.file_name.clone(),
                                                                11.5,
                                                                if selected {
                                                                    TEXT
                                                                } else {
                                                                    TEXT_SECONDARY
                                                                },
                                                            )),
                                                    )
                                                    .when(im.keyframe, |el| {
                                                        el.child(crate::ui::icons::icon(
                                                            crate::ui::icons::KEYFRAME,
                                                            8.0,
                                                            WARNING,
                                                        ))
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(8.0))
                                                    .child(label(
                                                        format!("{:+.2} EV", im.adj.total_ev()),
                                                        9.5,
                                                        if im.adj.total_ev().abs() > 1e-4 {
                                                            WARNING
                                                        } else {
                                                            TEXT_FAINT
                                                        },
                                                    ))
                                                    .child(label(
                                                        format!("L {:.2}", im.luma),
                                                        9.5,
                                                        TEXT_FAINT,
                                                    )),
                                            ),
                                    )
                                    .child(label(
                                        format!("#{:04}", ix + 1),
                                        9.0,
                                        if selected { TEXT_MUTED } else { TEXT_FAINT },
                                    ));
                                if im.error.is_some() {
                                    row = row.child(crate::ui::icons::icon(
                                        crate::ui::icons::WARNING,
                                        12.0,
                                        DANGER,
                                    ));
                                }
                                let entity2 = entity.clone();
                                row = row.on_click(move |_, _, cx| {
                                    entity2.update(cx, |this: &mut TimelapseApp, cx| {
                                        this.select(ix, cx)
                                    });
                                });
                                rows.push(row);
                            }
                        }
                        rows
                    },
                ),
            )
            .h_full()
            .into_any_element()
        } else {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.0))
                .px_6()
                .child(
                    div()
                        .size(px(48.0))
                        .rounded(px(12.0))
                        .bg(rgb(SURFACE))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(crate::ui::icons::icon(
                            crate::ui::icons::FOLDER,
                            22.0,
                            TEXT_FAINT,
                        )),
                )
                .child(label("未载入序列", 12.5, TEXT_SECONDARY).font_weight(FontWeight::MEDIUM))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(label("打开包含照片序列的文件夹，", 10.0, TEXT_FAINT))
                        .child(label("即可开始曝光校正与渲染。", 10.0, TEXT_FAINT)),
                )
                .child(
                    icon_button("empty-open", crate::ui::icons::FOLDER, "打开文件夹", false)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.tx.send(Msg::PickFolder).ok();
                            cx.notify();
                        })),
                )
                .into_any_element()
        };

        div()
            .w(px(284.0))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(label("序列", 9.0, TEXT_FAINT))
                            .when(n > 0, |el| {
                                el.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.0))
                                        .child(dot(SUCCESS))
                                        .child(label(format!("{n} 帧"), 9.5, TEXT_MUTED)),
                                )
                            }),
                    )
                    .when(n > 0, |el| {
                        el.child(
                            div()
                                .truncate()
                                .child(label(name, 13.5, TEXT).font_weight(FontWeight::SEMIBOLD)),
                        )
                    })
                    .when(!folder.is_empty(), |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .child(
                                    label(
                                        short_path(std::path::Path::new(&folder), 44),
                                        9.5,
                                        TEXT_FAINT,
                                    )
                                    .truncate(),
                                ),
                        )
                    })
                    .when(n > 0, |el| {
                        el.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(label(format!("{keys} 个关键帧"), 9.5, TEXT_FAINT))
                                .child(label("·", 9.5, TEXT_FAINT))
                                .child(label("单击选择帧", 9.5, TEXT_FAINT)),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .px_2()
                    .pt_2()
                    .child(list),
            )
    }

    fn center_panel(&self, cx: &mut Context<Self>) -> Div {
        let seq = self.seq.as_ref();
        let selected = self.selected;
        let current: Option<&ImageEntry> = seq.and_then(|s| s.images.get(selected));
        let preview_el = if self.playing {
            self.thumbs
                .get(self.selected)
                .and_then(|t| t.clone())
                .map(|image| {
                    img(ImageSource::Image(image))
                        .w_full()
                        .h_full()
                        .object_fit(ObjectFit::Contain)
                        .into_any_element()
                })
                .unwrap_or_else(|| placeholder_preview().into_any_element())
        } else if let Some(p) = &self.preview {
            img(ImageSource::Image(p.clone()))
                .w_full()
                .h_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        } else {
            placeholder_preview().into_any_element()
        };

        let overlay = current.map(|im| {
            div()
                .absolute()
                .top(px(12.0))
                .left(px(12.0))
                .right(px(12.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .h(px(24.0))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .rounded(px(5.0))
                        .bg(alpha(0x0b0e12, 0.82))
                        .border_1()
                        .border_color(alpha(0xffffff, 0.08))
                        .child(label(format!("#{:04}", selected + 1), 10.5, TEXT)),
                )
                .child(
                    div()
                        .h(px(24.0))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .rounded(px(5.0))
                        .bg(alpha(0x0b0e12, 0.82))
                        .border_1()
                        .border_color(alpha(0xffffff, 0.08))
                        .child(label(im.file_name.clone(), 10.5, TEXT_MUTED)),
                )
                .when(im.keyframe, |el| {
                    el.child(
                        div()
                            .h(px(24.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .rounded(px(5.0))
                            .bg(alpha(WARNING_SOFT, 0.88))
                            .border_1()
                            .border_color(alpha(WARNING, 0.22))
                            .child(crate::ui::icons::icon(
                                crate::ui::icons::KEYFRAME,
                                9.0,
                                WARNING,
                            ))
                            .child(label("关键帧", 10.0, WARNING)),
                    )
                })
                .child(div().flex_1())
                .child(
                    div()
                        .h(px(24.0))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(px(5.0))
                        .bg(alpha(0x0b0e12, 0.82))
                        .border_1()
                        .border_color(alpha(0xffffff, 0.08))
                        .child(label(
                            format!("{:+.2} EV", im.adj.total_ev()),
                            10.0,
                            if im.adj.total_ev().abs() > 1e-4 {
                                WARNING
                            } else {
                                TEXT_MUTED
                            },
                        ))
                        .child(label(format!("L {:.3}", im.luma), 10.0, TEXT_MUTED)),
                )
        });

        let play_icon = if self.playing {
            crate::ui::icons::PAUSE
        } else {
            crate::ui::icons::PLAY
        };
        let transport = div()
            .h(px(50.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px_4()
            .bg(rgb(PANEL))
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(
                icon_button("prev", crate::ui::icons::PREV, "上一帧", false)
                    .h(px(30.0))
                    .px(px(8.0))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        if this.selected > 0 {
                            let i = this.selected - 1;
                            this.select(i, cx);
                        }
                    })),
            )
            .child(
                icon_button("play", play_icon, if self.playing { "暂停" } else { "播放" }, true)
                    .h(px(30.0))
                    .px(px(12.0))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.playing = !this.playing;
                        this.last_play = Instant::now();
                        if !this.playing {
                            this.request_preview(cx);
                        }
                        cx.notify();
                    })),
            )
            .child(
                icon_button("next", crate::ui::icons::NEXT, "下一帧", false)
                    .h(px(30.0))
                    .px(px(8.0))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Some(s) = &this.seq {
                            if this.selected + 1 < s.images.len() {
                                let i = this.selected + 1;
                                this.select(i, cx);
                            }
                        }
                    })),
            )
            .child(divider_v())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .when(current.is_some(), |el| {
                        let total = self.seq.as_ref().map(|s| s.images.len()).unwrap_or(0);
                        el.child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap(px(4.0))
                                .child(label(
                                    format!("{:04}", selected + 1),
                                    13.0,
                                    TEXT,
                                ).font_weight(FontWeight::MEDIUM))
                                .child(label(format!("/ {total:04}"), 10.5, TEXT_FAINT)),
                        )
                    }),
            )
            .child(div().flex_1())
            .when(current.is_some(), |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            icon_button("keyframe", crate::ui::icons::KEYFRAME, "标记关键帧", false)
                                .h(px(30.0))
                                .px(px(8.0))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.toggle_keyframe(cx)
                                })),
                        )
                        .child(divider_v())
                        .child(label("关键帧", 9.5, TEXT_FAINT))
                        .child(
                            div()
                                .id("prev-kf")
                                .size(px(28.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.0))
                                .bg(rgb(SURFACE))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .cursor_pointer()
                                .hover(|s| {
                                    s.bg(rgb(CONTROL_HOVER))
                                        .border_color(rgb(BORDER_STRONG))
                                })
                                .child(crate::ui::icons::icon(
                                    crate::ui::icons::PREV,
                                    13.0,
                                    TEXT_MUTED,
                                ))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    if let Some(s) = &this.seq {
                                        if let Some(i) = s.prev_keyframe(this.selected) {
                                            this.select(i, cx);
                                        }
                                    }
                                })),
                        )
                        .child(
                            div()
                                .id("next-kf")
                                .size(px(28.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.0))
                                .bg(rgb(SURFACE))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .cursor_pointer()
                                .hover(|s| {
                                    s.bg(rgb(CONTROL_HOVER))
                                        .border_color(rgb(BORDER_STRONG))
                                })
                                .child(crate::ui::icons::icon(
                                    crate::ui::icons::NEXT,
                                    13.0,
                                    TEXT_MUTED,
                                ))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    if let Some(s) = &this.seq {
                                        if let Some(i) = s.next_keyframe(this.selected) {
                                            this.select(i, cx);
                                        }
                                    }
                                })),
                        ),
                )
            });

        let graph = if let Some(seq) = seq {
            crate::ui::graph::graph_view(seq, selected)
        } else {
            div()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .child(label("载入序列后显示亮度曲线", 10.5, TEXT_FAINT))
                .into_any_element()
        };
        let (luma_min, luma_max) = seq
            .map(|s| {
                s.images
                    .iter()
                    .map(|i| i.luma)
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                        (lo.min(v), hi.max(v))
                    })
            })
            .unwrap_or((0.0, 0.0));

        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(CANVAS_DEEP))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_6()
                    .child(preview_el)
                    .children(overlay),
            )
            .child(
                div()
                    .h(px(152.0))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .bg(rgb(CANVAS))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .h(px(32.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(14.0))
                            .px_4()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                label("亮度 / 曝光曲线", 10.5, TEXT_SECONDARY)
                                    .font_weight(FontWeight::MEDIUM),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .child(
                                        div()
                                            .w(px(10.0))
                                            .h(px(2.0))
                                            .rounded_full()
                                            .bg(rgb(ACCENT)),
                                    )
                                    .child(label("亮度", 9.5, TEXT_MUTED)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .child(
                                        div()
                                            .w(px(10.0))
                                            .h(px(2.0))
                                            .rounded_full()
                                            .bg(rgb(WARNING)),
                                    )
                                    .child(label("曝光修正", 9.5, TEXT_MUTED)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(crate::ui::icons::icon(
                                        crate::ui::icons::KEYFRAME,
                                        8.0,
                                        WARNING,
                                    ))
                                    .child(label("关键帧", 9.5, TEXT_MUTED)),
                            )
                            .child(div().flex_1())
                            .when(seq.is_some(), |el| {
                                el.child(label(
                                    format!("L {luma_min:.3} – {luma_max:.3}"),
                                    9.5,
                                    TEXT_FAINT,
                                ))
                            }),
                    )
                    .child(div().flex_1().min_h(px(0.0)).px_3().py_2().child(graph)),
            )
            .child(transport)
    }

    fn right_panel(&self, cx: &mut Context<Self>) -> Div {
        let tab_button = |id: &'static str, text: &'static str, t: Tab| {
            let active = self.tab == t;
            div()
                .id(id)
                .relative()
                .flex_1()
                .h(px(40.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_size(px(11.5))
                .font_weight(if active {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .text_color(rgb(if active { TEXT } else { TEXT_MUTED }))
                .hover(|s| s.text_color(rgb(TEXT_SECONDARY)))
                .child(text)
                .when(active, |el| {
                    el.child(
                        div()
                            .absolute()
                            .left(px(14.0))
                            .right(px(14.0))
                            .bottom(px(0.0))
                            .h(px(2.0))
                            .rounded_full()
                            .bg(rgb(ACCENT)),
                    )
                })
        };

        let content = match self.tab {
            Tab::Adjust => self.adjust_tab(cx),
            Tab::Tools => self.tools_tab(cx),
            Tab::Render => self.render_tab(cx),
        };

        div()
            .w(px(360.0))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_l_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h(px(41.0))
                    .flex_none()
                    .flex()
                    .px(px(6.0))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(tab_button("tab-adjust", "调整", Tab::Adjust).on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.tab = Tab::Adjust;
                            cx.notify();
                        }),
                    ))
                    .child(tab_button("tab-tools", "工具", Tab::Tools).on_click(cx.listener(
                        |this, _: &ClickEvent, _, cx| {
                            this.tab = Tab::Tools;
                            cx.notify();
                        },
                    )))
                    .child(tab_button("tab-render", "渲染", Tab::Render).on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.tab = Tab::Render;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .id("right-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(content),
            )
    }

    fn adjust_tab(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(seq) = &self.seq else {
            return vec![empty_hint("载入序列后可调整曝光、色彩与关键帧")];
        };
        let Some(im) = seq.images.get(self.selected) else {
            return vec![];
        };
        let mut out = Vec::new();
        out.push(
            div()
                .w_full()
                .min_w(px(0.0))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    label("单帧调整", 14.0, TEXT)
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .when(im.keyframe, |el| {
                                    el.child(
                                        div()
                                            .h(px(20.0))
                                            .px(px(6.0))
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(WARNING_SOFT))
                                            .child(crate::ui::icons::icon(
                                                crate::ui::icons::KEYFRAME,
                                                8.0,
                                                WARNING,
                                            ))
                                            .child(label("关键帧", 9.0, WARNING)),
                                    )
                                }),
                        )
                        .child(
                            small_button("reset-adj", "重置")
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    if let Some(seq) = &mut this.seq {
                                        if let Some(im) = seq.images.get_mut(this.selected) {
                                            im.adj = Default::default();
                                        }
                                    }
                                    this.dirty = true;
                                    this.request_preview(cx);
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .min_w(px(0.0))
                                .child(label(
                                    format!("帧 {:04}", self.selected + 1),
                                    9.0,
                                    TEXT_FAINT,
                                ))
                                .child(
                                    div()
                                        .truncate()
                                        .child(label(im.file_name.clone(), 11.5, TEXT_SECONDARY)),
                                ),
                        )
                        .child(
                            div()
                                .id("auto-key-toggle")
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(px(8.0))
                                .cursor_pointer()
                                .child(label("自动关键帧", 9.5, TEXT_MUTED))
                                .child(switch(self.auto_keyframe, ACCENT))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.auto_keyframe = !this.auto_keyframe;
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        );
        out.push(
            section_card(
                "曝光与白平衡",
                children![
                    self.slider_row(cx, AdjustColumn::Exposure),
                    self.slider_row(cx, AdjustColumn::Temperature),
                    self.slider_row(cx, AdjustColumn::Tint),
                ],
            )
            .into_any_element(),
        );
        out.push(
            section_card(
                "影调",
                children![
                    self.slider_row(cx, AdjustColumn::Contrast),
                    self.slider_row(cx, AdjustColumn::Highlights),
                    self.slider_row(cx, AdjustColumn::Shadows),
                    self.slider_row(cx, AdjustColumn::Whites),
                    self.slider_row(cx, AdjustColumn::Blacks),
                ],
            )
            .into_any_element(),
        );
        out.push(
            section_card(
                "色彩",
                children![
                    self.slider_row(cx, AdjustColumn::Saturation),
                    self.slider_row(cx, AdjustColumn::Vibrance),
                ],
            )
            .into_any_element(),
        );
        let keys = seq.images.iter().filter(|i| i.keyframe).count();
        let duration = seq.images.len() as f32 / seq.render.fps.max(1.0);
        let deflicker_applied = seq
            .images
            .iter()
            .filter(|i| i.adj.deflicker.abs() > 1e-5)
            .count();
        let flicker = self
            .deflicker_result
            .map(|(a, b)| format!("{a:.2} → {b:.2}"))
            .unwrap_or_else(|| {
                if deflicker_applied > 0 {
                    format!("已应用（{deflicker_applied} 帧）")
                } else {
                    "未执行".to_string()
                }
            });
        out.push(
            section_card(
                "序列概览",
                children![
                    info_row("图像数量", format!("{} 帧", seq.images.len())),
                    info_row("关键帧", format!("{keys} 个")),
                    info_row("时长 @ {:.0}fps", format!("{:.1} 秒", duration)),
                    info_row(
                        "平均亮度",
                        format!(
                            "{:.3}",
                            seq.images.iter().map(|i| i.luma).sum::<f32>()
                                / seq.images.len().max(1) as f32
                        )
                    ),
                    info_row("去闪烁前后", flicker),
                    info_row(
                        "输出",
                        format!(
                            "{}×{} · {}",
                            seq.render.width,
                            seq.render.height,
                            match seq.render.codec {
                                OutputCodec::H264 => "H.264",
                                OutputCodec::H265 => "H.265",
                                OutputCodec::ProRes => "ProRes",
                            }
                        )
                    ),
                    info_row(
                        "FFmpeg",
                        if self.ffmpeg_ok() {
                            "就绪"
                        } else {
                            "未找到"
                        }
                    ),
                ],
            )
            .into_any_element(),
        );
        out
    }

    fn slider_row(&self, cx: &mut Context<Self>, col: AdjustColumn) -> Div {
        let (min, max, step) = col.range();
        let value = self
            .seq
            .as_ref()
            .and_then(|s| s.images.get(self.selected))
            .map(|im| col.get(&im.adj))
            .unwrap_or(0.0);
        let segments = 48;
        let pos = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let active = (pos * (segments - 1) as f32).round() as usize;
        let modified = value.abs() > 1e-4;
        let mut track = div()
            .relative()
            .flex()
            .items_center()
            .h(px(20.0))
            .rounded_full()
            .overflow_hidden()
            .bg(rgb(TRACK))
            .flex_1();
        for i in 0..segments {
            let v = min + (max - min) * i as f32 / (segments - 1) as f32;
            let filled = i <= active;
            track = track.child(
                div()
                    .id(("seg", (col as usize) * 64 + i))
                    .h_full()
                    .flex_1()
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .child(div().w_full().h(px(6.0)).bg(rgb(if i == active {
                        ACCENT_HOVER
                    } else if filled {
                        ACCENT
                    } else {
                        TRACK
                    })))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_adjustment(col, v, cx);
                    })),
            );
        }
        track = track
            .child(
                div()
                    .absolute()
                    .left(relative(0.25))
                    .top(px(5.0))
                    .w(px(1.0))
                    .h(px(10.0))
                    .bg(rgb(BORDER_STRONG)),
            )
            .child(
                div()
                    .absolute()
                    .left(relative(0.5))
                    .top(px(5.0))
                    .w(px(1.0))
                    .h(px(10.0))
                    .bg(rgb(BORDER_STRONG)),
            )
            .child(
                div()
                    .absolute()
                    .left(relative(0.75))
                    .top(px(5.0))
                    .w(px(1.0))
                    .h(px(10.0))
                    .bg(rgb(BORDER_STRONG)),
            )
            .child(
                div()
                    .absolute()
                    .left(relative(pos))
                    .top(px(4.0))
                    .ml(px(-6.0))
                    .size(px(12.0))
                    .rounded_full()
                    .bg(rgb(0xf4f7fa))
                    .border_2()
                    .border_color(rgb(ACCENT)),
            );
        let display = if col == AdjustColumn::Exposure {
            format!("{value:+.2}")
        } else {
            format!("{value:+.0}")
        };
        let down = (value - step).max(min);
        let up = (value + step).min(max);
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .py(px(2.0))
            .child(
                div()
                    .w(px(54.0))
                    .flex_none()
                    .child(label(col.label(), 10.5, TEXT_MUTED)),
            )
            .child(track)
            .child(
                div()
                    .id(("dec", col as usize))
                    .size(px(20.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .text_color(rgb(TEXT_MUTED))
                    .hover(|s| s.bg(rgb(CONTROL_HOVER)).text_color(rgb(TEXT)))
                    .child("−")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_adjustment(col, down, cx);
                    })),
            )
            .child(
                div()
                    .id(("inc", col as usize))
                    .size(px(20.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .text_color(rgb(TEXT_MUTED))
                    .hover(|s| s.bg(rgb(CONTROL_HOVER)).text_color(rgb(TEXT)))
                    .child("+")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_adjustment(col, up, cx);
                    })),
            )
            .child(
                div()
                    .w(px(40.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(label(
                        display,
                        10.5,
                        if modified { TEXT } else { TEXT_FAINT },
                    )),
            )
    }

    fn tools_tab(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.seq.is_none() {
            return vec![empty_hint("载入序列后可使用 HG 向导、自动过渡与去闪烁")];
        }
        vec![
            self.hg_card(cx),
            self.transition_card(cx),
            self.deflicker_card(cx),
        ]
    }

    fn hg_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let seq = self.seq.as_ref().unwrap();
        let s = seq.settings.clone();
        let keys = seq.images.iter().filter(|i| i.keyframe).count();
        section_card_meta(
            "Holy Grail 向导",
            format!("{keys} 关键帧"),
            children![
                label("补偿日转夜拍摄时的相机曝光跳变", 10.0, TEXT_MUTED),
                self.setting_slider(cx, Setting::HgOffset, "偏移", s.hg_offset, -5.0, 5.0, 0.05),
                self.setting_slider(
                    cx,
                    Setting::HgStretch,
                    "拉伸",
                    s.hg_stretch,
                    -1.5,
                    1.5,
                    0.02
                ),
                self.setting_slider(cx, Setting::HgRotate, "旋转", s.hg_rotate, -4.0, 4.0, 0.04),
                self.setting_slider(
                    cx,
                    Setting::HgReduce,
                    "初始平滑",
                    s.hg_reduce_initial,
                    0.0,
                    1.0,
                    0.01
                ),
                div()
                    .flex()
                    .gap_2()
                    .child(
                        button("hg-auto", "自动优化", true)
                            .flex_1()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.apply_hg(true, cx);
                            })),
                    )
                    .child(
                        button("hg-apply", "应用", false)
                            .flex_1()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.apply_hg(false, cx);
                            })),
                    ),
                div()
                    .flex()
                    .gap_2()
                    .child(
                        button("hg-ends", "标记首尾关键帧", false)
                            .flex_1()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.mark_end_keyframes(cx);
                            })),
                    )
                    .child(
                        button("hg-reset", "重置", false)
                            .flex_1()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Some(seq) = &mut this.seq {
                                    seq.settings.hg_offset = 0.0;
                                    seq.settings.hg_stretch = 0.0;
                                    seq.settings.hg_rotate = 0.0;
                                    seq.settings.hg_reduce_initial = 0.5;
                                }
                                cx.notify();
                            })),
                    ),
            ],
        )
        .into_any_element()
    }

    fn transition_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self
            .seq
            .as_ref()
            .map(|s| s.settings.transition_mode)
            .unwrap_or_default();
        section_card(
            "关键帧自动过渡",
            children![
                label("在关键帧之间插值曝光、色温与全部影调参数", 10.0, TEXT_MUTED),
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(
                        choice_button("mode-smooth", "平滑", mode == TransitionMode::Smooth)
                            .flex_1()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Some(seq) = &mut this.seq {
                                    seq.settings.transition_mode = TransitionMode::Smooth;
                                }
                                this.dirty = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        choice_button("mode-linear", "线性", mode == TransitionMode::Linear)
                            .flex_1()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Some(seq) = &mut this.seq {
                                    seq.settings.transition_mode = TransitionMode::Linear;
                                }
                                this.dirty = true;
                                cx.notify();
                            })),
                    ),
                button("transition-apply", "应用自动过渡", true)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.auto_transition(cx),)),
            ],
        )
        .into_any_element()
    }

    fn deflicker_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let seq = self.seq.as_ref().unwrap();
        let s = seq.settings.clone();
        let acc = s.deflicker_accuracy;
        let result_text = self
            .deflicker_result
            .map(|(a, b)| format!("闪烁指数 {:.2} → {:.2}", a, b))
            .unwrap_or_else(|| "尚未执行去闪烁".into());
        section_card(
            "视觉去闪烁",
            children![
                self.setting_slider(
                    cx,
                    Setting::DeflickerSmooth,
                    "平滑度",
                    s.deflicker_smoothing,
                    0.02,
                    1.0,
                    0.01
                ),
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        label("精度", 10.5, TEXT_MUTED)
                            .w(px(54.0))
                            .flex_none(),
                    )
                    .child(
                        acc_button(cx, "粗略", DeflickerAccuracy::Coarse, acc).flex_1(),
                    )
                    .child(
                        acc_button(cx, "标准", DeflickerAccuracy::Normal, acc).flex_1(),
                    )
                    .child(acc_button(cx, "精细", DeflickerAccuracy::Fine, acc).flex_1()),
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .h(px(28.0))
                    .child(
                        label("多遍", 10.5, TEXT_MUTED)
                            .w(px(54.0))
                            .flex_none(),
                    )
                    .child(stepper_button(
                        "passes-dec",
                        "−",
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Some(seq) = &mut this.seq {
                                seq.settings.deflicker_passes =
                                    seq.settings.deflicker_passes.saturating_sub(1).max(1);
                            }
                            cx.notify();
                        }),
                    ))
                    .child(
                        div()
                            .w(px(40.0))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .child(label(
                                format!("{} 遍", s.deflicker_passes),
                                10.5,
                                TEXT_SECONDARY,
                            )),
                    )
                    .child(stepper_button(
                        "passes-inc",
                        "+",
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Some(seq) = &mut this.seq {
                                seq.settings.deflicker_passes =
                                    (seq.settings.deflicker_passes + 1).min(4);
                            }
                            cx.notify();
                        }),
                    ))
                    .child(div().flex_1())
                    .child(label("恒定模式", 10.5, TEXT_SECONDARY))
                    .child(
                        div()
                            .id("const-toggle")
                            .cursor_pointer()
                            .child(switch(s.deflicker_constant, ACCENT))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Some(seq) = &mut this.seq {
                                    seq.settings.deflicker_constant =
                                        !seq.settings.deflicker_constant;
                                }
                                this.dirty = true;
                                cx.notify();
                            })),
                    ),
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(dot(if self.deflicker_result.is_some() {
                        SUCCESS
                    } else {
                        TEXT_FAINT
                    }))
                    .child(label(result_text, 10.0, TEXT_MUTED)),
                div()
                    .flex()
                    .gap_2()
                    .child(
                        button("deflicker-run", "分析并去闪烁", true)
                            .flex_1()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.apply_deflicker(cx);
                            })),
                    )
                    .child(button("deflicker-clear", "清除", false).flex_1().on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.clear_deflicker(cx);
                        })
                    ),),
                button("xmp-export", "导出 XMP 边车", false)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.export_xmp(cx),)),
            ],
        )
        .into_any_element()
    }

    fn render_tab(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(seq) = &self.seq else {
            return vec![empty_hint("载入序列后可设置输出尺寸、帧率与编码")];
        };
        let r = seq.render.clone();
        let out = default_output(seq);
        let mut out_el = Vec::new();
        if self.ffmpeg_status.path.is_none() {
            out_el.push(
                section_card(
                    "需要 FFmpeg",
                    children![
                        label(
                            "未找到可用的 FFmpeg。首次运行会自动安装；也可在设置中手动下载、导入或切换版本。",
                            10.0,
                            TEXT_MUTED
                        ),
                        button("goto-ffmpeg-settings", "前往设置", true).on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.settings_open = true;
                                cx.notify();
                            })
                        ),
                    ],
                )
                .into_any_element(),
            );
        }
        out_el.push(
            section_card(
                "输出格式",
                children![
                    resolution_row(&r, cx),
                    preset_fps(&r, cx),
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            label("编码", 10.5, TEXT_MUTED)
                                .w(px(54.0))
                                .flex_none(),
                        )
                        .child(codec_button(cx, "H.264", OutputCodec::H264, r.codec).flex_1())
                        .child(codec_button(cx, "H.265", OutputCodec::H265, r.codec).flex_1())
                        .child(codec_button(cx, "ProRes", OutputCodec::ProRes, r.codec).flex_1()),
                ],
            )
            .into_any_element(),
        );
        out_el.push(
            section_card(
                "画质与动效",
                children![
                    toggle_row(
                        "高级导出设置",
                        self.advanced_export,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.advanced_export = !this.advanced_export;
                            cx.notify();
                        }),
                    ),
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .when(
                            self.advanced_export && r.codec != OutputCodec::ProRes,
                            |el| {
                                el.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            label("码率模式", 10.5, TEXT_MUTED)
                                                .w(px(54.0))
                                                .flex_none(),
                                        )
                                        .child(
                                            rate_button(
                                                cx,
                                                "质量优先",
                                                RateControl::Crf,
                                                r.rate_control,
                                            )
                                            .flex_1(),
                                        )
                                        .child(
                                            rate_button(
                                                cx,
                                                "目标码率",
                                                RateControl::Bitrate,
                                                r.rate_control,
                                            )
                                            .flex_1(),
                                        ),
                                )
                                .child(if r.rate_control == RateControl::Bitrate {
                                    self.render_slider(
                                        cx,
                                        RenderSetting::Bitrate,
                                        "目标码率",
                                        r.bitrate_mbps,
                                        2.0,
                                        100.0,
                                        1.0,
                                    )
                                } else {
                                    self.render_slider(
                                        cx,
                                        RenderSetting::Quality,
                                        "质量 (CRF)",
                                        r.quality as f32,
                                        10.0,
                                        30.0,
                                        1.0,
                                    )
                                })
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(
                                            label("编码预设", 10.5, TEXT_MUTED)
                                                .w(px(54.0))
                                                .flex_none(),
                                        )
                                        .child(
                                            encoder_button(
                                                cx,
                                                "快速",
                                                EncoderPreset::Fast,
                                                r.encoder_preset,
                                            )
                                            .flex_1(),
                                        )
                                        .child(
                                            encoder_button(
                                                cx,
                                                "均衡",
                                                EncoderPreset::Balanced,
                                                r.encoder_preset,
                                            )
                                            .flex_1(),
                                        )
                                        .child(
                                            encoder_button(
                                                cx,
                                                "高质量",
                                                EncoderPreset::Best,
                                                r.encoder_preset,
                                            )
                                            .flex_1(),
                                        ),
                                )
                                .child(label(
                                    if r.rate_control == RateControl::Bitrate {
                                        format!(
                                            "VBR 目标 {:.0} Mbps · 约 {:.1} MB/s",
                                            r.bitrate_mbps,
                                            r.bitrate_mbps / 8.0
                                        )
                                    } else {
                                        format!("CRF {} · 恒定质量", r.quality)
                                    },
                                    10.0,
                                    TEXT_MUTED,
                                ))
                            }
                        )
                        .when(
                            self.advanced_export && r.codec == OutputCodec::ProRes,
                            |el| {
                                el.child(label(
                                    "ProRes 使用 profile 3 固定质量，码率与 CRF 设置不生效。",
                                    10.0,
                                    TEXT_MUTED,
                                ))
                            }
                        )
                        .when(
                            !self.advanced_export && r.codec != OutputCodec::ProRes,
                            |el| {
                                el.child(self.render_slider(
                                    cx,
                                    RenderSetting::Quality,
                                    "质量 (CRF)",
                                    r.quality as f32,
                                    10.0,
                                    30.0,
                                    1.0,
                                ))
                            }
                        ),
                    toggle_row(
                        "运动模糊",
                        r.motion_blur,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Some(seq) = &mut this.seq {
                                seq.render.motion_blur = !seq.render.motion_blur;
                            }
                            cx.notify();
                        }),
                    ),
                    self.render_slider(
                        cx,
                        RenderSetting::Sharpen,
                        "锐化",
                        r.sharpen,
                        0.0,
                        2.0,
                        0.05
                    ),
                    toggle_row(
                        "写入 XMP 边车",
                        r.write_xmp,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Some(seq) = &mut this.seq {
                                seq.render.write_xmp = !seq.render.write_xmp;
                            }
                            cx.notify();
                        }),
                    ),
                    toggle_row(
                        "保留输出帧",
                        r.export_frames,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Some(seq) = &mut this.seq {
                                seq.render.export_frames = !seq.render.export_frames;
                            }
                            cx.notify();
                        }),
                    ),
                ],
            )
            .into_any_element(),
        );
        out_el.push(
            section_card(
                "输出",
                children![
                    label(short_text(&out.display().to_string(), 42), 10.0, TEXT_MUTED),
                    self.render_progress(),
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(
                            button("pick-output-dir", "输出目录", false)
                                .flex_1()
                                .on_click(
                                    cx.listener(|this, _: &ClickEvent, _, _| this.pick_output()),
                                ),
                        )
                        .child(
                            button("open-output-dir", "打开位置", false)
                                .flex_1()
                                .on_click(
                                    cx.listener(|this, _: &ClickEvent, _, _| this.open_output()),
                                ),
                        ),
                    if self.render_running {
                        button("cancel-render", "取消渲染", false)
                            .w_full()
                            .on_click(
                                cx.listener(|this, _: &ClickEvent, _, cx| this.cancel_render(cx)),
                            )
                    } else {
                        button("start-render", "开始渲染", true)
                            .w_full()
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.start_render(cx)
                            }))
                    },
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(dot(
                            if seq.workflow.stage == crate::core::model::WorkflowStage::Finished {
                                SUCCESS
                            } else {
                                TEXT_FAINT
                            },
                        ))
                        .child(label(
                            if seq.workflow.stage == crate::core::model::WorkflowStage::Finished {
                                "渲染完成"
                            } else {
                                "等待渲染"
                            },
                            10.0,
                            TEXT_MUTED,
                        ))
                        .child(div().flex_1())
                        .when_some(self.last_render.as_ref(), |el, path| {
                            el.child(label(
                                short_text(&path.display().to_string(), 22),
                                9.5,
                                TEXT_FAINT,
                            ))
                        }),
                ],
            )
            .into_any_element(),
        );
        out_el
    }

    fn render_progress(&self) -> Div {
        let (pct, text) = match &self.progress {
            Some((done, total, stage)) => {
                let pct = if *total > 0 {
                    *done as f32 / *total as f32
                } else {
                    0.0
                };
                (pct, format!("{stage}  {done}/{total}"))
            }
            None if self.render_running => (0.0, "准备中…".to_string()),
            None => (
                0.0,
                if self.last_render.is_some() {
                    "上次渲染已完成".into()
                } else {
                    "尚未渲染".into()
                },
            ),
        };
        let show_pct = self.progress.is_some();
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .w_full()
                    .h(px(6.0))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(rgb(CONTROL))
                    .child(
                        div()
                            .h_full()
                            .w(relative(pct.clamp(0.0, 1.0)))
                            .rounded_full()
                            .bg(rgb(ACCENT)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(label(
                        text,
                        10.0,
                        if self.render_running { ACCENT } else { TEXT_MUTED },
                    ))
                    .child(div().flex_1())
                    .when(show_pct, |el| {
                        el.child(label(format!("{:.0}%", pct * 100.0), 10.0, TEXT_FAINT))
                    }),
            )
    }

    fn setting_slider(
        &self,
        cx: &mut Context<Self>,
        which: Setting,
        name: &'static str,
        value: f32,
        min: f32,
        max: f32,
        step: f32,
    ) -> Div {
        let pos = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let down = (value - step).max(min);
        let up = (value + step).min(max);
        let track = track_slider(cx, name, pos, min, max, WARNING, move |this, v, cx| {
            this.set_setting(which, v, cx);
        });
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .py(px(2.0))
            .child(
                div()
                    .w(px(54.0))
                    .flex_none()
                    .child(label(name, 10.5, TEXT_MUTED)),
            )
            .child(track)
            .child(stepper_button(
                ("sd", which as usize),
                "−",
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.set_setting(which, down, cx);
                }),
            ))
            .child(stepper_button(
                ("si", which as usize),
                "+",
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.set_setting(which, up, cx);
                }),
            ))
            .child(
                div()
                    .w(px(44.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(label(
                        format!("{value:+.2}"),
                        10.5,
                        if value.abs() > 1e-4 { TEXT } else { TEXT_FAINT },
                    )),
            )
    }

    fn set_setting(&mut self, which: Setting, value: f32, cx: &mut Context<Self>) {
        if let Some(seq) = &mut self.seq {
            match which {
                Setting::HgOffset => seq.settings.hg_offset = value,
                Setting::HgStretch => seq.settings.hg_stretch = value,
                Setting::HgRotate => seq.settings.hg_rotate = value,
                Setting::HgReduce => seq.settings.hg_reduce_initial = value,
                Setting::DeflickerSmooth => seq.settings.deflicker_smoothing = value,
            }
        }
        self.dirty = true;
        cx.notify();
    }

    fn render_slider(
        &self,
        cx: &mut Context<Self>,
        which: RenderSetting,
        name: &'static str,
        value: f32,
        min: f32,
        max: f32,
        _step: f32,
    ) -> Div {
        let pos = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let track = track_slider(cx, name, pos, min, max, ACCENT, move |this, v, cx| {
            this.set_render_value(which, v, cx);
        });
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .py(px(2.0))
            .child(
                div()
                    .w(px(54.0))
                    .flex_none()
                    .child(label(name, 10.5, TEXT_MUTED)),
            )
            .child(track)
            .child(
                div()
                    .w(px(64.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(label(
                        match which {
                            RenderSetting::Quality => format!("Q{value:.0}"),
                            RenderSetting::Sharpen => format!("{value:.1}"),
                            RenderSetting::Bitrate => format!("{value:.0} Mbps"),
                        },
                        10.5,
                        TEXT,
                    )),
            )
    }

    fn set_render_value(&mut self, which: RenderSetting, value: f32, cx: &mut Context<Self>) {
        if let Some(seq) = &mut self.seq {
            match which {
                RenderSetting::Quality => seq.render.quality = value.round() as u32,
                RenderSetting::Sharpen => seq.render.sharpen = value,
                RenderSetting::Bitrate => seq.render.bitrate_mbps = value,
            }
        }
        self.dirty = true;
        cx.notify();
    }

    fn status_bar(&self) -> Div {
        let (count, fps, folder) = match &self.seq {
            Some(s) => (s.images.len(), s.render.fps, s.folder.display().to_string()),
            None => (0, 25.0, "—".into()),
        };
        let duration = count as f32 / fps.max(1.0);
        let luma = self
            .seq
            .as_ref()
            .and_then(|s| s.images.get(self.selected))
            .map(|i| i.luma)
            .unwrap_or(0.0);
        let ev = self
            .seq
            .as_ref()
            .and_then(|s| s.images.get(self.selected))
            .map(|i| i.adj.total_ev())
            .unwrap_or(0.0);
        let progress = self.progress.as_ref().map(|(done, total, stage)| {
            let pct = if *total > 0 {
                *done as f32 / *total as f32
            } else {
                0.0
            };
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .w(px(100.0))
                        .h(px(4.0))
                        .rounded_full()
                        .overflow_hidden()
                        .bg(rgb(CONTROL))
                        .child(
                            div()
                                .h_full()
                                .w(relative(pct.clamp(0.0, 1.0)))
                                .rounded_full()
                                .bg(rgb(ACCENT)),
                        ),
                )
                .child(label(
                    format!("{stage} {:.0}%", pct * 100.0),
                    10.0,
                    ACCENT,
                ))
                .into_any_element()
        });
        let active = self.render_running || self.busy;
        div()
            .h(px(28.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .bg(rgb(PANEL))
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(dot(if active {
                ACCENT
            } else if self.seq.is_some() {
                SUCCESS
            } else {
                TEXT_FAINT
            }))
            .child(label(self.status.clone(), 10.0, TEXT_SECONDARY))
            .child(div().flex_1())
            .children(progress)
            .child(label(
                format!("{count} 帧 · {duration:.1}s · {fps:.0}fps"),
                10.0,
                TEXT_MUTED,
            ))
            .child(divider_v())
            .child(label(format!("亮度 {luma:.3}"), 10.0, TEXT_MUTED))
            .child(label(format!("EV {ev:+.2}"), 10.0, TEXT_MUTED))
            .child(divider_v())
            .child(label(folder, 10.0, TEXT_FAINT))
            .when_some(self.toast.clone(), |el, (text, _)| {
                el.child(
                    div()
                        .h(px(20.0))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .rounded(px(4.0))
                        .bg(rgb(SELECTION))
                        .child(label(text, 10.0, ACCENT)),
                )
            })
    }
}

#[derive(Clone, Copy)]
enum Setting {
    HgOffset,
    HgStretch,
    HgRotate,
    HgReduce,
    DeflickerSmooth,
}

#[derive(Clone, Copy)]
enum RenderSetting {
    Quality,
    Sharpen,
    Bitrate,
}

fn alpha(hex: u32, a: f32) -> Rgba {
    let mut c = rgb(hex);
    c.a = a;
    c
}

fn section_card(title: &'static str, children: Vec<AnyElement>) -> Div {
    section_card_inner(title, None, children)
}

fn section_card_meta(
    title: &'static str,
    meta: impl Into<SharedString>,
    children: Vec<AnyElement>,
) -> Div {
    section_card_inner(title, Some(meta.into()), children)
}

fn section_card_inner(
    title: &'static str,
    meta: Option<SharedString>,
    children: Vec<AnyElement>,
) -> Div {
    div()
        .w_full()
        .min_w(px(0.0))
        .flex_none()
        .overflow_hidden()
        .p_4()
        .rounded(px(10.0))
        .bg(rgb(SURFACE))
        .border_1()
        .border_color(rgb(BORDER))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    label(title, 11.5, TEXT_SECONDARY)
                        .font_weight(FontWeight::MEDIUM),
                )
                .when_some(meta, |el, meta| {
                    el.child(label(meta, 9.5, TEXT_FAINT))
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .children(children),
        )
}

fn short_text(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let tail_len = max.saturating_sub(2);
    let head: String = chars.iter().take(1).collect();
    let tail: String = chars[chars.len().saturating_sub(tail_len)..]
        .iter()
        .collect();
    format!("{head}…{tail}")
}

fn short_path(path: &std::path::Path, max: usize) -> String {
    let text = path.display().to_string();
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text;
    }
    let tail_len = max.saturating_sub(9);
    let head: String = chars.iter().take(8).collect();
    let tail: String = chars[chars.len().saturating_sub(tail_len)..]
        .iter()
        .collect();
    format!("{head}…{tail}")
}

fn info_row(name: impl ToString, value: impl ToString) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .min_w(px(0.0))
        .child(
            div()
                .w(px(70.0))
                .flex_none()
                .child(label(name.to_string(), 10.0, TEXT_MUTED)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .justify_end()
                .child(label(short_text(&value.to_string(), 32), 10.0, TEXT_SECONDARY).truncate()),
        )
}

fn empty_hint(text: &'static str) -> AnyElement {
    div()
        .w_full()
        .flex_none()
        .py(px(24.0))
        .px_4()
        .rounded(px(10.0))
        .bg(rgb(SURFACE))
        .border_1()
        .border_color(rgb(BORDER))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .child(label(text, 10.5, TEXT_MUTED))
        .into_any_element()
}

fn placeholder_preview() -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .child(
            div()
                .size(px(64.0))
                .rounded(px(16.0))
                .bg(rgb(SURFACE))
                .border_1()
                .border_color(rgb(BORDER))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    label("TL", 18.0, ACCENT_LINE)
                        .font_weight(FontWeight::BOLD),
                ),
        )
        .child(
            label("打开一个包含照片序列的文件夹", 12.5, TEXT_SECONDARY)
                .font_weight(FontWeight::MEDIUM),
        )
        .child(label(
            "支持 JPEG / PNG / TIFF / RAW（DNG, CR2, CR3, NEF, ARW…）",
            9.5,
            TEXT_FAINT,
        ))
}

fn choice_button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(28.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .cursor_pointer()
        .text_size(px(10.5))
        .font_weight(if active {
            FontWeight::MEDIUM
        } else {
            FontWeight::NORMAL
        })
        .bg(rgb(if active { ACCENT_SOFT } else { SURFACE }))
        .text_color(rgb(if active { ACCENT } else { TEXT_MUTED }))
        .border_1()
        .border_color(rgb(if active { ACCENT_LINE } else { BORDER }))
        .hover(move |s| {
            if active {
                s.bg(rgb(ACCENT_SOFT))
            } else {
                s.bg(rgb(CONTROL_HOVER)).text_color(rgb(TEXT))
            }
        })
        .child(text.into())
}

fn acc_button(
    cx: &mut Context<TimelapseApp>,
    text: &'static str,
    value: DeflickerAccuracy,
    current: DeflickerAccuracy,
) -> Stateful<Div> {
    let active = value == current;
    choice_button(text, text, active).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
        if let Some(seq) = &mut this.seq {
            seq.settings.deflicker_accuracy = value;
        }
        cx.notify();
    }))
}

fn rate_button(
    cx: &mut Context<TimelapseApp>,
    text: &'static str,
    value: RateControl,
    current: RateControl,
) -> Stateful<Div> {
    let active = value == current;
    choice_button(text, text, active).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
        if let Some(seq) = &mut this.seq {
            seq.render.rate_control = value;
        }
        this.dirty = true;
        cx.notify();
    }))
}

fn encoder_button(
    cx: &mut Context<TimelapseApp>,
    text: &'static str,
    value: EncoderPreset,
    current: EncoderPreset,
) -> Stateful<Div> {
    let active = value == current;
    choice_button(text, text, active).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
        if let Some(seq) = &mut this.seq {
            seq.render.encoder_preset = value;
        }
        this.dirty = true;
        cx.notify();
    }))
}

fn codec_button(
    cx: &mut Context<TimelapseApp>,
    text: &'static str,
    value: OutputCodec,
    current: OutputCodec,
) -> Stateful<Div> {
    let active = value == current;
    choice_button(text, text, active).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
        if let Some(seq) = &mut this.seq {
            seq.render.codec = value;
            seq.render.format = if value == OutputCodec::ProRes {
                OutputFormat::Mov
            } else {
                OutputFormat::Mp4
            };
        }
        this.dirty = true;
        cx.notify();
    }))
}

fn resolution_row(r: &RenderSettings, cx: &mut Context<TimelapseApp>) -> Div {
    let mut row = div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(label("分辨率", 10.5, TEXT_MUTED).w(px(54.0)).flex_none());
    for (text, w, h) in [
        ("1080p", 1920u32, 1080u32),
        ("4K", 3840, 2160),
        ("竖屏", 1080, 1920),
    ] {
        let active = r.width == w && r.height == h;
        row = row.child(
            choice_button(text, text, active)
                .flex_1()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(seq) = &mut this.seq {
                        seq.render.width = w;
                        seq.render.height = h;
                    }
                    this.dirty = true;
                    cx.notify();
                })),
        );
    }
    row
}

fn preset_fps(r: &RenderSettings, cx: &mut Context<TimelapseApp>) -> Div {
    let mut row = div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(label("帧率", 10.5, TEXT_MUTED).w(px(54.0)).flex_none());
    for fps in [24.0f32, 25.0, 30.0, 60.0] {
        let active = (r.fps - fps).abs() < 0.01;
        row = row.child(
            choice_button(("fps", fps as u32), format!("{fps:.0}"), active)
                .flex_1()
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(seq) = &mut this.seq {
                        seq.render.fps = fps;
                    }
                    this.dirty = true;
                    cx.notify();
                })),
        );
    }
    row
}

fn toggle_row(
    text: &'static str,
    on: bool,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(text)
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_between()
        .cursor_pointer()
        .child(label(text, 10.5, TEXT_SECONDARY))
        .child(switch(on, ACCENT))
        .on_click(listener)
}

fn stepper_button(
    id: impl Into<ElementId>,
    text: &'static str,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(20.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .cursor_pointer()
        .text_color(rgb(TEXT_MUTED))
        .hover(|s| s.bg(rgb(CONTROL_HOVER)).text_color(rgb(TEXT)))
        .child(text)
        .on_click(listener)
}

fn track_slider(
    cx: &mut Context<TimelapseApp>,
    id: &'static str,
    pos: f32,
    min: f32,
    max: f32,
    color: u32,
    on_pick: impl Fn(&mut TimelapseApp, f32, &mut Context<TimelapseApp>) + 'static,
) -> Div {
    let segments = 48usize;
    let pos = pos.clamp(0.0, 1.0);
    let active = (pos * (segments - 1) as f32).round() as usize;
    let on_pick = std::rc::Rc::new(on_pick);
    let mut track = div()
        .relative()
        .flex()
        .items_center()
        .h(px(20.0))
        .rounded_full()
        .overflow_hidden()
        .bg(rgb(TRACK))
        .flex_1();
    for i in 0..segments {
        let v = min + (max - min) * i as f32 / (segments - 1) as f32;
        let filled = i <= active;
        let pick = on_pick.clone();
        track = track.child(
            div()
                .id((id, i))
                .h_full()
                .flex_1()
                .flex()
                .items_center()
                .cursor_pointer()
                .child(
                    div()
                        .w_full()
                        .h(px(6.0))
                        .bg(rgb(if filled { color } else { TRACK })),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    pick(this, v, cx);
                })),
        );
    }
    for f in [0.25, 0.5, 0.75] {
        track = track.child(
            div()
                .absolute()
                .left(relative(f))
                .top(px(5.0))
                .w(px(1.0))
                .h(px(10.0))
                .bg(rgb(BORDER_STRONG)),
        );
    }
    track.child(
        div()
            .absolute()
            .left(relative(pos))
            .top(px(4.0))
            .ml(px(-6.0))
            .size(px(12.0))
            .rounded_full()
            .bg(rgb(0xf4f7fa))
            .border_2()
            .border_color(rgb(color)),
    )
}

impl Drop for TimelapseApp {
    fn drop(&mut self) {
        if self.dirty {
            self.save_now();
        }
    }
}

impl Render for TimelapseApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overlay = if self.settings_open {
            Some(self.settings_dialog(cx))
        } else {
            None
        };
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(CANVAS))
            .text_color(rgb(TEXT))
            .font_family(".SystemUIFont")
            .text_size(px(12.5))
            .child(self.title_bar())
            .child(self.top_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .child(self.left_panel(cx))
                    .child(self.center_panel(cx))
                    .child(self.right_panel(cx)),
            )
            .child(self.status_bar())
            .when_some(overlay, |el, dialog| el.child(dialog))
    }
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = crate::ui::icons::asset(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        Ok(Some(std::fs::read(path)?.into()))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}
