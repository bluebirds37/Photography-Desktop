# 延时摄影助手 · 中文文档

[English](../en/README.md) · **简体中文**

> 版本 0.1.0 · Rust 2021 + GPUI 0.2.2 · macOS / Windows

延时摄影助手（Timelapse Assistant）是一套独立的延时摄影后期工具。它把序列分析、关键帧调色、Holy Grail 曝光补偿、关键帧过渡、视觉去闪烁、XMP 边车导出与视频渲染放进同一个桌面程序，不依赖 Lightroom、After Effects、Java 或外部脚本。

![主界面](../assets/screenshots/overview.png)

## 文档地图

| # | 文档 | 内容 | 关键配图 |
|---|------|------|----------|
| 1 | [安装与首次运行](01-installation.md) | 系统要求、源码构建、启动参数、首次运行、支持格式、项目目录 | 空状态、设置 |
| 2 | [界面总览](02-interface.md) | 窗口分区、顶部栏、序列列表、预览区、曲线、右侧工作页、状态栏 | 主界面、列表、曲线 |
| 3 | [序列与关键帧](03-sequences-and-keyframes.md) | 导入校验、排序、缩略图、播放、关键帧语义、自动关键帧、自动保存 | 序列列表、关键帧预览 |
| 4 | [单帧调整](04-adjustments.md) | 10 个调整参数、范围与步进、重置、EV 合成、XMP 映射 | 调整面板、预览 |
| 5 | [工具：HG / 过渡 / 去闪烁 / XMP](05-tools.md) | 四个工具的原理、参数、操作步骤与验收标准 | 工具页、四张工具卡 |
| 6 | [渲染与输出](06-rendering.md) | 分辨率、帧率、编码、CRF/码率、预设、运动模糊、进度与输出规则 | 渲染页、进度、完成 |
| 7 | [设置与 FFmpeg 组件](07-settings-ffmpeg.md) | 设置弹窗、组件目录、导入/下载/切换/卸载、版本清单 | 设置弹窗 |
| 8 | [命令行与文件格式](08-cli-and-files.md) | `tlcore` 全部命令、`project.json`、XMP 字段、目录结构 | 主界面、XMP 导出 |
| 9 | [架构与实现](09-architecture.md) | 模块划分、线程模型、数据流、渲染管线、测试 | 数据流图 |
| 10 | [故障排查](10-troubleshooting.md) | 常见问题、诊断命令、日志与恢复方式 | 设置、渲染页 |

## 三分钟上手

```bash
# 1. 构建（macOS / Windows 相同）
cargo build --release

# 2. 启动并直接打开一个序列目录
#    macOS
open "dist/Timelapse Assistant.app" --args "/path/to/sequence"
#    或在终端运行
target/release/timelapse_assistant "/path/to/sequence"
#    Windows
target\release\timelapse_assistant.exe "D:\photos\sequence"
```

程序打开后会立即扫描目录、解析 EXIF、生成缩略图与亮度曲线。随后按顺序操作：

1. 在左侧列表或底部按钮标记关键帧；
2. 在「工具」页运行 Holy Grail 向导；
3. 为首尾关键帧调色，点击「应用自动过渡」；
4. 运行「视觉去闪烁」；
5. 在「渲染」页选择输出并点击「开始渲染」。

也可以完全跳过界面，用 `tlcore` 跑同一条流水线：

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

## 阅读约定

- 数值范围统一写成 `最小值 … 最大值（步进）`，例如 `−5 … +5（0.05）`。
- 「帧号」在界面上从 1 开始显示，`--frame` 参数同样使用 1 基编号。
- 截图来自 macOS 构建；Windows 构建的布局、文案与交互完全一致，仅窗口按钮和文件对话框样式不同。
- 界面没有全局键盘快捷键；所有操作通过鼠标点击、拖动或滚轮完成。命令行参数见第 1 与第 8 篇。
- 文中「项目目录」指序列文件夹下的 `.lrtassist/`，移动序列时请连同该目录一起移动。

## 功能一览

| 能力 | 说明 |
|------|------|
| 序列导入 | JPEG / PNG / TIFF / DNG / CR2 / CR3 / NEF / ARW / RAF / ORF / RW2 / SRW / PEF |
| RAW 处理 | 优先用 `rawler` 去马赛克；失败时回退到内嵌 JPEG 预览 |
| 分析 | EXIF 曝光三元组、拍摄时间、平均亮度、灰世界白平衡代理值 |
| 关键帧 | 手动标记、首尾快捷、上一/下一关键帧、自动关键帧 |
| Holy Grail | 偏移 / 拉伸 / 旋转 / 初始平滑，自动优化 352 次评估 |
| 自动过渡 | 10 个调整列独立插值，线性或 smoothstep 平滑 |
| 视觉去闪烁 | LOESS 平滑 + 多遍 EV 修正，粗略 / 标准 / 精细，恒定模式 |
| XMP | Camera Raw 兼容字段 + `lrt:` 工作流标记，保留原有无关元数据 |
| 渲染 | 1080p / 4K / 竖屏，24/25/30/60 fps，H.264 / H.265 / ProRes |
| 项目管理 | `.lrtassist/project.json` 原子保存，2 秒防抖自动保存 |
| 无界面 CLI | `tlcore` 覆盖分析、HG、过渡、去闪烁、XMP、渲染与 FFmpeg 管理 |

---

下一篇：[安装与首次运行 →](01-installation.md)
