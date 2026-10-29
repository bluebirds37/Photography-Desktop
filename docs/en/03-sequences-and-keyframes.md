# 3. Sequences and keyframes

**English** · [简体中文](../zh/03-sequences-and-keyframes.md) · [Documentation home](README.md)

A sequence is the basic unit of work: one folder, an ordered set of photos, and the analysis, adjustments and keyframes stored in `.lrtassist/project.json`. Keyframes are the anchors for exposure and colour transitions.

![Sequence list and current frame](../assets/screenshots/sequence-list.png)

## 3.1 Importing a sequence

### Entry points

- **Open folder** in the top bar;
- **Open folder** in the empty-state card;
- A folder passed on the command line;
- **Rescan** in the top bar to re-import the current folder.

### Scan rules

| Rule | Behaviour |
|------|-----------|
| Depth | First level only, no recursion |
| Extensions | Listed below; case-insensitive |
| Ordering | Natural filename order; numeric runs compare by value |
| Exclusions | Hidden files, files inside `.lrtassist/`, unsupported extensions |
| Empty folder | Error; the current sequence is left unchanged |
| Non-directory argument | Ignored |

Supported extensions: `jpg` `jpeg` `png` `tif` `tiff` `dng` `cr2` `cr3` `nef` `arw` `raf` `orf` `rw2` `srw` `pef`.

### Per-frame analysis

| Field | Source | Used for |
|-------|--------|----------|
| Filename / size / mtime | Filesystem | Cache invalidation |
| Capture time | EXIF `DateTimeOriginal` | Time ordering, duration reference |
| ISO / shutter / aperture | EXIF | Camera EV for HG |
| Camera EV | `log2(N²/t) − log2(ISO/100) + bias` | HG exposure trend |
| Mean luminance | sRGB relative luminance of the 320 px preview | Graph, deflicker, status bar |
| White-balance proxy | Log ratio of gray-world R/G/B | HG white-balance compensation |
| Thumbnail | 640 px long-edge JPEG | Left list |
| Error | Decode failure text | List marker and diagnostics |

### Cache and rescan

Thumbnails live in `.lrtassist/thumbs/`. On rescan, if both size and modification time are unchanged and the thumbnail exists, it is reused without decoding. Re-analysis happens when:

- The file was replaced or re-exported;
- Size or modification time changed;
- The thumbnail was deleted;
- The project entry and disk disagree.

> Known limitation: editing only `project.json` (for example with `tlcore keyframes`) is not picked up by **Rescan**, because the app treats the in-memory sequence as authoritative. Restart the app, or re-mark keyframes in the UI.

## 3.2 Selection and playback

- Click any row in the left list to select that frame; the preview, Adjust tab and status bar update together.
- Previous / Next move one frame and do not wrap.
- Play advances at the render fps and wraps from the last frame to the first.
- You can select another frame while playing; playback continues from the new position.
- The preview uses graded proxy images, not the final render resolution.

## 3.3 Keyframe semantics

Keyframes are anchors for **auto transition**: frames between two keyframes take interpolated values, while frames outside the outermost keyframes hold the nearest value.

| Action | Entry point | Result |
|--------|-------------|--------|
| Mark / unmark | **Mark keyframe** in the preview | Toggles the current frame; status bar confirms |
| First / last keyframes | **Mark first & last keyframes** on the HG card | Sets frame 1 and the last frame as keyframes |
| Previous / next keyframe | Two arrow buttons in the preview | Jumps to adjacent keyframes, no wrap |
| Auto keyframe | Switch on the Adjust tab (default on) | Any slider edit marks the current frame as a keyframe |

### Keyframes and interpolation

- With exactly one keyframe, **Apply auto transition** broadcasts that keyframe's values to every frame;
- With zero keyframes the UI reports "At least two keyframes are required" and changes nothing;
- Keyframe values themselves are never overwritten by interpolation; they are preserved exactly;
- Frames before the first and after the last keyframe hold the boundary value (no extrapolation);
- Smooth mode uses the smoothstep `u²(3−2u)`, whose slope is zero at the keyframes, avoiding corners at the start and end of a ramp;
- Linear mode interpolates directly by `u`.

### Recommended keyframe placement

| Scenario | Suggestion |
|----------|------------|
| Day to night | First frame, the point where exposure starts changing noticeably, last frame |
| Static-camera flicker | Few keyframes needed; deflicker works independently |
| Multiple exposure steps | One keyframe at the start and end of each step |
| Creative grading | At least first and last, with optional style-change anchors in between |

Keyframe counts appear in the top workflow strip, the left header, the HG card and the Sequence overview.

## 3.4 Autosave and project management

| Trigger | Behaviour |
|---------|-----------|
| Any edit | Mark dirty; save after 2 seconds without further edits |
| Import / render / deflicker running | Save is deferred until the task ends |
| **Save** clicked | Save immediately |
| Window closed | Save, then exit |
| Before render starts | Force a save so rendering uses the latest parameters |

Saving is atomic:

1. Serialise to `project.json.tmp`;
2. Rename to `project.json` after the write completes;
3. On failure keep the old file and report the error.

`project.json` contains:

```text
folder / name / images[] / settings / workflow / render / sort_by_time / hdr / saved_at
```

For every field see [CLI and file formats](08-cli-and-files.md).

## 3.5 Common states

| State | Meaning | Next step |
|-------|---------|-----------|
| Empty | No sequence imported | Open a folder |
| Loaded, no keyframes | Analysis and thumbnails only | Mark keyframes or grade directly |
| One keyframe | Transition broadcasts a single frame | Add first and last keyframes |
| Two or more keyframes | Transition is available | Apply it on the Tools tab |
| Deflicker applied | Every frame has a non-zero `deflicker` EV | Inspect total EV on Adjust |
| Render complete | `workflow.stage = Finished` | Use **Show in folder** on Render |

---

[← Previous: Interface overview](02-interface.md) · [Next: Per-frame adjustments →](04-adjustments.md)
