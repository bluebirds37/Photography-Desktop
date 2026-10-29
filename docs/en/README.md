# Timelapse Assistant · English Documentation

**English** · [简体中文](../zh/README.md)

> Version 0.1.0 · Rust 2021 + GPUI 0.2.2 · macOS / Windows

Timelapse Assistant is a standalone timelapse post-production suite. It brings sequence analysis, keyframe grading, Holy Grail exposure compensation, keyframe transitions, visual deflicker, XMP sidecars and video rendering into one desktop application — no Lightroom, After Effects, Java or external scripts required.

![Main window](../assets/screenshots/overview.png)

## Documentation map

| # | Document | Contents | Key images |
|---|----------|----------|------------|
| 1 | [Installation and first run](01-installation.md) | Requirements, source build, launch arguments, first run, formats, project layout | Empty state, settings |
| 2 | [Interface overview](02-interface.md) | Window regions, top bar, sequence list, preview, graph, right tabs, status bar | Main window, list, graph |
| 3 | [Sequences and keyframes](03-sequences-and-keyframes.md) | Import validation, ordering, thumbnails, playback, keyframe semantics, autosave | Sequence list, keyframe preview |
| 4 | [Per-frame adjustments](04-adjustments.md) | Ten parameters, ranges and steps, reset, EV composition, XMP mapping | Adjust tab, preview |
| 5 | [Tools: HG / transition / deflicker / XMP](05-tools.md) | Principles, parameters, steps and acceptance checks | Tools tab, four cards |
| 6 | [Rendering and output](06-rendering.md) | Resolution, fps, codecs, CRF/bitrate, presets, motion blur, progress, output rules | Render tab, progress, completion |
| 7 | [Settings and FFmpeg](07-settings-ffmpeg.md) | Settings dialog, component store, import/download/switch/uninstall, version list | Settings dialog |
| 8 | [CLI and file formats](08-cli-and-files.md) | Every `tlcore` command, `project.json`, XMP fields, directory layout | Main window, XMP toast |
| 9 | [Architecture and implementation](09-architecture.md) | Modules, threading, data flow, render pipeline, tests | Data-flow diagram |
| 10 | [Troubleshooting](10-troubleshooting.md) | Common issues, diagnostics, recovery | Settings, render tab |

## Three-minute start

```bash
# 1. Build (same on macOS and Windows)
cargo build --release

# 2. Launch with a sequence folder
#    macOS
open "dist/Timelapse Assistant.app" --args "/path/to/sequence"
#    or from a terminal
target/release/timelapse_assistant "/path/to/sequence"
#    Windows
target\release\timelapse_assistant.exe "D:\photos\sequence"
```

The app scans the folder immediately, parses EXIF, builds thumbnails and the luminance graph. Then:

1. Mark keyframes in the left list or the transport bar;
2. Run the Holy Grail wizard on the Tools tab;
3. Grade the first and last keyframes, then click **Apply auto transition**;
4. Run **Visual deflicker**;
5. Pick output settings on the Render tab and click **Start render**.

You can skip the UI entirely and run the same pipeline with `tlcore`:

```bash
tlcore demo ./.demo
tlcore analyze ./.demo
tlcore hg ./.demo
tlcore keyframes ./.demo
tlcore transition ./.demo
tlcore deflicker ./.demo
tlcore xmp ./.demo
tlcore render ./.demo ./out.mp4
```

## Conventions

- Ranges are written as `min … max (step)`, for example `−5 … +5 (0.05)`.
- Frame numbers are 1-based in the UI and in the `--frame` argument.
- Screenshots come from the macOS build. The Windows build has identical layout, labels and interaction; only window buttons and file dialogs differ.
- There are no global keyboard shortcuts. Every action is performed with the mouse through clicks, trackpad or wheel. Launch arguments are documented in pages 1 and 8.
- "Project directory" means `.lrtassist/` inside the sequence folder. Move it together with the sequence.

## Feature summary

| Capability | Details |
|------------|---------|
| Sequence import | JPEG / PNG / TIFF / DNG / CR2 / CR3 / NEF / ARW / RAF / ORF / RW2 / SRW / PEF |
| RAW handling | `rawler` demosaic first; embedded JPEG preview as fallback |
| Analysis | EXIF exposure triple, capture time, mean luminance, gray-world WB proxy |
| Keyframes | Manual marking, first/last shortcut, previous/next, automatic keyframing |
| Holy Grail | Offset / stretch / rotate / initial smoothing, 352-evaluation optimizer |
| Auto transition | Ten adjustment columns interpolated independently, linear or smoothstep |
| Visual deflicker | LOESS smoothing + multi-pass EV correction, coarse/normal/fine, constant mode |
| XMP | Camera Raw compatible fields + `lrt:` markers, unrelated metadata preserved |
| Rendering | 1080p / 4K / vertical, 24/25/30/60 fps, H.264 / H.265 / ProRes |
| Project persistence | Atomic `.lrtassist/project.json`, 2-second debounced autosave |
| Headless CLI | `tlcore` covers analysis, HG, transition, deflicker, XMP, render and FFmpeg management |

---

Next: [Installation and first run →](01-installation.md)
