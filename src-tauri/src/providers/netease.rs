//! 网易云音乐 provider（移植自 `listen1_ext/js/provider/netease.js`）。
use crate::http::{query_param, urlencode, Http};
use crate::models::*;
use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit, KeyIvInit};
use anyhow::{anyhow, bail, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use md5::{Digest, Md5};
use num_bigint::BigUint;
use rand::Rng;
use regex_lite::Regex;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PREFIX: &str = "ne";

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128EcbEnc = ecb::Encryptor<aes::Aes128>;

const WEAPI_MODULUS: &str = "00e0b509f6259df8642dbc35662901477df22677ec152b5ff68ace615bb7b72\
5152b3ab17a876aea8a5aa76d2e417629ec4ee341f56135fccf695280104e0312ecbd\
a92557c93870114af6c9d05c4f7f0c3685b7a46bee255932575cce10b424d813cfe48\
75d3e82047b97ddef52741d546b8e289dc6935b3ece0462db0a22b8e7";
const WEAPI_NONCE: &[u8; 16] = b"0CoJUm6Qyw8W8jud";
const WEAPI_IV: &[u8; 16] = b"0102030405060708";
const WEAPI_PUBKEY: u32 = 0x010001;
const EAPI_KEY: &[u8; 16] = b"e82ckenh8dichen8";

fn create_secret_key(size: usize) -> String {
    const CHOICE: &[u8] = b"012345679abcdef";
    let mut rng = rand::thread_rng();
    (0..size)
        .map(|_| {
            let idx = rng.gen_range(0..CHOICE.len());
            CHOICE[idx] as char
        })
        .collect()
}

fn aes_cbc_encrypt(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>> {
    let enc = Aes128CbcEnc::new_from_slices(key, iv)
        .map_err(|e| anyhow!("AES-CBC key/iv 长度错误: {e}"))?;
    Ok(enc.encrypt_padded_vec_mut::<Pkcs7>(data))
}

fn aes_ecb_encrypt(data: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    let enc = Aes128EcbEnc::new_from_slice(key)
        .map_err(|e| anyhow!("AES-ECB key 长度错误: {e}"))?;
    Ok(enc.encrypt_padded_vec_mut::<Pkcs7>(data))
}

fn rsa_encrypt(text: &str) -> String {
    let rev: Vec<u8> = text.bytes().rev().collect();
    let b = BigUint::from_bytes_be(&rev);
    let e = BigUint::from(WEAPI_PUBKEY);
    let n = BigUint::parse_bytes(WEAPI_MODULUS.as_bytes(), 16).unwrap_or_default();
    let enc = b.modpow(&e, &n);
    format!("{:0>256}", enc.to_str_radix(16))
}

/// 网易云 weapi 加密：AES-128-CBC + PKCS7 两次加密 + Base64，配合无填充 RSA 加密随机密钥
fn weapi(object: &Value) -> Result<[(&'static str, String); 2]> {
    let text = serde_json::to_string(object)?;
    let sec_key = create_secret_key(16);
    let first = BASE64.encode(aes_cbc_encrypt(text.as_bytes(), WEAPI_NONCE, WEAPI_IV)?);
    let params = BASE64.encode(aes_cbc_encrypt(first.as_bytes(), sec_key.as_bytes(), WEAPI_IV)?);
    let enc_sec_key = rsa_encrypt(&sec_key);
    Ok([("params", params), ("encSecKey", enc_sec_key)])
}

/// 网易云 eapi 加密：MD5 摘要 + AES-128-ECB + PKCS7 转大写 Hex
fn eapi(url: &str, object: &Value) -> Result<[(&'static str, String); 1]> {
    let text = match object.as_str() {
        Some(s) => s.to_string(),
        None => serde_json::to_string(object)?,
    };
    let message = format!("nobody{url}use{text}md5forencrypt");
    let digest = hex::encode(Md5::digest(message.as_bytes()));
    let data = format!("{url}-36cd479b6b5-{text}-36cd479b6b5-{digest}");
    let encrypted = aes_ecb_encrypt(data.as_bytes(), EAPI_KEY)?;
    Ok([("params", hex::encode_upper(encrypted))])
}

fn ensure_cookie(http: &Http) {
    let domain = "https://music.163.com";
    let has_nuid = http.get_cookie(domain, "_ntes_nuid").is_some();
    let has_nnid = http.get_cookie(domain, "_ntes_nnid3").is_some();
    let has_nmtid = http.get_cookie(domain, "NMTID").is_some();
    if !has_nuid || !has_nnid || !has_nmtid {
        let nuid = create_secret_key(32);
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let nnid = format!("{nuid},{now_ms}");
        http.set_cookie(domain, &format!("_ntes_nuid={nuid}"));
        http.set_cookie(domain, &format!("_ntes_nnid3={nnid}"));
        http.set_cookie(domain, "NMTID=0");
    }
}

fn val_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn is_playable(song: &Value) -> bool {
    let fee = song["fee"].as_i64().unwrap_or(0);
    fee != 4 && fee != 1
}

/// 解析 `/weapi/v3/song/detail` 返回的单曲（字段为 `ar` / `al`）
fn parse_v3_track(track_json: &Value) -> Track {
    let id = val_str(&track_json["id"]);
    let ar = track_json["ar"]
        .as_array()
        .and_then(|a| a.first())
        .unwrap_or(&Value::Null);
    let al = &track_json["al"];
    let artist_id = val_str(&ar["id"]);
    let album_id = val_str(&al["id"]);
    Track {
        id: format!("netrack_{id}"),
        title: val_str(&track_json["name"]),
        artist: val_str(&ar["name"]),
        artist_id: if artist_id.is_empty() {
            String::new()
        } else {
            format!("neartist_{artist_id}")
        },
        album: val_str(&al["name"]),
        album_id: if album_id.is_empty() {
            String::new()
        } else {
            format!("nealbum_{album_id}")
        },
        source: "netease".to_string(),
        source_url: format!("https://music.163.com/#/song?id={id}"),
        img_url: val_str(&al["picUrl"]),
        disabled: false,
        extra: Default::default(),
    }
}

/// 解析 `/api/search/pc`、`/api/album`、`/api/artist` 返回的单曲（字段为 `artists` / `album`）
fn parse_api_track(song_info: &Value) -> Track {
    let id = val_str(&song_info["id"]);
    let ar = song_info["artists"]
        .as_array()
        .or_else(|| song_info["ar"].as_array())
        .and_then(|a| a.first())
        .unwrap_or(&Value::Null);
    let al = if song_info["album"].is_object() {
        &song_info["album"]
    } else {
        &song_info["al"]
    };
    let artist_id = val_str(&ar["id"]);
    let album_id = val_str(&al["id"]);
    Track {
        id: format!("netrack_{id}"),
        title: val_str(&song_info["name"]),
        artist: val_str(&ar["name"]),
        artist_id: if artist_id.is_empty() {
            String::new()
        } else {
            format!("neartist_{artist_id}")
        },
        album: val_str(&al["name"]),
        album_id: if album_id.is_empty() {
            String::new()
        } else {
            format!("nealbum_{album_id}")
        },
        source: "netease".to_string(),
        source_url: format!("https://music.163.com/#/song?id={id}"),
        img_url: val_str(&al["picUrl"]),
        disabled: !is_playable(song_info),
        extra: Default::default(),
    }
}

async fn show_toplist(http: &Http, offset: u32) -> Result<Vec<PlaylistInfo>> {
    if offset > 0 {
        return Ok(Vec::new());
    }
    let url = "https://music.163.com/weapi/toplist/detail";
    let form = weapi(&json!({}))?;
    let res: Value = http.post_form_json(url, &form).await?;
    let list = res["list"].as_array().cloned().unwrap_or_default();
    let result = list
        .iter()
        .filter_map(|item| {
            let id = val_str(&item["id"]);
            if id.is_empty() {
                return None;
            }
            Some(PlaylistInfo {
                cover_img_url: val_str(&item["coverImgUrl"]),
                id: format!("neplaylist_{id}"),
                source_url: format!("https://music.163.com/#/playlist?id={id}"),
                title: val_str(&item["name"]),
                ..Default::default()
            })
        })
        .collect();
    Ok(result)
}

fn parse_discover_playlist_html(html: &str) -> Vec<PlaylistInfo> {
    let section = match html.find("m-cvrlst") {
        Some(idx) => &html[idx..],
        None => html,
    };
    let Ok(li_re) = Regex::new(r"(?s)<li[^>]*>(.*?)</li>") else {
        return Vec::new();
    };
    let Ok(img_re) = Regex::new(r#"<img[^>]+src="([^"]+)""#) else {
        return Vec::new();
    };
    let Ok(link_re) = Regex::new(
        r#"<a[^>]+title="([^"]*)"[^>]+href="([^"]+)"|<a[^>]+href="([^"]+)"[^>]+title="([^"]*)""#,
    ) else {
        return Vec::new();
    };

    let mut result = Vec::new();
    for cap in li_re.captures_iter(section) {
        let block = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
        let img = img_re
            .captures(block)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().replace("140y140", "512y512"))
            .unwrap_or_default();
        let Some(lcap) = link_re.captures(block) else {
            continue;
        };
        let (title, href) = if let (Some(t), Some(h)) = (lcap.get(1), lcap.get(2)) {
            (t.as_str(), h.as_str())
        } else if let (Some(h), Some(t)) = (lcap.get(3), lcap.get(4)) {
            (t.as_str(), h.as_str())
        } else {
            continue;
        };
        let full_href = if href.starts_with("http") {
            href.to_string()
        } else {
            format!("https://music.163.com{}", href)
        };
        let Some(id) = query_param(&full_href, "id") else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        result.push(PlaylistInfo {
            cover_img_url: img,
            title: title.to_string(),
            id: format!("neplaylist_{id}"),
            source_url: format!("https://music.163.com/#/playlist?id={id}"),
            ..Default::default()
        });
    }
    result
}

pub async fn show_playlist(http: &Http, offset: u32, filter_id: &str) -> Result<Vec<PlaylistInfo>> {
    if filter_id == "toplist" {
        return show_toplist(http, offset).await;
    }

    // 优先使用 weapi/playlist/list 接口（比抓 HTML 稳定），若为空再 fallback 到原版 HTML 解析
    let cat = if filter_id.is_empty() {
        "全部".to_string()
    } else {
        filter_id.replace("%26", "&").replace("%2F", "/")
    };
    let api_url = "https://music.163.com/weapi/playlist/list";
    if let Ok(form) = weapi(&json!({
        "cat": cat,
        "order": "hot",
        "limit": 35,
        "offset": offset,
        "total": true,
    })) {
        if let Ok(res) = http.post_form_json::<Value>(api_url, &form).await {
            if let Some(arr) = res["playlists"].as_array() {
                let items: Vec<PlaylistInfo> = arr
                    .iter()
                    .filter_map(|item| {
                        let id = val_str(&item["id"]);
                        if id.is_empty() {
                            return None;
                        }
                        Some(PlaylistInfo {
                            cover_img_url: val_str(&item["coverImgUrl"])
                                .replace("140y140", "512y512"),
                            title: val_str(&item["name"]),
                            id: format!("neplaylist_{id}"),
                            source_url: format!("https://music.163.com/#/playlist?id={id}"),
                            ..Default::default()
                        })
                    })
                    .collect();
                if !items.is_empty() {
                    return Ok(items);
                }
            }
        }
    }

    let filter = if filter_id.is_empty() {
        String::new()
    } else if filter_id.contains('%') {
        format!("&cat={filter_id}")
    } else {
        format!("&cat={}", urlencode(filter_id))
    };
    let target_url = format!(
        "https://music.163.com/discover/playlist/?order=hot{filter}&limit=35&offset={offset}"
    );
    let html = http.get_text(&target_url).await?;
    Ok(parse_discover_playlist_html(&html))
}

async fn fetch_playlist_tracks_chunk(http: &Http, chunk_ids: &[String]) -> Result<Vec<Track>> {
    let target_url = "https://music.163.com/weapi/v3/song/detail";
    let c = format!(
        "[{}]",
        chunk_ids
            .iter()
            .map(|id| format!("{{\"id\":{id}}}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let ids = format!("[{}]", chunk_ids.join(","));
    let form = weapi(&json!({
        "c": c,
        "ids": ids,
    }))?;
    let res: Value = http.post_form_json(target_url, &form).await?;
    let songs = res["songs"].as_array().cloned().unwrap_or_default();
    Ok(songs.iter().map(parse_v3_track).collect())
}

async fn ne_get_playlist(http: &Http, list_id: &str) -> Result<Playlist> {
    ensure_cookie(http);
    let target_url = "https://music.163.com/weapi/v3/playlist/detail";
    let form = weapi(&json!({
        "id": list_id,
        "offset": 0,
        "total": true,
        "limit": 1000,
        "n": 1000,
        "csrf_token": "",
    }))?;
    let res_data: Value = http.post_form_json(target_url, &form).await?;
    let playlist = &res_data["playlist"];
    if playlist.is_null() {
        bail!("未找到网易云歌单: {list_id}");
    }
    let info = PlaylistInfo {
        id: format!("neplaylist_{list_id}"),
        cover_img_url: val_str(&playlist["coverImgUrl"]),
        title: val_str(&playlist["name"]),
        source_url: format!("https://music.163.com/#/playlist?id={list_id}"),
        ..Default::default()
    };

    let track_ids: Vec<String> = playlist["trackIds"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let id = val_str(&item["id"]);
                    (!id.is_empty()).then_some(id)
                })
                .collect()
        })
        .unwrap_or_default();

    let mut tracks = Vec::with_capacity(track_ids.len());
    for chunk in track_ids.chunks(1000) {
        if let Ok(mut batch) = fetch_playlist_tracks_chunk(http, chunk).await {
            tracks.append(&mut batch);
        }
    }

    if tracks.is_empty() {
        if let Some(arr) = playlist["tracks"].as_array() {
            tracks = arr.iter().map(parse_v3_track).collect();
        }
    }

    Ok(Playlist { info, tracks })
}

async fn ne_album(http: &Http, album_id: &str) -> Result<Playlist> {
    let target_url = format!("https://music.163.com/api/album/{album_id}");
    let data: Value = http.get_json(&target_url).await?;
    let album = &data["album"];
    if album.is_null() {
        bail!("未找到网易云专辑: {album_id}");
    }
    let id = {
        let s = val_str(&album["id"]);
        if s.is_empty() {
            album_id.to_string()
        } else {
            s
        }
    };
    let info = PlaylistInfo {
        cover_img_url: val_str(&album["picUrl"]),
        title: val_str(&album["name"]),
        id: format!("nealbum_{id}"),
        source_url: format!("https://music.163.com/#/album?id={id}"),
        ..Default::default()
    };
    let songs = album["songs"]
        .as_array()
        .or_else(|| data["songs"].as_array())
        .cloned()
        .unwrap_or_default();
    let tracks = songs.iter().map(parse_api_track).collect();
    Ok(Playlist { info, tracks })
}

async fn ne_artist(http: &Http, artist_id: &str) -> Result<Playlist> {
    let target_url = format!("https://music.163.com/api/artist/{artist_id}");
    let data: Value = http.get_json(&target_url).await?;
    let artist = &data["artist"];
    if artist.is_null() {
        bail!("未找到网易云歌手: {artist_id}");
    }
    let id = {
        let s = val_str(&artist["id"]);
        if s.is_empty() {
            artist_id.to_string()
        } else {
            s
        }
    };
    let info = PlaylistInfo {
        cover_img_url: val_str(&artist["picUrl"]),
        title: val_str(&artist["name"]),
        id: format!("neartist_{id}"),
        source_url: format!("https://music.163.com/#/artist?id={id}"),
        ..Default::default()
    };
    let hot_songs = data["hotSongs"].as_array().cloned().unwrap_or_default();
    let tracks = hot_songs.iter().map(parse_api_track).collect();
    Ok(Playlist { info, tracks })
}

pub async fn get_playlist(http: &Http, list_id: &str) -> Result<Playlist> {
    let (prefix, raw_id) = list_id
        .split_once('_')
        .ok_or_else(|| anyhow!("无效的网易云歌单 id: {list_id}"))?;
    match prefix {
        "neplaylist" => ne_get_playlist(http, raw_id).await,
        "nealbum" => ne_album(http, raw_id).await,
        "neartist" => ne_artist(http, raw_id).await,
        _ => bail!("未知的网易云歌单类型: {list_id}"),
    }
}

pub async fn bootstrap_track(http: &Http, track: &Track) -> Result<Sound> {
    let target_url = "https://interface3.music.163.com/eapi/song/enhance/player/url";
    let eapi_url = "/api/song/enhance/player/url";
    let song_id = track.id.strip_prefix("netrack_").unwrap_or(&track.id);

    let d = json!({
        "ids": format!("[{song_id}]"),
        "br": 999000,
    });
    let form = eapi(eapi_url, &d)?;
    http.set_cookie("https://interface3.music.163.com", "os=pc");

    let res_data: Value = http.post_form_json(target_url, &form).await?;
    let item = res_data["data"]
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow!("网易云未返回播放地址"))?;
    let url = item["url"].as_str().unwrap_or_default();
    if url.is_empty() {
        bail!("网易云歌曲无可用播放地址: {}", track.id);
    }
    let br = item["br"].as_f64().unwrap_or(0.0);
    let bitrate = format!("{}kbps", (br / 1000.0).round() as i64);

    Ok(Sound {
        url: url.to_string(),
        bitrate,
        platform: "netease".to_string(),
    })
}

pub async fn search(
    http: &Http,
    keyword: &str,
    page: u32,
    kind: SearchType,
) -> Result<SearchResult> {
    ensure_cookie(http);
    let target_url = "https://music.163.com/api/search/pc";
    let ne_search_type = match kind {
        SearchType::Track => "1",
        SearchType::Playlist => "1000",
    };
    let offset = 20 * page.saturating_sub(1);
    let form = [
        ("s", keyword.to_string()),
        ("offset", offset.to_string()),
        ("limit", "20".to_string()),
        ("type", ne_search_type.to_string()),
    ];

    let mut data: Value = http
        .post_form_json(target_url, &form)
        .await
        .unwrap_or(Value::Null);

    // 若未加密接口未返回有效结果，fallback 到 weapi/search/get
    if data["result"].is_null() {
        if let Ok(weapi_form) = weapi(&json!({
            "s": keyword,
            "offset": offset,
            "limit": 20,
            "type": ne_search_type,
        })) {
            if let Ok(v) = http
                .post_form_json::<Value>("https://music.163.com/weapi/search/get", &weapi_form)
                .await
            {
                data = v;
            }
        }
    }

    let result_obj = &data["result"];
    match kind {
        SearchType::Track => {
            let songs = result_obj["songs"].as_array().cloned().unwrap_or_default();
            let tracks = songs.iter().map(parse_api_track).collect();
            let total = result_obj["songCount"].as_u64().unwrap_or(0);
            Ok(SearchResult {
                tracks,
                playlists: Vec::new(),
                total,
            })
        }
        SearchType::Playlist => {
            let raw_list = result_obj["playlists"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let playlists = raw_list
                .iter()
                .filter_map(|info| {
                    let id = val_str(&info["id"]);
                    if id.is_empty() {
                        return None;
                    }
                    Some(PlaylistInfo {
                        id: format!("neplaylist_{id}"),
                        title: val_str(&info["name"]),
                        source_url: format!("https://music.163.com/#/playlist?id={id}"),
                        cover_img_url: val_str(&info["coverImgUrl"]),
                        author: val_str(&info["creator"]["nickname"]),
                        count: info["trackCount"].as_u64(),
                    })
                })
                .collect();
            let total = result_obj["playlistCount"].as_u64().unwrap_or(0);
            Ok(SearchResult {
                tracks: Vec::new(),
                playlists,
                total,
            })
        }
    }
}

pub async fn lyric(http: &Http, track: &Track) -> Result<Lyric> {
    let track_id = track
        .id
        .rsplit_once('_')
        .map(|(_, id)| id)
        .unwrap_or(&track.id);
    let target_url = "https://music.163.com/weapi/song/lyric?csrf_token=";
    let form = weapi(&json!({
        "id": track_id,
        "lv": -1,
        "tv": -1,
        "csrf_token": "",
    }))?;
    let res_data: Value = http.post_form_json(target_url, &form).await?;
    let lrc = res_data["lrc"]["lyric"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let mut tlrc = res_data["tlyric"]["lyric"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if !tlrc.is_empty() {
        tlrc = tlrc.replace([' ', '\\'], "");
        if let Ok(re) = Regex::new(r"\u{2005}+") {
            tlrc = re.replace_all(&tlrc, " ").into_owned();
        }
    }
    Ok(Lyric {
        lyric: lrc,
        tlyric: tlrc,
    })
}

pub async fn parse_url(_http: &Http, url: &str) -> Option<String> {
    let mut u = url.replace(
        "music.163.com/#/discover/toplist?",
        "music.163.com/#/playlist?",
    );
    u = u.replace("music.163.com/#/my/m/music/", "music.163.com/");
    u = u.replace("music.163.com/#/m/", "music.163.com/");
    u = u.replace("music.163.com/#/", "music.163.com/");

    if u.contains("//music.163.com/playlist") {
        let re = Regex::new(r"//music\.163\.com/playlist/([0-9]+)").ok()?;
        let id = re
            .captures(&u)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .or_else(|| query_param(&u, "id"))?;
        (!id.is_empty()).then(|| format!("neplaylist_{id}"))
    } else if u.contains("//music.163.com/artist") {
        let re = Regex::new(r"//music\.163\.com/artist/([0-9]+)").ok()?;
        let id = re
            .captures(&u)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .or_else(|| query_param(&u, "id"))?;
        (!id.is_empty()).then(|| format!("neartist_{id}"))
    } else if u.contains("//music.163.com/album") {
        let re = Regex::new(r"//music\.163\.com/album/([0-9]+)").ok()?;
        let id = re
            .captures(&u)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .or_else(|| query_param(&u, "id"))?;
        (!id.is_empty()).then(|| format!("nealbum_{id}"))
    } else {
        None
    }
}

pub fn playlist_filters() -> Filters {
    Filters {
        recommend: filters(&[
            ("", "全部"),
            ("toplist", "排行榜"),
            ("流行", "流行"),
            ("民谣", "民谣"),
            ("电子", "电子"),
            ("舞曲", "舞曲"),
            ("说唱", "说唱"),
            ("轻音乐", "轻音乐"),
            ("爵士", "爵士"),
            ("乡村", "乡村"),
        ]),
        all: vec![
            FilterCategory {
                category: "语种".to_string(),
                filters: filters(&[
                    ("华语", "华语"),
                    ("欧美", "欧美"),
                    ("日语", "日语"),
                    ("韩语", "韩语"),
                    ("粤语", "粤语"),
                ]),
            },
            FilterCategory {
                category: "风格".to_string(),
                filters: filters(&[
                    ("流行", "流行"),
                    ("民谣", "民谣"),
                    ("电子", "电子"),
                    ("舞曲", "舞曲"),
                    ("说唱", "说唱"),
                    ("轻音乐", "轻音乐"),
                    ("爵士", "爵士"),
                    ("乡村", "乡村"),
                    ("R%26B%2FSoul", "R&B/Soul"),
                    ("古典", "古典"),
                    ("民族", "民族"),
                    ("英伦", "英伦"),
                    ("金属", "金属"),
                    ("朋克", "朋克"),
                    ("蓝调", "蓝调"),
                    ("雷鬼", "雷鬼"),
                    ("世界音乐", "世界音乐"),
                    ("拉丁", "拉丁"),
                    ("New Age", "New Age"),
                    ("古风", "古风"),
                    ("后摇", "后摇"),
                    ("Bossa Nova", "Bossa Nova"),
                ]),
            },
            FilterCategory {
                category: "场景".to_string(),
                filters: filters(&[
                    ("清晨", "清晨"),
                    ("夜晚", "夜晚"),
                    ("学习", "学习"),
                    ("工作", "工作"),
                    ("午休", "午休"),
                    ("下午茶", "下午茶"),
                    ("地铁", "地铁"),
                    ("驾车", "驾车"),
                    ("运动", "运动"),
                    ("旅行", "旅行"),
                    ("散步", "散步"),
                    ("酒吧", "酒吧"),
                ]),
            },
            FilterCategory {
                category: "情感".to_string(),
                filters: filters(&[
                    ("怀旧", "怀旧"),
                    ("清新", "清新"),
                    ("浪漫", "浪漫"),
                    ("伤感", "伤感"),
                    ("治愈", "治愈"),
                    ("放松", "放松"),
                    ("孤独", "孤独"),
                    ("感动", "感动"),
                    ("兴奋", "兴奋"),
                    ("快乐", "快乐"),
                    ("安静", "安静"),
                    ("思念", "思念"),
                ]),
            },
            FilterCategory {
                category: "主题".to_string(),
                filters: filters(&[
                    ("综艺", "综艺"),
                    ("影视原声", "影视原声"),
                    ("ACG", "ACG"),
                    ("儿童", "儿童"),
                    ("校园", "校园"),
                    ("游戏", "游戏"),
                    ("70后", "70后"),
                    ("80后", "80后"),
                    ("90后", "90后"),
                    ("网络歌曲", "网络歌曲"),
                    ("KTV", "KTV"),
                    ("经典", "经典"),
                    ("翻唱", "翻唱"),
                    ("吉他", "吉他"),
                    ("钢琴", "钢琴"),
                    ("器乐", "器乐"),
                    ("榜单", "榜单"),
                    ("00后", "00后"),
                ]),
            },
        ],
    }
}
