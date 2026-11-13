//! 酷我音乐 provider，忠实移植自 `_ref/listen1_ext/js/provider/kuwo.js`。
use crate::http::{parse_json, query_param, urlencode, Http};
use crate::models::*;
use anyhow::{anyhow, bail, Result};
use regex_lite::Regex;
use serde_json::Value;

pub const PREFIX: &str = "kw";

const KUWO_DOMAIN: &str = "https://www.kuwo.cn";
const COOKIE_NAME: &str = "Hm_Iuvt_cdb524f42f23cer9b268564v7y735ewrq2324";

/// 模拟 JS `parseInt(str)`：忽略前导空白，截取连续数字（含首字符符号）并转为 f64
fn js_parse_int(s: &str) -> f64 {
    let s = s.trim_start();
    let end = s
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '+' || c == '-'))))
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    s[..end].parse::<f64>().unwrap_or(0.0)
}

/// 模拟 JS `Number.prototype.toString()`（针对大整数变成科学计数法 `1.23e+89` 的行为）
fn js_num_to_str(val: f64) -> String {
    if !val.is_finite() {
        return "0".to_string();
    }
    if val.abs() >= 1e21 {
        let s = format!("{:e}", val);
        if let Some((mantissa, exp)) = s.split_once('e') {
            if !exp.starts_with('-') && !exp.starts_with('+') {
                return format!("{mantissa}e+{exp}");
            }
        }
        s
    } else {
        format!("{}", val.trunc() as i128)
    }
}

/// 移植自 `kuwo.js` 顶部的 `function h(t, e)`，用于根据 cookie token 与 cookie 名生成请求头 `Secret`
fn kuwo_secret(t: &str, e: &str) -> String {
    if e.is_empty() {
        return String::new();
    }
    let mut n = String::new();
    for code in e.encode_utf16() {
        n.push_str(&code.to_string());
    }
    let r = n.len() / 5;
    let n_bytes = n.as_bytes();
    let mut o_str = String::with_capacity(5);
    for k in 1..=5 {
        if let Some(&b) = n_bytes.get(k * r) {
            o_str.push(b as char);
        }
    }
    let o: i64 = o_str.parse().unwrap_or(0);
    if o < 2 {
        return String::new();
    }
    let l: i64 = ((e.encode_utf16().count() + 1) / 2) as i64;
    let c: i64 = (1i64 << 31) - 1;

    let d: u32 = {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        rng.gen_range(0..100_000_000)
    };
    n.push_str(&d.to_string());
    while n.len() > 10 {
        let a = js_parse_int(&n[..10]);
        let b = js_parse_int(&n[10..]);
        n = js_num_to_str(a + b);
    }
    let n_val: i64 = n.parse().unwrap_or(0);
    let mut state: i64 = (o.wrapping_mul(n_val).wrapping_add(l)).rem_euclid(c);
    let mut f = String::new();
    for code in t.encode_utf16() {
        let mask = ((state as f64 / c as f64) * 255.0).floor() as u32;
        let h_val = (code as u32) ^ mask;
        f.push_str(&format!("{:02x}", h_val));
        state = (o.wrapping_mul(state).wrapping_add(l)).rem_euclid(c);
    }
    f.push_str(&format!("{:08x}", d));
    f
}

fn html_decode(s: &str) -> String {
    let entities = [
        ("&amp;", "&"),
        ("&apos;", "'"),
        ("&#x27;", "'"),
        ("&#x2F;", "/"),
        ("&#39;", "'"),
        ("&#47;", "/"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&nbsp;", " "),
        ("&quot;", "\""),
    ];
    let mut text = s.to_string();
    for (ent, rep) in entities {
        if text.contains(ent) {
            text = text.replace(ent, rep);
        }
    }
    text
}

fn fix_json(data: &str) -> String {
    data.replace('\'', "\"")
}

fn parse_kuwo_json(text: &str) -> Result<Value> {
    parse_json::<Value>(text).or_else(|_| parse_json::<Value>(&fix_json(text)))
}

fn val_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn val_u64(v: &Value) -> u64 {
    match v {
        Value::Number(n) => n.as_u64().unwrap_or(0),
        Value::String(s) => s.trim().parse::<u64>().unwrap_or(0),
        _ => 0,
    }
}

fn val_u64_opt(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn num2str(num: i64) -> String {
    format!("{}{}", num / 10, num.rem_euclid(10))
}

fn kw_convert_song2(item: &Value) -> Track {
    let rid = val_str(&item["rid"]);
    let mut track = Track {
        id: format!("kwtrack_{rid}"),
        title: html_decode(&val_str(&item["name"])),
        artist: html_decode(&val_str(&item["artist"])),
        artist_id: format!("kwartist_{}", val_str(&item["artistid"])),
        album: html_decode(&val_str(&item["album"])),
        album_id: format!("kwalbum_{}", val_str(&item["albumid"])),
        source: "kuwo".to_string(),
        source_url: format!("https://www.kuwo.cn/play_detail/{rid}"),
        img_url: val_str(&item["pic"]),
        disabled: false,
        ..Default::default()
    };
    track.set_extra("lyric_url", rid);
    track
}

fn kw_convert_song3(item: &Value) -> Track {
    let mut song_id = val_str(&item["DC_TARGETID"]);
    if song_id.is_empty() {
        let musicrid = val_str(&item["MUSICRID"]);
        song_id = musicrid.rsplit('_').next().unwrap_or_default().to_string();
    }
    let name = if item.get("NAME").is_some() {
        val_str(&item["NAME"])
    } else {
        val_str(&item["SONGNAME"])
    };
    let pic_short = val_str(&item["web_albumpic_short"]);
    let img_url = if pic_short.is_empty() {
        String::new()
    } else if pic_short.starts_with("http") {
        pic_short
    } else {
        format!("https://img2.kuwo.cn/star/albumcover/{pic_short}")
    };
    let mut track = Track {
        id: format!("kwtrack_{song_id}"),
        title: html_decode(&name),
        artist: html_decode(&val_str(&item["ARTIST"])),
        artist_id: format!("kwartist_{}", val_str(&item["ARTISTID"])),
        album: html_decode(&val_str(&item["ALBUM"])),
        album_id: format!("kwalbum_{}", val_str(&item["ALBUMID"])),
        source: "kuwo".to_string(),
        source_url: format!("https://www.kuwo.cn/play_detail/{song_id}"),
        img_url,
        disabled: false,
        ..Default::default()
    };
    track.set_extra("lyric_url", song_id);
    track
}

async fn kw_get_token(http: &Http, force_refresh: bool) -> String {
    if !force_refresh {
        if let Some(cookie) = http.get_cookie(KUWO_DOMAIN, COOKIE_NAME) {
            return cookie;
        }
    }
    let _ = http.get("https://www.kuwo.cn/").send().await;
    http.get_cookie(KUWO_DOMAIN, COOKIE_NAME).unwrap_or_default()
}

async fn kw_cookie_get(http: &Http, url: &str) -> Result<Value> {
    let token = kw_get_token(http, false).await;
    let secret = kuwo_secret(&token, COOKIE_NAME);
    let resp = http
        .get(url)
        .header("Secret", secret)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let data = parse_kuwo_json(&resp)?;
    if data.get("success").and_then(|v| v.as_bool()) == Some(false) {
        let token2 = kw_get_token(http, true).await;
        let secret2 = kuwo_secret(&token2, COOKIE_NAME);
        let resp2 = http
            .get(url)
            .header("Secret", secret2)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        return parse_kuwo_json(&resp2);
    }
    Ok(data)
}

async fn kw_render_tracks(http: &Http, playlist_type: &str, list_id: &str, page: u32) -> Result<Vec<Track>> {
    let tracks_url = match playlist_type {
        "kwplaylist" => format!(
            "https://www.kuwo.cn/api/www/playlist/playListInfo?pid={list_id}&pn={page}&rn=100&httpsStatus=1"
        ),
        "kwalbum" => format!(
            "https://www.kuwo.cn/api/www/album/albumInfo?albumId={list_id}&pn={page}&rn=100&httpsStatus=1"
        ),
        other => bail!("未知的酷我歌单类型: {other}"),
    };
    let data = kw_cookie_get(http, &tracks_url).await?;
    let tracks = data["data"]["musicList"]
        .as_array()
        .map(|arr| arr.iter().map(kw_convert_song2).collect())
        .unwrap_or_default();
    Ok(tracks)
}

pub async fn search(http: &Http, keyword: &str, page: u32, kind: SearchType) -> Result<SearchResult> {
    let curpage = page.max(1);
    let pn = curpage - 1;
    let encoded_kw = urlencode(keyword);

    match kind {
        SearchType::Track => {
            let target_url = format!(
                "https://www.kuwo.cn/search/searchMusicBykeyWord?vipver=1&client=kt&ft=music&cluster=0&strategy=2012&encoding=utf8&rformat=json&mobi=1&issubtitle=1&show_copyright_off=1&pn={pn}&rn=20&all={encoded_kw}"
            );
            let Ok(data) = kw_cookie_get(http, &target_url).await else {
                return Ok(SearchResult::default());
            };
            if let Some(arr) = data.get("abslist").and_then(|v| v.as_array()) {
                let tracks = arr.iter().map(kw_convert_song3).collect();
                let total = val_u64(&data["HIT"]);
                return Ok(SearchResult {
                    tracks,
                    playlists: Vec::new(),
                    total,
                });
            }
            Ok(SearchResult::default())
        }
        SearchType::Playlist => {
            let target_url = format!(
                "https://www.kuwo.cn/api/www/search/searchPlayListBykeyWord?key={encoded_kw}&pn={curpage}&rn=30"
            );
            let Ok(data) = kw_cookie_get(http, &target_url).await else {
                return Ok(SearchResult::default());
            };
            if let Some(d) = data.get("data") {
                let playlists = d["list"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .map(|item| {
                                let id = val_str(&item["id"]);
                                PlaylistInfo {
                                    id: format!("kwplaylist_{id}"),
                                    title: html_decode(&val_str(&item["name"])),
                                    cover_img_url: val_str(&item["img"]),
                                    source_url: format!("https://www.kuwo.cn/playlist_detail/{id}"),
                                    author: html_decode(&val_str(&item["uname"])),
                                    count: val_u64_opt(&item["total"]),
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let total = val_u64(&d["total"]);
                return Ok(SearchResult {
                    tracks: Vec::new(),
                    playlists,
                    total,
                });
            }
            Ok(SearchResult::default())
        }
    }
}

pub async fn show_playlist(http: &Http, offset: u32, filter_id: &str) -> Result<Vec<PlaylistInfo>> {
    let pn = offset / 25 + 1;
    let target_url = if filter_id.is_empty() || filter_id == "hot" || filter_id == "new" {
        let order = if filter_id == "new" { "new" } else { "hot" };
        format!(
            "https://www.kuwo.cn/api/pc/classify/playlist/getRcmPlayList?pn={pn}&rn=25&order={order}&httpsStatus=1"
        )
    } else {
        format!(
            "https://www.kuwo.cn/api/pc/classify/playlist/getTagPlayList?pn={pn}&rn=25&id={}&httpsStatus=1",
            urlencode(filter_id)
        )
    };

    let resp = match kw_cookie_get(http, &target_url).await {
        Ok(v) => v,
        Err(_) => http.get_json::<Value>(&target_url).await?,
    };
    let Some(data) = resp.get("data") else {
        return Ok(Vec::new());
    };
    let list = data["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|item| {
                    let id = val_str(&item["id"]);
                    PlaylistInfo {
                        cover_img_url: val_str(&item["img"]),
                        title: html_decode(&val_str(&item["name"])),
                        id: format!("kwplaylist_{id}"),
                        source_url: format!("https://www.kuwo.cn/playlist_detail/{id}"),
                        ..Default::default()
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(list)
}

pub fn playlist_filters() -> Filters {
    Filters::default()
}

async fn kw_get_playlist(http: &Http, raw_id: &str) -> Result<Playlist> {
    let target_url = format!(
        "https://nplserver.kuwo.cn/pl.svc?op=getlistinfo&pn=0&rn=0&encode=utf-8&keyset=pl2012&pcmp4=1&pid={raw_id}&vipver=MUSIC_9.0.2.0_W1&newver=1"
    );
    let text = http.get_text(&target_url).await?;
    let data = parse_kuwo_json(&text)?;

    let id_val = {
        let s = val_str(&data["id"]);
        if s.is_empty() {
            raw_id.to_string()
        } else {
            s
        }
    };
    let info = PlaylistInfo {
        cover_img_url: val_str(&data["pic"]).replace("_150.jpg", "_400.jpg"),
        title: html_decode(&val_str(&data["title"])),
        id: format!("kwplaylist_{id_val}"),
        source_url: format!("https://www.kuwo.cn/playlist_detail/{id_val}"),
        ..Default::default()
    };

    let total = val_u64(&data["total"]);
    let pages = ((total as u32) + 99) / 100;
    let mut tracks = Vec::new();
    for page in 1..=pages {
        if let Ok(mut page_tracks) = kw_render_tracks(http, "kwplaylist", raw_id, page).await {
            tracks.append(&mut page_tracks);
        }
    }
    Ok(Playlist { info, tracks })
}

async fn kw_album(http: &Http, album_id: &str) -> Result<Playlist> {
    let target_url = format!(
        "https://search.kuwo.cn/r.s?pn=0&rn=0&stype=albuminfo&albumid={album_id}&alflac=1&pcmp4=1&encoding=utf8&vipver=MUSIC_8.7.7.0_W4"
    );
    let text = http.get_text(&target_url).await?;
    let data = parse_kuwo_json(&text)?;

    let id_val = {
        let s = val_str(&data["albumid"]);
        if s.is_empty() {
            album_id.to_string()
        } else {
            s
        }
    };
    let info = PlaylistInfo {
        cover_img_url: val_str(&data["hts_img"]).replace("/120/", "/400/"),
        title: html_decode(&val_str(&data["name"])),
        id: format!("kwalbum_{id_val}"),
        source_url: format!("https://www.kuwo.cn/album_detail/{id_val}"),
        ..Default::default()
    };

    let total = val_u64(&data["songnum"]);
    let pages = ((total as u32) + 99) / 100;
    let mut tracks = Vec::new();
    for page in 1..=pages {
        if let Ok(mut page_tracks) = kw_render_tracks(http, "kwalbum", album_id, page).await {
            tracks.append(&mut page_tracks);
        }
    }
    Ok(Playlist { info, tracks })
}

async fn kw_artist(http: &Http, artist_id: &str) -> Result<Playlist> {
    let info_url = format!("https://www.kuwo.cn/api/www/artist/artist?artistid={artist_id}");
    let info_resp = kw_cookie_get(http, &info_url).await?;
    let data = &info_resp["data"];

    let id_val = {
        let s = val_str(&data["id"]);
        if s.is_empty() {
            artist_id.to_string()
        } else {
            s
        }
    };
    let info = PlaylistInfo {
        cover_img_url: val_str(&data["pic300"]),
        title: html_decode(&val_str(&data["name"])),
        id: format!("kwartist_{id_val}"),
        source_url: format!("https://www.kuwo.cn/singer_detail/{id_val}"),
        ..Default::default()
    };

    let songs_url = format!(
        "https://www.kuwo.cn/api/www/artist/artistMusic?artistid={artist_id}&pn=1&rn=50"
    );
    let songs_resp = kw_cookie_get(http, &songs_url).await?;
    let tracks = songs_resp["data"]["list"]
        .as_array()
        .map(|arr| arr.iter().map(kw_convert_song2).collect())
        .unwrap_or_default();

    Ok(Playlist { info, tracks })
}

pub async fn get_playlist(http: &Http, list_id: &str) -> Result<Playlist> {
    let (playlist_type, raw_id) = list_id
        .split_once('_')
        .ok_or_else(|| anyhow!("无效的酷我歌单 id: {list_id}"))?;
    let raw_id = raw_id.rsplit('_').next().unwrap_or(raw_id);
    match playlist_type {
        "kwplaylist" => kw_get_playlist(http, raw_id).await,
        "kwalbum" => kw_album(http, raw_id).await,
        "kwartist" => kw_artist(http, raw_id).await,
        other => bail!("未知的酷我歌单类型: {other}"),
    }
}

pub async fn bootstrap_track(http: &Http, track: &Track) -> Result<Sound> {
    let song_id = track.id.strip_prefix("kwtrack_").unwrap_or(&track.id);

    // 1. 原版 kuwo.js 使用的 web playUrl 接口
    let target_url = format!(
        "https://www.kuwo.cn/api/v1/www/music/playUrl?mid={song_id}&type=music&httpsStatus=1&reqId=&plat=web_www&from="
    );
    if let Ok(resp) = kw_cookie_get(http, &target_url).await {
        let url = val_str(&resp["data"]["url"]);
        if url.starts_with("http") {
            return Ok(Sound {
                url,
                bitrate: String::new(),
                platform: "kuwo".to_string(),
            });
        }
    }

    // 2. 备选：酷我 antiserver 接口
    let anti_url = format!(
        "https://antiserver.kuwo.cn/anti.s?type=convert_url&rid=MUSIC_{song_id}&format=mp3|aac&response=url"
    );
    if let Ok(text) = http.get_text(&anti_url).await {
        let trimmed = text.trim();
        if trimmed.starts_with("http") {
            return Ok(Sound {
                url: trimmed.to_string(),
                bitrate: String::new(),
                platform: "kuwo".to_string(),
            });
        }
    }

    // 3. 备选：酷我 mobi.kuwo.cn 接口
    let mobi_url = format!(
        "http://mobi.kuwo.cn/mobi.s?f=web&user=0&source=kwplayer_ar_5.1.0.0_B_jiakong_vh.apk&type=convert_url_with_sign&rid={song_id}&br=320kmp3"
    );
    if let Ok(resp) = http.get_json::<Value>(&mobi_url).await {
        let url = val_str(&resp["data"]["url"]);
        if url.starts_with("http") {
            let br = val_str(&resp["data"]["bitrate"]);
            return Ok(Sound {
                url,
                bitrate: if br.is_empty() { String::new() } else { format!("{br}kbps") },
                platform: "kuwo".to_string(),
            });
        }
    }

    bail!("无法获取酷我音乐播放地址: {}", track.id)
}

fn kw_get_lrc(arr: &[Value]) -> String {
    let mut lyric = String::new();
    for item in arr {
        let t = val_str(&item["time"]).parse::<f64>().unwrap_or(0.0);
        let m = (t / 60.0) as i64;
        let s = (t - (m as f64) * 60.0) as i64;
        let ms = ((t - (m as f64) * 60.0 - (s as f64)) * 100.0) as i64;
        let line_lyric = val_str(&item["lineLyric"]);
        lyric.push_str(&format!(
            "[{}:{}.{}]{}\n",
            num2str(m),
            num2str(s),
            num2str(ms),
            line_lyric
        ));
    }
    lyric
}

fn kw_generate_translation(lrclist: Option<&Vec<Value>>) -> Lyric {
    if let Some(arr) = lrclist {
        return Lyric {
            lyric: kw_get_lrc(arr),
            tlyric: String::new(),
        };
    }
    Lyric::default()
}

pub async fn lyric(http: &Http, track: &Track) -> Result<Lyric> {
    let track_id = track
        .extra_str("lyric_url")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| track.id.strip_prefix("kwtrack_").unwrap_or(&track.id));
    let target_url = format!(
        "https://m.kuwo.cn/newh5/singles/songinfoandlrc?musicId={track_id}"
    );
    let Ok(data) = http.get_json::<Value>(&target_url).await else {
        return Ok(Lyric::default());
    };
    if data.get("status").and_then(|v| v.as_i64()) == Some(200) {
        return Ok(kw_generate_translation(data["data"]["lrclist"].as_array()));
    }
    Ok(Lyric::default())
}

pub async fn parse_url(_http: &Http, myurl: &str) -> Option<String> {
    let mut url = myurl.to_string();
    if let Ok(re) = Regex::new(r"kuwo\.cn/(h5app|newh5(?:app)?)/") {
        url = re.replace(&url, "kuwo.cn/").into_owned();
    }
    if let Ok(re) = Regex::new(r"kuwo\.cn/(album/|\?albumid=)") {
        url = re.replace(&url, "kuwo.cn/album_detail/").into_owned();
    }
    if let Ok(re) = Regex::new(r"kuwo\.cn/(artist|singers)/") {
        url = re.replace(&url, "kuwo.cn/singer_detail/").into_owned();
    }
    if let Ok(re) = Regex::new(r"kuwo\.cn/playlist/") {
        url = re.replace(&url, "kuwo.cn/playlist_detail/").into_owned();
    }

    if url.contains("kuwo.cn/playlist_detail") {
        let re = Regex::new(r"kuwo\.cn/playlist_detail/([0-9]+)").ok()?;
        let id = re
            .captures(&url)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .or_else(|| query_param(&url, "pid"))?;
        if !id.is_empty() {
            return Some(format!("kwplaylist_{id}"));
        }
    } else if url.contains("kuwo.cn/singer_detail") {
        let re = Regex::new(r"kuwo\.cn/singer_detail/([0-9]+)").ok()?;
        let id = re
            .captures(&url)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .or_else(|| query_param(&url, "id"))?;
        if !id.is_empty() {
            return Some(format!("kwartist_{id}"));
        }
    } else if url.contains("kuwo.cn/album_detail") {
        let re = Regex::new(r"kuwo\.cn/album_detail/([0-9]+)").ok()?;
        if let Some(id) = re.captures(&url).and_then(|c| c.get(1).map(|m| m.as_str())) {
            if !id.is_empty() {
                return Some(format!("kwalbum_{id}"));
            }
        }
    }
    None
}
