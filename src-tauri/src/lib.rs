pub mod http;
pub mod models;
pub mod providers;
pub mod settings;
pub mod stream_proxy;

use crate::http::{urlencode, Http};
use crate::models::*;
use crate::settings::Settings;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tauri::{AppHandle, Emitter, Manager, State};

#[cfg(desktop)]
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    WindowEvent,
};

pub struct AppState {
    pub http: Arc<Http>,
    pub settings: RwLock<Settings>,
    pub settings_path: PathBuf,
    pub stream_port: u16,
}

#[derive(Serialize)]
pub struct ProviderMeta {
    pub name: &'static str,
    pub prefix: &'static str,
    pub label: &'static str,
}

#[derive(Serialize)]
pub struct BootstrapInfo {
    pub stream_port: u16,
    pub providers: Vec<ProviderMeta>,
    pub settings: Settings,
}

fn proxied_stream_url(port: u16, raw_url: &str) -> String {
    format!("http://127.0.0.1:{port}/stream?url={}", urlencode(raw_url))
}

#[tauri::command]
fn api_init(state: State<'_, AppState>) -> BootstrapInfo {
    let providers = providers::PROVIDERS
        .iter()
        .map(|&(name, prefix, label)| ProviderMeta { name, prefix, label })
        .collect();
    BootstrapInfo {
        stream_port: state.stream_port,
        providers,
        settings: state.settings.read().unwrap().clone(),
    }
}

#[tauri::command]
async fn api_search(
    state: State<'_, AppState>,
    source: String,
    keywords: String,
    page: u32,
    search_type: SearchType,
) -> Result<SearchResult, String> {
    let page = page.max(1);
    if source == "allmusic" {
        // 并行搜索所有国内平台（若用户选全平台搜索）
        let http = state.http.clone();
        let kw = keywords.clone();
        let (ne, qq, kg, kw_res, bi, mg) = tokio::join!(
            providers::search(&http, "netease", &kw, page, search_type),
            providers::search(&http, "qq", &kw, page, search_type),
            providers::search(&http, "kugou", &kw, page, search_type),
            providers::search(&http, "kuwo", &kw, page, search_type),
            providers::search(&http, "bilibili", &kw, page, search_type),
            providers::search(&http, "migu", &kw, page, search_type),
        );
        let lists = [ne, qq, kg, kw_res, bi, mg];
        let mut tracks = Vec::new();
        let mut playlists = Vec::new();
        let max_t = lists
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|r| r.tracks.len())
            .max()
            .unwrap_or(0);
        for i in 0..max_t {
            for r in lists.iter().filter_map(|r| r.as_ref().ok()) {
                if let Some(t) = r.tracks.get(i) {
                    tracks.push(t.clone());
                }
            }
        }
        let max_p = lists
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|r| r.playlists.len())
            .max()
            .unwrap_or(0);
        for i in 0..max_p {
            for r in lists.iter().filter_map(|r| r.as_ref().ok()) {
                if let Some(p) = r.playlists.get(i) {
                    playlists.push(p.clone());
                }
            }
        }
        return Ok(SearchResult {
            total: (tracks.len().max(playlists.len()) * 5) as u64,
            tracks,
            playlists,
        });
    }
    providers::search(&state.http, &source, &keywords, page, search_type)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn api_show_playlist(
    state: State<'_, AppState>,
    source: String,
    offset: u32,
    filter_id: String,
) -> Result<Vec<PlaylistInfo>, String> {
    providers::show_playlist(&state.http, &source, offset, &filter_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn api_playlist_filters(source: String) -> Result<Filters, String> {
    providers::playlist_filters(&source).map_err(|e| e.to_string())
}

#[tauri::command]
async fn api_get_playlist(state: State<'_, AppState>, list_id: String) -> Result<Playlist, String> {
    providers::get_playlist(&state.http, &list_id)
        .await
        .map_err(|e| e.to_string())
}

/// 清洗歌名以便跨平台匹配（去除括号附加词与大小写差异）
fn normalize_song_title(s: &str) -> String {
    let lower = s.trim().to_lowercase();
    let base = lower
        .split(['(', '（', '-', '—', '【', '['])
        .next()
        .unwrap_or(&lower)
        .trim();
    if base.is_empty() {
        lower
    } else {
        base.to_string()
    }
}

/// 获取歌曲可播放地址；若原平台失败且开启了「自动换源」，依次从备选平台搜索同名同歌手曲目替换
#[tauri::command]
async fn api_bootstrap_track(state: State<'_, AppState>, track: Track) -> Result<Sound, String> {
    let http = state.http.clone();
    let port = state.stream_port;

    if !track.disabled {
        if let Ok(mut sound) = providers::bootstrap_track(&http, &track).await {
            if !sound.url.is_empty() {
                sound.url = proxied_stream_url(port, &sound.url);
                return Ok(sound);
            }
        }
    }

    let auto_choose = state.settings.read().map(|s| s.auto_choose_source).unwrap_or(true);
    if !auto_choose || track.source == "youtube" {
        return Err("当前歌曲无法播放（可能为 VIP 专享或无版权）".into());
    }

    // 固定按全量高可用音源顺序依次兜底（优先酷我与 B站，保证 VIP 曲目也能顺畅播放）
    let failover_sources = ["kuwo", "bilibili", "netease", "qq", "kugou", "migu"];
    let clean_title = normalize_song_title(&track.title);
    let query = if track.artist.trim().is_empty() || track.artist == "未知" {
        track.title.clone()
    } else {
        format!("{} {}", track.title, track.artist)
    };

    for src in failover_sources {
        if src == track.source {
            continue;
        }
        let Ok(res) = providers::search(&http, src, &query, 1, SearchType::Track).await else {
            continue;
        };
        for (idx, cand) in res.tracks.iter().take(6).enumerate() {
            if cand.disabled {
                continue;
            }
            let cand_clean = normalize_song_title(&cand.title);
            let title_match = idx == 0
                || (!clean_title.is_empty()
                    && (cand_clean.contains(&clean_title) || clean_title.contains(&cand_clean)));
            if !title_match {
                continue;
            }
            if let Ok(mut sound) = providers::bootstrap_track(&http, cand).await {
                if !sound.url.is_empty() {
                    sound.url = proxied_stream_url(port, &sound.url);
                    return Ok(sound);
                }
            }
        }
    }

    Err("当前歌曲在所有备选音源均无法播放".into())
}

#[tauri::command]
async fn api_lyric(state: State<'_, AppState>, track: Track) -> Result<Lyric, String> {
    providers::lyric(&state.http, &track)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn api_parse_url(state: State<'_, AppState>, url: String) -> Result<Option<String>, String> {
    Ok(providers::parse_url(&state.http, &url).await)
}

#[tauri::command]
fn api_save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    new_settings: Settings,
) -> Result<(), String> {
    state
        .http
        .reconfigure(&new_settings.domestic_proxy, &new_settings.overseas_proxy);
    #[cfg(desktop)]
    let shortcut_changed = state.settings.read().unwrap().global_shortcut != new_settings.global_shortcut;
    #[cfg(desktop)]
    let enable_sc = new_settings.global_shortcut;
    *state.settings.write().unwrap() = new_settings.clone();
    new_settings.save(&state.settings_path);
    #[cfg(desktop)]
    if shortcut_changed {
        update_shortcuts(&app, enable_sc);
    }
    #[cfg(not(desktop))]
    let _ = &app;
    Ok(())
}

#[tauri::command]
fn api_update_tray(app: AppHandle, title: String, artist: String) {
    #[cfg(desktop)]
    if let Some(tray) = app.tray_by_id("main-tray") {
        let tip = if artist.trim().is_empty() {
            format!("TuneBox - {title}")
        } else {
            format!("TuneBox - {title} - {artist}")
        };
        let _ = tray.set_tooltip(Some(tip.as_str()));
    }
    #[cfg(not(desktop))]
    let _ = (app, title, artist);
}

#[cfg(desktop)]
fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let i_show = MenuItem::with_id(app, "show", "显示 / 隐藏主窗口", true, None::<&str>)?;
    let i_sep = PredefinedMenuItem::separator(app)?;
    let i_play = MenuItem::with_id(app, "space", "播放 / 暂停", true, None::<&str>)?;
    let i_prev = MenuItem::with_id(app, "left", "上一首", true, None::<&str>)?;
    let i_next = MenuItem::with_id(app, "right", "下一首", true, None::<&str>)?;
    let i_sep2 = PredefinedMenuItem::separator(app)?;
    let i_quit = MenuItem::with_id(app, "quit", "退出 TuneBox", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[&i_show, &i_sep, &i_play, &i_prev, &i_next, &i_sep2, &i_quit],
    )?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("TuneBox 音盒")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "show" => toggle_main_window(app),
            "space" | "left" | "right" => {
                let _ = app.emit("global-shortcut", ev.id().as_ref());
            }
            "quit" => {
                std::process::exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                toggle_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

#[cfg(desktop)]
fn toggle_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let visible = w.is_visible().unwrap_or(true);
        let minimized = w.is_minimized().unwrap_or(false);
        if visible && !minimized {
            let _ = w.hide();
        } else {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
    }
}

#[cfg(desktop)]
fn update_shortcuts(app: &AppHandle, enable: bool) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    if !enable {
        return;
    }
    let mappings = [
        ("CommandOrControl+Alt+Space", "space"),
        ("CommandOrControl+Alt+Left", "left"),
        ("CommandOrControl+Alt+Right", "right"),
        ("MediaPlayPause", "space"),
        ("MediaPreviousTrack", "left"),
        ("MediaNextTrack", "right"),
    ];
    for (accel, action) in mappings {
        let action = action.to_string();
        let _ = gs.on_shortcut(accel, move |app, _sc, ev| {
            if ev.state == ShortcutState::Pressed {
                let _ = app.emit("global-shortcut", action.as_str());
            }
        });
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());

    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build());

    let builder = builder.setup(|app| {
        let cfg_dir = app
            .path()
            .app_config_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("tunebox"));
        let settings_path = cfg_dir.join("settings.json");
        let settings = Settings::load(&settings_path);
        let http = Arc::new(Http::new(&settings.domestic_proxy, &settings.overseas_proxy));
        let stream_port = tauri::async_runtime::block_on(stream_proxy::spawn_stream_server(http.clone()));

        #[cfg(desktop)]
        let enable_sc = settings.global_shortcut;

        app.manage(AppState {
            http,
            settings: RwLock::new(settings),
            settings_path,
            stream_port,
        });

        #[cfg(desktop)]
        {
            let _ = setup_tray(app.handle());
            update_shortcuts(app.handle(), enable_sc);
        }
        Ok(())
    });

    #[cfg(desktop)]
    let builder = builder.on_window_event(|window, event| {
        if window.label() == "main" {
            if matches!(event, WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed) {
                // 关闭主窗口或任务栏右击关闭时直接彻底退出进程，不残留后台进程
                std::process::exit(0);
            }
        }
    });

    builder
        .invoke_handler(tauri::generate_handler![
            api_init,
            api_search,
            api_show_playlist,
            api_playlist_filters,
            api_get_playlist,
            api_bootstrap_track,
            api_lyric,
            api_parse_url,
            api_save_settings,
            api_update_tray,
        ])
        .run(tauri::generate_context!())
        .expect("启动 TuneBox 失败");
}
