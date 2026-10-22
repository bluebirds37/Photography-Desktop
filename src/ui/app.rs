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

const BG0: u32 = 0x0e1114;
const BG1: u32 = 0x14181d;
const BG2: u32 = 0x1b2027;
const BG3: u32 = 0x232a33;
const BORDER: u32 = 0x2c333d;
const TEXT: u32 = 0xd9dee5;
const MUTED: u32 = 0x8a94a1;
const ACCENT: u32 = 0x62a8ff;
const AMBER: u32 = 0xe8b05c;
const GREEN: u32 = 0x5ed39a;
const RED: u32 = 0xef7070;

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
    Settings,
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
                        Some("settings") => Tab::Settings,
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
                            self.status = "FFmpeg 安装失败".into();
                            self.toast = Some((format!("FFmpeg 安装失败: {e}"), Instant::now()));
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
            self.tab = Tab::Settings;
            self.toast = Some((
                "尚未安装 FFmpeg，请先在设置页安装或导入".into(),
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
        if let Some(path) = &self.last_render {
            let _ = std::process::Command::new("explorer")
                .arg("/select,")
                .arg(path)
                .spawn();
        }
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
        .h(px(30.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_sm()
        .cursor_pointer()
        .bg(rgb(if primary { ACCENT } else { BG3 }))
        .text_color(rgb(if primary { 0x08111c } else { TEXT }))
        .border_1()
        .border_color(rgb(if primary { ACCENT } else { BORDER }))
        .hover(|s| {
            if primary {
                s.opacity(0.9)
            } else {
                s.bg(rgb(0x2b333e))
            }
        })
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
        .text_color(rgb(MUTED))
        .hover(move |s| {
            if close {
                s.bg(rgb(0xc42b1c)).text_color(rgb(0xffffff))
            } else {
                s.bg(rgb(0x2b333e)).text_color(rgb(TEXT))
            }
        })
        .child(text)
}

fn settings_button(id: &'static str, text: &'static str, primary: bool) -> Stateful<Div> {
    button(id, text, primary).h(px(36.0)).px(px(14.0))
}

fn chip(text: impl Into<SharedString>, active: bool, done: bool) -> Div {
    let color = if active {
        ACCENT
    } else if done {
        GREEN
    } else {
        MUTED
    };
    div()
        .h(px(24.0))
        .px(px(9.0))
        .flex()
        .items_center()
        .rounded_full()
        .text_xs()
        .bg(rgb(if active {
            0x1b2c44
        } else if done {
            0x152b22
        } else {
            BG2
        }))
        .text_color(rgb(color))
        .border_1()
        .border_color(rgb(if active { 0x35577f } else { BORDER }))
        .child(text.into())
}

impl TimelapseApp {
    fn title_bar(&self) -> Div {
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
            .gap_2()
            .px_3()
            .window_control_area(WindowControlArea::Drag)
            .child(label("延时摄影助手", 11.0, MUTED))
            .when(!seq_name.is_empty(), |el| {
                el.child(label("—", 10.0, 0x4b5562))
                    .child(label(seq_name, 10.0, 0x6d7783))
            });
        div()
            .h(px(32.0))
            .flex_none()
            .flex()
            .items_center()
            .bg(rgb(BG0))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(drag)
            .child(window_button("win-min", "–", WindowControlArea::Min, false))
            .child(window_button("win-max", "□", WindowControlArea::Max, false))
            .child(window_button(
                "win-close",
                "✕",
                WindowControlArea::Close,
                true,
            ))
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
        div()
            .h(px(58.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .bg(rgb(BG1))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .size(px(32.0))
                    .rounded_md()
                    .bg(rgb(ACCENT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0x07101a))
                    .font_weight(FontWeight::BOLD)
                    .child("TL"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(label("延时摄影助手", 15.0, TEXT).font_weight(FontWeight::SEMIBOLD))
                    .child(label("Rust · GPUI · 独立后期流程", 10.5, MUTED)),
            )
            .child(div().w(px(12.0)))
            .child(
                button("open-folder", "打开文件夹", true).on_click(cx.listener(
                    |this, _: &ClickEvent, _, cx| {
                        this.tx.send(Msg::PickFolder).ok();
                        cx.notify();
                    },
                )),
            )
            .child(button("save-project", "保存", false).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    this.save_now();
                    cx.notify();
                },
            )))
            .when(image_count > 0, |el| {
                el.child(
                    button("refresh-seq", "重新扫描", false).on_click(cx.listener(
                        |this, _: &ClickEvent, _, cx| {
                            if let Some(seq) = &this.seq {
                                let folder = seq.folder.clone();
                                this.import(folder, cx);
                            }
                        },
                    )),
                )
            })
            .child(div().flex_1())
            .when(image_count > 0, |el| {
                el.child(chip("初始化", image_count > 0, image_count > 0))
                    .child(chip(
                        format!("关键帧 {keyframes}"),
                        keyframes > 0,
                        keyframes > 0,
                    ))
                    .child(chip("HG 向导", wf.hg_wizard, wf.hg_wizard))
                    .child(chip("自动过渡", wf.auto_transition, wf.auto_transition))
                    .child(chip("去闪烁", wf.visual_deflicker, wf.visual_deflicker))
                    .child(chip(
                        "渲染",
                        wf.stage == crate::core::model::WorkflowStage::Finished,
                        false,
                    ))
            })
            .child({
                let (text, color) = if self.ffmpeg_busy {
                    ("FFmpeg 安装中…".to_string(), AMBER)
                } else if let Some(v) = &self.ffmpeg_status.version {
                    (format!("FFmpeg {v}"), GREEN)
                } else if self.ffmpeg_status.source == "system" {
                    ("FFmpeg 系统".to_string(), GREEN)
                } else {
                    ("安装 FFmpeg".to_string(), RED)
                };
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("ffmpeg-status")
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(BG3)))
                            .child(label(text, 11.0, color))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.tab = Tab::Settings;
                                cx.notify();
                            })),
                    )
                    .child(
                        button("render-top", "渲染视频", false).on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                this.start_render(cx);
                            },
                        )),
                    )
                    .child(
                        div()
                            .id("settings-top")
                            .h(px(30.0))
                            .px(px(12.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(if self.tab == Tab::Settings {
                                0x1b2c44
                            } else {
                                BG3
                            }))
                            .text_color(rgb(if self.tab == Tab::Settings {
                                ACCENT
                            } else {
                                TEXT
                            }))
                            .border_1()
                            .border_color(rgb(if self.tab == Tab::Settings {
                                0x35577f
                            } else {
                                BORDER
                            }))
                            .hover(|s| s.bg(rgb(0x2b333e)))
                            .child("设置")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.tab = if this.tab == Tab::Settings {
                                    Tab::Adjust
                                } else {
                                    Tab::Settings
                                };
                                cx.notify();
                            })),
                    )
            })
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

    fn settings_tab(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
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
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(label(stage.clone(), 10.0, ACCENT))
                .child(
                    div().w_full().h(px(6.0)).rounded_full().bg(rgb(BG3)).child(
                        div()
                            .h_full()
                            .w(px(300.0 * pct.clamp(0.0, 1.0)))
                            .rounded_full()
                            .bg(rgb(ACCENT)),
                    ),
                )
        });

        let mut versions = div().flex().flex_col().gap_1();
        if self.ffmpeg_versions.is_empty() {
            versions = versions.child(label("尚未安装内置版本", 10.0, MUTED));
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
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .px_2()
                    .rounded_sm()
                    .bg(rgb(if is_selected { 0x1b2c44 } else { BG3 }))
                    .child(label(
                        v.version.clone(),
                        11.0,
                        if is_selected { ACCENT } else { TEXT },
                    ))
                    .child(label(src.to_string(), 9.5, MUTED))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id(SharedString::from(format!("ffuse-{}", v.version)))
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_xs()
                            .bg(rgb(if is_selected { 0x203247 } else { 0x2b333e }))
                            .text_color(rgb(if is_selected { ACCENT } else { TEXT }))
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
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_xs()
                            .bg(rgb(0x3a2426))
                            .text_color(rgb(RED))
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
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(label("组件设置", 14.0, TEXT).font_weight(FontWeight::SEMIBOLD))
                        .child(label("FFmpeg 安装与版本管理", 10.0, MUTED)),
                )
                .child(
                    button("settings-back", "返回调整", false).on_click(cx.listener(
                        |this, _: &ClickEvent, _, cx| {
                            this.tab = Tab::Adjust;
                            cx.notify();
                        },
                    )),
                )
                .into_any_element(),
            section_card(
                "视频组件 · FFmpeg",
                children![
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
                                    .child(
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
                                                            let _ =
                                                                tx.send(InstallEvent::Finished(r));
                                                        },
                                                        cx,
                                                    );
                                                },
                                            )),
                                    )
                                    .child(
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
                                            ),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .gap_2()
                                            .child(
                                                settings_button("ff-import", "导入系统 FFmpeg", false)
                                                    .flex_1()
                                                    .on_click(cx.listener(
                                                        |this, _: &ClickEvent, _, cx| {
                                                            this.spawn_install(
                                                                |tx| {
                                                                    let r = ffmpeg::import_system()
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
                                            .child(
                                                settings_button("ff-open", "打开组件目录", false)
                                                    .flex_1()
                                                    .on_click(cx.listener(
                                                        |this, _: &ClickEvent, _, _| {
                                                            let _ = std::process::Command::new(
                                                                "explorer",
                                                            )
                                                            .arg(ffmpeg::components_root())
                                                            .spawn();
                                                            this.status = "已打开组件目录".into();
                                                        },
                                                    )),
                                            ),
                                    ),
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
                        MUTED
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
                        "首次运行会自动导入系统 FFmpeg；若未找到则从镜像快速安装，也可手动下载最新版或从压缩包导入。",
                        10.0,
                        MUTED
                    ),
                ],
            )
            .into_any_element(),
        ]
    }

    fn left_panel(&self, cx: &mut Context<Self>) -> Div {
        let (n, name, folder) = match &self.seq {
            Some(s) => (
                s.images.len(),
                s.name.clone(),
                s.folder.display().to_string(),
            ),
            None => (0, "未载入序列".to_string(), String::new()),
        };
        let entity = cx.entity();
        let list = if n > 0 {
            let this_preview = self.preview_generation;
            let _ = this_preview;
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
                                    .h(px(58.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .px_2()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(rgb(if selected { 0x203247 } else { BG1 }))
                                    .hover(|s| s.bg(rgb(if selected { 0x203247 } else { BG2 })))
                                    .child(
                                        div()
                                            .w(px(58.0))
                                            .h(px(42.0))
                                            .rounded_sm()
                                            .bg(rgb(0x090b0e))
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
                                            .gap_0p5()
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .child(label(
                                                        im.file_name.clone(),
                                                        11.0,
                                                        if selected { TEXT } else { 0xc6ccd4 },
                                                    ))
                                                    .when(im.keyframe, |el| {
                                                        el.child(label("◆", 10.0, AMBER))
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(label(
                                                        format!("{:+.2} EV", im.adj.total_ev()),
                                                        10.0,
                                                        MUTED,
                                                    ))
                                                    .child(label(
                                                        format!("L {:.2}", im.luma),
                                                        10.0,
                                                        MUTED,
                                                    )),
                                            ),
                                    );
                                if im.error.is_some() {
                                    row = row.child(label("!", 11.0, RED));
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
                .items_center()
                .justify_center()
                .child(label("尚未载入图像", 12.0, MUTED))
                .into_any_element()
        };

        div()
            .w(px(276.0))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(BG1))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(label(name, 13.0, TEXT).font_weight(FontWeight::SEMIBOLD))
                    .child(label(format!("{n} 张 · 序列"), 10.5, MUTED))
                    .when(!folder.is_empty(), |el| {
                        el.child(label(folder, 9.5, 0x6d7783).truncate())
                    }),
            )
            .child(list)
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
                .top(px(10.0))
                .left(px(10.0))
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .bg(alpha(0x0b0e12, 0.85))
                        .child(label(format!("#{:04}", selected + 1), 10.5, TEXT)),
                )
                .child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .bg(alpha(0x0b0e12, 0.85))
                        .child(label(im.file_name.clone(), 10.5, MUTED)),
                )
                .when(im.keyframe, |el| {
                    el.child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .bg(alpha(0x33280f, 0.9))
                            .child(label("关键帧", 10.0, AMBER)),
                    )
                })
        });

        let transport = div()
            .h(px(44.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .child(button("prev", "上一帧", false).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    if this.selected > 0 {
                        let i = this.selected - 1;
                        this.select(i, cx);
                    }
                },
            )))
            .child(
                button("play", if self.playing { "暂停" } else { "播放" }, true).on_click(
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.playing = !this.playing;
                        this.last_play = Instant::now();
                        if !this.playing {
                            this.request_preview(cx);
                        }
                        cx.notify();
                    }),
                ),
            )
            .child(button("next", "下一帧", false).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    if let Some(s) = &this.seq {
                        if this.selected + 1 < s.images.len() {
                            let i = this.selected + 1;
                            this.select(i, cx);
                        }
                    }
                },
            )))
            .child(div().flex_1())
            .child(
                button("keyframe", "标记关键帧", false)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_keyframe(cx))),
            )
            .child(button("prev-kf", "上一关键帧", false).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    if let Some(s) = &this.seq {
                        if let Some(i) = s.prev_keyframe(this.selected) {
                            this.select(i, cx);
                        }
                    }
                },
            )))
            .child(button("next-kf", "下一关键帧", false).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    if let Some(s) = &this.seq {
                        if let Some(i) = s.next_keyframe(this.selected) {
                            this.select(i, cx);
                        }
                    }
                },
            )))
            .when(current.is_some(), |el| {
                el.child(label(
                    format!("{:.2} EV", current.map(|i| i.adj.total_ev()).unwrap_or(0.0)),
                    11.0,
                    AMBER,
                ))
            });

        let graph = if let Some(seq) = seq {
            crate::ui::graph::graph_view(seq, selected)
        } else {
            div()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .child(label("亮度曲线", 11.0, MUTED))
                .into_any_element()
        };

        div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(BG0))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_5()
                    .child(preview_el)
                    .children(overlay),
            )
            .child(
                div()
                    .h(px(156.0))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .bg(rgb(0x0b0e12))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .h(px(26.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_4()
                            .px_3()
                            .child(label("亮度 / 曝光曲线", 10.5, MUTED))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().size(px(6.0)).rounded_full().bg(rgb(0x58c7d8)))
                                    .child(label("亮度", 10.0, MUTED)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().size(px(6.0)).rounded_full().bg(rgb(AMBER)))
                                    .child(label("曝光修正", 10.0, MUTED)),
                            )
                            .child(div().flex_1())
                            .child(label("◆ 关键帧", 10.0, AMBER)),
                    )
                    .child(graph),
            )
            .child(transport)
    }

    fn right_panel(&self, cx: &mut Context<Self>) -> Div {
        let tab_button = |id: &'static str, text: &'static str, t: Tab| {
            let active = self.tab == t;
            div()
                .id(id)
                .flex_1()
                .h(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .bg(rgb(if active { BG3 } else { BG1 }))
                .text_color(rgb(if active { TEXT } else { MUTED }))
                .text_sm()
                .child(text)
        };

        let content = match self.tab {
            Tab::Adjust => self.adjust_tab(cx),
            Tab::Tools => self.tools_tab(cx),
            Tab::Render => self.render_tab(cx),
            Tab::Settings => self.settings_tab(cx),
        };

        div()
            .w(px(348.0))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(BG1))
            .border_l_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .p_2()
                    .flex()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        tab_button("tab-adjust", "调整", Tab::Adjust).on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                this.tab = Tab::Adjust;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        tab_button("tab-tools", "工具", Tab::Tools).on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                this.tab = Tab::Tools;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        tab_button("tab-render", "渲染", Tab::Render).on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                this.tab = Tab::Render;
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .id("right-scroll")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .p_3()
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
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(label("单帧调整", 13.0, TEXT).font_weight(FontWeight::SEMIBOLD))
                        .child(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .bg(rgb(if im.keyframe { 0x33280f } else { BG2 }))
                                .child(label(
                                    if im.keyframe {
                                        "关键帧"
                                    } else {
                                        "非关键帧"
                                    },
                                    9.5,
                                    if im.keyframe { AMBER } else { MUTED },
                                )),
                        ),
                )
                .child(div().w_full().child(label(
                    short_text(&format!("#{} · {}", self.selected + 1, im.file_name), 38),
                    10.0,
                    MUTED,
                )))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("auto-key-toggle")
                                .flex_1()
                                .h(px(28.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .cursor_pointer()
                                .text_xs()
                                .bg(rgb(if self.auto_keyframe { 0x1b2c44 } else { BG2 }))
                                .border_1()
                                .border_color(rgb(if self.auto_keyframe {
                                    0x35577f
                                } else {
                                    BORDER
                                }))
                                .text_color(rgb(if self.auto_keyframe { ACCENT } else { MUTED }))
                                .child(if self.auto_keyframe {
                                    "自动关键帧：开"
                                } else {
                                    "自动关键帧：关"
                                })
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.auto_keyframe = !this.auto_keyframe;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id("reset-adj")
                                .w(px(64.0))
                                .h(px(28.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .cursor_pointer()
                                .text_xs()
                                .bg(rgb(BG2))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .text_color(rgb(MUTED))
                                .hover(|s| s.bg(rgb(BG3)))
                                .child("重置")
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
        let segments = 36;
        let pos = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let active = (pos * (segments - 1) as f32).round() as usize;
        let mut track = div()
            .flex()
            .items_center()
            .h(px(18.0))
            .gap(px(1.0))
            .flex_1();
        for i in 0..segments {
            let v = min + (max - min) * i as f32 / (segments - 1) as f32;
            let filled = i <= active;
            track = track.child(
                div()
                    .id(("seg", i))
                    .h(px(if i == active { 16.0 } else { 10.0 }))
                    .flex_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(rgb(if filled { ACCENT } else { 0x2a313a }))
                    .hover(|s| s.bg(rgb(if filled { 0x8cc0ff } else { 0x3a4350 })))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_adjustment(col, v, cx);
                    })),
            );
        }
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
            .gap_2()
            .py_0p5()
            .child(
                div()
                    .w(px(58.0))
                    .flex_none()
                    .child(label(col.label(), 11.0, MUTED)),
            )
            .child(
                div()
                    .id(("dec", col as usize))
                    .size(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .bg(rgb(BG3))
                    .cursor_pointer()
                    .child(label("−", 12.0, TEXT))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_adjustment(col, down, cx);
                    })),
            )
            .child(track)
            .child(
                div()
                    .id(("inc", col as usize))
                    .size(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .bg(rgb(BG3))
                    .cursor_pointer()
                    .child(label("+", 12.0, TEXT))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_adjustment(col, up, cx);
                    })),
            )
            .child(
                div()
                    .w(px(42.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(label(display, 10.5, TEXT)),
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
        section_card(
            "Holy Grail 向导",
            children![
                label("补偿日转夜拍摄时的相机曝光跳变", 10.0, MUTED),
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
                label("在关键帧之间插值曝光、色温与全部影调参数", 10.0, MUTED),
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("mode-smooth")
                            .flex_1()
                            .h(px(28.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(if mode == TransitionMode::Smooth {
                                0x1b2c44
                            } else {
                                BG2
                            }))
                            .text_color(rgb(if mode == TransitionMode::Smooth {
                                ACCENT
                            } else {
                                MUTED
                            }))
                            .text_xs()
                            .child("平滑")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Some(seq) = &mut this.seq {
                                    seq.settings.transition_mode = TransitionMode::Smooth;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("mode-linear")
                            .flex_1()
                            .h(px(28.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(if mode == TransitionMode::Linear {
                                0x1b2c44
                            } else {
                                BG2
                            }))
                            .text_color(rgb(if mode == TransitionMode::Linear {
                                ACCENT
                            } else {
                                MUTED
                            }))
                            .text_xs()
                            .child("线性")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Some(seq) = &mut this.seq {
                                    seq.settings.transition_mode = TransitionMode::Linear;
                                }
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
                    .gap_2()
                    .child(label("精度", 11.0, MUTED).w(px(58.0)))
                    .child(acc_button(cx, "粗略", DeflickerAccuracy::Coarse, acc))
                    .child(acc_button(cx, "标准", DeflickerAccuracy::Normal, acc))
                    .child(acc_button(cx, "精细", DeflickerAccuracy::Fine, acc)),
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(label("多遍", 11.0, MUTED).w(px(58.0)))
                    .child(
                        div()
                            .id("passes-dec")
                            .size(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .bg(rgb(BG3))
                            .cursor_pointer()
                            .child("−")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Some(seq) = &mut this.seq {
                                    seq.settings.deflicker_passes =
                                        seq.settings.deflicker_passes.saturating_sub(1).max(1);
                                }
                                cx.notify();
                            })),
                    )
                    .child(label(format!("{} 遍", s.deflicker_passes), 11.0, TEXT))
                    .child(
                        div()
                            .id("passes-inc")
                            .size(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .bg(rgb(BG3))
                            .cursor_pointer()
                            .child("+")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Some(seq) = &mut this.seq {
                                    seq.settings.deflicker_passes =
                                        (seq.settings.deflicker_passes + 1).min(4);
                                }
                                cx.notify();
                            })),
                    )
                    .child(label("恒定模式", 11.0, MUTED))
                    .child(
                        div()
                            .id("const-toggle")
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(rgb(if s.deflicker_constant { 0x1b2c44 } else { BG3 }))
                            .text_color(rgb(if s.deflicker_constant { ACCENT } else { MUTED }))
                            .text_xs()
                            .child(if s.deflicker_constant { "开" } else { "关" })
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Some(seq) = &mut this.seq {
                                    seq.settings.deflicker_constant =
                                        !seq.settings.deflicker_constant;
                                }
                                cx.notify();
                            })),
                    ),
                label(result_text, 10.0, MUTED),
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
                            "未找到可用的 FFmpeg。首次运行会自动安装；也可在设置页手动下载、导入或切换版本。",
                            10.0,
                            MUTED
                        ),
                        button("goto-ffmpeg-settings", "前往设置", true).on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.tab = Tab::Settings;
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
                        .gap_2()
                        .child(label("编码", 11.0, MUTED).w(px(58.0)))
                        .child(codec_button(cx, "H.264", OutputCodec::H264, r.codec))
                        .child(codec_button(cx, "H.265", OutputCodec::H265, r.codec))
                        .child(codec_button(cx, "ProRes", OutputCodec::ProRes, r.codec)),
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
                                        .gap_2()
                                        .child(label("码率模式", 11.0, MUTED).w(px(58.0)))
                                        .child(rate_button(
                                            cx,
                                            "质量优先",
                                            RateControl::Crf,
                                            r.rate_control,
                                        ))
                                        .child(rate_button(
                                            cx,
                                            "目标码率",
                                            RateControl::Bitrate,
                                            r.rate_control,
                                        )),
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
                                        .gap_2()
                                        .child(label("编码预设", 11.0, MUTED).w(px(58.0)))
                                        .child(encoder_button(
                                            cx,
                                            "快速",
                                            EncoderPreset::Fast,
                                            r.encoder_preset,
                                        ))
                                        .child(encoder_button(
                                            cx,
                                            "均衡",
                                            EncoderPreset::Balanced,
                                            r.encoder_preset,
                                        ))
                                        .child(encoder_button(
                                            cx,
                                            "高质量",
                                            EncoderPreset::Best,
                                            r.encoder_preset,
                                        )),
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
                                    MUTED,
                                ))
                            }
                        )
                        .when(
                            self.advanced_export && r.codec == OutputCodec::ProRes,
                            |el| {
                                el.child(label(
                                    "ProRes 使用 profile 3 固定质量，码率与 CRF 设置不生效。",
                                    10.0,
                                    MUTED,
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
                    label(short_text(&out.display().to_string(), 42), 10.0, MUTED),
                    self.render_progress(),
                    div()
                        .flex()
                        .gap_2()
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
                        button("cancel-render", "取消渲染", false).on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| this.cancel_render(cx)),
                        )
                    } else {
                        button("start-render", "开始渲染", true).on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| this.start_render(cx)),
                        )
                    },
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(label("渲染完成", 10.5, MUTED))
                        .child(div().size(px(8.0)).rounded_full().bg(rgb(
                            if seq.workflow.stage == crate::core::model::WorkflowStage::Finished {
                                GREEN
                            } else {
                                BG3
                            }
                        )),),
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
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div().w_full().h(px(6.0)).rounded_full().bg(rgb(BG3)).child(
                    div()
                        .h_full()
                        .w(px(300.0 * pct))
                        .rounded_full()
                        .bg(rgb(ACCENT)),
                ),
            )
            .child(label(
                text,
                10.0,
                if self.render_running { ACCENT } else { MUTED },
            ))
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
        let segments = 36;
        let pos = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let active = (pos * (segments - 1) as f32).round() as usize;
        let mut track = div()
            .flex()
            .items_center()
            .h(px(16.0))
            .gap(px(1.0))
            .flex_1();
        for i in 0..segments {
            let v = min + (max - min) * i as f32 / (segments - 1) as f32;
            let filled = i <= active;
            track = track.child(
                div()
                    .id(("sett", i))
                    .h(px(if i == active { 14.0 } else { 9.0 }))
                    .flex_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(rgb(if filled { AMBER } else { 0x2a313a }))
                    .hover(|s| s.bg(rgb(if filled { 0xf3c887 } else { 0x3a4350 })))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_setting(which, v, cx);
                    })),
            );
        }
        let down = (value - step).max(min);
        let up = (value + step).min(max);
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(58.0))
                    .flex_none()
                    .child(label(name, 11.0, MUTED)),
            )
            .child(
                div()
                    .id(("sd", which as usize))
                    .size(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .bg(rgb(BG3))
                    .cursor_pointer()
                    .child("−")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_setting(which, down, cx);
                    })),
            )
            .child(track)
            .child(
                div()
                    .id(("si", which as usize))
                    .size(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .bg(rgb(BG3))
                    .cursor_pointer()
                    .child("+")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_setting(which, up, cx);
                    })),
            )
            .child(
                div()
                    .w(px(42.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(label(format!("{value:+.2}"), 10.5, TEXT)),
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
        let segments = 36;
        let pos = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let active = (pos * (segments - 1) as f32).round() as usize;
        let mut track = div()
            .flex()
            .items_center()
            .h(px(16.0))
            .gap(px(1.0))
            .flex_1();
        for i in 0..segments {
            let v = min + (max - min) * i as f32 / (segments - 1) as f32;
            let filled = i <= active;
            track = track.child(
                div()
                    .id(("rend", i))
                    .h(px(if i == active { 14.0 } else { 9.0 }))
                    .flex_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .bg(rgb(if filled { ACCENT } else { 0x2a313a }))
                    .hover(|s| s.bg(rgb(if filled { 0x8cc0ff } else { 0x3a4350 })))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_render_value(which, v, cx);
                    })),
            );
        }
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(58.0))
                    .flex_none()
                    .child(label(name, 11.0, MUTED)),
            )
            .child(track)
            .child(
                div()
                    .w(px(58.0))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(label(
                        match which {
                            RenderSetting::Quality => format!("{value:.0}"),
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
                .gap_2()
                .child(label(format!("{stage} {:.0}%", pct * 100.0), 10.0, ACCENT))
                .child(
                    div()
                        .w(px(120.0))
                        .h(px(4.0))
                        .rounded_full()
                        .bg(rgb(BG3))
                        .child(
                            div()
                                .h_full()
                                .w(px(120.0 * pct))
                                .rounded_full()
                                .bg(rgb(ACCENT)),
                        ),
                )
                .into_any_element()
        });
        div()
            .h(px(26.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .bg(rgb(BG1))
            .border_t_1()
            .border_color(rgb(BORDER))
            .text_xs()
            .child(label(self.status.clone(), 10.0, MUTED))
            .child(div().flex_1())
            .children(progress)
            .child(label(
                format!("{count} 帧 · {:.1}s @ {:.0}fps", duration, fps),
                10.0,
                MUTED,
            ))
            .child(label(format!("亮度 {luma:.3} · EV {ev:+.2}"), 10.0, MUTED))
            .child(label(folder, 10.0, 0x65707c))
            .when_some(self.toast.clone(), |el, (text, _)| {
                el.child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .bg(rgb(0x203247))
                        .text_color(rgb(ACCENT))
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
    div()
        .w_full()
        .min_w(px(0.0))
        .overflow_hidden()
        .p_3()
        .rounded_lg()
        .bg(rgb(BG2))
        .border_1()
        .border_color(rgb(BORDER))
        .flex()
        .flex_col()
        .gap_2()
        .child(label(title, 12.5, TEXT).font_weight(FontWeight::SEMIBOLD))
        .children(children)
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
        .gap_2()
        .py_0p5()
        .min_w(px(0.0))
        .child(
            div()
                .w(px(76.0))
                .flex_none()
                .child(label(name.to_string(), 10.5, MUTED)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .justify_end()
                .child(label(short_text(&value.to_string(), 34), 10.5, TEXT).truncate()),
        )
}

fn empty_hint(text: &'static str) -> AnyElement {
    div()
        .w_full()
        .p_4()
        .rounded_lg()
        .bg(rgb(BG2))
        .border_1()
        .border_color(rgb(BORDER))
        .flex()
        .items_center()
        .justify_center()
        .child(label(text, 11.0, MUTED))
        .into_any_element()
}

fn placeholder_preview() -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .child(
            div()
                .size(px(64.0))
                .rounded_lg()
                .bg(rgb(BG2))
                .border_1()
                .border_color(rgb(BORDER))
                .flex()
                .items_center()
                .justify_center()
                .child(label("TL", 20.0, 0x3c4652).font_weight(FontWeight::BOLD)),
        )
        .child(label("打开一个包含照片序列的文件夹", 12.0, MUTED))
        .child(label(
            "支持 JPEG / PNG / TIFF / RAW (DNG, CR2, CR3, NEF, ARW…)",
            10.0,
            0x5d6773,
        ))
}

fn acc_button(
    cx: &mut Context<TimelapseApp>,
    text: &'static str,
    value: DeflickerAccuracy,
    current: DeflickerAccuracy,
) -> Stateful<Div> {
    let active = value == current;
    div()
        .id(text)
        .px_2()
        .py_1()
        .rounded_sm()
        .cursor_pointer()
        .text_xs()
        .bg(rgb(if active { 0x1b2c44 } else { BG3 }))
        .text_color(rgb(if active { ACCENT } else { MUTED }))
        .child(text)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
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
    div()
        .id(text)
        .px_2()
        .py_1()
        .rounded_sm()
        .cursor_pointer()
        .text_xs()
        .bg(rgb(if active { 0x1b2c44 } else { BG3 }))
        .text_color(rgb(if active { ACCENT } else { MUTED }))
        .child(text)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
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
    div()
        .id(text)
        .px_2()
        .py_1()
        .rounded_sm()
        .cursor_pointer()
        .text_xs()
        .bg(rgb(if active { 0x1b2c44 } else { BG3 }))
        .text_color(rgb(if active { ACCENT } else { MUTED }))
        .child(text)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
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
    div()
        .id(text)
        .px_2()
        .py_1()
        .rounded_sm()
        .cursor_pointer()
        .text_xs()
        .bg(rgb(if active { 0x1b2c44 } else { BG3 }))
        .text_color(rgb(if active { ACCENT } else { MUTED }))
        .child(text)
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            if let Some(seq) = &mut this.seq {
                seq.render.codec = value;
                seq.render.format = if value == OutputCodec::ProRes {
                    OutputFormat::Mov
                } else {
                    OutputFormat::Mp4
                };
            }
            cx.notify();
        }))
}

fn resolution_row(r: &RenderSettings, cx: &mut Context<TimelapseApp>) -> Div {
    let mut row = div()
        .flex()
        .items_center()
        .gap_2()
        .child(label("分辨率", 11.0, MUTED).w(px(58.0)));
    for (text, w, h) in [
        ("1080p", 1920u32, 1080u32),
        ("4K", 3840, 2160),
        ("竖屏", 1080, 1920),
    ] {
        let active = r.width == w && r.height == h;
        row = row.child(
            div()
                .id(text)
                .px_2()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .text_xs()
                .bg(rgb(if active { 0x1b2c44 } else { BG3 }))
                .text_color(rgb(if active { ACCENT } else { MUTED }))
                .child(text)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(seq) = &mut this.seq {
                        seq.render.width = w;
                        seq.render.height = h;
                    }
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
        .gap_2()
        .child(label("帧率", 11.0, MUTED).w(px(58.0)));
    for fps in [24.0f32, 25.0, 30.0, 60.0] {
        let active = (r.fps - fps).abs() < 0.01;
        row = row.child(
            div()
                .id(("fps", fps as u32))
                .px_2()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .text_xs()
                .bg(rgb(if active { 0x1b2c44 } else { BG3 }))
                .text_color(rgb(if active { ACCENT } else { MUTED }))
                .child(format!("{fps:.0}"))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(seq) = &mut this.seq {
                        seq.render.fps = fps;
                    }
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
        .flex()
        .items_center()
        .justify_between()
        .py_0p5()
        .cursor_pointer()
        .child(label(text, 11.0, MUTED))
        .child(
            div()
                .w(px(30.0))
                .h(px(16.0))
                .rounded_full()
                .p(px(2.0))
                .bg(rgb(if on { ACCENT } else { BG3 }))
                .flex()
                .child(
                    div()
                        .size(px(12.0))
                        .rounded_full()
                        .bg(rgb(if on { 0x07101a } else { 0x7a8492 }))
                        .when(on, |el| el.ml(px(14.0))),
                ),
        )
        .on_click(listener)
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
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG0))
            .text_color(rgb(TEXT))
            .font_family("Microsoft YaHei")
            .text_sm()
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
    }
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Some(std::fs::read(path)?.into()))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}
