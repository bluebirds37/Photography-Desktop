# 延时摄影助手（Timelapse Assistant）

一套独立的延时摄影后期工具，使用 **Rust + GPUI** 构建。它把序列分析、关键帧调色、曝光过渡、视觉去闪烁、XMP 边车导出和视频渲染整合进一个桌面程序，无需外部编辑器或 Java 运行时。

![主界面](docs/screenshots/app-main.png)



## 功能

- **序列导入**：JPEG / PNG / TIFF / DNG / CR2 / CR3 / NEF / ARW / RAF / ORF / RW2 等；自动解析 EXIF、按文件名自然排序或按拍摄时间排序。
- **RAW 支持**：内嵌 JPEG 预览用于快速缩略图和视觉分析；支持的 RAW 由纯 Rust `rawler` 去马赛克，失败时自动回退到内嵌预览。
- **关键帧工作流**：标记/取消关键帧、上一/下一关键帧、首尾关键帧、自动关键帧。
- **Holy Grail 向导**：根据相机曝光/亮度曲线补偿日转夜拍摄中的曝光跳变；提供偏移、拉伸、旋转、初始平滑四个参数，以及 351 次评估的自动优化。
- **自动过渡**：在关键帧之间对曝光、色温、色调、对比度、高光、阴影、黑白场、饱和度等全部列做线性或平滑插值。
- **视觉去闪烁**：按当前调整渲染缩略图 → 计算亮度曲线 → LOESS 平滑 → 写回 EV 修正；支持精度、平滑度、多遍和恒定模式。
- **XMP 边车**：为每张图写 Adobe Camera Raw 兼容的 `crs:*` 字段，并保留已有 XMP 中的无关元数据；同时写入本程序的工作流标记，便于再次载入时恢复状态。
- **渲染**：并行渲染帧，支持 1080p / 4K / 竖屏、24/25/30/60 fps、H.264 / H.265 / ProRes；高级导出支持 CRF 质量优先或目标码率（VBR）、快速/均衡/高质量编码预设、运动模糊、锐化与保留输出帧。
- **内置 FFmpeg 组件**：首次运行自动导入系统 FFmpeg，若不存在则从镜像快速安装；设置页支持下载最新版、从压缩包安装、导入系统、切换版本、卸载版本。
- **暗色窗口**：自绘标题栏与原生窗口控制（最小化/最大化/关闭），不再出现浅色系统标题栏。
- **项目管理**：在序列目录下生成 `.lrtassist/project.json`，自动保存调整、关键帧和设置；缩略图缓存位于 `.lrtassist/thumbs`。
- **无界面 CLI**：`tlcore` 可用于批处理、自动化测试和无头渲染。

## 快速开始

已构建版本位于 `dist/`：

- `dist/timelapse_assistant.exe`：桌面程序
- `dist/tlcore.exe`：批处理 CLI
- `dist/components/`：首次运行自动安装的 FFmpeg 组件

要求：

- Windows 10/11
- Rust stable（从源码构建时）
- FFmpeg 无需预装：首次运行自动导入系统版本或从镜像下载

从源码构建：

```bash
cargo build --release
```

打开界面，可直接传入序列目录：

```bash
target/release/timelapse_assistant.exe "D:/photos/my-timelapse"
```

生成测试序列并跑完整流程：

```bash
target/release/tlcore.exe demo ./.demo
target/release/tlcore.exe analyze ./.demo
target/release/tlcore.exe hg ./.demo
target/release/tlcore.exe keyframes ./.demo
target/release/tlcore.exe transition ./.demo
target/release/tlcore.exe deflicker ./.demo
target/release/tlcore.exe xmp ./.demo
target/release/tlcore.exe render ./.demo ./out.mp4
```

## 推荐工作流

1. **打开文件夹**：分析全部帧，生成缩略图和亮度/白平衡统计。
2. **标关键帧**：在左侧列表或底部按钮标记关键帧；调整任意滑块会自动把当前帧设为关键帧（可关闭“自动关键帧”）。
3. **Holy Grail 向导**：点击“自动优化”或手动调参后“应用”，补偿拍摄过程中相机曝光/亮度的变化。
4. **关键帧调色**：为首帧和尾帧设置曝光、色温、对比度等创意参数。
5. **自动过渡**：在关键帧之间插值，形成平滑的日转夜/夜转日过渡。
6. **视觉去闪烁**：对已调色的序列渲染预览、测亮度、LOESS 平滑并写入 EV 修正；可多遍执行。
7. **渲染**：选择分辨率、帧率、编码与码率，点击“开始渲染”；XMP 会自动导出（可在渲染设置中关闭）。
8. **交付**：得到 MP4/MOV，或使用导出的 XMP 边车进入其他支持 XMP 的后期软件。

## 界面

- **顶部**：打开文件夹、保存、重新扫描、工作流状态、FFmpeg 状态、渲染、设置。
- **左侧**：序列列表，显示缩略图、曝光修正、亮度与关键帧标记。
- **中央**：画面预览、亮度/曝光曲线、关键帧标记、播放与逐帧导航。
- **右侧**：调整、工具、渲染三个工作页；设置通过顶部按钮进入。
- **设置**：FFmpeg 组件的安装、导入、切换与卸载。

## FFmpeg 组件

组件目录默认为可执行文件旁的 `components/ffmpeg/`，安装目录只读时回退到：

```text
%LOCALAPPDATA%\TimelapseAssistant\components
```

每个版本位于 `components/ffmpeg/<version>/`，包含 `ffmpeg.exe`、`ffprobe.exe` 及同目录 DLL。`versions.json` 记录当前选择、所有已安装版本和首次自动安装状态。

首次运行：

1. 找到系统 FFmpeg（`PATH` 与常见安装目录）时，复制到组件目录并选中；
2. 未找到时，从镜像快速安装 FFmpeg 4.1.0；
3. 镜像失败时回退官方 essentials 最新版；
4. 设置页可手动下载最新版、从 ZIP/TGZ 安装、导入系统版本、切换、卸载或回退系统 FFmpeg。

## CLI

```text
tlcore demo <folder>         生成测试序列
tlcore analyze <folder>      导入、EXIF、缩略图、亮度分析
tlcore hg <folder>           应用 Holy Grail（使用已保存参数）
tlcore hgopt <folder>        自动搜索 stretch/rotate
tlcore keyframes <folder>    标记首/中/尾关键帧（测试用）
tlcore transition <folder>   关键帧自动过渡
tlcore deflicker <folder>    视觉预览去闪烁
tlcore report <folder>       输出闪烁指数与曝光范围
tlcore xmp <folder>          写 XMP 边车
tlcore render <folder> [out] 渲染并编码
tlcore ffmpeg-status         查看当前组件与已安装版本
tlcore ffmpeg-import         导入系统 FFmpeg
tlcore ffmpeg-install        从镜像快速安装 FFmpeg
tlcore ffmpeg-install-latest 从官方源下载最新版
tlcore ffmpeg-use <version>  切换内置版本
tlcore ffmpeg-system         回退到系统 FFmpeg
tlcore ffmpeg-uninstall <v>  卸载内置版本
```

## 项目结构

```text
src/
  main.rs                  GPUI 桌面入口
  lib.rs
  core/
    model.rs               序列、调整、设置、项目持久化
    analysis.rs            扫描、EXIF、预览/RAW 解码、缩略图、视觉亮度
    loess.rs               tricube 局部线性回归（LOESS）
    deflicker.rs           视觉去闪烁
    holy_grail.rs          曝光曲线与自动优化
    transition.rs          关键帧插值
    xmp.rs                 XMP 边车读写
    ffmpeg.rs              FFmpeg 组件安装与版本管理
    image_pipeline.rs      曝光/白平衡/影调/色彩处理
    render.rs              并行渲染与 ffmpeg 编码
  ui/
    app.rs                 GPUI 主界面
    graph.rs               亮度/曝光曲线
  bin/tlcore.rs            无界面 CLI
docs/
  用户手册.md
  算法设计.md
  开发指南.md
```

## 数据目录

每个序列目录下：

```text
.lrtassist/
  project.json             调整、关键帧、渲染设置、工作流状态
  thumbs/                  640px 缩略图缓存
  render/frames/           渲染临时帧（完成后清理）
  exported_frames/         勾选“保留输出帧”时的帧输出
```

视频输出路径可在渲染页选择；默认输出到序列目录，文件名包含时间戳。

## 已知边界

- 影调处理是自研的轻量管线（曝光、白平衡、对比度、高光/阴影、黑白场、饱和度/自然饱和度），不是完整的商业调色引擎。
- 视觉去闪烁使用 640px 缩略图，而不是全分辨率预览；对曝光/白平衡闪烁足够，对极端局部调整会有差异。
- RAW 解码依赖 `rawler` 的相机支持；不支持的特殊压缩会回退到内嵌 JPEG 预览。
- 暂未实现外部目录同步、HDR 合成、局部蒙版、镜头校正、稳定与高级运动模糊。
- 目标码率为 VBR 平均值，简单画面可能低于目标值；需要严格固定码率时可改用 CRF 或后续扩展两遍编码。
- 内置组件优先使用系统 FFmpeg 导入；无系统版本时默认从镜像安装 FFmpeg 4.1.0，也可在设置中下载最新版。

## 文档

- [用户手册](docs/用户手册.md)：界面、工作流、渲染与常见问题。
- [算法设计](docs/算法设计.md)：LOESS、去闪烁、曝光过渡、渲染管线。
- [开发指南](docs/开发指南.md)：模块结构、数据流、构建、测试与发布。

## 第三方依赖

- [GPUI](https://github.com/zed-industries/zed)：桌面 UI 框架
- [image](https://github.com/image-rs/image)：图像编解码
- [rawler](https://github.com/dnglab/dnglab)：RAW 解码
- [rayon](https://github.com/rayon-rs/rayon)：数据并行
- [FFmpeg](https://ffmpeg.org/)：视频编码，运行时安装，遵循其自身许可证

各依赖的具体许可证以对应项目与 `Cargo.lock` 中的版本为准。
