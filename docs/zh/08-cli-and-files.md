# 8. 命令行与文件格式

[English](../en/08-cli-and-files.md) · [文档首页](README.md)

`tlcore` 是无界面入口，覆盖与界面相同的分析、Holy Grail、过渡、去闪烁、XMP 与渲染流程，适合批处理、CI 和故障排查。所有命令都直接读写序列目录下的 `.lrtassist/project.json`。

![XMP 导出提示](../assets/screenshots/xmp-export.png)

## 8.1 调用方式

```bash
tlcore <command> [args]
```

- 成功返回退出码 `0`；
- 失败把 `错误: <原因>` 写到 stderr，并返回退出码 `1`；
- 不带参数时返回 `missing command`；
- 未知命令返回 `unknown command <name>`。

## 8.2 命令总表

| 命令 | 参数 | 作用 |
|------|------|------|
| `demo` | `<folder>` | 生成 180 帧 640×360 合成测试序列 |
| `analyze` | `<folder>` | 导入、EXIF、缩略图、亮度分析，保存项目 |
| `hg` | `<folder>` | 用已保存的 HG 参数应用到序列 |
| `hgopt` | `<folder>` | 搜索拉伸/旋转并应用，共 352 次评估 |
| `keyframes` | `<folder>` | 清除现有标记，标记第 1、中间、最后帧 |
| `transition` | `<folder>` | 在关键帧之间插值曝光、色温、色调 |
| `deflicker` | `<folder>` | 按缩略图视觉曲线计算去闪烁 EV |
| `report` | `<folder>` | 打印帧数、关键帧、亮度范围、闪烁指数、曝光范围 |
| `xmp` | `<folder>` | 为所有未跳过图像写 XMP |
| `render` | `<folder> [out]` | 渲染并编码；省略 `out` 时使用默认命名 |
| `ffmpeg-status` | — | 当前来源、版本、路径、组件目录、已安装版本 |
| `ffmpeg-import` | — | 导入系统 FFmpeg |
| `ffmpeg-install` | — | 从镜像安装 |
| `ffmpeg-install-latest` | — | 从官方源安装最新版 |
| `ffmpeg-use` | `<version>` | 切换到内置版本 |
| `ffmpeg-system` | — | 回退系统 FFmpeg |
| `ffmpeg-uninstall` | `<version>` | 卸载内置版本 |

## 8.3 典型流水线

### 从零生成并渲染测试序列

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

`demo` 要求目标目录为空或不存在；已存在非空目录会报错 `demo folder is not empty`。

### 只检查分析结果

```bash
tlcore analyze "/Volumes/SSD/sunset"
tlcore report "/Volumes/SSD/sunset"
```

`report` 示例输出：

```text
帧数: 180
关键帧: 3
亮度 0.121..0.884
闪烁指数 去闪烁前 2.41 / 后 0.37
曝光修正 -0.42..0.35 EV
```

### 批量处理多个序列

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

> 批处理前建议先备份 `.lrtassist/project.json`；这些命令会覆盖序列中的调整与关键帧。

## 8.4 命令细节与差异

### `analyze`

- 复用已有 `project.json` 中的条目；
- 仅当大小、修改时间或缩略图变化时重新解码；
- 输出 `分析完成: N 张, 平均亮度 x, 工作流 <stage>`；
- 始终按文件名自然排序，并保存项目。

### `hg` 与 `hgopt`

- `hg` 使用 `project.json` 中保存的 `hg_offset` / `hg_stretch` / `hg_rotate` / `hg_reduce_initial`；
- `hgopt` 先搜索拉伸与旋转（352 次评估），把结果四舍五入到 0.01 写回设置，再应用；
- 两者都会覆盖每帧的曝光、色温、色调，并设置 `workflow.hg_wizard = true`；
- 输出评分与曝光范围。

### `keyframes`

- 清除所有关键帧；
- 标记第 1 帧、第 `n/2` 帧、第 n 帧；
- 设置 `workflow.keyframes = 3`；
- 适合测试与演示，不适用于精确创作。

### `transition`

> 注意：CLI 的 `transition` 目前只插值**曝光、色温、色调**三列；界面中的「应用自动过渡」会插值全部 10 个创意列。需要完整过渡时请使用界面执行一次，或使用 `render` 前在 GUI 中应用。

- 至少需要一个关键帧；没有时返回「没有关键帧，请先在界面中标记关键帧」；
- 使用与界面相同的平滑/线性算法和边界保持规则；
- 设置 `workflow.auto_transition = true`。

### `deflicker`

- 的输入是缩略图视觉曲线（`visual_curve_from_thumbs`），不是全分辨率；
- `base` 修正向量固定为 0，因此结果只反映当前预览亮度；
- 使用项目中的平滑度、精度、遍数与恒定模式；
- 输出平均与最大修正 EV；
- 设置 `workflow.visual_deflicker = true`。

### `render`

- 读取项目的 `render` 设置；
- 第二个参数覆盖输出路径；
- 打印阶段、每 10 帧进度、完成路径；
- 失败时打印 `失败: ...` 并返回非零退出码；
- 使用与界面相同的并行度、编码参数与临时帧目录。

## 8.5 `project.json`

项目文件是 UTF-8 JSON，采用 `serde(default)`，缺失字段会使用默认值，便于向后兼容。

### 顶层

| 字段 | 类型 | 说明 |
|------|------|------|
| `folder` | string | 序列绝对路径（保存时 canonicalize） |
| `name` | string | 序列名，通常为文件夹名 |
| `images` | array | 帧条目 |
| `settings` | object | 序列级工具参数 |
| `workflow` | object | 工作流状态 |
| `render` | object | 渲染设置 |
| `sort_by_time` | bool | 是否按拍摄时间排序 |
| `hdr` | bool | HDR 标记（保留字段） |
| `saved_at` | string? | 最近保存时间（RFC 3339） |

### `images[]`

| 字段 | 类型 | 说明 |
|------|------|------|
| `file_name` | string | 文件名（不含目录） |
| `timestamp` | number? | EXIF 拍摄时间（Unix 秒） |
| `camera_ev` | number? | 由光圈/快门/ISO 计算的 EV |
| `iso` `shutter` `aperture` | number? | EXIF 原始值 |
| `luma` | number | 平均相对亮度 0..1 |
| `wb_r` `wb_b` | number | 灰世界白平衡，归一化到绿通道 |
| `thumb` | string? | 相对 `.lrtassist/` 的缩略图路径 |
| `size` | number | 文件字节数 |
| `modified` | number? | 修改时间（Unix 秒） |
| `width` `height` | number | 原图尺寸 |
| `error` | string? | 解码错误 |
| `adj` | object | 调整参数 |
| `keyframe` | bool | 关键帧 |
| `starred` | bool | 星标（保留字段） |
| `skipped` | bool | 跳过导出 XMP |

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

| 字段 | 默认 | 说明 |
|------|------|------|
| `deflicker_smoothing` | 0.5 | 0.02 … 1.0 |
| `deflicker_accuracy` | `"normal"` | `"Coarse"` / `"normal"` / `"Fine"` |
| `deflicker_passes` | 2 | 1 … 4 |
| `deflicker_constant` | false | 恒定模式 |
| `deflicker_multipass` | true | 是否多遍 |
| `hg_offset` | 0.0 | −5 … 5 |
| `hg_stretch` | 0.0 | −1.5 … 1.5 |
| `hg_rotate` | 0.0 | −4 … 4 |
| `hg_reduce_initial` | 0.5 | 0 … 1 |
| `transition_mode` | `"Smooth"` | `"Smooth"` / `"Linear"` |

### `workflow`

| 字段 | 说明 |
|------|------|
| `stage` | `Initialized` / `Keyframes` / `HolyGrail` / `AutoTransition` / `VisualPreviews` / `Deflicker` / `Finished` |
| `keyframes` | 关键帧数量 |
| `hg_wizard` | 是否运行过 HG |
| `auto_transition` | 是否运行过自动过渡 |
| `visual_deflicker` | 是否运行过去闪烁 |
| `deflicker_passes` | 最近一次去闪烁的遍数 |

### `render`

| 字段 | 默认 | 说明 |
|------|------|------|
| `fps` | 25.0 | 24 / 25 / 30 / 60 |
| `width` `height` | 1920 × 1080 | 1080p / 4K / 竖屏 |
| `codec` | `H264` | `H264` / `H265` / `ProRes` |
| `format` | `Mp4` | `Mp4` / `Mov` |
| `quality` | 16 | CRF 10 … 30 |
| `rate_control` | `Crf` | `Crf` / `Bitrate` |
| `bitrate_mbps` | 20.0 | 2 … 100 |
| `encoder_preset` | `Balanced` | `Fast` / `Balanced` / `Best` |
| `motion_blur` | false | 运动模糊开关 |
| `motion_blur_frames` | 2 | 混合帧数，内部夹紧 2 … 9 |
| `output_path` | null | 自定义输出路径 |
| `export_frames` | false | 保留输出帧 |
| `frame_format` | `"png"` | 中间帧格式 |
| `write_xmp` | true | 渲染前写 XMP |
| `sharpen` | 0.0 | 0 … 2 |
| `deflicker` | true | 渲染时是否应用去闪烁修正 |

## 8.6 XMP 边车

### 命名

```
<原文件名去扩展名>.xmp
```

例如 `IMG_0001.CR3` → `IMG_0001.xmp`。

### 生成策略

- 文件不存在：写完整文档；
- 文件已存在且包含 `<rdf:Description>`：只更新 `crs:*` 与 `lrt:*` 标签，保留其他元数据；
- 文件存在但无法更新：回退为完整文档，避免丢失调整；
- 写入采用 `xmp.tmp` + 重命名，保证原子性；
- `skipped = true` 的帧会跳过。

### 示例

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

说明：

- `crs:Exposure2012` 是总 EV（创意 + 去闪烁）；
- `lrt:Exposure2012` 是纯创意曝光；
- 色温只在 `WhiteBalance=Custom` 时写入，值为 `5500 + 色温 × 20`；
- 数值最多 4 位小数并去掉尾随 0。

## 8.7 目录与临时文件

| 路径 | 内容 | 生命周期 |
|------|------|----------|
| `.lrtassist/project.json` | 项目状态 | 持久 |
| `.lrtassist/project.json.tmp` | 保存中间文件 | 重命名后消失 |
| `.lrtassist/thumbs/` | 缩略图缓存 | 持久，可安全删除 |
| `.lrtassist/render/frames/` | 渲染中间帧 | 完成 / 取消 / 失败后删除 |
| `.lrtassist/exported_frames/` | 保留的输出帧 | 持久，重新渲染会覆盖 |
| `<序列目录>/*.xmp` | XMP 边车 | 持久 |
| `<序列目录>/<name>_<timestamp>.<ext>` | 默认输出视频 | 持久 |

---

[← 上一篇：设置与 FFmpeg 组件](07-settings-ffmpeg.md) · [下一篇：架构与实现 →](09-architecture.md)
