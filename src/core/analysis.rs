//! Sequence import: filename ordering, EXIF, preview extraction and luminance
//! statistics. JPEG previews inside raw files are decoded with the JPEG DCT
//! scaler, which keeps import fast even for 50 MP captures.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use image::{DynamicImage, RgbaImage};
use rayon::prelude::*;

use crate::core::image_pipeline::{analyze_pixels, develop, fit, mean_luma, ToneTable};
use crate::core::model::{supported_extension, ImageEntry, Sequence, PROJECT_DIR, THUMB_DIR};

const THUMB_MAX: u32 = 640;

fn analysis_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads((num_cpus::get() / 2).clamp(2, 6))
            .build()
            .expect("analysis thread pool")
    })
}
const ANALYSIS_MAX: u32 = 320;

#[derive(Clone, Debug, Default)]
pub struct ExifInfo {
    pub timestamp: Option<f64>,
    pub iso: Option<f32>,
    pub shutter: Option<f32>,
    pub aperture: Option<f32>,
    pub exposure_bias: Option<f32>,
}

impl ExifInfo {
    /// Camera exposure setting in EV100. Changes here are what the Holy Grail
    /// wizard compensates.
    pub fn camera_ev(&self) -> Option<f32> {
        let (t, n, iso) = (self.shutter?, self.aperture?, self.iso.unwrap_or(100.0));
        if t <= 0.0 || n <= 0.0 || iso <= 0.0 {
            return None;
        }
        let ev = (n * n / t).log2() - (iso / 100.0).log2();
        Some(ev + self.exposure_bias.unwrap_or(0.0))
    }
}

pub fn read_exif(path: &Path) -> ExifInfo {
    let mut info = ExifInfo::default();
    let Ok(file) = File::open(path) else {
        return info;
    };
    let mut reader = BufReader::new(file);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) else {
        return info;
    };
    let get = |tag: exif::Tag| exif.get_field(tag, exif::In::PRIMARY);
    if let Some(f) = get(exif::Tag::ExposureTime) {
        if let exif::Value::Rational(ref v) = f.value {
            if let Some(r) = v.first() {
                if r.denom != 0 {
                    info.shutter = Some(r.num as f32 / r.denom as f32);
                }
            }
        }
    }
    if let Some(f) = get(exif::Tag::FNumber) {
        if let exif::Value::Rational(ref v) = f.value {
            if let Some(r) = v.first() {
                if r.denom != 0 {
                    info.aperture = Some(r.num as f32 / r.denom as f32);
                }
            }
        }
    }
    if let Some(f) = get(exif::Tag::PhotographicSensitivity) {
        info.iso = f.value.get_uint(0).map(|v| v as f32);
    }
    if let Some(f) = get(exif::Tag::ExposureBiasValue) {
        if let exif::Value::SRational(ref v) = f.value {
            if let Some(r) = v.first() {
                if r.denom != 0 {
                    info.exposure_bias = Some(r.num as f32 / r.denom as f32);
                }
            }
        }
    }
    let dt = get(exif::Tag::DateTimeOriginal)
        .or_else(|| get(exif::Tag::DateTime))
        .and_then(|f| f.display_value().to_string().parse::<String>().ok());
    if let Some(dt) = dt {
        if let Ok(naive) =
            chrono::NaiveDateTime::parse_from_str(dt.trim_matches('"'), "%Y-%m-%d %H:%M:%S")
        {
            info.timestamp = Some(naive.and_utc().timestamp() as f64);
        } else if let Ok(naive) =
            chrono::NaiveDateTime::parse_from_str(dt.trim_matches('"'), "%Y:%m:%d %H:%M:%S")
        {
            info.timestamp = Some(naive.and_utc().timestamp() as f64);
        }
    }
    info
}

fn is_jpeg(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg")
    )
}

fn is_raw(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("dng" | "cr2" | "cr3" | "nef" | "arw" | "raf" | "orf" | "rw2" | "srw" | "pef")
    )
}

/// Decode a JPEG at a reduced DCT scale.
pub fn decode_jpeg_scaled(path: &Path, max_dim: u32) -> Result<DynamicImage> {
    let img = image::open(path)?;
    Ok(fit(img, max_dim))
}

/// Render every cached thumbnail with the current adjustments and measure its
/// luminance. The deflicker pass therefore sees the graded image, not just the
/// original camera histogram.
pub fn visual_curve_from_thumbs(seq: &Sequence) -> Vec<f32> {
    let table = ToneTable::new();
    analysis_pool().install(|| {
        seq.images
            .par_iter()
            .map(|im| {
                let src = im
                    .thumb
                    .as_ref()
                    .and_then(|t| image::open(folder_thumb(seq, t)).ok())
                    .or_else(|| decode_preview(&im.path(&seq.folder), 480).ok());
                match src {
                    Some(img) => {
                        let mut adj = im.adj.clone();
                        adj.deflicker = 0.0;
                        let out = develop(&img, &adj, Some(480), &table);
                        mean_luma(&out).clamp(1e-5, 1.0)
                    }
                    None => (im.luma.max(1e-5) * im.adj.exposure.exp2()).clamp(1e-5, 1.0),
                }
            })
            .collect()
    })
}

fn folder_thumb(seq: &Sequence, rel: &str) -> PathBuf {
    seq.folder.join(PROJECT_DIR).join(rel)
}

/// Find the largest embedded JPEG and decode it. Many raw formats store a
/// full-size JPEG preview; this is both faster and more robust than demosaicing
/// for the analysis stage.
pub fn extract_embedded_jpeg(path: &Path) -> Result<DynamicImage> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    // Cap the scan at 96 MB to keep memory bounded on exotic files.
    let cap = len.min(96 * 1024 * 1024) as usize;
    let mut buf = vec![0u8; cap];
    file.read_exact(&mut buf)?;

    let mut best: Option<(usize, usize)> = None;
    let mut i = 0usize;
    while i + 3 < buf.len() {
        if buf[i] == 0xFF && buf[i + 1] == 0xD8 && buf[i + 2] == 0xFF {
            // Find the matching EOI. Nested SOI markers are ignored.
            let mut j = i + 2;
            let mut end = None;
            while j + 1 < buf.len() {
                if buf[j] == 0xFF && buf[j + 1] == 0xD9 {
                    end = Some(j + 2);
                    break;
                }
                j += 1;
            }
            if let Some(e) = end {
                let size = e - i;
                if best.map(|(s, _)| size > s).unwrap_or(true) {
                    best = Some((size, i));
                }
                i = e;
                continue;
            }
        }
        i += 1;
    }
    let (size, start) = best.ok_or_else(|| anyhow!("no embedded JPEG preview"))?;
    let img = image::load_from_memory(&buf[start..start + size])?;
    Ok(img)
}

/// Full decode, including raw demosaicing.
pub fn decode_full(path: &Path) -> Result<DynamicImage> {
    if is_jpeg(path)
        || matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .as_deref(),
            Some("png" | "tif" | "tiff")
        )
    {
        return image::open(path).with_context(|| format!("decode {}", path.display()));
    }
    #[cfg(feature = "rawler")]
    if let Ok(raw) = rawler::decode_file(path) {
        let dev = rawler::imgop::develop::RawDevelop::default();
        if let Ok(intermediate) = dev.develop_intermediate(&raw) {
            if let Some(img24) = intermediate.to_dynamic_image() {
                // rawler 0.6 depends on image 0.24; convert through raw RGBA bytes.
                let rgba = img24.to_rgba8();
                let (w, h) = (rgba.width(), rgba.height());
                let raw = rgba.into_raw();
                if let Some(buf) = RgbaImage::from_raw(w, h, raw) {
                    return Ok(DynamicImage::ImageRgba8(buf));
                }
            }
        }
    }
    // Fall back to the embedded preview for raw files we cannot demosaic.
    extract_embedded_jpeg(path)
}

/// Fast preview decode used by the browser and filmstrip.
pub fn decode_preview(path: &Path, max_dim: u32) -> Result<DynamicImage> {
    if is_jpeg(path) {
        return decode_jpeg_scaled(path, max_dim);
    }
    if is_raw(path) {
        match extract_embedded_jpeg(path) {
            Ok(img) => return Ok(fit(img, max_dim)),
            Err(_) => {
                #[cfg(feature = "rawler")]
                {
                    let full = decode_full(path)?;
                    return Ok(fit(full, max_dim));
                }
                #[allow(unreachable_code)]
                return Err(anyhow!("cannot decode raw preview"));
            }
        }
    }
    let img = image::open(path)?;
    Ok(fit(img, max_dim))
}

fn fnv1a(s: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn thumb_name(file_name: &str) -> String {
    format!("{:016x}.jpg", fnv1a(file_name))
}

fn save_thumb(img: &RgbaImage, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dynamic = DynamicImage::ImageRgba8(img.clone());
    let mut out = File::create(path)?;
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 86);
    enc.encode_image(&dynamic)?;
    Ok(())
}

fn file_modified(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

fn analyze_one(folder: &Path, name: &str) -> ImageEntry {
    let path = folder.join(name);
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let modified = file_modified(&path);
    let exif = read_exif(&path);
    let mut entry = ImageEntry {
        file_name: name.to_string(),
        timestamp: exif.timestamp,
        iso: exif.iso,
        shutter: exif.shutter,
        aperture: exif.aperture,
        camera_ev: exif.camera_ev(),
        size,
        modified,
        luma: 0.5,
        wb_r: 1.0,
        wb_b: 1.0,
        ..ImageEntry::default()
    };

    match decode_preview(&path, THUMB_MAX) {
        Ok(preview) => {
            entry.width = preview.width();
            entry.height = preview.height();
            let analysis = fit(preview.clone(), ANALYSIS_MAX).to_rgba8();
            let (luma, r, b) = analyze_pixels(&analysis);
            entry.luma = luma;
            entry.wb_r = r;
            entry.wb_b = b;
            let thumb = preview.to_rgba8();
            let rel = format!("{THUMB_DIR}/{}", thumb_name(name));
            let abs = folder.join(PROJECT_DIR).join(&rel);
            if let Err(e) = save_thumb(&thumb, &abs) {
                entry.error = Some(format!("thumbnail: {e}"));
            } else {
                entry.thumb = Some(rel);
            }
        }
        Err(e) => {
            entry.error = Some(format!("preview: {e}"));
        }
    }
    entry
}

/// Scan and analyze a folder. Existing entries are reused when the file size and
/// modification time are unchanged, preserving edits and keyframes.
pub fn import_folder(
    folder: &Path,
    existing: Option<Sequence>,
    progress: Option<&(dyn Fn(usize, usize, &str) + Sync)>,
) -> Result<Sequence> {
    let folder = folder
        .canonicalize()
        .unwrap_or_else(|_| folder.to_path_buf());
    if !folder.is_dir() {
        return Err(anyhow!("{} is not a directory", folder.display()));
    }
    let mut names: Vec<String> = std::fs::read_dir(&folder)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && supported_extension(Path::new(n)))
        .collect();
    names.sort_by(|a, b| crate::core::model::natural_cmp(a, b));
    if names.is_empty() {
        return Err(anyhow!("文件夹中没有支持的图像"));
    }

    let mut previous: HashMap<String, ImageEntry> = HashMap::new();
    let mut seq = existing.unwrap_or_else(|| Sequence {
        folder: folder.clone(),
        name: folder
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sequence".into()),
        ..Sequence::default()
    });
    for im in seq.images.drain(..) {
        previous.insert(im.file_name.clone(), im);
    }
    seq.folder = folder.clone();
    if seq.name.is_empty() {
        seq.name = folder
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sequence".into());
    }

    let total = names.len();
    let done = AtomicUsize::new(0);
    let entries: Vec<ImageEntry> = analysis_pool().install(|| {
        names
            .par_iter()
            .map(|name| {
                let path = folder.join(name);
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let modified = file_modified(&path);
                let reusable = previous.get(name).filter(|old| {
                    old.size == size
                        && old.modified == modified
                        && old
                            .thumb
                            .as_ref()
                            .is_some_and(|t| folder.join(PROJECT_DIR).join(t).exists())
                });
                let entry = if let Some(old) = reusable {
                    old.clone()
                } else {
                    analyze_one(&folder, name)
                };
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if let Some(cb) = progress {
                    cb(n, total, name);
                }
                entry
            })
            .collect()
    });

    seq.images = entries;
    seq.images
        .sort_by(|a, b| crate::core::model::natural_cmp(&a.file_name, &b.file_name));
    seq.save()?;
    Ok(seq)
}

/// Return the number of supported files without decoding anything.
pub fn count_supported(folder: &Path) -> usize {
    std::fs::read_dir(folder)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| supported_extension(Path::new(&e.file_name())))
                .count()
        })
        .unwrap_or(0)
}

pub fn thumb_abs_path(folder: &Path, rel: &str) -> PathBuf {
    folder.join(PROJECT_DIR).join(rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_ev_matches_stops() {
        let a = ExifInfo {
            shutter: Some(1.0 / 100.0),
            aperture: Some(4.0),
            iso: Some(100.0),
            ..ExifInfo::default()
        };
        let b = ExifInfo {
            shutter: Some(1.0 / 50.0),
            aperture: Some(4.0),
            iso: Some(100.0),
            ..ExifInfo::default()
        };
        assert!((a.camera_ev().unwrap() - b.camera_ev().unwrap() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn thumbnail_names_are_stable() {
        assert_eq!(thumb_name("IMG_0001.JPG"), thumb_name("IMG_0001.JPG"));
        assert_ne!(thumb_name("IMG_0001.JPG"), thumb_name("IMG_0002.JPG"));
    }
}
