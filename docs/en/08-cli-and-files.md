# 8. CLI and file formats

**English** · [简体中文](../zh/08-cli-and-files.md) · [Documentation home](README.md)

`tlcore` is the headless entry point. It covers the same analysis, Holy Grail, transition, deflicker, XMP and render pipeline as the UI, which makes it suitable for batch processing, CI and diagnostics. All commands read and write `.lrtassist/project.json` in the sequence folder.

![XMP export toast](../assets/screenshots/xmp-export.png)

## 8.1 Invocation

```bash
tlcore <command> [args]
```

- Exit code `0` on success;
- Writes `error: <reason>` to stderr and exits `1` on failure;
- With no arguments it returns `missing command`;
- An unknown command returns `unknown command <name>`.

## 8.2 Commands

| Command | Arguments | Purpose |
|---------|-----------|---------|
| `demo` | `<folder>` | Create a 180-frame 640×360 synthetic sequence |
| `analyze` | `<folder>` | Import, EXIF, thumbnails, luminance analysis; save the project |
| `hg` | `<folder>` | Apply the saved HG parameters |
| `hgopt` | `<folder>` | Search stretch/rotate and apply; 352 evaluations |
| `keyframes` | `<folder>` | Clear markers, then mark first, middle and last frames |
| `transition` | `<folder>` | Interpolate exposure, temperature and tint between keyframes |
| `deflicker` | `<folder>` | Compute deflicker EV from the thumbnail visual curve |
| `report` | `<folder>` | Print frame count, keyframes, luminance range, flicker score, exposure range |
| `xmp` | `<folder>` | Write XMP for every non-skipped image |
| `render` | `<folder> [out]` | Render and encode; default naming when `out` is omitted |
| `ffmpeg-status` | — | Source, version, path, store and installed versions |
| `ffmpeg-import` | — | Import system FFmpeg |
| `ffmpeg-install` | — | Install from the mirror |
| `ffmpeg-install-latest` | — | Install the latest official build |
| `ffmpeg-use` | `<version>` | Switch to a managed version |
| `ffmpeg-system` | — | Fall back to system FFmpeg |
| `ffmpeg-uninstall` | `<version>` | Uninstall a managed version |

## 8.3 Typical pipelines

### Generate and render a test sequence

```bash
tlcore demo ./.demo
tlcore analyze ./.demo
tlcore keyframes ./.demo
tlcore hgopt ./.demo
tlcore transition ./.demo
tlcore deflicker ./.demo
tlcore report ./.demo
tlcore xmp ./.demo
tlcore render ./.demo ./out.mp4
```

`demo` requires an empty or non-existent folder; a non-empty folder fails with `demo folder is not empty`.

### Inspect analysis only

```bash
tlcore analyze "/Volumes/SSD/sunset"
tlcore report "/Volumes/SSD/sunset"
```

Sample `report` output:

```text
frames: 180
keyframes: 3
luminance 0.121..0.884
flicker score before 2.41 / after 0.37
exposure correction -0.42..0.35 EV
```

### Batch processing

```bash
for d in /Volumes/SSD/*/; do
  tlcore analyze "$d"
  tlcore hgopt "$d"
  tlcore keyframes "$d"
  tlcore transition "$d"
  tlcore deflicker "$d"
  tlcore xmp "$d"
  tlcore render "$d"
done
```

> Back up `.lrtassist/project.json` before batch runs; these commands overwrite adjustments and keyframes.

## 8.4 Command details and differences

### `analyze`

- Reuses entries from an existing `project.json`;
- Re-decodes only when size, modification time or thumbnail changed;
- Prints `analysis complete: N images, mean luminance x, workflow <stage>`;
- Always natural-sorts by filename and saves the project.

### `hg` and `hgopt`

- `hg` uses `hg_offset` / `hg_stretch` / `hg_rotate` / `hg_reduce_initial` from `project.json`;
- `hgopt` searches stretch and rotate (352 evaluations), rounds the result to 0.01, writes it back, then applies;
- Both overwrite every frame's exposure, temperature and tint and set `workflow.hg_wizard = true`;
- Both print the score and exposure range.

### `keyframes`

- Clears all keyframes;
- Marks frame 1, frame `n/2` and frame n;
- Sets `workflow.keyframes = 3`;
- Intended for tests and demos, not precise creative work.

### `transition`

> Note: the CLI `transition` currently interpolates only **exposure, temperature and tint**. The UI's **Apply auto transition** interpolates all ten creative columns. For a complete transition, run it once in the UI, or apply it in the GUI before `render`.

- Requires at least one keyframe; otherwise it returns "no keyframes, mark some in the UI first";
- Uses the same smooth/linear algorithm and hold rules as the UI;
- Sets `workflow.auto_transition = true`.

### `deflicker`

- Input is the thumbnail visual curve (`visual_curve_from_thumbs`), not full resolution;
- The base correction vector is fixed at 0, so the result reflects current preview luminance only;
- Uses the project's smoothing, accuracy, passes and constant mode;
- Prints mean and maximum correction EV;
- Sets `workflow.visual_deflicker = true`.

### `render`

- Reads the project's `render` settings;
- The second argument overrides the output path;
- Prints stages, progress every 10 frames, and the final path;
- Prints `failed: ...` and returns non-zero on failure;
- Uses the same parallelism, encoder arguments and temporary frame directory as the UI.

## 8.5 `project.json`

The project file is UTF-8 JSON using `serde(default)`, so missing fields fall back to defaults for forward/backward compatibility.

### Top level

| Field | Type | Notes |
|-------|------|-------|
| `folder` | string | Absolute sequence path (canonicalised on save) |
| `name` | string | Sequence name, usually the folder name |
| `images` | array | Frame entries |
| `settings` | object | Sequence-level tool parameters |
| `workflow` | object | Workflow state |
| `render` | object | Render settings |
| `sort_by_time` | bool | Sort by capture time instead of filename |
| `hdr` | bool | HDR marker (reserved) |
| `saved_at` | string? | Last save time (RFC 3339) |

### `images[]`

| Field | Type | Notes |
|-------|------|-------|
| `file_name` | string | Filename without directory |
| `timestamp` | number? | EXIF capture time (Unix seconds) |
| `camera_ev` | number? | EV computed from aperture/shutter/ISO |
| `iso` `shutter` `aperture` | number? | Raw EXIF values |
| `luma` | number | Mean relative luminance 0..1 |
| `wb_r` `wb_b` | number | Gray-world white balance, normalised to green |
| `thumb` | string? | Thumbnail path relative to `.lrtassist/` |
| `size` | number | File size in bytes |
| `modified` | number? | Modification time (Unix seconds) |
| `width` `height` | number | Source dimensions |
| `error` | string? | Decode error |
| `adj` | object | Adjustment values |
| `keyframe` | bool | Keyframe flag |
| `starred` | bool | Star flag (reserved) |
| `skipped` | bool | Skip XMP export |

### `adj`

```json
{
  "exposure": 0.0,
  "temperature": 0.0,
  "tint": 0.0,
  "contrast": 0.0,
  "highlights": 0.0,
  "shadows": 0.0,
  "whites": 0.0,
  "blacks": 0.0,
  "saturation": 0.0,
  "vibrance": 0.0,
  "deflicker": 0.0
}
```

### `settings`

| Field | Default | Notes |
|-------|---------|-------|
| `deflicker_smoothing` | 0.5 | 0.02 … 1.0 |
| `deflicker_accuracy` | `"normal"` | `"Coarse"` / `"normal"` / `"Fine"` |
| `deflicker_passes` | 2 | 1 … 4 |
| `deflicker_constant` | false | Constant mode |
| `deflicker_multipass` | true | Multi-pass |
| `hg_offset` | 0.0 | −5 … 5 |
| `hg_stretch` | 0.0 | −1.5 … 1.5 |
| `hg_rotate` | 0.0 | −4 … 4 |
| `hg_reduce_initial` | 0.5 | 0 … 1 |
| `transition_mode` | `"Smooth"` | `"Smooth"` / `"Linear"` |

### `workflow`

| Field | Notes |
|-------|-------|
| `stage` | `Initialized` / `Keyframes` / `HolyGrail` / `AutoTransition` / `VisualPreviews` / `Deflicker` / `Finished` |
| `keyframes` | Keyframe count |
| `hg_wizard` | HG has run |
| `auto_transition` | Auto transition has run |
| `visual_deflicker` | Deflicker has run |
| `deflicker_passes` | Passes used by the last deflicker run |

### `render`

| Field | Default | Notes |
|-------|---------|-------|
| `fps` | 25.0 | 24 / 25 / 30 / 60 |
| `width` `height` | 1920 × 1080 | 1080p / 4K / vertical |
| `codec` | `H264` | `H264` / `H265` / `ProRes` |
| `format` | `Mp4` | `Mp4` / `Mov` |
| `quality` | 16 | CRF 10 … 30 |
| `rate_control` | `Crf` | `Crf` / `Bitrate` |
| `bitrate_mbps` | 20.0 | 2 … 100 |
| `encoder_preset` | `Balanced` | `Fast` / `Balanced` / `Best` |
| `motion_blur` | false | Motion-blur switch |
| `motion_blur_frames` | 2 | Blend count, clamped 2 … 9 internally |
| `output_path` | null | Custom output path |
| `export_frames` | false | Keep output frames |
| `frame_format` | `"png"` | Intermediate frame format |
| `write_xmp` | true | Write XMP before rendering |
| `sharpen` | 0.0 | 0 … 2 |
| `deflicker` | true | Apply deflicker corrections during render |

## 8.6 XMP sidecars

### Naming

```
<original filename without extension>.xmp
```

For example, `IMG_0001.CR3` → `IMG_0001.xmp`.

### Generation strategy

- File missing: write a complete document;
- File exists and contains `<rdf:Description>`: update only `crs:*` and `lrt:*` tags, preserving other metadata;
- File exists but cannot be updated: fall back to a complete document rather than losing edits;
- Writes use `xmp.tmp` + rename for atomicity;
- Frames with `skipped = true` are skipped.

### Example

```xml
<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Timelapse Assistant">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
        xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
        xmlns:lrt="urn:timelapse-assistant:workflow:1.0"
        xmlns:tiff="http://ns.adobe.com/tiff/1.0/">
      <crs:Version>15.0</crs:Version>
      <crs:ProcessVersion>15.4</crs:ProcessVersion>
      <crs:HasSettings>true</crs:HasSettings>
      <crs:WhiteBalance>Custom</crs:WhiteBalance>
      <crs:Temperature>5700</crs:Temperature>
      <crs:Tint>10</crs:Tint>
      <crs:Exposure2012>0.4</crs:Exposure2012>
      <crs:Contrast2012>12</crs:Contrast2012>
      <crs:Highlights2012>-20</crs:Highlights2012>
      <crs:Shadows2012>18</crs:Shadows2012>
      <crs:Whites2012>5</crs:Whites2012>
      <crs:Blacks2012>-8</crs:Blacks2012>
      <crs:Saturation>6</crs:Saturation>
      <crs:Vibrance>10</crs:Vibrance>
      <crs:HasCrop>false</crs:HasCrop>
      <lrt:Keyframe>True</lrt:Keyframe>
      <lrt:Deflicker>-0.1</lrt:Deflicker>
      <lrt:Exposure2012>0.5</lrt:Exposure2012>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>
```

Notes:

- `crs:Exposure2012` is total EV (creative + deflicker);
- `lrt:Exposure2012` is creative exposure only;
- Temperature is written only when `WhiteBalance=Custom`, as `5500 + temperature × 20`;
- Numbers use at most four decimals with trailing zeros removed.

## 8.7 Directories and temporary files

| Path | Content | Lifetime |
|------|---------|----------|
| `.lrtassist/project.json` | Project state | Persistent |
| `.lrtassist/project.json.tmp` | Save intermediate | Removed by rename |
| `.lrtassist/thumbs/` | Thumbnail cache | Persistent; safe to delete |
| `.lrtassist/render/frames/` | Intermediate render frames | Deleted on complete / cancel / failure |
| `.lrtassist/exported_frames/` | Kept output frames | Persistent; overwritten on re-render |
| `<sequence folder>/*.xmp` | XMP sidecars | Persistent |
| `<sequence folder>/<name>_<timestamp>.<ext>` | Default video output | Persistent |

---

[← Previous: Settings and FFmpeg](07-settings-ffmpeg.md) · [Next: Architecture and implementation →](09-architecture.md)
