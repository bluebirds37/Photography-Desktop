# 2. Interface overview

**English** · [简体中文](../zh/02-interface.md) · [Documentation home](README.md)

The window is a fixed five-region layout: a custom title bar, a top toolbar, a left sequence list, a centre preview with graph, and right-hand work tabs, plus a status bar. Settings do not occupy a right-hand tab; they open as a modal from the gear in the top-right corner.

![Full interface](../assets/screenshots/overview.png)

## 2.1 Window regions

| Region | Size | Purpose |
|--------|------|---------|
| Title bar | 34 px high | Drag the window; native close / minimize / maximize |
| Top toolbar | 58 px high | Open, save, rescan, workflow state, FFmpeg, render, settings |
| Left sequence list | 284 px wide | Browse and select frames, see EV and luminance |
| Centre preview | flexible | Image preview, transport, luminance / exposure graph |
| Right work tabs | 360 px wide | Adjust, Tools, Render |
| Status bar | 28 px high | Current action, progress, frame count, fps, luminance, EV, path, toast |

The window defaults to 1480×920 with a minimum of 1180×720. At minimum size the preview compresses while the side panels keep their fixed widths.

## 2.2 Top toolbar

Left to right:

| Control | Behaviour |
|---------|-----------|
| `TL` mark and subtitle | Decorative |
| Open folder | Pick a directory and import a sequence |
| Save | Write `project.json` immediately (normally 2-second debounced autosave) |
| Rescan | Re-import the current directory; filenames, luminance and thumbnails refresh |
| Workflow steps | Initialize → Keyframes → HG wizard → Auto transition → Deflicker → Render; completed steps are highlighted |
| FFmpeg pill | Shows source/version; click to open Settings |
| Render video | Same as **Start render** on the Render tab; checks FFmpeg first |
| Gear | Open / close the Settings dialog |

**Save**, **Rescan**, **Render video** and the workflow steps appear only after a sequence is loaded. Without a sequence the top bar keeps only Open folder, the FFmpeg pill and the gear.

> The workflow strip is read-only and never navigates. It reflects the `workflow` field in the project file and survives a restart.

## 2.3 Left sequence list

![Sequence list](../assets/screenshots/sequence-list.png)

- The header shows **Sequence**, total frame count and keyframe count, plus the hint "click to select a frame";
- Each row shows an index (`#0001`), thumbnail, filename, exposure correction (`+0.35 EV`) and mean luminance (`L 0.482`);
- The selected row is outlined in the accent colour; keyframes carry a yellow marker;
- Clicking any row switches the current frame. Selection does not start playback;
- The list scrolls. Importing leaves you on frame 1; `--frame N` selects the requested frame;
- The empty state shows a "No sequence loaded" card and an **Open folder** button.

## 2.4 Centre preview

Once a sequence is loaded the preview shows the **graded** current frame. The info strip above it contains the frame number, keyframe badge, total EV and luminance.

![Keyframe preview](../assets/screenshots/preview-keyframe.png)

### Transport

| Control | Behaviour |
|---------|-----------|
| Previous / Next | Move one frame; does not wrap at either end |
| Play / Pause | Plays at the render fps, wrapping from the last frame to the first |
| Frame number | `current / total`, both 1-based |
| Mark keyframe | Toggles the current frame; the status bar reports "Keyframe marked / cleared" |
| Previous / Next keyframe | Jumps to the adjacent keyframe; stays put when none exists |

Playback is preview-only and never writes to the project. When it stops, the current frame stays where it stopped.

### Luminance / exposure graph

![Luminance and exposure graph](../assets/screenshots/preview-curve.png)

The graph draws:

- **Luminance**: the mean luminance analysed for each frame;
- **Exposure**: each frame's total EV (creative exposure + deflicker correction);
- **Keyframes**: vertical markers at keyframe positions.

The top of the graph shows the luminance range `L min – max`. Use it to inspect the day-to-night trend, exposure steps and deflicker results. The graph is not clickable; select frames in the list or with the transport.

## 2.5 Right work tabs

The right panel has three tabs: **Adjust**, **Tools** and **Render**. Switching tabs does not change the current frame or the project state.

### Adjust

![Adjust tab](../assets/screenshots/adjust.png)

- Header **Single-frame adjustments**, with a yellow badge when the current frame is a keyframe;
- **Reset** restores all ten creative parameters of the current frame to defaults;
- Shows the frame number and filename, with the **Auto keyframe** switch on the right;
- Three cards: Exposure & white balance, Tone, Colour;
- The **Sequence overview** at the bottom summarises frame and keyframe counts, duration, mean luminance, deflicker before/after, output format and FFmpeg state.

See [Per-frame adjustments](04-adjustments.md) for ranges and behaviour.

### Tools

Three cards — Holy Grail wizard, Keyframe auto transition, Visual deflicker — plus an **Export XMP sidecars** button. See [Tools](05-tools.md).

### Render

Three cards: Output format, Quality & motion, Output. If FFmpeg is missing, an extra notice card with **Go to Settings** appears at the top. See [Rendering and output](06-rendering.md).

## 2.6 Status bar

From left to right:

1. Status dot: green when a sequence is loaded, accent blue while busy or rendering;
2. Current action text, e.g. "Ready", "Rendering 155/180", "Render complete: 180 frames";
3. Render or analysis progress bar and percentage (only while running);
4. `frames · duration · fps`;
5. `Luminance 0.xxx` and `EV +0.00`;
6. Sequence folder path;
7. A temporary toast (e.g. "Exported 180 XMP files") that disappears after 4 seconds.

## 2.7 Settings dialog

![Settings dialog](../assets/screenshots/settings.png)

- Opened from the gear or the FFmpeg pill;
- A centred 712 px modal with a dimmed backdrop;
- The main card on the left manages FFmpeg components; the right column holds version, path and action notes;
- Close with the `×` in the top-right or by clicking the backdrop;
- The UI behind remains visible, but the backdrop intercepts clicks to prevent accidental edits.

Settings contains only app-level preferences (currently FFmpeg). It is deliberately separate from per-frame adjustments and render settings so image operations are not mixed with configuration. See [Settings and FFmpeg](07-settings-ffmpeg.md).

## 2.8 Interaction conventions

- No global keyboard shortcuts. Everything is done with the mouse.
- Sliders support clicking a track segment and `−` / `+` stepper buttons; dragging is not required, so trackpad and mouse behave the same.
- Values are displayed as `+0.35 EV`, `0.482`, `+12` and so on.
- Any edit marks the project dirty and saves after 2 seconds; closing the window also saves.
- While a long task runs (import, deflicker, render, FFmpeg install), the status dot turns blue and some buttons are disabled or ignore repeated clicks.

---

[← Previous: Installation and first run](01-installation.md) · [Next: Sequences and keyframes →](03-sequences-and-keyframes.md)
