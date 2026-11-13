//! 哔哩哔哩（Bilibili）音频与视频分 P 音源实现（WBI 签名）。
use crate::http::{urlencode, Http};
use crate::models::*;
use anyhow::{anyhow, bail, Result};
use md5::{Digest, Md5};
use regex_lite::Regex;
use serde_json::Value;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PREFIX: &str = "bi";

const MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

static WBI_KEY: RwLock<Option<(String, String)>> = RwLock::new(None);

fn val_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn normalize_img_url(url: &str) -> String {
    if url.starts_with("//") {
        format!("https:{url}")
    } else {
        url.to_string()
    }
}

/// 对应原版 `bilibili.htmlDecode`（剥离高亮 HTML 标签并解码常见 HTML 实体）
fn html_decode(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let without_tags = match Regex::new(r"<[^>]*>") {
        Ok(re) => re.replace_all(value, "").into_owned(),
        Err(_) => value.to_string(),
    };
    let mut s = without_tags
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ");

    if s.contains("&#") {
        if let Ok(re_dec) = Regex::new(r"&#(\d+);") {
            s = re_dec
                .replace_all(&s, |caps: &regex_lite::Captures| {
                    caps.get(1)
                        .and_then(|m| m.as_str().parse::<u32>().ok())
                        .and_then(char::from_u32)
                        .map(|c| c.to_string())
                        .unwrap_or_default()
                })
                .into_owned();
        }
        if let Ok(re_hex) = Regex::new(r"&#[xX]([0-9a-fA-F]+);") {
            s = re_hex
                .replace_all(&s, |caps: &regex_lite::Captures| {
                    caps.get(1)
                        .and_then(|m| u32::from_str_radix(m.as_str(), 16).ok())
                        .and_then(char::from_u32)
                        .map(|c| c.to_string())
                        .unwrap_or_default()
                })
                .into_owned();
        }
    }

    s.replace("&amp;", "&")
}

/// 与 JS `encodeURIComponent` 对齐（空格编码为 `%20`，保留 `~`）
fn encode_uri_component(s: &str) -> String {
    urlencode(s).replace('+', "%20").replace("%7E", "~")
}

fn extract_wbi_part(url: &str) -> Option<String> {
    let file = url.rsplit('/').next()?;
    let (stem, _) = file.rsplit_once('.')?;
    if stem.is_empty() {
        None
    } else {
        Some(stem.to_string())
    }
}

async fn fetch_wbi_key(http: &Http) -> Result<(String, String)> {
    let v: Value = http
        .get_json("https://api.bilibili.com/x/web-interface/nav")
        .await?;
    let wbi_img = &v["data"]["wbi_img"];
    let img_url = wbi_img["img_url"].as_str().unwrap_or_default();
    let sub_url = wbi_img["sub_url"].as_str().unwrap_or_default();
    let img_key = extract_wbi_part(img_url).ok_or_else(|| anyhow!("无法提取 B站 wbi img_key"))?;
    let sub_key = extract_wbi_part(sub_url).ok_or_else(|| anyhow!("无法提取 B站 wbi sub_key"))?;
    Ok((img_key, sub_key))
}

fn clear_wbi_key() {
    if let Ok(mut guard) = WBI_KEY.write() {
        *guard = None;
    }
}

async fn get_wbi_key(http: &Http) -> Result<(String, String)> {
    if let Ok(guard) = WBI_KEY.read() {
        if let Some(cached) = guard.clone() {
            return Ok(cached);
        }
    }
    let key = fetch_wbi_key(http).await?;
    if let Ok(mut guard) = WBI_KEY.write() {
        *guard = Some(key.clone());
    }
    Ok(key)
}

fn get_mixin_key(original: &str) -> String {
    let bytes = original.as_bytes();
    let mut temp = String::with_capacity(32);
    for &idx in MIXIN_KEY_ENC_TAB.iter().take(32) {
        if let Some(&b) = bytes.get(idx) {
            temp.push(b as char);
        }
    }
    temp
}

async fn enc_wbi(http: &Http, params: &[(&str, String)]) -> Result<String> {
    let (img_key, sub_key) = get_wbi_key(http).await?;
    let mixin_key = get_mixin_key(&format!("{img_key}{sub_key}"));
    let curr_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut pairs: Vec<(String, String)> = params
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    pairs.push(("wts".to_string(), curr_time.to_string()));
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let query: Vec<String> = pairs
        .into_iter()
        .map(|(k, v)| {
            let filtered: String = v
                .chars()
                .filter(|c| !matches!(c, '!' | '\'' | '(' | ')' | '*'))
                .collect();
            format!(
                "{}={}",
                encode_uri_component(&k),
                encode_uri_component(&filtered)
            )
        })
        .collect();

    let query_string = query.join("&");
    let mut hasher = Md5::new();
    hasher.update(format!("{query_string}{mixin_key}").as_bytes());
    let wbi_sign = hex::encode(hasher.finalize());

    Ok(format!("{query_string}&w_rid={wbi_sign}"))
}

/// 确保已设置 `buvid3` cookie（原版默认写 `buvid3=0`，此处同时尝试从 finger/spi 获取有效值）
async fn ensure_buvid(http: &Http) {
    if http.get_cookie("https://api.bilibili.com", "buvid3").is_some() {
        return;
    }
    http.set_cookie("https://api.bilibili.com", "buvid3=0");
    http.set_cookie("https://www.bilibili.com", "buvid3=0");
    if let Ok(v) = http
        .get_json::<Value>("https://api.bilibili.com/x/frontend/finger/spi")
        .await
    {
        if let Some(b3) = v["data"]["b_3"].as_str().filter(|s| !s.is_empty()) {
            http.set_cookie("https://api.bilibili.com", &format!("buvid3={b3}"));
            http.set_cookie("https://www.bilibili.com", &format!("buvid3={b3}"));
        }
        if let Some(b4) = v["data"]["b_4"].as_str().filter(|s| !s.is_empty()) {
            http.set_cookie("https://api.bilibili.com", &format!("buvid4={b4}"));
            http.set_cookie("https://www.bilibili.com", &format!("buvid4={b4}"));
        }
    }
}

async fn wrap_wbi_request(http: &Http, url: &str, params: &[(&str, String)]) -> Result<Value> {
    ensure_buvid(http).await;
    if let Ok(qs) = enc_wbi(http, params).await {
        let target_url = format!("{url}?{qs}");
        if let Ok(v) = http.get_json::<Value>(&target_url).await {
            if v["code"].as_i64().unwrap_or(0) == 0 {
                return Ok(v);
            }
        }
    }
    // 失败时清空 wbi_key 重试一次，避免因 wbi_key 过期导致请求失败
    clear_wbi_key();
    let qs = enc_wbi(http, params).await?;
    let target_url = format!("{url}?{qs}");
    http.get_json::<Value>(&target_url).await
}

fn bi_convert_song(song_info: &Value) -> Track {
    let id = val_to_string(&song_info["id"]);
    let uid = val_to_string(&song_info["uid"]);
    let mut track = Track {
        id: format!("bitrack_{id}"),
        title: song_info["title"].as_str().unwrap_or_default().to_string(),
        artist: song_info["uname"].as_str().unwrap_or_default().to_string(),
        artist_id: format!("biartist_{uid}"),
        source: "bilibili".to_string(),
        source_url: format!("https://www.bilibili.com/audio/au{id}"),
        img_url: normalize_img_url(song_info["cover"].as_str().unwrap_or_default()),
        ..Default::default()
    };
    if let Some(lyric) = song_info["lyric"].as_str().filter(|s| !s.is_empty()) {
        track.set_extra("lyric_url", lyric);
    }
    track
}

fn bi_convert_song2(song_info: &Value) -> Track {
    let bvid = val_to_string(&song_info["bvid"]);
    let mid = val_to_string(&song_info["mid"]);
    let img_url = normalize_img_url(song_info["pic"].as_str().unwrap_or_default());
    Track {
        id: format!("bitrack_v_{bvid}"),
        title: html_decode(song_info["title"].as_str().unwrap_or_default()),
        artist: html_decode(song_info["author"].as_str().unwrap_or_default()),
        artist_id: format!("biartist_v_{mid}"),
        source: "bilibili".to_string(),
        source_url: format!("https://www.bilibili.com/{bvid}"),
        img_url,
        ..Default::default()
    }
}

fn bi_convert_song3(song_info: &Value, bvid: &str, author: &Value, default_img: &str) -> Track {
    let img_url = match song_info["first_frame"].as_str().filter(|s| !s.is_empty()) {
        Some(frame) => normalize_img_url(frame),
        None => normalize_img_url(default_img),
    };
    let cid = val_to_string(&song_info["cid"]);
    let page = val_to_string(&song_info["page"]);
    let mid = val_to_string(&author["mid"]);
    Track {
        id: format!("bitrack_v_{bvid}-{cid}"),
        title: html_decode(song_info["part"].as_str().unwrap_or_default()),
        artist: html_decode(author["name"].as_str().unwrap_or_default()),
        artist_id: format!("biartist_v_{mid}"),
        source: "bilibili".to_string(),
        source_url: format!("https://www.bilibili.com/{bvid}/?p={page}"),
        img_url,
        ..Default::default()
    }
}

pub async fn show_playlist(http: &Http, offset: u32, _filter_id: &str) -> Result<Vec<PlaylistInfo>> {
    let page = offset / 20 + 1;
    let target_url =
        format!("https://www.bilibili.com/audio/music-service-c/web/menu/hit?ps=20&pn={page}");
    let v: Value = http.get_json(&target_url).await?;
    let arr = v["data"]["data"].as_array().cloned().unwrap_or_default();
    let result = arr
        .iter()
        .map(|item| {
            let menu_id = val_to_string(&item["menuId"]);
            PlaylistInfo {
                cover_img_url: normalize_img_url(item["cover"].as_str().unwrap_or_default()),
                title: item["title"].as_str().unwrap_or_default().to_string(),
                id: format!("biplaylist_{menu_id}"),
                source_url: format!("https://www.bilibili.com/audio/am{menu_id}"),
                ..Default::default()
            }
        })
        .collect();
    Ok(result)
}

pub fn playlist_filters() -> Filters {
    Filters::default()
}

async fn bi_get_playlist(http: &Http, list_id: &str) -> Result<Playlist> {
    let sid = list_id.rsplit('_').next().unwrap_or_default();
    let info_url =
        format!("https://www.bilibili.com/audio/music-service-c/web/menu/info?sid={sid}");
    let info_res: Value = http.get_json(&info_url).await?;
    let data = &info_res["data"];
    let info = PlaylistInfo {
        cover_img_url: normalize_img_url(data["cover"].as_str().unwrap_or_default()),
        title: data["title"].as_str().unwrap_or_default().to_string(),
        id: format!("biplaylist_{sid}"),
        source_url: format!("https://www.bilibili.com/audio/am{sid}"),
        ..Default::default()
    };

    let songs_url = format!(
        "https://www.bilibili.com/audio/music-service-c/web/song/of-menu?pn=1&ps=100&sid={sid}"
    );
    let songs_res: Value = http.get_json(&songs_url).await?;
    let tracks = songs_res["data"]["data"]
        .as_array()
        .map(|arr| arr.iter().map(bi_convert_song).collect())
        .unwrap_or_default();

    Ok(Playlist { info, tracks })
}

async fn bi_album() -> Result<Playlist> {
    Ok(Playlist::default())
}

async fn bi_track(http: &Http, list_id: &str) -> Result<Playlist> {
    let raw_id = list_id.rsplit('_').next().unwrap_or_default();
    let bvid = raw_id.split('-').next().unwrap_or(raw_id);
    let target_url = format!("https://api.bilibili.com/x/web-interface/view?bvid={bvid}");
    let v: Value = http.get_json(&target_url).await?;
    let data = &v["data"];
    let default_img = normalize_img_url(data["pic"].as_str().unwrap_or_default());
    let info = PlaylistInfo {
        cover_img_url: default_img.clone(),
        title: data["title"].as_str().unwrap_or_default().to_string(),
        id: format!("bitrack_v_{bvid}"),
        source_url: format!("https://www.bilibili.com/{bvid}"),
        ..Default::default()
    };
    let author = &data["owner"];
    let tracks = data["pages"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|item| bi_convert_song3(item, bvid, author, &default_img))
                .collect()
        })
        .unwrap_or_default();

    Ok(Playlist { info, tracks })
}

async fn bi_artist(http: &Http, list_id: &str) -> Result<Playlist> {
    let parts: Vec<&str> = list_id.split('_').collect();
    let artist_id = parts.last().copied().unwrap_or_default();

    let acc_res = wrap_wbi_request(
        http,
        "https://api.bilibili.com/x/space/wbi/acc/info",
        &[("mid", artist_id.to_string())],
    )
    .await?;
    let acc_data = &acc_res["data"];
    let info = PlaylistInfo {
        cover_img_url: normalize_img_url(acc_data["face"].as_str().unwrap_or_default()),
        title: acc_data["name"].as_str().unwrap_or_default().to_string(),
        id: format!("biartist_{artist_id}"),
        source_url: format!("https://space.bilibili.com/{artist_id}/#/audio"),
        ..Default::default()
    };

    if parts.len() == 3 {
        let arc_res = wrap_wbi_request(
            http,
            "https://api.bilibili.com/x/space/wbi/arc/search",
            &[
                ("mid", artist_id.to_string()),
                ("pn", "1".to_string()),
                ("ps", "25".to_string()),
                ("order", "click".to_string()),
                ("index", "1".to_string()),
            ],
        )
        .await?;
        let tracks = arc_res["data"]["list"]["vlist"]
            .as_array()
            .map(|arr| arr.iter().map(bi_convert_song2).collect())
            .unwrap_or_default();
        return Ok(Playlist { info, tracks });
    }

    let target_url = format!(
        "https://api.bilibili.com/audio/music-service-c/web/song/upper?pn=1&ps=0&order=2&uid={artist_id}"
    );
    let res: Value = http.get_json(&target_url).await?;
    let tracks = res["data"]["data"]
        .as_array()
        .map(|arr| arr.iter().map(bi_convert_song).collect())
        .unwrap_or_default();

    Ok(Playlist { info, tracks })
}

pub async fn get_playlist(http: &Http, list_id: &str) -> Result<Playlist> {
    let prefix = list_id.split('_').next().unwrap_or_default();
    match prefix {
        "biplaylist" => bi_get_playlist(http, list_id).await,
        "bialbum" => bi_album().await,
        "biartist" => bi_artist(http, list_id).await,
        "bitrack" => bi_track(http, list_id).await,
        other => bail!("未知的 B站歌单类型: {other}"),
    }
}

pub async fn parse_url(_http: &Http, url: &str) -> Option<String> {
    let re = Regex::new(r"//www\.bilibili\.com/audio/am([0-9]+)").ok()?;
    let caps = re.captures(url)?;
    let playlist_id = caps.get(1)?.as_str();
    Some(format!("biplaylist_{playlist_id}"))
}

pub async fn bootstrap_track(http: &Http, track: &Track) -> Result<Sound> {
    ensure_buvid(http).await;
    let track_id = track.id.as_str();
    if let Some(rest) = track_id.strip_prefix("bitrack_v_") {
        let (bvid, cid_from_id) = match rest.split_once('-') {
            Some((b, c)) if !c.is_empty() => (b, Some(c.to_string())),
            _ => (rest, None),
        };

        let cid = match cid_from_id.or_else(|| track.extra_str("cid").map(|s| s.to_string())) {
            Some(c) if !c.is_empty() => c,
            _ => {
                // 优先调用轻量免风控的 pagelist 接口获取 cid，避免 view 接口返回 412
                let pagelist_url =
                    format!("https://api.bilibili.com/x/player/pagelist?bvid={bvid}");
                let mut resolved_cid = String::new();
                if let Ok(vp) = http.get_json::<Value>(&pagelist_url).await {
                    resolved_cid = val_to_string(&vp["data"][0]["cid"]);
                }
                if resolved_cid.is_empty() {
                    let view_url =
                        format!("https://api.bilibili.com/x/web-interface/view?bvid={bvid}");
                    if let Ok(v) = http.get_json::<Value>(&view_url).await {
                        resolved_cid = val_to_string(&v["data"]["pages"][0]["cid"]);
                        if resolved_cid.is_empty() {
                            resolved_cid = val_to_string(&v["data"]["cid"]);
                        }
                    }
                }
                resolved_cid
            }
        };

        if cid.is_empty() {
            bail!("无法获取 B站视频 cid: {bvid}");
        }

        let play_url =
            format!("https://api.bilibili.com/x/player/playurl?fnval=16&bvid={bvid}&cid={cid}");
        let v2: Value = http.get_json(&play_url).await?;
        let data = &v2["data"];

        if let Some(audio_arr) = data["dash"]["audio"].as_array() {
            if let Some(first) = audio_arr.first() {
                let url = first["baseUrl"]
                    .as_str()
                    .or_else(|| first["base_url"].as_str())
                    .unwrap_or_default();
                if !url.is_empty() {
                    return Ok(Sound {
                        url: url.to_string(),
                        platform: "bilibili".to_string(),
                        ..Default::default()
                    });
                }
            }
        }

        // 兼容非 dash 的 durl 返回结构
        if let Some(durl_arr) = data["durl"].as_array() {
            if let Some(first) = durl_arr.first() {
                let url = first["url"].as_str().unwrap_or_default();
                if !url.is_empty() {
                    return Ok(Sound {
                        url: url.to_string(),
                        platform: "bilibili".to_string(),
                        ..Default::default()
                    });
                }
            }
        }

        bail!("未能从 B站 playurl 获取有效音频流: {bvid}");
    }

    let song_id = track_id.strip_prefix("bitrack_").unwrap_or(track_id);
    let target_url =
        format!("https://www.bilibili.com/audio/music-service-c/web/url?sid={song_id}");
    let v: Value = http.get_json(&target_url).await?;
    if v["code"].as_i64() == Some(0) {
        if let Some(url) = v["data"]["cdns"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|u| u.as_str())
            .filter(|s| !s.is_empty())
        {
            return Ok(Sound {
                url: url.to_string(),
                platform: "bilibili".to_string(),
                ..Default::default()
            });
        }
    }

    bail!("无法获取 B站音频地址: {song_id}")
}

pub async fn search(
    http: &Http,
    keyword: &str,
    page: u32,
    kind: SearchType,
) -> Result<SearchResult> {
    if kind == SearchType::Playlist {
        return Ok(SearchResult::default());
    }

    ensure_buvid(http).await;

    let encoded_kw = encode_uri_component(keyword);
    let target_url = format!(
        "https://api.bilibili.com/x/web-interface/search/type?__refresh__=true&_extra=&context=&page={page}&page_size=42&platform=pc&highlight=1&single_column=0&keyword={encoded_kw}&category_id=&search_type=video&dynamic_offset=0&preload=true&com2co=true"
    );

    let mut v = http.get_json::<Value>(&target_url).await.unwrap_or_default();
    if v["data"]["result"].as_array().is_none() {
        // 若直接请求被风控拦截，回退至 WBI 签名版搜索接口
        if let Ok(wbi_v) = wrap_wbi_request(
            http,
            "https://api.bilibili.com/x/web-interface/wbi/search/type",
            &[
                ("__refresh__", "true".to_string()),
                ("_extra", String::new()),
                ("context", String::new()),
                ("page", page.to_string()),
                ("page_size", "42".to_string()),
                ("platform", "pc".to_string()),
                ("highlight", "1".to_string()),
                ("single_column", "0".to_string()),
                ("keyword", keyword.to_string()),
                ("category_id", String::new()),
                ("search_type", "video".to_string()),
                ("dynamic_offset", "0".to_string()),
                ("preload", "true".to_string()),
                ("com2co", "true".to_string()),
            ],
        )
        .await
        {
            v = wbi_v;
        }
    }

    let data = &v["data"];
    let tracks = data["result"]
        .as_array()
        .map(|arr| arr.iter().map(bi_convert_song2).collect())
        .unwrap_or_default();
    let total = data["numResults"].as_u64().unwrap_or(0);

    Ok(SearchResult {
        tracks,
        playlists: Vec::new(),
        total,
    })
}

pub async fn lyric(http: &Http, track: &Track) -> Result<Lyric> {
    if let Some(lyric_url) = track.extra_str("lyric_url").filter(|s| s.starts_with("http")) {
        if let Ok(text) = http.get_text(lyric_url).await {
            return Ok(Lyric {
                lyric: text,
                ..Default::default()
            });
        }
    }
    Ok(Lyric::default())
}
