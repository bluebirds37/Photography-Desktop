# 7. 设置与 FFmpeg 组件

[English](../en/07-settings-ffmpeg.md) · [文档首页](README.md)

设置是一个模态弹窗，从顶部右侧齿轮或 FFmpeg 状态胶囊打开。它只包含应用级配置，目前是 FFmpeg 组件的安装、导入、切换与卸载；图像调整与渲染参数仍在右侧工作页，二者互不混排。

![设置弹窗](../assets/screenshots/settings.png)

## 7.1 弹窗结构

| 区域 | 内容 |
|------|------|
| 标题栏 | 设置图标、标题「设置」、副标题「应用偏好与 FFmpeg 组件管理」、关闭按钮 |
| 左侧主卡 | FFmpeg 当前状态、安装 / 导入 / 下载 / 切换操作 |
| 右侧卡片 | 版本列表、组件路径、系统回退说明 |
| 背景 | 压暗遮罩，点击遮罩关闭 |

- 弹窗宽 712 px，最大高度 748 px，内容超出时内部滚动；
- 点击弹窗外区域或右上角 `×` 关闭；
- 打开设置不会中断正在进行的导入或渲染，但部分操作会互斥。

## 7.2 FFmpeg 状态

顶部状态胶囊与设置卡片显示三种状态：

| 显示 | 含义 |
|------|------|
| `FFmpeg <版本>` | 正在使用组件目录中的某个内置版本 |
| `FFmpeg 系统` | 正在使用系统 `PATH` 或常见目录中的 FFmpeg |
| `需要 FFmpeg` | 未找到可用可执行文件 |

颜色：绿色表示可用，红色表示缺失，黄色表示正在安装。

## 7.3 组件目录

### 位置

```text
<可执行文件目录>/components/ffmpeg/
```

如果程序目录不可写（例如安装在只读位置），回退到：

| 平台 | 回退路径 |
|------|----------|
| Windows | `%LOCALAPPDATA%\TimelapseAssistant\components` |
| macOS / Linux | `%LOCALAPPDATA%` 未设置时使用系统临时目录：`<临时目录>/TimelapseAssistant/components` |

### 结构

```text
components/
  ffmpeg/
    versions.json
    <version>/
      ffmpeg(.exe)
      ffprobe(.exe)
      *.dll            # Windows 构建附带
```

`versions.json` 记录：

- 当前选中的版本（或 `system`）；
- 所有已安装版本的目录名、来源、安装时间；
- 是否已完成首次自动安装。

### 查找顺序

1. `versions.json` 中选中的内置版本；
2. 环境变量 `TIMELAPSE_FFMPEG` 指向的可执行文件；
3. 系统 `PATH` 中的 `ffmpeg`；
3. macOS 常见路径：`/opt/homebrew/bin/ffmpeg`、`/usr/local/bin/ffmpeg`、`/usr/bin/ffmpeg`；
4. Windows 常见安装目录（由系统查找逻辑覆盖）。

找到第一个可用文件即停止。

## 7.4 平台差异

| 能力 | Windows | macOS |
|------|---------|-------|
| 首次自动安装 | 支持：先导入系统版本，未找到时镜像优先、官方源回退 | 支持导入系统版本；未找到时不下载，提示手动导入 |
| 导入系统 FFmpeg | 支持，复制到组件目录 | 支持，复制到组件目录 |
| 下载最新版 | 支持 | 支持下载，但构建主要面向 Windows |
| 从压缩包安装 | 支持 ZIP / TGZ | 支持 ZIP / TGZ |
| 切换内置版本 | 支持 | 支持 |
| 回退系统 FFmpeg | 支持 | 支持 |
| 卸载内置版本 | 支持 | 支持 |

> macOS 上首次运行如果找到系统 FFmpeg，也会复制到组件目录并选中；之后可在版本列表中切回系统版本。若组件目录不可写，则回退到临时目录。

## 7.5 操作说明

### 自动安装（首次运行）

Windows 与 macOS 首次启动时都会自动执行：

1. 查找系统 FFmpeg，找到则导入并选中；
2. 未找到时：Windows 从镜像下载快速安装包，失败后回退官方 essentials 最新版；macOS 不下载，提示在设置中导入；
3. 安装完成后写入 `versions.json` 并选中新版本（或记录本次尝试失败，避免每次启动重复）。

安装过程状态胶囊显示「FFmpeg 安装中」，状态栏与卡片显示阶段与下载进度。

### 导入系统 FFmpeg

点击「导入系统」：

- 在 `PATH` 与常见目录中查找；
- 找到后复制 `ffmpeg`、`ffprobe` 及同目录 DLL 到组件目录；
- 版本号从 `ffmpeg -version` 解析；
- 成功后选中新导入版本。

适合系统已有 FFmpeg、但希望固定版本或隔离环境的场景。

### 下载最新版

从官方源下载 Windows essentials 构建，解压后安装到组件目录。需要网络连接；下载失败会保留当前版本并在卡片中提示错误。

### 从压缩包安装

选择一个 ZIP 或 TGZ：

1. 解压到临时目录；
2. 递归查找 `ffmpeg` / `ffmpeg.exe`；
3. 复制可执行文件与同目录运行库到 `components/ffmpeg/<version>/`；
4. 解析版本并写入清单。

适合离线机器或自定义构建。

### 切换版本

版本列表中每个内置版本都可点击「使用」。切换只修改 `versions.json` 的当前选择，不移动文件。切换后新的渲染会使用新版本；正在运行的渲染不受影响。

### 回退系统 FFmpeg

点击「使用系统」清除内置选择。之后按查找顺序回退到系统可执行文件。如果系统没有 FFmpeg，状态会变为「需要 FFmpeg」。

### 卸载版本

点击版本旁的「卸载」删除对应目录并从清单移除。不能卸载当前正在使用的版本；请先切换到其他版本或系统 FFmpeg。

## 7.6 命令行等价操作

所有设置操作都可以在终端完成：

```bash
tlcore ffmpeg-status          # 当前来源、版本、路径、组件目录、已安装版本
tlcore ffmpeg-import          # 导入系统 FFmpeg
tlcore ffmpeg-install         # 从镜像安装
tlcore ffmpeg-install-latest  # 从官方源安装最新版
tlcore ffmpeg-use <version>   # 切换到内置版本
tlcore ffmpeg-system          # 回退系统 FFmpeg
tlcore ffmpeg-uninstall <v>   # 卸载内置版本
```

`ffmpeg-status` 输出示例：

```text
当前来源: managed
当前版本: 7.1
可执行文件: /Applications/Timelapse Assistant.app/Contents/MacOS/components/ffmpeg/7.1/ffmpeg
组件目录: /Applications/Timelapse Assistant.app/Contents/MacOS/components
已安装内置版本: 1
  7.1 (official · 2026-10-29T15:02:11+08:00)
```

详见 [命令行与文件格式](08-cli-and-files.md)。

## 7.7 故障与恢复

| 现象 | 可能原因 | 处理 |
|------|----------|------|
| 状态一直「需要 FFmpeg」 | 系统无 FFmpeg，自动安装失败 | 安装 Homebrew FFmpeg 后点「导入系统」，或从压缩包安装 |
| 安装卡在下载 | 网络不可达或镜像超时 | 稍后重试，或改用「从压缩包安装」 |
| 切换版本后仍显示旧版本 | 渲染进程仍在使用旧文件 | 重启程序；确认 `versions.json` 的当前选择 |
| 卸载失败 | 该版本正在使用 | 先切换到其他版本 |
| macOS 提示无法打开 | 未签名构建被 Gatekeeper 拦截 | 在「隐私与安全性」允许，或本地重新构建 |
| 组件目录不可写 | 程序安装在只读位置 | 程序会自动回退到用户数据目录；也可移动程序到可写位置 |

> 删除 `versions.json` 会让程序忘记当前选择，但不会删除已安装目录；下次启动会重新探测并可能重新写入清单。

---

[← 上一篇：渲染与输出](06-rendering.md) · [下一篇：命令行与文件格式 →](08-cli-and-files.md)
