use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const PROJECT_DIR: &str = ".lrtassist";
pub const PROJECT_FILE: &str = "project.json";
pub const THUMB_DIR: &str = "thumbs";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Adjustments {
    /// Creative exposure compensation in EV.
    pub exposure: f32,
    /// Relative white-balance shift in mired-ish UI units (-100..100).
    pub temperature: f32,
    pub tint: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub saturation: f32,
    pub vibrance: f32,
    /// Deflicker correction in EV, computed by the analysis engine.
    pub deflicker: f32,
}

impl Adjustments {
    pub fn total_ev(&self) -> f32 {
        self.exposure + self.deflicker
    }

    pub fn is_identity(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageEntry {
    pub file_name: String,
    pub timestamp: Option<f64>,
    pub camera_ev: Option<f32>,
    pub iso: Option<f32>,
    pub shutter: Option<f32>,
    pub aperture: Option<f32>,
    /// Mean relative luminance (sRGB, 0..1) of the analyzed preview.
    pub luma: f32,
    /// Gray-world white balance estimate, normalized to green.
    pub wb_r: f32,
    pub wb_b: f32,
    pub thumb: Option<String>,
    pub size: u64,
    pub modified: Option<u64>,
    pub width: u32,
    pub height: u32,
    pub error: Option<String>,
    pub adj: Adjustments,
    pub keyframe: bool,
    pub starred: bool,
    pub skipped: bool,
}

impl ImageEntry {
    pub fn path(&self, folder: &Path) -> PathBuf {
        folder.join(&self.file_name)
    }

    pub fn thumb_path(&self, folder: &Path) -> Option<PathBuf> {
        self.thumb
            .as_ref()
            .map(|t| folder.join(PROJECT_DIR).join(t))
    }

    /// Estimated white balance temperature proxy from the gray-world channels.
    pub fn wb_proxy(&self) -> f32 {
        let ratio = (self.wb_r.max(1e-4) / self.wb_b.max(1e-4)).log2();
        ratio * 120.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdjustColumn {
    Exposure,
    Temperature,
    Tint,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Saturation,
    Vibrance,
}

impl AdjustColumn {
    pub fn get(self, a: &Adjustments) -> f32 {
        match self {
            Self::Exposure => a.exposure,
            Self::Temperature => a.temperature,
            Self::Tint => a.tint,
            Self::Contrast => a.contrast,
            Self::Highlights => a.highlights,
            Self::Shadows => a.shadows,
            Self::Whites => a.whites,
            Self::Blacks => a.blacks,
            Self::Saturation => a.saturation,
            Self::Vibrance => a.vibrance,
        }
    }

    pub fn set(self, a: &mut Adjustments, v: f32) {
        match self {
            Self::Exposure => a.exposure = v,
            Self::Temperature => a.temperature = v,
            Self::Tint => a.tint = v,
            Self::Contrast => a.contrast = v,
            Self::Highlights => a.highlights = v,
            Self::Shadows => a.shadows = v,
            Self::Whites => a.whites = v,
            Self::Blacks => a.blacks = v,
            Self::Saturation => a.saturation = v,
            Self::Vibrance => a.vibrance = v,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Exposure => "曝光",
            Self::Temperature => "色温",
            Self::Tint => "色调",
            Self::Contrast => "对比度",
            Self::Highlights => "高光",
            Self::Shadows => "阴影",
            Self::Whites => "白色",
            Self::Blacks => "黑色",
            Self::Saturation => "饱和度",
            Self::Vibrance => "自然饱和度",
        }
    }

    pub fn range(self) -> (f32, f32, f32) {
        match self {
            Self::Exposure => (-5.0, 5.0, 0.05),
            Self::Temperature => (-100.0, 100.0, 1.0),
            Self::Tint => (-100.0, 100.0, 1.0),
            Self::Contrast => (-100.0, 100.0, 1.0),
            Self::Highlights => (-100.0, 100.0, 1.0),
            Self::Shadows => (-100.0, 100.0, 1.0),
            Self::Whites => (-100.0, 100.0, 1.0),
            Self::Blacks => (-100.0, 100.0, 1.0),
            Self::Saturation => (-100.0, 100.0, 1.0),
            Self::Vibrance => (-100.0, 100.0, 1.0),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransitionMode {
    Linear,
    #[default]
    Smooth,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeflickerAccuracy {
    Coarse,
    #[serde(rename = "normal")]
    Normal,
    Fine,
}

impl Default for DeflickerAccuracy {
    fn default() -> Self {
        Self::Normal
    }
}

impl DeflickerAccuracy {
    /// Absolute luminance threshold below which a correction is treated as noise.
    pub fn epsilon(self) -> f32 {
        match self {
            Self::Coarse => 0.005,
            Self::Normal => 0.002,
            Self::Fine => 0.0005,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SequenceSettings {
    pub deflicker_smoothing: f32,
    pub deflicker_accuracy: DeflickerAccuracy,
    pub deflicker_passes: u32,
    pub deflicker_constant: bool,
    pub deflicker_multipass: bool,
    pub hg_offset: f32,
    pub hg_stretch: f32,
    pub hg_rotate: f32,
    pub hg_reduce_initial: f32,
    pub transition_mode: TransitionMode,
}

impl Default for SequenceSettings {
    fn default() -> Self {
        Self {
            deflicker_smoothing: 0.5,
            deflicker_accuracy: DeflickerAccuracy::Normal,
            deflicker_passes: 2,
            deflicker_constant: false,
            deflicker_multipass: true,
            hg_offset: 0.0,
            hg_stretch: 0.0,
            hg_rotate: 0.0,
            hg_reduce_initial: 0.5,
            transition_mode: TransitionMode::Smooth,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorkflowStage {
    #[default]
    Initialized,
    Keyframes,
    HolyGrail,
    AutoTransition,
    VisualPreviews,
    Deflicker,
    Finished,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkflowState {
    pub stage: WorkflowStage,
    pub keyframes: usize,
    pub hg_wizard: bool,
    pub auto_transition: bool,
    pub visual_deflicker: bool,
    pub deflicker_passes: u32,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum RateControl {
    #[default]
    Crf,
    Bitrate,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum EncoderPreset {
    Fast,
    #[default]
    Balanced,
    Best,
}

impl EncoderPreset {
    pub fn x264(self) -> &'static str {
        match self {
            Self::Fast => "veryfast",
            Self::Balanced => "medium",
            Self::Best => "slow",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutputCodec {
    H264,
    H265,
    ProRes,
}

impl Default for OutputCodec {
    fn default() -> Self {
        Self::H264
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutputFormat {
    Mp4,
    Mov,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderSettings {
    pub fps: f32,
    pub width: u32,
    pub height: u32,
    pub codec: OutputCodec,
    pub format: OutputFormat,
    pub quality: u32,
    pub rate_control: RateControl,
    pub bitrate_mbps: f32,
    pub encoder_preset: EncoderPreset,
    pub motion_blur: bool,
    pub motion_blur_frames: u32,
    pub output_path: Option<PathBuf>,
    pub export_frames: bool,
    pub frame_format: String,
    pub write_xmp: bool,
    pub sharpen: f32,
    pub deflicker: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            fps: 25.0,
            width: 1920,
            height: 1080,
            codec: OutputCodec::H264,
            format: OutputFormat::Mp4,
            quality: 16,
            rate_control: RateControl::Crf,
            bitrate_mbps: 20.0,
            encoder_preset: EncoderPreset::Balanced,
            motion_blur: false,
            motion_blur_frames: 2,
            output_path: None,
            export_frames: false,
            frame_format: "png".into(),
            write_xmp: true,
            sharpen: 0.0,
            deflicker: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Sequence {
    pub folder: PathBuf,
    pub name: String,
    pub images: Vec<ImageEntry>,
    pub settings: SequenceSettings,
    pub workflow: WorkflowState,
    pub render: RenderSettings,
    pub sort_by_time: bool,
    pub hdr: bool,
    pub saved_at: Option<String>,
}

impl Default for Sequence {
    fn default() -> Self {
        Self {
            folder: PathBuf::new(),
            name: String::new(),
            images: Vec::new(),
            settings: SequenceSettings::default(),
            workflow: WorkflowState::default(),
            render: RenderSettings::default(),
            sort_by_time: false,
            hdr: false,
            saved_at: None,
        }
    }
}

impl Sequence {
    pub fn project_dir(folder: &Path) -> PathBuf {
        folder.join(PROJECT_DIR)
    }

    pub fn project_path(folder: &Path) -> PathBuf {
        Self::project_dir(folder).join(PROJECT_FILE)
    }

    pub fn load(folder: &Path) -> Result<Option<Self>> {
        let path = Self::project_path(folder);
        if !path.exists() {
            return Ok(None);
        }
        let data =
            std::fs::read(&path).with_context(|| format!("read project {}", path.display()))?;
        let mut seq: Sequence = serde_json::from_slice(&data)
            .with_context(|| format!("parse project {}", path.display()))?;
        seq.folder = folder.to_path_buf();
        Ok(Some(seq))
    }

    pub fn save(&mut self) -> Result<()> {
        self.folder = self
            .folder
            .canonicalize()
            .unwrap_or_else(|_| self.folder.clone());
        let dir = Self::project_dir(&self.folder);
        std::fs::create_dir_all(&dir)?;
        self.saved_at = Some(chrono::Local::now().to_rfc3339());
        let data = serde_json::to_vec_pretty(self)?;
        let path = dir.join(PROJECT_FILE);
        let tmp = dir.join(format!("{PROJECT_FILE}.tmp"));
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn keyframe_indices(&self) -> Vec<usize> {
        self.images
            .iter()
            .enumerate()
            .filter(|(_, im)| im.keyframe)
            .map(|(i, _)| i)
            .collect()
    }

    pub fn selected_or_all(&self, selected: Option<usize>) -> Vec<usize> {
        match selected {
            Some(i) if i < self.images.len() => vec![i],
            _ => (0..self.images.len()).collect(),
        }
    }

    pub fn luma_values(&self) -> Vec<f32> {
        self.images.iter().map(|im| im.luma).collect()
    }

    /// Render-time luminance: analyzed luma scaled by the current exposure correction.
    pub fn corrected_luma(&self) -> Vec<f32> {
        self.images
            .iter()
            .map(|im| (im.luma.max(1e-5) * (im.adj.total_ev()).exp2()).clamp(1e-5, 1.0))
            .collect()
    }

    pub fn sort_images(&mut self) {
        if self.sort_by_time {
            self.images.sort_by(|a, b| {
                a.timestamp
                    .partial_cmp(&b.timestamp)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| natural_cmp(&a.file_name, &b.file_name))
            });
        } else {
            self.images
                .sort_by(|a, b| natural_cmp(&a.file_name, &b.file_name));
        }
    }

    pub fn next_keyframe(&self, from: usize) -> Option<usize> {
        self.images
            .iter()
            .enumerate()
            .skip(from + 1)
            .find(|(_, im)| im.keyframe)
            .map(|(i, _)| i)
    }

    pub fn prev_keyframe(&self, from: usize) -> Option<usize> {
        (0..from.min(self.images.len()))
            .rev()
            .find(|&i| self.images[i].keyframe)
    }

    pub fn reset_adjustments(&mut self) {
        for im in &mut self.images {
            im.adj = Adjustments::default();
        }
    }
}

/// Filename ordering that keeps numeric runs in numeric order (IMG_2 before IMG_10).
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ac), Some(bc)) => {
                if ac.is_ascii_digit() && bc.is_ascii_digit() {
                    let mut an = String::new();
                    while let Some(c) = ai.peek().copied().filter(|c| c.is_ascii_digit()) {
                        an.push(c);
                        ai.next();
                    }
                    let mut bn = String::new();
                    while let Some(c) = bi.peek().copied().filter(|c| c.is_ascii_digit()) {
                        bn.push(c);
                        bi.next();
                    }
                    let av = an.trim_start_matches('0');
                    let bv = bn.trim_start_matches('0');
                    let ord = av.len().cmp(&bv.len()).then_with(|| av.cmp(bv));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                } else {
                    let ord = ac.to_ascii_lowercase().cmp(&bc.to_ascii_lowercase());
                    if ord != Ordering::Equal {
                        return ord;
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

pub fn supported_extension(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some(
            "jpg"
                | "jpeg"
                | "png"
                | "tif"
                | "tiff"
                | "dng"
                | "cr2"
                | "cr3"
                | "nef"
                | "arw"
                | "raf"
                | "orf"
                | "rw2"
                | "srw"
                | "pef"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_sorts_numbers() {
        let mut v = vec!["IMG_10.JPG", "IMG_2.JPG", "IMG_1.JPG"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["IMG_1.JPG", "IMG_2.JPG", "IMG_10.JPG"]);
    }

    #[test]
    fn project_roundtrip() {
        let dir = std::env::temp_dir().join(format!("lrtassist-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut seq = Sequence {
            folder: dir.clone(),
            name: "test".into(),
            ..Sequence::default()
        };
        seq.images.push(ImageEntry {
            file_name: "a.jpg".into(),
            luma: 0.5,
            ..ImageEntry::default()
        });
        seq.settings.hg_stretch = 0.4;
        seq.save().unwrap();
        let back = Sequence::load(&dir).unwrap().unwrap();
        assert_eq!(back.images.len(), 1);
        assert!((back.settings.hg_stretch - 0.4).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
