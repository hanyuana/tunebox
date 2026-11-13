//! YouTube / YouTube Music 平台接入（通过 InnerTube v1 接口 + 可选 yt-dlp 兜底）
//!
//! 所有发往 `*.youtube.com` / `*.googlevideo.com` 的请求在 `Http` 层会自动走 `overseas_proxy`（挂梯子代理）。
use crate::http::{query_param, Http};
use crate::models::*;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

pub const PREFIX: &str = "yt";

const ANDROID_VR_UA: &str =
    "com.google.android.apps.youtube.vr.oculus/1.60.19 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip";
const IOS_UA: &str =
    "com.google.ios.youtube/19.45.4 (iPhone16,2; U; CPU iOS 18_1_0 like Mac OS X;)";
const WEB_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// 根据 googlevideo.com 流 URL 中的 `c=` 参数返回对应的客户端 User-Agent，避免 403
pub fn ua_for_stream(url: &str) -> &'static str {
    if url.contains("c=ANDROID_VR") {
        ANDROID_VR_UA
    } else if url.contains("c=IOS") {
        IOS_UA
    } else {
        WEB_UA
    }
}

fn web_context() -> Value {
    json!({
        "client": {
            "clientName": "WEB",
            "clientVersion": "2.20250101.00.00",
            "hl": "zh-CN",
            "gl": "US"
        }
    })
}

/// 从 InnerTube 的 runs / simpleText 结构中提取纯文本
fn text_of(v: &Value) -> String {
    if let Some(s) = v.get("simpleText").and_then(|x| x.as_str()) {
        return s.to_string();
    }
    if let Some(arr) = v.get("runs").and_then(|x| x.as_array()) {
        return arr
            .iter()
            .filter_map(|r| r.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

fn best_thumb(v: &Value, video_id: &str) -> String {
    if let Some(arr) = v
        .get("thumbnail")
        .and_then(|t| t.get("thumbnails"))
        .or_else(|| v.get("thumbnails"))
        .and_then(|x| x.as_array())
    {
        if let Some(u) = arr.last().and_then(|t| t.get("url")).and_then(|u| u.as_str()) {
            if u.starts_with("//") {
                return format!("https:{u}");
            }
            return u.to_string();
        }
    }
    if !video_id.is_empty() {
        format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg")
    } else {
        String::new()
    }
}

fn parse_video_renderer(vr: &Value) -> Option<Track> {
    let vid = vr.get("videoId")?.as_str()?;
    let title = text_of(vr.get("title").unwrap_or(&Value::Null));
    if title.is_empty() {
        return None;
    }
    let owner_runs = vr
        .get("ownerText")
        .or_else(|| vr.get("longBylineText"))
        .or_else(|| vr.get("shortBylineText"))
        .unwrap_or(&Value::Null);
    let artist = text_of(owner_runs);
    let artist_id = owner_runs
        .get("runs")
        .and_then(|r| r.get(0))
        .and_then(|r| r.pointer("/navigationEndpoint/browseEndpoint/browseId"))
        .and_then(|s| s.as_str())
        .map(|id| format!("ytartist_{id}"))
        .unwrap_or_default();

    Some(Track {
        id: format!("yttrack_{vid}"),
        title,
        artist,
        artist_id,
        album: "YouTube".into(),
        album_id: String::new(),
        source: "youtube".into(),
        source_url: format!("https://www.youtube.com/watch?v={vid}"),
        img_url: best_thumb(vr, vid),
        disabled: false,
        extra: Default::default(),
    })
}

fn parse_playlist_renderer(pr: &Value) -> Option<PlaylistInfo> {
    let pid = pr.get("playlistId")?.as_str()?;
    let title = text_of(pr.get("title").unwrap_or(&Value::Null));
    if title.is_empty() {
        return None;
    }
    let author = text_of(
        pr.get("shortBylineText")
            .or_else(|| pr.get("longBylineText"))
            .unwrap_or(&Value::Null),
    );
    let count = pr
        .get("videoCount")
        .and_then(|v| v.as_str())
        .and_then(|s| s.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().ok());
    let cover = best_thumb(pr, "");
    Some(PlaylistInfo {
        id: format!("ytplaylist_{pid}"),
        title,
        cover_img_url: cover,
        source_url: format!("https://www.youtube.com/playlist?list={pid}"),
        author,
        count,
    })
}

/// 递归遍历 JSON 树收集 videoRenderer / playlistVideoRenderer / playlistRenderer / lockupViewModel
fn collect_items(v: &Value, tracks: &mut Vec<Track>, playlists: &mut Vec<PlaylistInfo>) {
    match v {
        Value::Object(map) => {
            if let Some(vr) = map.get("videoRenderer").or_else(|| map.get("playlistVideoRenderer")).or_else(|| map.get("gridVideoRenderer")) {
                if let Some(t) = parse_video_renderer(vr) {
                    tracks.push(t);
                }
                return;
            }
            if let Some(pr) = map.get("playlistRenderer").or_else(|| map.get("gridPlaylistRenderer")).or_else(|| map.get("compactPlaylistRenderer")) {
                if let Some(p) = parse_playlist_renderer(pr) {
                    playlists.push(p);
                }
                return;
            }
            // 新版 YouTube 搜索歌单有时返回 lockupViewModel
            if let Some(lvm) = map.get("lockupViewModel") {
                if let Some(p) = parse_lockup_playlist(lvm) {
                    playlists.push(p);
                }
                return;
            }
            for val in map.values() {
                collect_items(val, tracks, playlists);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_items(item, tracks, playlists);
            }
        }
        _ => {}
    }
}

fn parse_lockup_playlist(lvm: &Value) -> Option<PlaylistInfo> {
    let pid = lvm.get("contentId")?.as_str()?;
    let title = lvm
        .pointer("/metadata/lockupMetadataViewModel/title/content")
        .and_then(|v| v.as_str())?
        .to_string();
    let cover = lvm
        .pointer("/contentImage/collectionThumbnailViewModel/primaryThumbnail/thumbnailViewModel/image/sources")
        .and_then(|s| s.as_array())
        .and_then(|a| a.last())
        .and_then(|x| x.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string();
    Some(PlaylistInfo {
        id: format!("ytplaylist_{pid}"),
        title,
        cover_img_url: cover,
        source_url: format!("https://www.youtube.com/playlist?list={pid}"),
        author: String::new(),
        count: None,
    })
}

pub async fn search(
    http: &Http,
    keyword: &str,
    _page: u32,
    kind: SearchType,
) -> Result<SearchResult> {
    // params: EgIQAQ%3D%3D = 视频, EgIQAw%3D%3D = 播放列表
    let params = match kind {
        SearchType::Track => "EgIQAQ%3D%3D",
        SearchType::Playlist => "EgIQAw%3D%3D",
    };
    let body = json!({
        "context": web_context(),
        "query": keyword,
        "params": params,
    });
    let resp: Value = http
        .post("https://www.youtube.com/youtubei/v1/search?prettyPrint=false")
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut tracks = Vec::new();
    let mut playlists = Vec::new();
    collect_items(&resp, &mut tracks, &mut playlists);
    let total = match kind {
        SearchType::Track => tracks.len() as u64,
        SearchType::Playlist => playlists.len() as u64,
    };
    Ok(SearchResult { tracks, playlists, total })
}

/// 预设分类对应的搜索词（YouTube 没有无需登录的公开歌单分类墙，用搜索播放列表代替）
pub async fn show_playlist(http: &Http, offset: u32, filter_id: &str) -> Result<Vec<PlaylistInfo>> {
    if offset > 0 {
        return Ok(vec![]);
    }
    let q = match filter_id {
        "" | "hot" => "Top Hits Music Playlist",
        "cpop" => "华语流行音乐 歌单合集",
        "pop" => "Pop Music Playlist",
        "rock" => "Rock Classics Playlist",
        "edm" => "EDM Electronic Music Playlist",
        "lofi" => "Lofi Hip Hop Study Beats Playlist",
        "jpop" => "J-Pop Anime Music Playlist",
        "kpop" => "K-Pop Hits Playlist",
        "classical" => "Classical Piano Music Playlist",
        other => other,
    };
    let res = search(http, q, 1, SearchType::Playlist).await?;
    Ok(res.playlists)
}

pub fn playlist_filters() -> Filters {
    Filters {
        recommend: filters(&[
            ("", "热门"),
            ("cpop", "华语"),
            ("pop", "欧美流行"),
            ("lofi", "Lofi"),
            ("edm", "电子"),
            ("rock", "摇滚"),
            ("jpop", "日语/ACG"),
            ("kpop", "韩语"),
            ("classical", "古典/轻音乐"),
        ]),
        all: vec![],
    }
}

pub async fn get_playlist(http: &Http, list_id: &str) -> Result<Playlist> {
    let (kind, raw_id) = list_id.split_once('_').ok_or_else(|| anyhow!("无效的歌单 id"))?;
    let browse_id = match kind {
        "ytplaylist" => {
            if raw_id.starts_with("VL") {
                raw_id.to_string()
            } else {
                format!("VL{raw_id}")
            }
        }
        "ytartist" => raw_id.to_string(),
        _ => bail!("不支持的 YouTube 列表类型: {kind}"),
    };

    let body = json!({
        "context": web_context(),
        "browseId": browse_id,
    });
    let resp: Value = http
        .post("https://www.youtube.com/youtubei/v1/browse?prettyPrint=false")
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let title = text_of(
        resp.pointer("/metadata/playlistMetadataRenderer/title")
            .or_else(|| resp.pointer("/header/playlistHeaderRenderer/title"))
            .or_else(|| resp.pointer("/metadata/channelMetadataRenderer/title"))
            .unwrap_or(&Value::Null),
    );
    let title = if title.is_empty() {
        resp.pointer("/metadata/playlistMetadataRenderer/title")
            .or_else(|| resp.pointer("/metadata/channelMetadataRenderer/title"))
            .and_then(|v| v.as_str())
            .unwrap_or("YouTube 播放列表")
            .to_string()
    } else {
        title
    };

    let mut tracks = Vec::new();
    let mut dummy = Vec::new();
    collect_items(&resp, &mut tracks, &mut dummy);

    let cover = tracks
        .first()
        .map(|t| t.img_url.clone())
        .unwrap_or_default();
    let source_url = if kind == "ytartist" {
        format!("https://www.youtube.com/channel/{raw_id}")
    } else {
        format!("https://www.youtube.com/playlist?list={raw_id}")
    };

    Ok(Playlist {
        info: PlaylistInfo {
            id: list_id.to_string(),
            title,
            cover_img_url: cover,
            source_url,
            author: String::new(),
            count: Some(tracks.len() as u64),
        },
        tracks,
    })
}

/// 通过 InnerTube `/player` 接口提取直链音频流，依次尝试 ANDROID_VR 与 IOS 客户端；
/// 若失败且本机装了 `yt-dlp`，自动用 `yt-dlp -g` 兜底。
pub async fn bootstrap_track(http: &Http, track: &Track) -> Result<Sound> {
    let vid = track.id.strip_prefix("yttrack_").unwrap_or(&track.id);

    let clients = [
        (
            "ANDROID_VR",
            ANDROID_VR_UA,
            json!({
                "clientName": "ANDROID_VR",
                "clientVersion": "1.60.19",
                "deviceMake": "Oculus",
                "deviceModel": "Quest 3",
                "androidSdkVersion": 32,
                "osName": "Android",
                "osVersion": "12L",
                "hl": "en",
                "gl": "US"
            }),
        ),
        (
            "IOS",
            IOS_UA,
            json!({
                "clientName": "IOS",
                "clientVersion": "19.45.4",
                "deviceMake": "Apple",
                "deviceModel": "iPhone16,2",
                "osName": "iPhone",
                "osVersion": "18.1.0.22B83",
                "hl": "en",
                "gl": "US"
            }),
        ),
    ];

    for (_name, ua, client_ctx) in clients {
        let body = json!({
            "context": { "client": client_ctx },
            "videoId": vid,
            "contentCheckOk": true,
            "racyCheckOk": true,
        });
        let Ok(resp) = http
            .post("https://www.youtube.com/youtubei/v1/player?prettyPrint=false")
            .header(reqwest::header::USER_AGENT, ua)
            .json(&body)
            .send()
            .await
        else {
            continue;
        };
        let Ok(data) = resp.json::<Value>().await else {
            continue;
        };
        if let Some(sound) = pick_best_audio(&data) {
            return Ok(sound);
        }
    }

    // 兜底：若用户本机安装了 yt-dlp，调用它拿真实地址
    if let Ok(sound) = ytdlp_fallback(vid).await {
        return Ok(sound);
    }

    bail!("无法获取 YouTube 音频流（请检查海外代理是否连通）")
}

fn pick_best_audio(player_json: &Value) -> Option<Sound> {
    let adaptive = player_json
        .pointer("/streamingData/adaptiveFormats")
        .and_then(|v| v.as_array())?;
    let mut best: Option<(u64, String)> = None;
    for fmt in adaptive {
        let mime = fmt.get("mimeType").and_then(|v| v.as_str()).unwrap_or_default();
        if !mime.starts_with("audio/") {
            continue;
        }
        let Some(url) = fmt.get("url").and_then(|v| v.as_str()) else {
            continue;
        };
        let br = fmt.get("bitrate").and_then(|v| v.as_u64()).unwrap_or(0);
        // 优先选 audio/mp4 (m4a)，兼容性最好
        let score = br + if mime.contains("mp4") { 1_000_000 } else { 0 };
        if best.as_ref().map_or(true, |(b, _)| score > *b) {
            best = Some((score, url.to_string()));
        }
    }
    if let Some((score, url)) = best {
        let kbps = (score % 1_000_000) / 1000;
        return Some(Sound {
            url,
            bitrate: format!("{kbps}kbps"),
            platform: "youtube".into(),
        });
    }
    // 退而求其次：普通 formats（音视频合并流）
    let formats = player_json
        .pointer("/streamingData/formats")
        .and_then(|v| v.as_array())?;
    let url = formats
        .first()
        .and_then(|f| f.get("url"))
        .and_then(|u| u.as_str())?;
    Some(Sound {
        url: url.to_string(),
        bitrate: "128kbps".into(),
        platform: "youtube".into(),
    })
}

async fn ytdlp_fallback(vid: &str) -> Result<Sound> {
    let url = format!("https://www.youtube.com/watch?v={vid}");
    let out = tokio::process::Command::new("yt-dlp")
        .args(["-f", "bestaudio[ext=m4a]/bestaudio", "-g", "--no-playlist", &url])
        .output()
        .await?;
    if !out.status.success() {
        bail!("yt-dlp 退出失败");
    }
    let stream_url = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if stream_url.is_empty() {
        bail!("yt-dlp 未返回 URL");
    }
    Ok(Sound {
        url: stream_url,
        bitrate: "yt-dlp".into(),
        platform: "youtube".into(),
    })
}

/// 尝试将 YouTube 字幕（captionTracks json3 格式）转换为 LRC 歌词
pub async fn lyric(http: &Http, track: &Track) -> Result<Lyric> {
    let vid = track.id.strip_prefix("yttrack_").unwrap_or(&track.id);
    let body = json!({
        "context": {
            "client": {
                "clientName": "ANDROID_VR",
                "clientVersion": "1.60.19",
                "androidSdkVersion": 32,
                "hl": "zh-CN",
                "gl": "US"
            }
        },
        "videoId": vid,
    });
    let Ok(resp) = http
        .post("https://www.youtube.com/youtubei/v1/player?prettyPrint=false")
        .header(reqwest::header::USER_AGENT, ANDROID_VR_UA)
        .json(&body)
        .send()
        .await
    else {
        return Ok(Lyric::default());
    };
    let Ok(data) = resp.json::<Value>().await else {
        return Ok(Lyric::default());
    };
    let Some(tracks) = data
        .pointer("/captions/playerCaptionsTracklistRenderer/captionTracks")
        .and_then(|v| v.as_array())
    else {
        return Ok(Lyric::default());
    };
    let Some(base_url) = tracks
        .first()
        .and_then(|t| t.get("baseUrl"))
        .and_then(|u| u.as_str())
    else {
        return Ok(Lyric::default());
    };
    let sub_url = format!("{base_url}&fmt=json3");
    let Ok(sub_json) = http.get_json::<Value>(&sub_url).await else {
        return Ok(Lyric::default());
    };
    let mut lrc = String::new();
    if let Some(events) = sub_json.get("events").and_then(|v| v.as_array()) {
        for ev in events {
            let ms = ev.get("tStartMs").and_then(|v| v.as_u64()).unwrap_or(0);
            let Some(segs) = ev.get("segs").and_then(|v| v.as_array()) else {
                continue;
            };
            let line: String = segs
                .iter()
                .filter_map(|s| s.get("utf8").and_then(|u| u.as_str()))
                .collect::<Vec<_>>()
                .join("")
                .replace('\n', " ")
                .trim()
                .to_string();
            if line.is_empty() {
                continue;
            }
            let min = ms / 60_000;
            let sec = (ms % 60_000) / 1000;
            let cs = (ms % 1000) / 10;
            lrc.push_str(&format!("[{min:02}:{sec:02}.{cs:02}]{line}\n"));
        }
    }
    Ok(Lyric { lyric: lrc, tlyric: String::new() })
}

pub async fn parse_url(_http: &Http, url: &str) -> Option<String> {
    if !url.contains("youtube.com") && !url.contains("youtu.be") {
        return None;
    }
    if let Some(list) = query_param(url, "list") {
        return Some(format!("ytplaylist_{list}"));
    }
    if let Some(idx) = url.find("/channel/") {
        let rest = &url[idx + "/channel/".len()..];
        let id = rest.split(['/', '?', '&']).next()?;
        if !id.is_empty() {
            return Some(format!("ytartist_{id}"));
        }
    }
    None
}
