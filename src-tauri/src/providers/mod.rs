//! 各音乐平台的实现。
//!
//! 每个平台模块都必须提供以下函数（签名完全一致，由下面的 `dispatch!` 宏统一分发）：
//!
//! ```ignore
//! pub const PREFIX: &str;            // 两个字母的 id 前缀，例如 "ne"
//! pub async fn search(http: &Http, keyword: &str, page: u32, kind: SearchType) -> Result<SearchResult>;
//! pub async fn show_playlist(http: &Http, offset: u32, filter_id: &str) -> Result<Vec<PlaylistInfo>>;
//! pub fn playlist_filters() -> Filters;
//! pub async fn get_playlist(http: &Http, list_id: &str) -> Result<Playlist>;   // list_id 带前缀，如 "neplaylist_123"
//! pub async fn bootstrap_track(http: &Http, track: &Track) -> Result<Sound>;   // 无法播放时返回 Err
//! pub async fn lyric(http: &Http, track: &Track) -> Result<Lyric>;
//! pub async fn parse_url(http: &Http, url: &str) -> Option<String>;           // 识别平台链接，返回歌单 id
//! ```
use crate::http::Http;
use crate::models::*;
use anyhow::{anyhow, Result};

pub mod bilibili;
pub mod kugou;
pub mod kuwo;
pub mod migu;
pub mod netease;
pub mod qq;
pub mod youtube;

/// 平台列表：(名称, 前缀, 显示名)
pub const PROVIDERS: &[(&str, &str, &str)] = &[
    ("netease", netease::PREFIX, "网易云"),
    ("qq", qq::PREFIX, "QQ音乐"),
    ("kugou", kugou::PREFIX, "酷狗"),
    ("kuwo", kuwo::PREFIX, "酷我"),
    ("bilibili", bilibili::PREFIX, "B站"),
    ("migu", migu::PREFIX, "咪咕"),
    ("youtube", youtube::PREFIX, "YouTube"),
];

pub fn name_by_id(id: &str) -> Option<&'static str> {
    let prefix = id.get(..2)?;
    PROVIDERS.iter().find(|p| p.1 == prefix).map(|p| p.0)
}

macro_rules! dispatch {
    ($source:expr, $func:ident ( $($arg:expr),* )) => {
        match $source {
            "netease" => netease::$func($($arg),*).await,
            "qq" => qq::$func($($arg),*).await,
            "kugou" => kugou::$func($($arg),*).await,
            "kuwo" => kuwo::$func($($arg),*).await,
            "bilibili" => bilibili::$func($($arg),*).await,
            "migu" => migu::$func($($arg),*).await,
            "youtube" => youtube::$func($($arg),*).await,
            other => Err(anyhow!("未知的平台: {other}")),
        }
    };
}

pub async fn search(http: &Http, source: &str, keyword: &str, page: u32, kind: SearchType) -> Result<SearchResult> {
    dispatch!(source, search(http, keyword, page, kind))
}

pub async fn show_playlist(http: &Http, source: &str, offset: u32, filter_id: &str) -> Result<Vec<PlaylistInfo>> {
    dispatch!(source, show_playlist(http, offset, filter_id))
}

pub fn playlist_filters(source: &str) -> Result<Filters> {
    Ok(match source {
        "netease" => netease::playlist_filters(),
        "qq" => qq::playlist_filters(),
        "kugou" => kugou::playlist_filters(),
        "kuwo" => kuwo::playlist_filters(),
        "bilibili" => bilibili::playlist_filters(),
        "migu" => migu::playlist_filters(),
        "youtube" => youtube::playlist_filters(),
        other => return Err(anyhow!("未知的平台: {other}")),
    })
}

pub async fn get_playlist(http: &Http, list_id: &str) -> Result<Playlist> {
    let source = name_by_id(list_id).ok_or_else(|| anyhow!("无法识别的歌单 id: {list_id}"))?;
    dispatch!(source, get_playlist(http, list_id))
}

pub async fn bootstrap_track(http: &Http, track: &Track) -> Result<Sound> {
    let source = if track.source.is_empty() {
        name_by_id(&track.id).unwrap_or_default()
    } else {
        track.source.as_str()
    };
    dispatch!(source, bootstrap_track(http, track))
}

pub async fn lyric(http: &Http, track: &Track) -> Result<Lyric> {
    let source = name_by_id(&track.id).unwrap_or(track.source.as_str());
    dispatch!(source, lyric(http, track))
}

/// 依次让各平台尝试识别链接
pub async fn parse_url(http: &Http, url: &str) -> Option<String> {
    let futs = (
        netease::parse_url(http, url),
        qq::parse_url(http, url),
        kugou::parse_url(http, url),
        kuwo::parse_url(http, url),
        bilibili::parse_url(http, url),
        migu::parse_url(http, url),
        youtube::parse_url(http, url),
    );
    let (a, b, c, d, e, f, g) = tokio::join!(futs.0, futs.1, futs.2, futs.3, futs.4, futs.5, futs.6);
    a.or(b).or(c).or(d).or(e).or(f).or(g)
}
