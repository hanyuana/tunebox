//! 应用设置（持久化到配置目录下的 settings.json）
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "url", rename_all = "lowercase")]
pub enum ProxyMode {
    /// 跟随系统代理（环境变量 / Windows 注册表 / macOS 系统设置）
    System,
    /// 直连，不走代理
    Direct,
    /// 自定义代理，例如 `http://127.0.0.1:7890`、`socks5://127.0.0.1:7891`
    Custom(String),
}

impl Default for ProxyMode {
    fn default() -> Self {
        ProxyMode::System
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 国内音乐平台使用的代理
    pub domestic_proxy: ProxyMode,
    /// YouTube 等海外平台使用的代理（挂梯子时用）
    pub overseas_proxy: ProxyMode,
    /// 歌曲不可播放时自动换源
    pub auto_choose_source: bool,
    pub auto_choose_source_list: Vec<String>,
    pub global_shortcut: bool,
    pub close_to_tray: bool,
    pub window_width: f64,
    pub window_height: f64,
    pub window_maximized: bool,
    pub lyric_window: Option<(f64, f64, f64, f64)>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            domestic_proxy: ProxyMode::Direct,
            overseas_proxy: ProxyMode::System,
            auto_choose_source: true,
            auto_choose_source_list: vec!["kuwo".into(), "qq".into(), "migu".into()],
            global_shortcut: true,
            close_to_tray: true,
            window_width: 1000.0,
            window_height: 670.0,
            window_maximized: false,
            lyric_window: None,
        }
    }
}

impl Settings {
    pub fn load(path: &PathBuf) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &PathBuf) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, s);
        }
    }
}
