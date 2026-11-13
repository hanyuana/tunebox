# TuneBox (音盒) — 极光轻量级跨平台音乐播放器 (Desktop & Android)

**TuneBox (音盒)** 是一款基于 **Rust + Tauri 2 + 原生无框架前端** 完全从零构建的轻量级跨平台音乐聚合播放器，同时支持 **Windows / macOS / Linux 桌面端** 与 **Android 移动端**。

## ✨ 核心特性

1. **极致轻量与秒级冷启动**
   - 摒弃臃肿的 Chromium 内核打包方案，采用系统原生 WebView + Rust 异步后端（`tokio` + `reqwest` + `axum`）。
   - Windows 单文件体积仅 **~5.2 MB**（压缩包 **~2.4 MB**），内存占用降低 70%+，双击秒开。
2. **全新原创「极光（Aurora）」视觉设计与响应式交互**
   - 采用原创极光青紫（Cyan-Indigo）毛玻璃主题、原创声波均衡器图标与自适应布局。
   - **桌面端**提供三栏式高效布局、系统托盘控制、全局快捷键与透明置顶桌面悬浮歌词。
   - **Android 移动端**自适应切换为底部导航栏、触控播放栏与抽屉式歌词/播放队列。
3. **7 大音源 Rust 原生实现**
   - 网易云音乐 (`netease`)
   - QQ 音乐 (`qq`)
   - 酷狗音乐 (`kugou`)
   - 酷我音乐 (`kuwo`)
   - 哔哩哔哩音频与视频分 P (`bilibili`)
   - 咪咕音乐 (`migu`)
   - **YouTube / YouTube Music (`youtube`)**：内置 InnerTube API 解析与可选 `yt-dlp` 兜底。
4. **国内外分流代理（支持挂代理畅听 YouTube）**
   - 在「设置与代理」中可分别为 **YouTube（海外音源）** 与 **国内音源** 独立设置连接方式：
     - **YouTube / 海外**：支持跟随系统代理、自定义 `http://` 或 `socks5://` 代理（如 `http://127.0.0.1:7890`）、或 TUN 直连。
     - **国内音源**：默认直连，避免开启海外代理时影响国内音源速度或触发地区限制。
5. **本地流媒体中转服务 (`stream_proxy`) & 智能跨平台换源**
   - 自动注入防盗链请求头并完整透传 HTTP `Range`，支持任意拖拽进度条。
   - 遇到无版权或灰色曲目时，自动从备选音源检索同名曲目无缝替换播放。

---

## 🚀 构建指南

### 1. 桌面端构建（Windows / Linux / macOS）
```bash
cd src-tauri
cargo build --release --bin tunebox
```

### 2. Android 移动端构建（生成 `.apk`）
Tauri 2 原生支持将 Rust 后端通过 Android NDK 编译为 `.so` 动态库并打包为 Android APK：

1. 安装 Android SDK、NDK（26+）及 Rust Android 交叉编译目标：
   ```bash
   rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
   cargo install tauri-cli --version "^2.0.0"
   ```
2. 在 `src-tauri/Cargo.toml` 中将 `[lib]` 的 `crate-type` 设为 `["staticlib", "cdylib", "rlib"]`，然后初始化并构建 APK：
   ```bash
   cd src-tauri
   cargo tauri android init
   cargo tauri android build --apk --target aarch64
   ```
3. 也可直接使用仓库内的 [scripts/build-android-and-desktop.yml](scripts/build-android-and-desktop.yml) 在 GitHub Actions 云端一键自动编译生成 Android `.apk` 与三端桌面安装包。

---

## 📄 开源协议
本项目基于 [MIT License](LICENSE) 开源。
