# TuneBox (音盒) — 轻量级全平台聚合音乐播放器

使用 **Rust + Tauri 2 + 原生无框架前端** 构建的跨平台桌面聚合音乐播放器。

## 核心特性
1. **超轻体积与秒开速度**：使用系统原生 WebView2 代替 Electron 内置 Chromium，安装包体积从 ~80MB 降至 **~5–8MB**，冷启动接近秒开。
2. **全平台音源 Rust 原生实现**：
   - 网易云音乐 (`netease` - weapi/eapi AES+RSA 加密)
   - QQ 音乐 (`qq`)
   - 酷狗音乐 (`kugou`)
   - 酷我音乐 (`kuwo`)
   - 哔哩哔哩音频/分P视频 (`bilibili` - WBI 签名)
   - 咪咕音乐 (`migu`)
   - **YouTube / YouTube Music (`youtube` - InnerTube API + 可选 `yt-dlp` 兜底)**
3. **分流代理（支持挂梯子访问 YouTube）**：
   - 在「设置与代理」中可单独为 **YouTube（海外）** 与 **国内音源** 分别指定代理模式（跟随系统代理 / 自定义 `http://` 或 `socks5://` 代理 / 直连），互不干扰。
4. **本地流媒体中转服务 (`stream_proxy`)**：
   - 自动注入各平台防盗链 `Referer` / `Origin` / `User-Agent`，完整透传 HTTP `Range`，支持拖拽进度条与 YouTube 梯子流式播放。
5. **桌面级体验**：
   - 系统托盘控制、全局快捷键 (`Ctrl+Alt+Space/Left/Right` 与多媒体键)、透明置顶桌面悬浮歌词、灰色无版权歌曲自动跨平台换源、支持歌单备份 JSON 导入导出。
