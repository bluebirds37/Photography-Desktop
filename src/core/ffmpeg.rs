//! Managed FFmpeg component store.
//!
//! Components live next to the executable (`<app>/components/ffmpeg/<version>`)
//! with an AppData fallback when the install directory is read-only. On first
//! run the app imports a system FFmpeg if one exists, otherwise downloads the
//! official gyan.dev essentials build. Settings can install, switch, uninstall
//! versions, import a system copy, or fall back to the system executable.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};

pub const LATEST_URL: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip";
/// Fast npm mirror package (FFmpeg 4.1, static, ~22 MB) used for first-run
/// auto-install in networks where the official downloads are slow.
pub const MIRROR_URL: &str =
    "https://registry.npmmirror.com/@ffmpeg-installer/win32-x64/-/win32-x64-4.1.0.tgz";

/// Name of the FFmpeg executable on this platform.
pub const EXE_NAME: &str = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
/// Both bundled download sources ship Windows builds only.
pub const DOWNLOADABLE: bool = cfg!(windows);

#[derive(Clone, Debug)]
pub enum InstallEvent {
    Stage(String),
    Progress {
        stage: String,
        downloaded: u64,
        total: Option<u64>,
    },
    Finished(Result<String, String>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManagedVersion {
    pub version: String,
    pub dir: String,
    pub source: String,
    pub installed_at: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FfmpegStatus {
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    /// "managed", "system", "env", or "none".
    pub source: String,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Manifest {
    selected: Option<String>,
    versions: Vec<ManagedVersion>,
    auto_attempted: bool,
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Runtime component root. Prefer the application directory (portable layout),
/// fall back to LocalAppData when the app is installed read-only.
pub fn components_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let local = exe_dir().join("components");
        if fs::create_dir_all(&local).is_ok() {
            return local;
        }
        let fallback = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("TimelapseAssistant")
            .join("components");
        let _ = fs::create_dir_all(&fallback);
        fallback
    })
    .clone()
}

pub fn ffmpeg_root() -> PathBuf {
    components_root().join("ffmpeg")
}

fn manifest_path() -> PathBuf {
    ffmpeg_root().join("versions.json")
}

fn load_manifest() -> Manifest {
    let path = manifest_path();
    fs::read(&path)
        .ok()
        .and_then(|d| serde_json::from_slice(&d).ok())
        .unwrap_or_default()
}

fn save_manifest(m: &Manifest) -> Result<()> {
    fs::create_dir_all(ffmpeg_root())?;
    let path = manifest_path();
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

pub fn managed_versions() -> Vec<ManagedVersion> {
    load_manifest()
        .versions
        .into_iter()
        .filter(|v| version_exe(v).exists())
        .collect()
}

fn version_exe(v: &ManagedVersion) -> PathBuf {
    ffmpeg_root().join(&v.dir).join(EXE_NAME)
}

pub fn selected_version() -> Option<ManagedVersion> {
    let m = load_manifest();
    let selected = m.selected.as_ref()?;
    m.versions
        .into_iter()
        .find(|v| &v.version == selected && version_exe(v).exists())
}

pub fn select(version: Option<&str>) -> Result<()> {
    let mut m = load_manifest();
    if let Some(v) = version {
        if !m.versions.iter().any(|x| x.version == v) {
            return Err(anyhow!("未安装该 FFmpeg 版本: {v}"));
        }
    }
    m.selected = version.map(|s| s.to_string());
    save_manifest(&m)
}

pub fn uninstall(version: &str) -> Result<()> {
    let mut m = load_manifest();
    let Some(entry) = m.versions.iter().find(|v| v.version == version).cloned() else {
        return Ok(());
    };
    let dir = ffmpeg_root().join(&entry.dir);
    // Only remove directories that are directly under our component root.
    if dir.parent() == Some(ffmpeg_root().as_path()) && dir.exists() {
        fs::remove_dir_all(&dir).with_context(|| format!("delete {}", dir.display()))?;
    }
    m.versions.retain(|v| v.version != version);
    if m.selected.as_deref() == Some(version) {
        m.selected = m.versions.first().map(|v| v.version.clone());
    }
    save_manifest(&m)
}

fn system_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in ["ffmpeg.exe", "ffmpeg"] {
                let p = dir.join(name);
                if p.exists() {
                    out.push(p);
                }
            }
        }
    }
    // GUI launches (Finder/Dock) get a minimal PATH, so probe the usual
    // install locations directly as well.
    for p in [
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
        r"C:\Program Files\ffmpeg\bin\ffmpeg.exe",
        r"C:\Program Files (x86)\ffmpeg\bin\ffmpeg.exe",
        r"C:\ffmpeg\bin\ffmpeg.exe",
    ] {
        let p = PathBuf::from(p);
        if p.exists() {
            out.push(p);
        }
    }
    out
}

pub fn system_ffmpeg() -> Option<PathBuf> {
    system_candidates().into_iter().next()
}

pub fn find_ffmpeg() -> Option<PathBuf> {
    if let Some(v) = selected_version() {
        return Some(version_exe(&v));
    }
    if let Ok(p) = std::env::var("TIMELAPSE_FFMPEG") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    system_ffmpeg()
}

pub fn status() -> FfmpegStatus {
    if let Some(v) = selected_version() {
        return FfmpegStatus {
            path: Some(version_exe(&v)),
            version: Some(v.version),
            source: "managed".into(),
        };
    }
    if let Ok(p) = std::env::var("TIMELAPSE_FFMPEG") {
        let p = PathBuf::from(p);
        if p.exists() {
            return FfmpegStatus {
                version: detect_version(&p),
                path: Some(p),
                source: "env".into(),
            };
        }
    }
    if let Some(p) = system_ffmpeg() {
        return FfmpegStatus {
            version: detect_version(&p),
            path: Some(p),
            source: "system".into(),
        };
    }
    FfmpegStatus {
        path: None,
        version: None,
        source: "none".into(),
    }
}

pub fn detect_version(exe: &Path) -> Option<String> {
    let out = Command::new(exe).arg("-version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next()?;
    let idx = first.find("version ")? + "version ".len();
    let token = first[idx..].split_whitespace().next()?;
    let head = token.split('-').next()?;
    if head.contains('.') && head.chars().all(|c| c.is_ascii_digit() || c == '.') {
        Some(head.to_string())
    } else {
        // Nightly builds report tokens such as N-92722-gf22fcd4483.
        None
    }
}

fn sanitize_dir(version: &str) -> String {
    version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('.')
        .to_string()
}

fn unique_dir(version: &str, m: &Manifest) -> String {
    let base = sanitize_dir(version);
    let base = if base.is_empty() {
        "ffmpeg".to_string()
    } else {
        base
    };
    let mut candidate = base.clone();
    let mut n = 2;
    while m.versions.iter().any(|v| v.dir == candidate) || ffmpeg_root().join(&candidate).exists() {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    candidate
}

fn find_named(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = find_named(&p, name) {
                return Some(found);
            }
        } else if p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.eq_ignore_ascii_case(name))
            .unwrap_or(false)
        {
            return Some(p);
        }
    }
    None
}

/// Copy an executable together with sibling ffprobe/ffplay from its `bin` folder.
fn copy_runtime(from_exe: &Path, to_dir: &Path) -> Result<()> {
    let from_dir = from_exe
        .parent()
        .ok_or_else(|| anyhow!("ffmpeg has no parent directory"))?;
    fs::create_dir_all(to_dir)?;
    for entry in fs::read_dir(from_dir)? {
        let entry = entry?;
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        let lower = name.to_ascii_lowercase();
        let wanted = ["ffmpeg", "ffprobe", "ffplay"]
            .iter()
            .any(|b| lower == *b || lower == format!("{b}.exe"))
            || lower.ends_with(".dll");
        if wanted {
            fs::copy(&p, to_dir.join(name.as_ref()))
                .with_context(|| format!("copy {}", p.display()))?;
        }
    }
    if !to_dir.join(EXE_NAME).exists() {
        return Err(anyhow!("未找到 {} 可执行文件", EXE_NAME));
    }
    Ok(())
}

fn register(version: String, dir: String, source: &str) -> Result<ManagedVersion> {
    let mut m = load_manifest();
    m.versions.retain(|v| v.version != version);
    let entry = ManagedVersion {
        version: version.clone(),
        dir,
        source: source.into(),
        installed_at: chrono::Local::now().to_rfc3339(),
    };
    m.versions.push(entry.clone());
    m.selected = Some(version);
    save_manifest(&m)?;
    Ok(entry)
}

fn install_from_extracted(
    src_exe: &Path,
    label: Option<&str>,
    source: &str,
) -> Result<ManagedVersion> {
    let detected = detect_version(src_exe);
    let version = detected
        .or_else(|| label.map(|s| s.to_string()))
        .ok_or_else(|| anyhow!("无法识别 FFmpeg 版本"))?;
    let mut m = load_manifest();
    let dir = unique_dir(&version, &m);
    let target = ffmpeg_root().join(&dir);
    // If replacing an existing managed version, remove its old directory.
    if let Some(old) = m.versions.iter().find(|v| v.version == version).cloned() {
        let old_dir = ffmpeg_root().join(&old.dir);
        if old_dir.parent() == Some(ffmpeg_root().as_path()) && old_dir.exists() {
            let _ = fs::remove_dir_all(&old_dir);
        }
    }
    copy_runtime(src_exe, &target)?;
    // The manifest may have changed during copy; refresh before registering.
    m = load_manifest();
    let dir = if target.exists() {
        dir
    } else {
        m.versions.first().map(|v| v.dir.clone()).unwrap_or(dir)
    };
    register(version, dir, source)
}

pub fn install_from_zip(
    zip_path: &Path,
    source: &str,
    tx: Option<Sender<InstallEvent>>,
) -> Result<ManagedVersion> {
    let staging = ffmpeg_root()
        .join("downloads")
        .join(format!("staging-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    if let Some(tx) = &tx {
        let _ = tx.send(InstallEvent::Stage("解压 FFmpeg".into()));
    }
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = staging.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut f = File::create(&out)?;
            std::io::copy(&mut entry, &mut f)?;
        }
    }
    let exe = find_named(&staging, EXE_NAME).ok_or_else(|| {
        anyhow!(
            "压缩包中没有 {}（该平台需要 {} 可执行文件）",
            EXE_NAME,
            EXE_NAME
        )
    })?;
    let label = zip_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());
    let result = install_from_extracted(&exe, label.as_deref(), source);
    let _ = fs::remove_dir_all(&staging);
    result
}

pub fn install_from_tar_gz(
    archive_path: &Path,
    source: &str,
    tx: Option<Sender<InstallEvent>>,
) -> Result<ManagedVersion> {
    let staging = ffmpeg_root()
        .join("downloads")
        .join(format!("staging-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    if let Some(tx) = &tx {
        let _ = tx.send(InstallEvent::Stage("解压 FFmpeg".into()));
    }
    let file = File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(&staging)?;
    let exe = find_named(&staging, EXE_NAME).ok_or_else(|| {
        anyhow!(
            "压缩包中没有 {}（该平台需要 {} 可执行文件）",
            EXE_NAME,
            EXE_NAME
        )
    })?;
    let label = find_named(&staging, "package.json")
        .and_then(|p| fs::read(p).ok())
        .and_then(|d| serde_json::from_slice::<serde_json::Value>(&d).ok())
        .and_then(|v| {
            v.get("version")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            archive_path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        });
    let result = install_from_extracted(&exe, label.as_deref(), source);
    let _ = fs::remove_dir_all(&staging);
    result
}

pub fn install_from_archive(
    path: &Path,
    source: &str,
    tx: Option<Sender<InstallEvent>>,
) -> Result<ManagedVersion> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.ends_with(".zip") {
        install_from_zip(path, source, tx)
    } else if name.ends_with(".tgz") || name.ends_with(".tar.gz") {
        install_from_tar_gz(path, source, tx)
    } else {
        Err(anyhow!("仅支持 .zip / .tgz / .tar.gz 压缩包"))
    }
}

pub fn download_and_install(url: &str, tx: Sender<InstallEvent>) -> Result<ManagedVersion> {
    fs::create_dir_all(ffmpeg_root().join("downloads"))?;
    let is_tar = url.contains(".tgz") || url.contains(".tar.gz");
    let extension = if is_tar { "tgz" } else { "zip" };
    let part = ffmpeg_root()
        .join("downloads")
        .join(format!("ffmpeg-{}.part", std::process::id()));
    let archive_path = part.with_extension(extension);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(25))
        .timeout_read(Duration::from_secs(120))
        .build();
    let _ = tx.send(InstallEvent::Stage("连接下载服务器".into()));
    let resp = agent
        .get(url)
        .call()
        .with_context(|| format!("download {url}"))?;
    let total = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok());
    let mut reader = resp.into_reader();
    let mut file = File::create(&part)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut downloaded = 0u64;
    let mut last_report = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;
        if downloaded - last_report > 1024 * 1024 || Some(downloaded) == total {
            last_report = downloaded;
            let _ = tx.send(InstallEvent::Progress {
                stage: "下载 FFmpeg".into(),
                downloaded,
                total,
            });
        }
    }
    file.flush()?;
    drop(file);
    fs::rename(&part, &archive_path)?;
    let result = if is_tar {
        install_from_tar_gz(&archive_path, "mirror", Some(tx.clone()))
    } else {
        install_from_zip(&archive_path, "download", Some(tx.clone()))
    };
    let _ = fs::remove_file(&archive_path);
    result
}

pub fn import_system() -> Result<ManagedVersion> {
    let sys = system_ffmpeg().ok_or_else(|| anyhow!("未找到系统 FFmpeg"))?;
    let mut m = load_manifest();
    // If PATH already points at our managed store, just select it.
    if let Some(v) = m.versions.iter().find(|v| {
        version_exe(v) == sys
            || fs::canonicalize(version_exe(v)).ok() == fs::canonicalize(&sys).ok()
    }) {
        let version = v.version.clone();
        m.selected = Some(version.clone());
        save_manifest(&m)?;
        return Ok(v.clone());
    }
    let detected = detect_version(&sys);
    if let Some(version) = &detected {
        if let Some(existing) = m.versions.iter().find(|v| &v.version == version) {
            let existing = existing.clone();
            m.selected = Some(existing.version.clone());
            save_manifest(&m)?;
            return Ok(existing);
        }
    }
    let version = detected.unwrap_or_else(|| "system".into());
    let dir = unique_dir(&version, &m);
    let target = ffmpeg_root().join(&dir);
    copy_runtime(&sys, &target)?;
    register(version, dir, "system")
}

/// Called once on first launch. Imports a system build when available, otherwise
/// downloads the official essentials build.
pub fn should_auto_install() -> bool {
    let m = load_manifest();
    !m.auto_attempted && m.versions.is_empty()
}

pub fn auto_install(tx: Sender<InstallEvent>) {
    let m = load_manifest();
    if m.auto_attempted || !m.versions.is_empty() {
        return;
    }
    let result = if system_ffmpeg().is_some() {
        let _ = tx.send(InstallEvent::Stage("导入系统 FFmpeg".into()));
        import_system().map(|v| v.version)
    } else if DOWNLOADABLE {
        // The npm mirror is fast in mainland China; fall back to the official
        // latest essentials build if the mirror is unavailable.
        match download_and_install(MIRROR_URL, tx.clone()) {
            Ok(v) => Ok(v.version),
            Err(first) => {
                let _ = tx.send(InstallEvent::Stage(format!(
                    "镜像安装失败，尝试官方源: {first}"
                )));
                download_and_install(LATEST_URL, tx.clone()).map(|v| v.version)
            }
        }
    } else {
        Err(anyhow!("未在系统中找到 FFmpeg，请在设置中导入"))
    };
    let mut m = load_manifest();
    m.auto_attempted = true;
    let _ = save_manifest(&m);
    let _ = tx.send(InstallEvent::Finished(result.map_err(|e| format!("{e:#}"))));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_versions() {
        assert_eq!(sanitize_dir("7.1.1 / full"), "7.1.1___full");
        assert_eq!(sanitize_dir("n7.1"), "n7.1");
    }

    #[test]
    fn detects_local_ffmpeg() {
        let path = find_ffmpeg();
        if let Some(p) = path {
            assert!(p.exists());
            if let Some(v) = detect_version(&p) {
                assert!(!v.is_empty());
            }
        }
    }
}
