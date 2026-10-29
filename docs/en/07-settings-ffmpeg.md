# 7. Settings and FFmpeg

**English** · [简体中文](../zh/07-settings-ffmpeg.md) · [Documentation home](README.md)

Settings is a modal dialog opened from the gear in the top-right or from the FFmpeg pill. It holds only app-level configuration — currently installing, importing, switching and uninstalling FFmpeg. Image adjustments and render settings remain on the right-hand tabs, so they never mix.

![Settings dialog](../assets/screenshots/settings.png)

## 7.1 Dialog layout

| Region | Content |
|--------|---------|
| Header | Settings icon, title **Settings**, subtitle "App preferences and FFmpeg components", close button |
| Left main card | Current FFmpeg state and install / import / download / switch actions |
| Right cards | Version list, component path, system fallback notes |
| Backdrop | Dimmed overlay; click to close |

- The modal is 712 px wide and at most 748 px tall; content scrolls internally;
- Close with the backdrop or the `×` in the top-right;
- Opening Settings does not interrupt an import or render, though some actions are mutually exclusive.

## 7.2 FFmpeg state

The top pill and the Settings card show one of three states:

| Display | Meaning |
|---------|---------|
| `FFmpeg <version>` | A managed version in the component store is active |
| `FFmpeg system` | FFmpeg from `PATH` or a common location is active |
| `FFmpeg required` | No usable executable found |

Colours: green means available, red means missing, yellow means installing.

## 7.3 Component store

### Location

```text
<executable directory>/components/ffmpeg/
```

If the app directory is not writable (for example, installed in a read-only location), the store falls back to:

| Platform | Fallback path |
|----------|---------------|
| Windows | `%LOCALAPPDATA%\TimelapseAssistant\components` |
| macOS / Linux | When `%LOCALAPPDATA%` is unset, the system temporary directory: `<temp>/TimelapseAssistant/components` |

### Structure

```text
components/
  ffmpeg/
    versions.json
    <version>/
      ffmpeg(.exe)
      ffprobe(.exe)
      *.dll            # bundled with Windows builds
```

`versions.json` records:

- The selected version (or `system`);
- Every installed version's directory name, source and install time;
- Whether first-run auto-install has completed.

### Lookup order

1. The managed version selected in `versions.json`;
2. An executable pointed to by the `TIMELAPSE_FFMPEG` environment variable;
3. `ffmpeg` on the system `PATH`;
3. Common macOS paths: `/opt/homebrew/bin/ffmpeg`, `/usr/local/bin/ffmpeg`, `/usr/bin/ffmpeg`;
4. Common Windows install locations covered by the platform lookup.

The first usable executable wins.

## 7.4 Platform differences

| Capability | Windows | macOS |
|------------|---------|-------|
| First-run auto-install | Yes: imports a system build, otherwise mirror first with official fallback | Imports a system build; does not download when missing and prompts manual import |
| Import system FFmpeg | Yes, copies into the store | Yes, copies into the store |
| Download latest | Yes | Supported, but builds target Windows primarily |
| Install from archive | ZIP / TGZ | ZIP / TGZ |
| Switch managed version | Yes | Yes |
| Fall back to system | Yes | Yes |
| Uninstall managed version | Yes | Yes |

> On macOS, a system FFmpeg found on first run is also copied into the store and selected; you can switch back to the system build in the version list afterwards. If the store is not writable, it falls back to the temporary directory.

## 7.5 Actions

### Auto-install (first run)

On both Windows and macOS, the first launch:

1. Looks for system FFmpeg and imports/selects it if found;
2. Otherwise: Windows downloads the fast mirror package and falls back to the latest official essentials build; macOS does not download and prompts you to import one in Settings;
3. Writes `versions.json` and selects the new version (or records the failed attempt so it is not repeated on every launch).

During install the pill reads **Installing FFmpeg**, and the status bar and card show stages and download progress.

### Import system FFmpeg

**Import system**:

- Searches `PATH` and common locations;
- Copies `ffmpeg`, `ffprobe` and adjacent DLLs into the component store;
- Parses the version from `ffmpeg -version`;
- Selects the imported version on success.

Useful when the system already has FFmpeg but you want a pinned or isolated build.

### Download latest

Downloads the official Windows essentials build and installs it into the store. Requires a network connection; on failure the current version is kept and the card reports the error.

### Install from archive

Choose a ZIP or TGZ:

1. Extract to a temporary directory;
2. Recursively locate `ffmpeg` / `ffmpeg.exe`;
3. Copy the executable and adjacent runtime libraries into `components/ffmpeg/<version>/`;
4. Parse the version and update the manifest.

Useful for offline machines or custom builds.

### Switch version

Each managed version has a **Use** action. Switching only changes the selection in `versions.json`; files are not moved. New renders use the new version; a render already running is unaffected.

### Fall back to system FFmpeg

**Use system** clears the managed selection. Lookup then falls back to the system executable. If none exists, the state becomes **FFmpeg required**.

### Uninstall a version

**Uninstall** removes the directory and the manifest entry. The currently active version cannot be uninstalled; switch to another version or to the system build first.

## 7.6 CLI equivalents

Every Settings action is available in a terminal:

```bash
tlcore ffmpeg-status          # source, version, path, store, installed versions
tlcore ffmpeg-import          # import system FFmpeg
tlcore ffmpeg-install         # install from the mirror
tlcore ffmpeg-install-latest  # install the latest official build
tlcore ffmpeg-use <version>   # switch to a managed version
tlcore ffmpeg-system          # fall back to system FFmpeg
tlcore ffmpeg-uninstall <v>   # uninstall a managed version
```

Sample `ffmpeg-status` output:

```text
source: managed
version: 7.1
executable: /Applications/Timelapse Assistant.app/Contents/MacOS/components/ffmpeg/7.1/ffmpeg
store: /Applications/Timelapse Assistant.app/Contents/MacOS/components
installed managed versions: 1
  7.1 (official · 2026-10-29T15:02:11+08:00)
```

See [CLI and file formats](08-cli-and-files.md).

## 7.7 Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| State stays **FFmpeg required** | No system FFmpeg and auto-install failed | Install Homebrew FFmpeg then **Import system**, or install from an archive |
| Download stalls | Network unreachable or mirror timeout | Retry later, or use **Install from archive** |
| Old version after switching | A render process still holds the old binary | Restart the app; check the selection in `versions.json` |
| Uninstall refuses | The version is active | Switch to another version first |
| macOS refuses to open the app | Gatekeeper blocks an unsigned build | Allow under Privacy & Security, or rebuild locally |
| Store not writable | App installed in a read-only location | The app falls back to the user data directory; or move the app somewhere writable |

> Deleting `versions.json` makes the app forget its selection but does not delete installed directories. The next launch re-detects and may rewrite the manifest.

---

[← Previous: Rendering and output](06-rendering.md) · [Next: CLI and file formats →](08-cli-and-files.md)
