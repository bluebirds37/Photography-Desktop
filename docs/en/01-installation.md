# 1. Installation and first run

**English** · [简体中文](../zh/01-installation.md) · [Documentation home](README.md)

This page covers system requirements, building from source, launch arguments, first-run behaviour, supported formats and the project layout. By the end you should have a sequence open with thumbnails and a luminance graph.

![Empty state on first launch](../assets/screenshots/empty-state.png)

## 1.1 Requirements

| Item | Requirement |
|------|-------------|
| OS | macOS 11 or newer (Apple Silicon / Intel); Windows 10 / 11 64-bit |
| Memory | 8 GB or more recommended; 4K sequences and multi-pass deflicker use more |
| Disk | About 30 MB for the app; thumbnails and render frames are extra |
| Build tools | Rust stable (2021 edition) and a C/C++ linker |
| FFmpeg | Not required up front. The first run imports a system build or guides installation |
| Display | Default window 1480×920, minimum 1180×720 |

> The UI uses a dark theme. The title bar is drawn by the app and keeps only the native minimize / maximize / close buttons.

## 1.2 Build from source

From the repository root:

```bash
cargo build --release
```

Outputs:

| Platform | Desktop app | CLI |
|----------|-------------|-----|
| macOS | `target/release/timelapse_assistant` | `target/release/tlcore` |
| Windows | `target\release\timelapse_assistant.exe` | `target\release\tlcore.exe` |

Most build time goes into GPUI and the image decoders. To verify algorithms without opening a window, run `cargo test --release`.

### macOS app bundle

The repository ships a runnable bundle at `dist/Timelapse Assistant.app`. After rebuilding, copy the new binary into it:

```bash
cargo build --release
cp target/release/timelapse_assistant \
   "dist/Timelapse Assistant.app/Contents/MacOS/timelapse_assistant"
open "dist/Timelapse Assistant.app"
```

An unsigned build downloaded from the network may be blocked by Gatekeeper. Allow it under **System Settings → Privacy & Security**, or rebuild locally.

### Windows executable

```powershell
cargo build --release
.\target\release\timelapse_assistant.exe
```

A release folder typically looks like:

```text
dist/
  timelapse_assistant.exe
  tlcore.exe
  components/            # created on first run
```

## 1.3 Launch arguments

The desktop app accepts a positional folder and two options:

```text
timelapse_assistant [folder] [--tab adjust|tools|render|settings] [--frame N]
```

| Argument | Effect | Example |
|----------|--------|---------|
| `folder` | Import this sequence immediately; must be an existing directory | `timelapse_assistant ~/Pictures/sunset` |
| `--tab` | Open the given right-hand tab; `settings` also opens the settings dialog and stays on Adjust | `--tab render` |
| `--frame` | Select frame N (1-based); out-of-range values keep frame 1 | `--frame 91` |

Examples:

```bash
# Open a sequence directly on the Render tab
target/release/timelapse_assistant "/Volumes/SSD/sunset" --tab render

# Open a sequence at its middle frame
target/release/timelapse_assistant "/Volumes/SSD/sunset" --frame 91
```

Started without arguments, the app shows the empty state; click **Open folder** to choose a directory.

## 1.4 First run and FFmpeg

FFmpeg is checked on first launch:

1. Search `PATH` and common install locations (`/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`);
2. When a system FFmpeg is found, import it into the component store and select it (the executable is copied on every platform);
3. If nothing is found: Windows tries the fast mirror first, then the official source; macOS does not auto-download and asks you to import one in Settings.

The FFmpeg pill in the top-right shows **FFmpeg <version>**, **FFmpeg system** or **FFmpeg required**. Click it to open Settings. See [Settings and FFmpeg](07-settings-ffmpeg.md).

![FFmpeg card in Settings](../assets/screenshots/settings.png)

> Even without FFmpeg you can import, grade, run HG / transition / deflicker and export XMP. Only **Start render** is blocked, and it opens Settings automatically.

## 1.5 Supported formats

| Category | Extensions | Notes |
|----------|------------|-------|
| Regular images | `.jpg` `.jpeg` `.png` `.tif` `.tiff` | Decoded directly |
| Camera RAW | `.dng` `.cr2` `.cr3` `.nef` `.arw` `.raf` `.orf` `.rw2` `.srw` `.pef` | `rawler` demosaic first, embedded preview fallback |

Import behaviour:

- Only the first level of the selected folder is scanned; subfolders are not traversed;
- Extensions are case-insensitive;
- Files are naturally sorted (`IMG_2` before `IMG_10`); if capture times are present, the project can be sorted by time instead;
- Each file gets a thumbnail with a 640 px long edge; analysis uses a 320 px preview;
- A failed decode does not abort the import. The error is stored on that frame and shown in the list.

## 1.6 Project directory

Importing creates the following inside the sequence folder:

```text
<sequence folder>/
  IMG_0001.JPG
  IMG_0002.JPG
  ...
  .lrtassist/
    project.json          # metadata, adjustments, keyframes, settings, workflow state
    thumbs/               # thumbnail cache
    render/frames/        # intermediate frames (deleted on completion or cancel)
    exported_frames/      # final frames when "Keep output frames" is enabled
```

- `project.json` is saved atomically (write temp file, then rename), so an interruption cannot corrupt it;
- Reopening the same folder reuses thumbnails; they are regenerated only when size or modification time changes;
- Deleting `.lrtassist/` clears all adjustments, keyframes and caches but never touches the original photos;
- Move or back up `.lrtassist/` together with the sequence to preserve the working state.

## 1.7 First-run checklist

1. The empty-state card and the **Open folder** button are visible;
2. Opening a folder with at least 10 JPEGs populates the thumbnail list;
3. The centre shows the current frame and the luminance / exposure graph;
4. The FFmpeg pill is green (**FFmpeg system** or a version number);
5. Moving any slider on the Adjust tab changes the EV value in the status bar;
6. After quitting and reopening the same folder, adjustments and keyframes are still there.

---

[← Documentation home](README.md) · [Next: Interface overview →](02-interface.md)
