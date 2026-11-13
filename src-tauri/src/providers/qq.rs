//! QQ 音乐（QQ Music）音源实现。
use crate::http::{ parse_json, query_param, urlencode, Http };
use crate::models::*;
use anyhow::{ anyhow, bail, Result };
use base64::{ engine::general_purpose::STANDARD as BASE64, Engine as _ };
use regex_lite::Regex;
use serde_json::{ json, Value };

pub const PREFIX: &str = "qq";

/// 反转义 HTML 实体（支持常见命名实体、`&#123;` 十进制实体与 `&#x1A;` 十六进制实体）
fn html_decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after_amp = &rest[amp + 1..];
        let max_scan = after_amp.len().min(16);
        if let Some(semi) = after_amp[..max_scan].find(';') {
            let entity = &after_amp[..semi];
            let decoded = if let Some(hex_str) = entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
            {
                u32::from_str_radix(hex_str, 16).ok().and_then(char::from_u32)
            } else if let Some(dec_str) = entity.strip_prefix('#') {
                dec_str.parse::<u32>().ok().and_then(char::from_u32)
            } else {
                match entity {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" | "#39" => Some('\''),
                    "nbsp" => Some(' '),
                    _ => None,
                }
            };
            if let Some(ch) = decoded {
                out.push(ch);
                rest = &after_amp[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after_amp;
    }
    out.push_str(rest);
    out
}

fn val_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn qq_get_image_url(qqimgid: &str, img_type: &str) -> String {
    if qqimgid.is_empty() {
        return String::new();
    }
    let category = match img_type {
        "artist" => "T001R300x300M000",
        "album" => "T002R300x300M000",
        _ => "",
    };
    format!("https://y.gtimg.cn/music/photo_new/{category}{qqimgid}.jpg")
}

/// 对应原版 `qq_is_playable(song)`：
/// switch 转为二进制字符串后去掉最低位（bit 0），再取新的最低位（即 bit 1: `play_lq`）。
fn qq_is_playable(song: &Value) -> bool {
    let switch_val = song
        .get("switch")
        .or_else(|| song.pointer("/action/switch"))
        .and_then(|v| v.as_u64());
    match switch_val {
        Some(sw) => ((sw >> 1) & 1) == 1,
        None => true,
    }
}

/// 对应原版 `qq_convert_song(song)`（旧版歌单、专辑详情接口格式）
fn qq_convert_song(song: &Value) -> Track {
    let songmid = song["songmid"].as_str().unwrap_or_default();
    let songname = html_decode(song["songname"].as_str().unwrap_or_default());
    let (artist, artist_mid) = song["singer"]
        .as_array()
        .and_then(|arr| arr.first())
        .map(|s| {
            (
                html_decode(s["name"].as_str().unwrap_or_default()),
                s["mid"].as_str().unwrap_or_default(),
            )
        })
        .unwrap_or_default();
    let album = html_decode(song["albumname"].as_str().unwrap_or_default());
    let albummid = song["albummid"].as_str().unwrap_or_default();

    Track {
        id: format!("qqtrack_{songmid}"),
        title: songname,
        artist,
        artist_id: format!("qqartist_{artist_mid}"),
        album,
        album_id: format!("qqalbum_{albummid}"),
        img_url: qq_get_image_url(albummid, "album"),
        source: "qq".to_string(),
        source_url: format!("https://y.qq.com/#type=song&mid={songmid}&tpl=yqq_song_detail"),
        disabled: !qq_is_playable(song),
        extra: Default::default(),
    }
}

/// 对应原版 `qq_convert_song2(song)`（新版 musicu.fcg 歌手、搜索、排行榜歌曲格式）
fn qq_convert_song2(song: &Value, check_playable: bool) -> Track {
    let mid = song["mid"].as_str().unwrap_or_default();
    let name = html_decode(song["name"].as_str().unwrap_or_default());
    let (artist, artist_mid) = song["singer"]
        .as_array()
        .and_then(|arr| arr.first())
        .map(|s| {
            (
                html_decode(s["name"].as_str().unwrap_or_default()),
                s["mid"].as_str().unwrap_or_default(),
            )
        })
        .unwrap_or_default();
    let album = html_decode(song["album"]["name"].as_str().unwrap_or_default());
    let albummid = song["album"]["mid"].as_str().unwrap_or_default();

    // 原版 qq_convert_song2 写死 `url: ''`，但新版接口在 `song.action.switch` 中带有可播标记；
    // 若存在 switch 字段则按 bit 1 判断是否可播放，否则保持与原版一致。
    let disabled = if check_playable {
        if song.get("switch").is_some() || song.pointer("/action/switch").is_some() {
            !qq_is_playable(song)
        } else {
            true
        }
    } else {
        false
    };

    Track {
        id: format!("qqtrack_{mid}"),
        title: name,
        artist,
        artist_id: format!("qqartist_{artist_mid}"),
        album,
        album_id: format!("qqalbum_{albummid}"),
        img_url: qq_get_image_url(albummid, "album"),
        source: "qq".to_string(),
        source_url: format!("https://y.qq.com/#type=song&mid={mid}&tpl=yqq_song_detail"),
        disabled,
        extra: Default::default(),
    }
}

async fn qq_show_toplist(http: &Http, offset: u32) -> Result<Vec<PlaylistInfo>> {
    if offset > 0 {
        return Ok(Vec::new());
    }
    let url = "https://c.y.qq.com/v8/fcg-bin/fcg_myqq_toplist.fcg?g_tk=5381&inCharset=utf-8&outCharset=utf-8&notice=0&format=json&uin=0&needNewCode=1&platform=h5";
    let data: Value = http.get_json(url).await?;
    let mut result = Vec::new();
    if let Some(list) = data["data"]["topList"].as_array() {
        for item in list {
            let id = val_to_string(&item["id"]);
            result.push(PlaylistInfo {
                cover_img_url: item["picUrl"].as_str().unwrap_or_default().to_string(),
                id: format!("qqtoplist_{id}"),
                source_url: format!("https://y.qq.com/n/yqq/toplist/{id}.html"),
                title: html_decode(item["topTitle"].as_str().unwrap_or_default()),
                author: String::new(),
                count: None,
            });
        }
    }
    Ok(result)
}

pub async fn show_playlist(http: &Http, offset: u32, filter_id: &str) -> Result<Vec<PlaylistInfo>> {
    if filter_id == "toplist" {
        return qq_show_toplist(http, offset).await;
    }
    let category_id = if filter_id.is_empty() { "10000000" } else { filter_id };
    let rnd: f64 = rand::random();
    let target_url = format!(
        "https://c.y.qq.com/splcloud/fcgi-bin/fcg_get_diss_by_tag.fcg?picmid=1&rnd={rnd}&g_tk=732560869&loginUin=0&hostUin=0&format=json&inCharset=utf8&outCharset=utf-8&notice=0&platform=yqq.json&needNewCode=0&categoryId={category_id}&sortId=5&sin={offset}&ein={}",
        29 + offset
    );

    let data: Value = http.get_json(&target_url).await?;
    let mut playlists = Vec::new();
    if let Some(list) = data["data"]["list"].as_array() {
        for item in list {
            let dissid = val_to_string(&item["dissid"]);
            playlists.push(PlaylistInfo {
                cover_img_url: item["imgurl"].as_str().unwrap_or_default().to_string(),
                title: html_decode(item["dissname"].as_str().unwrap_or_default()),
                id: format!("qqplaylist_{dissid}"),
                source_url: format!("https://y.qq.com/n/ryqq/playlist/{dissid}"),
                author: String::new(),
                count: None,
            });
        }
    }
    Ok(playlists)
}

pub fn playlist_filters() -> Filters {
    // 数据来自 `https://c.y.qq.com/splcloud/fcgi-bin/fcg_get_diss_tag_conf.fcg`（与原版 get_playlist_filters 完全一致）
    let all = vec![
        FilterCategory {
            category: "语种".to_string(),
            filters: filters(&[
                ("165", "国语"),
                ("167", "英语"),
                ("168", "韩语"),
                ("166", "粤语"),
                ("169", "日语"),
                ("170", "小语种"),
                ("203", "闽南语"),
                ("204", "法语"),
                ("205", "拉丁语"),
            ]),
        },
        FilterCategory {
            category: "流派".to_string(),
            filters: filters(&[
                ("6", "流行"),
                ("15", "轻音乐"),
                ("11", "摇滚"),
                ("28", "民谣"),
                ("8", "R&B"),
                ("153", "嘻哈"),
                ("24", "电子"),
                ("27", "古典"),
                ("18", "乡村"),
                ("22", "蓝调"),
                ("21", "爵士"),
                ("164", "新世纪"),
                ("25", "拉丁"),
                ("218", "后摇"),
                ("219", "中国传统"),
                ("220", "世界音乐"),
            ]),
        },
        FilterCategory {
            category: "主题".to_string(),
            filters: filters(&[
                ("39", "ACG"),
                ("136", "经典"),
                ("146", "网络歌曲"),
                ("133", "影视"),
                ("141", "KTV热歌"),
                ("131", "儿歌"),
                ("145", "中国风"),
                ("194", "古风"),
                ("148", "情歌"),
                ("196", "城市"),
                ("197", "现场音乐"),
                ("199", "背景音乐"),
                ("200", "佛教音乐"),
                ("201", "UP主"),
                ("202", "乐器"),
                ("226", "MC喊麦"),
                ("14", "DJ"),
            ]),
        },
        FilterCategory {
            category: "心情".to_string(),
            filters: filters(&[
                ("52", "伤感"),
                ("122", "安静"),
                ("117", "快乐"),
                ("116", "治愈"),
                ("125", "励志"),
                ("59", "甜蜜"),
                ("55", "寂寞"),
                ("126", "宣泄"),
                ("68", "思念"),
            ]),
        },
        FilterCategory {
            category: "场景".to_string(),
            filters: filters(&[
                ("78", "睡前"),
                ("102", "夜店"),
                ("101", "学习"),
                ("99", "运动"),
                ("85", "开车"),
                ("76", "约会"),
                ("94", "工作"),
                ("81", "旅行"),
                ("103", "派对"),
                ("222", "婚礼"),
                ("223", "咖啡馆"),
                ("224", "跳舞"),
                ("16", "校园"),
            ]),
        },
    ];

    let recommend = filters(&[
        ("", "全部"),
        ("toplist", "排行榜"),
        ("6", "流行"),
        ("15", "轻音乐"),
        ("11", "摇滚"),
        ("28", "民谣"),
        ("8", "R&B"),
        ("153", "嘻哈"),
        ("24", "电子"),
        ("27", "古典"),
    ]);

    Filters { recommend, all }
}

fn get_toplist_url(id: i64, period: Option<&str>, limit: u32) -> String {
    let mut param = json!({
        "topid": id,
        "num": limit,
    });
    if let Some(p) = period {
        if !p.is_empty() {
            param["period"] = Value::String(p.to_string());
        }
    }
    let payload = json!({
        "comm": {
            "cv": 1602,
            "ct": 20,
        },
        "toplist": {
            "module": "musicToplist.ToplistInfoServer",
            "method": "GetDetail",
            "param": param,
        },
    });
    format!(
        "https://u.y.qq.com/cgi-bin/musicu.fcg?format=json&inCharset=utf8&outCharset=utf-8&platform=yqq.json&needNewCode=0&data={}",
        urlencode(&payload.to_string())
    )
}

async fn get_periods(http: &Http, topid: i64) -> Option<String> {
    let period_url = "https://c.y.qq.com/node/pc/wk_v15/top.html";
    let html = http.get_text(period_url).await.ok()?;
    let re_list = Regex::new(
        r#"<i class="play_cover__btn c_tx_link js_icon_play" data-listkey=".+?" data-listname=".+?" data-tid=".+?" data-date=".+?" .+?<\/i>"#,
    )
    .ok()?;
    let re_period = Regex::new(
        r#"data-listname="(.+?)" data-tid=".*?\/(.+?)" data-date="(.+?)" .+?<\/i>"#,
    )
    .ok()?;

    let topid_str = topid.to_string();
    for m in re_list.find_iter(&html) {
        if let Some(caps) = re_period.captures(m.as_str()) {
            let tid = caps.get(2).map(|c| c.as_str()).unwrap_or_default();
            if tid == topid_str {
                return caps.get(3).map(|c| c.as_str().to_string());
            }
        }
    }
    None
}

async fn qq_toplist(http: &Http, raw_id: &str) -> Result<Playlist> {
    let list_id: i64 = raw_id.parse().unwrap_or(0);
    let period = get_periods(http, list_id).await;
    let limit = 100;
    let target_url = get_toplist_url(list_id, period.as_deref(), limit);

    let data: Value = http.get_json(&target_url).await?;
    let toplist_data = &data["toplist"]["data"];
    let mut tracks = Vec::new();
    if let Some(list) = toplist_data["songInfoList"].as_array() {
        for song in list {
            tracks.push(qq_convert_song2(song, false));
        }
    }

    let info = PlaylistInfo {
        cover_img_url: toplist_data["data"]["frontPicUrl"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        title: html_decode(toplist_data["data"]["title"].as_str().unwrap_or_default()),
        id: format!("qqtoplist_{list_id}"),
        source_url: format!("https://y.qq.com/n/yqq/toplist/{list_id}.html"),
        author: String::new(),
        count: None,
    };

    Ok(Playlist { info, tracks })
}

async fn qq_get_playlist(http: &Http, list_id: &str) -> Result<Playlist> {
    let target_url = format!(
        "https://i.y.qq.com/qzone-music/fcg-bin/fcg_ucc_getcdinfo_byids_cp.fcg?type=1&json=1&utf8=1&onlysong=0&nosign=1&disstid={list_id}&g_tk=5381&loginUin=0&hostUin=0&format=json&inCharset=GB2312&outCharset=utf-8&notice=0&platform=yqq&needNewCode=0"
    );
    let data: Value = http.get_json(&target_url).await?;
    let cd = &data["cdlist"][0];

    let info = PlaylistInfo {
        cover_img_url: cd["logo"].as_str().unwrap_or_default().to_string(),
        title: html_decode(cd["dissname"].as_str().unwrap_or_default()),
        id: format!("qqplaylist_{list_id}"),
        source_url: format!("https://y.qq.com/n/ryqq/playlist/{list_id}"),
        author: String::new(),
        count: None,
    };

    let tracks = cd["songlist"]
        .as_array()
        .map(|arr| arr.iter().map(qq_convert_song).collect())
        .unwrap_or_default();

    Ok(Playlist { info, tracks })
}

async fn qq_album(http: &Http, album_id: &str) -> Result<Playlist> {
    let target_url = format!(
        "https://i.y.qq.com/v8/fcg-bin/fcg_v8_album_info_cp.fcg?platform=h5page&albummid={album_id}&g_tk=938407465&uin=0&format=json&inCharset=utf-8&outCharset=utf-8&notice=0&platform=h5&needNewCode=1&_=1459961045571"
    );
    let data: Value = http.get_json(&target_url).await?;
    let album_data = &data["data"];

    let info = PlaylistInfo {
        cover_img_url: qq_get_image_url(album_id, "album"),
        title: html_decode(album_data["name"].as_str().unwrap_or_default()),
        id: format!("qqalbum_{album_id}"),
        source_url: format!("https://y.qq.com/#type=album&mid={album_id}"),
        author: String::new(),
        count: None,
    };

    let tracks = album_data["list"]
        .as_array()
        .map(|arr| arr.iter().map(qq_convert_song).collect())
        .unwrap_or_default();

    Ok(Playlist { info, tracks })
}

async fn qq_artist(http: &Http, artist_id: &str) -> Result<Playlist> {
    let payload = json!({
        "comm": {
            "ct": 24,
            "cv": 0,
        },
        "singer": {
            "method": "get_singer_detail_info",
            "param": {
                "sort": 5,
                "singermid": artist_id,
                "sin": 0,
                "num": 50,
            },
            "module": "music.web_singer_info_svr",
        },
    });
    let target_url = format!(
        "https://u.y.qq.com/cgi-bin/musicu.fcg?format=json&loginUin=0&hostUin=0inCharset=utf8&outCharset=utf-8&platform=yqq.json&needNewCode=0&data={}",
        urlencode(&payload.to_string())
    );

    let data: Value = http.get_json(&target_url).await?;
    let singer_data = &data["singer"]["data"];

    let info = PlaylistInfo {
        cover_img_url: qq_get_image_url(artist_id, "artist"),
        title: html_decode(
            singer_data["singer_info"]["name"]
                .as_str()
                .unwrap_or_default(),
        ),
        id: format!("qqartist_{artist_id}"),
        source_url: format!("https://y.qq.com/#type=singer&mid={artist_id}"),
        author: String::new(),
        count: None,
    };

    let tracks = singer_data["songlist"]
        .as_array()
        .map(|arr| arr.iter().map(|item| qq_convert_song2(item, true)).collect())
        .unwrap_or_default();

    Ok(Playlist { info, tracks })
}

pub async fn get_playlist(http: &Http, list_id: &str) -> Result<Playlist> {
    let (prefix, id) = list_id
        .split_once('_')
        .ok_or_else(|| anyhow!("无效的 QQ 歌单 ID: {list_id}"))?;
    match prefix {
        "qqplaylist" => qq_get_playlist(http, id).await,
        "qqalbum" => qq_album(http, id).await,
        "qqartist" => qq_artist(http, id).await,
        "qqtoplist" => qq_toplist(http, id).await,
        _ => bail!("未知的 QQ 歌单类型: {prefix}"),
    }
}

pub async fn search(http: &Http, keyword: &str, page: u32, kind: SearchType) -> Result<SearchResult> {
    let target_url = "https://u.y.qq.com/cgi-bin/musicu.fcg";
    let search_type = match kind {
        SearchType::Track => 0,
        SearchType::Playlist => 3,
    };
    let limit = 50;
    let page_num = page.max(1);

    // 注：原版 comm 使用 `ct: "19", cv: "1859"`，现未登录请求会返回 `req.code = 2001`（强制登录风控）；
    // 改用 `ct: 11, cv: "1003006"` 配合同一 `DoSearchForQQMusicDesktop` 方法可免登录返回完全一致的数据结构。
    let query = json!({
        "comm": {
            "ct": 11,
            "cv": "1003006",
            "v": "1003006",
            "os_ver": "12",
            "phonetype": "0",
            "tmeAppID": "qqmusiclight",
            "nettype": "1020",
            "uin": "0",
        },
        "req": {
            "method": "DoSearchForQQMusicDesktop",
            "module": "music.search.SearchCgiService",
            "param": {
                "grp": 1,
                "num_per_page": limit,
                "page_num": page_num,
                "query": keyword,
                "search_type": search_type,
            },
        },
    });

    let text = http
        .post(target_url)
        .json(&query)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let data: Value = parse_json(&text)?;

    let req_data = &data["req"]["data"];
    let total = req_data["meta"]["sum"].as_u64().unwrap_or(0);

    match kind {
        SearchType::Track => {
            let tracks = req_data["body"]["song"]["list"]
                .as_array()
                .map(|arr| arr.iter().map(|item| qq_convert_song2(item, true)).collect())
                .unwrap_or_default();
            Ok(SearchResult {
                tracks,
                playlists: Vec::new(),
                total,
            })
        }
        SearchType::Playlist => {
            let playlists = req_data["body"]["songlist"]["list"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|info| {
                            let dissid = val_to_string(&info["dissid"]);
                            PlaylistInfo {
                                id: format!("qqplaylist_{dissid}"),
                                title: html_decode(info["dissname"].as_str().unwrap_or_default()),
                                source_url: format!("https://y.qq.com/n/ryqq/playlist/{dissid}"),
                                cover_img_url: info["imgurl"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string(),
                                author: html_decode(
                                    info["creator"]["name"].as_str().unwrap_or_default(),
                                ),
                                count: info["song_count"].as_u64(),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(SearchResult {
                tracks: Vec::new(),
                playlists,
                total,
            })
        }
    }
}

pub async fn bootstrap_track(http: &Http, track: &Track) -> Result<Sound> {
    let song_id = track
        .id
        .strip_prefix("qqtrack_")
        .unwrap_or(track.id.as_str());
    let target_url = "https://u.y.qq.com/cgi-bin/musicu.fcg";
    let guid = "10000";
    let uin = "0";

    // 对应原版 fileType = '128' -> M500 + songId + songId + .mp3
    let file = format!("M500{song_id}{song_id}.mp3");
    let req_data = json!({
        "req_1": {
            "module": "vkey.GetVkeyServer",
            "method": "CgiGetVkey",
            "param": {
                "filename": [file],
                "guid": guid,
                "songmid": [song_id],
                "songtype": [0],
                "uin": uin,
                "loginflag": 1,
                "platform": "20",
            },
        },
        "loginUin": uin,
        "comm": {
            "uin": uin,
            "format": "json",
            "ct": 24,
            "cv": 0,
        },
    });

    let text = http
        .post(target_url)
        .json(&req_data)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let data: Value = parse_json(&text)?;

    let req1_data = &data["req_1"]["data"];
    let purl = req1_data["midurlinfo"][0]["purl"]
        .as_str()
        .unwrap_or_default();
    if purl.is_empty() {
        bail!("QQ 音乐需要 VIP 或无法播放该歌曲: {}", track.id);
    }

    let sip = req1_data["sip"]
        .as_array()
        .and_then(|arr| arr.iter().find_map(|v| v.as_str().filter(|s| !s.is_empty())))
        .unwrap_or("https://ws.stream.qqmusic.qq.com/");
    let url = format!("{sip}{purl}");

    let prefix = purl.get(..4).unwrap_or_default();
    let bitrate = match prefix {
        "C400" => "M4A",
        "M500" => "128kbps",
        "M800" => "320kbps",
        "A000" => "APE",
        "F000" => "FLAC",
        _ => "",
    }
    .to_string();

    Ok(Sound {
        url,
        bitrate,
        platform: "qq".to_string(),
    })
}

/// 解码歌词字段（优先尝试 Base64 解码，若已是明文 LRC 则直接反转义 HTML 实体）
fn decode_lyric_field(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if !trimmed.contains('[') {
        if let Ok(bytes) = BASE64.decode(trimmed) {
            if let Ok(decoded) = String::from_utf8(bytes) {
                return html_decode(&decoded);
            }
        }
    }
    html_decode(trimmed)
}

pub async fn lyric(http: &Http, track: &Track) -> Result<Lyric> {
    let track_id = track
        .id
        .strip_prefix("qqtrack_")
        .unwrap_or(track.id.as_str());
    let target_url = format!(
        "https://i.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg?songmid={track_id}&g_tk=5381&format=json&inCharset=utf8&outCharset=utf-8"
    );

    let data: Value = http.get_json(&target_url).await?;
    let lrc = decode_lyric_field(data["lyric"].as_str().unwrap_or_default());
    let tlrc = decode_lyric_field(data["trans"].as_str().unwrap_or_default()).replace("//", "");

    Ok(Lyric {
        lyric: lrc,
        tlyric: tlrc,
    })
}

pub async fn parse_url(http: &Http, url: &str) -> Option<String> {
    let patterns = [
        r"//y\.qq\.com/n/yqq/playlist/([0-9]+)",
        r"//y\.qq\.com/n/ryqq/playlist/([0-9]+)",
        r"//y\.qq\.com/n/yqq/playsquare/([0-9]+)",
        r"//y\.qq\.com/n/m/detail/taoge/index\.html\?id=([0-9]+)",
    ];
    for pat in patterns {
        if let Ok(re) = Regex::new(pat) {
            if let Some(caps) = re.captures(url) {
                if let Some(id) = caps.get(1) {
                    return Some(format!("qqplaylist_{}", id.as_str()));
                }
            }
        }
    }

    if let Ok(re_short) = Regex::new(r"//c\.y\.qq\.com/base/fcgi-bin/u\?__=([0-9a-zA-Z]+)") {
        if re_short.is_match(url) {
            let resp = http.get(url).send().await.ok()?;
            let final_url = resp.url().as_str();
            if let Some(id) = query_param(final_url, "id") {
                if !id.is_empty() {
                    return Some(format!("qqplaylist_{id}"));
                }
            }
            for pat in patterns {
                if let Ok(re) = Regex::new(pat) {
                    if let Some(caps) = re.captures(final_url) {
                        if let Some(id) = caps.get(1) {
                            return Some(format!("qqplaylist_{}", id.as_str()));
                        }
                    }
                }
            }
        }
    }

    None
}
