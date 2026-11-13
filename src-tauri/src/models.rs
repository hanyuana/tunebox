//! 通用数据结构。字段名尽量与原版 listen1 保持一致，方便前端与导入导出兼容。
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Track {
    /// 带平台前缀的 id，例如 `netrack_123`、`qqtrack_xxx`、`yttrack_xxx`
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub artist_id: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub album_id: String,
    /// 平台名：netease / qq / kugou / kuwo / bilibili / migu / youtube
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_url: String,
    #[serde(default)]
    pub img_url: String,
    /// true 表示该歌曲在源平台不可播放（需要会员等），播放时会走自动换源
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
    /// 平台私有的额外字段（如 lyric_url、hash、cid 等），会原样在前后端之间往返
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Track {
    pub fn extra_str(&self, key: &str) -> Option<&str> {
        self.extra.get(key).and_then(|v| v.as_str())
    }
    pub fn set_extra(&mut self, key: &str, v: impl Into<Value>) {
        self.extra.insert(key.to_string(), v.into());
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlaylistInfo {
    /// 例如 `neplaylist_123`、`nealbum_123`、`neartist_123`
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub cover_img_url: String,
    #[serde(default)]
    pub source_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Playlist {
    pub info: PlaylistInfo,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchType {
    Track,
    Playlist,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchResult {
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub playlists: Vec<PlaylistInfo>,
    #[serde(default)]
    pub total: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Sound {
    /// 真实音频地址（前端会通过本地中转服务播放）
    pub url: String,
    #[serde(default)]
    pub bitrate: String,
    #[serde(default)]
    pub platform: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Lyric {
    #[serde(default)]
    pub lyric: String,
    /// 翻译歌词
    #[serde(default)]
    pub tlyric: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filter {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterCategory {
    pub category: String,
    pub filters: Vec<Filter>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Filters {
    pub recommend: Vec<Filter>,
    pub all: Vec<FilterCategory>,
}

/// 便捷构造 Filter 列表：`filters(&[("", "全部"), ("toplist", "排行榜")])`
pub fn filters(items: &[(&str, &str)]) -> Vec<Filter> {
    items
        .iter()
        .map(|(id, name)| Filter { id: id.to_string(), name: name.to_string() })
        .collect()
}
