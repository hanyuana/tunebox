//! 咪咕音乐（migu）平台实现，移植自 `_ref/listen1_ext/js/provider/migu.js`。
use crate::http::{urlencode, Http};
use crate::models::*;
use anyhow::{anyhow, bail, Result};
use regex_lite::Regex;
use serde_json::Value;

pub const PREFIX: &str = "mg";

fn val_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn val_u64(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
}

fn fix_img_url(url: &str) -> String {
    if url.is_empty() {
        String::new()
    } else if url.starts_with("//") {
        format!("https:{url}")
    } else if url.starts_with('/') {
        format!("https://d.musicapp.migu.cn{url}")
    } else {
        url.to_string()
    }
}

/// 对应原版 `mg_convert_song`（用于歌单、专辑、歌手歌曲列表及排行榜 API 返回的条目）
fn convert_song(song: &Value) -> Track {
    let copyright_id = val_str(&song["copyrightId"]);
    let title = val_str(&song["songName"]);

    let (artist, artist_id_raw) = if let Some(first) = song["artists"]
        .as_array()
        .and_then(|arr| arr.first())
    {
        (val_str(&first["name"]), val_str(&first["id"]))
    } else {
        (val_str(&song["singer"]), val_str(&song["singerId"]))
    };

    let album_id_raw = val_str(&song["albumId"]);
    let (album, album_id) = if !album_id_raw.is_empty() && album_id_raw != "1" {
        (val_str(&song["album"]), format!("mgalbum_{album_id_raw}"))
    } else {
        (String::new(), "mgalbum_".to_string())
    };

    let img_url = song["albumImgs"]
        .get(1)
        .or_else(|| song["albumImgs"].get(0))
        .map(|v| fix_img_url(&val_str(&v["img"])))
        .unwrap_or_default();

    let lyric_url = val_str(&song["lrcUrl"]);
    let tlyric_url = val_str(&song["trcUrl"]);
    let quality = val_str(&song["toneControl"]);
    let song_id = val_str(&song["songId"]);
    let content_id = val_str(&song["contentId"]);

    let disabled = song["copyright"].as_i64() == Some(0)
        || song["copyright"].as_str() == Some("0");

    let mut track = Track {
        id: format!("mgtrack_{copyright_id}"),
        title,
        artist,
        artist_id: format!("mgartist_{artist_id_raw}"),
        album,
        album_id,
        source: "migu".to_string(),
        source_url: format!("https://music.migu.cn/v3/music/song/{copyright_id}"),
        img_url,
        disabled,
        ..Default::default()
    };
    track.set_extra("lyric_url", lyric_url);
    track.set_extra("tlyric_url", tlyric_url);
    track.set_extra("quality", quality);
    track.set_extra("song_id", song_id);
    track.set_extra("content_id", content_id);
    track
}

/// 对应原版 `mg_convert_song2`（用于搜索接口返回的条目）
fn convert_song2(song: &Value) -> Track {
    let copyright_id = val_str(&song["copyrightId"]);
    let title = val_str(&song["songName"]);

    let (artist, artist_id_raw) = if let Some(first) = song["singerList"]
        .as_array()
        .and_then(|arr| arr.first())
    {
        (val_str(&first["name"]), val_str(&first["id"]))
    } else {
        (val_str(&song["singer"]), val_str(&song["singerId"]))
    };

    let album_id_raw = val_str(&song["albumId"]);
    let (album, album_id) = if !album_id_raw.is_empty() && album_id_raw != "1" {
        (val_str(&song["album"]), format!("mgalbum_{album_id_raw}"))
    } else {
        (String::new(), "mgalbum_".to_string())
    };

    let img_url = fix_img_url(&val_str(&song["img1"]));

    let mut lyric_url = val_str(&song["ext"]["lrcUrl"]);
    if lyric_url.is_empty() {
        lyric_url = val_str(&song["lrcUrl"]);
    }
    let mut tlyric_url = val_str(&song["ext"]["trcUrl"]);
    if tlyric_url.is_empty() {
        tlyric_url = val_str(&song["trcUrl"]);
    }

    let mut quality = val_str(&song["toneControl"]);
    if quality.is_empty() {
        // 新版 PC 搜索接口通过 audioFormats 标识最高音质，回退推断 toneControl
        if let Some(formats) = song["audioFormats"].as_array() {
            let has_zq = formats
                .iter()
                .any(|f| val_str(&f["formatType"]).starts_with("ZQ"));
            let has_sq = formats.iter().any(|f| val_str(&f["formatType"]) == "SQ");
            let has_hq = formats.iter().any(|f| val_str(&f["formatType"]) == "HQ");
            if has_zq {
                quality = "111111".to_string();
            } else if has_sq {
                quality = "111100".to_string();
            } else if has_hq {
                quality = "110000".to_string();
            }
        }
    }

    let song_id = val_str(&song["songId"]);
    let content_id = val_str(&song["contentId"]);

    let disabled = song["copyright"].as_i64() == Some(0)
        || song["copyright"].as_str() == Some("0");

    let mut track = Track {
        id: format!("mgtrack_{copyright_id}"),
        title,
        artist,
        artist_id: format!("mgartist_{artist_id_raw}"),
        album,
        album_id,
        source: "migu".to_string(),
        source_url: format!("https://music.migu.cn/v3/music/song/{copyright_id}"),
        img_url,
        disabled,
        ..Default::default()
    };
    track.set_extra("lyric_url", lyric_url);
    track.set_extra("tlyric_url", tlyric_url);
    track.set_extra("quality", quality);
    track.set_extra("song_id", song_id);
    track.set_extra("content_id", content_id);
    track
}

async fn render_tracks(http: &Http, playlist_type: &str, list_id: &str, page: u32) -> Result<Vec<Track>> {
    let tracks_url = match playlist_type {
        "mgplaylist" => format!(
            "https://app.c.nf.migu.cn/MIGUM2.0/v1.0/user/queryMusicListSongs.do?musicListId={list_id}&pageNo={page}&pageSize=50"
        ),
        "mgalbum" => format!(
            "https://app.c.nf.migu.cn/MIGUM2.0/v1.0/content/queryAlbumSong?albumId={list_id}&pageNo={page}&pageSize=50"
        ),
        _ => return Ok(Vec::new()),
    };

    let resp: Value = http.get_json(&tracks_url).await?;
    let list = if playlist_type == "mgplaylist" {
        resp["list"].as_array()
    } else {
        resp["data"]["songList"].as_array()
    };
    Ok(list
        .map(|arr| arr.iter().map(convert_song).collect())
        .unwrap_or_default())
}

async fn show_toplist(http: &Http, offset: u32) -> Result<Vec<PlaylistInfo>> {
    if offset > 0 {
        return Ok(Vec::new());
    }

    let mut result = vec![
        PlaylistInfo {
            cover_img_url: "https://cdnmusic.migu.cn/tycms_picture/20/02/36/20020512065402_360x360_2997.png".to_string(),
            title: "尖叫新歌榜".to_string(),
            id: "mgtoplist_27553319".to_string(),
            source_url: String::new(),
            ..Default::default()
        },
        PlaylistInfo {
            cover_img_url: "https://cdnmusic.migu.cn/tycms_picture/20/04/99/200408163640868_360x360_6587.png".to_string(),
            title: "尖叫热歌榜".to_string(),
            id: "mgtoplist_27186466".to_string(),
            source_url: String::new(),
            ..Default::default()
        },
        PlaylistInfo {
            cover_img_url: "https://cdnmusic.migu.cn/tycms_picture/20/04/99/200408163702795_360x360_1614.png".to_string(),
            title: "尖叫原创榜".to_string(),
            id: "mgtoplist_27553408".to_string(),
            source_url: String::new(),
            ..Default::default()
        },
    ];

    let url = "https://app.c.nf.migu.cn/MIGUM3.0/v1.0/template/rank-list/release?dataVersion=1616469593718&templateVersion=9";
    if let Ok(resp) = http.get_json::<Value>(url).await {
        let parse_board = |idx: usize, skip: usize| -> Vec<PlaylistInfo> {
            resp["data"]["contentItemList"]
                .get(idx)
                .and_then(|v| v["itemList"].as_array())
                .map(|arr| {
                    arr.iter()
                        .skip(skip)
                        .filter_map(|item| {
                            let param = &item["displayLogId"]["param"];
                            let rank_id = val_str(&param["rankId"]);
                            if rank_id.is_empty() {
                                return None;
                            }
                            Some(PlaylistInfo {
                                cover_img_url: val_str(&item["imageUrl"]),
                                title: val_str(&param["rankName"]),
                                id: format!("mgtoplist_{rank_id}"),
                                source_url: String::new(),
                                ..Default::default()
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };

        // 原版对 contentItemList[4].itemList 执行了 splice(0, 2)，若当前返回不足 2 条则保留全部
        let board4_len = resp["data"]["contentItemList"]
            .get(4)
            .and_then(|v| v["itemList"].as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let skip4 = if board4_len > 2 { 2 } else { 0 };
        let migu_board = parse_board(4, skip4);
        let global_board = parse_board(7, 0);

        result.extend(migu_board);
        result.extend(global_board);
    }

    Ok(result)
}

pub async fn show_playlist(http: &Http, offset: u32, filter_id: &str) -> Result<Vec<PlaylistInfo>> {
    if filter_id == "toplist" {
        return show_toplist(http, offset).await;
    }

    let page_size = 30;
    let page = offset / page_size + 1;
    let target_url = if filter_id.is_empty() {
        format!(
            "https://app.c.nf.migu.cn/MIGUM2.0/v2.0/content/getMusicData.do?count={page_size}&start={page}&templateVersion=5&type=1"
        )
    } else {
        format!(
            "https://app.c.nf.migu.cn/MIGUM3.0/v1.0/template/musiclistplaza-listbytag?pageNumber={page}&tagId={filter_id}&templateVersion=1"
        )
    };

    let resp: Value = http.get_json(&target_url).await?;
    let items = if filter_id.is_empty() {
        resp["data"]["contentItemList"]
            .get(0)
            .and_then(|v| v["itemList"].as_array())
    } else {
        resp["data"]["contentItemList"]["itemList"].as_array()
    };

    let re = Regex::new(r"id=([0-9]+)&").ok();
    let mut result = Vec::new();
    if let Some(arr) = items {
        for item in arr {
            let action_url = val_str(&item["actionUrl"]);
            let id = re
                .as_ref()
                .and_then(|r| r.captures(&action_url))
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            result.push(PlaylistInfo {
                cover_img_url: val_str(&item["imageUrl"]),
                title: val_str(&item["title"]),
                id: format!("mgplaylist_{id}"),
                source_url: format!("https://music.migu.cn/v3/music/playlist/{id}"),
                ..Default::default()
            });
        }
    }

    Ok(result)
}

/// 移植自原版 `get_playlist_filters`（原版通过 `musiclistplaza-hottaglist` 与 `musiclistplaza-taglist` 获取，此处固化其返回结构）
pub fn playlist_filters() -> Filters {
    Filters {
        recommend: filters(&[
            ("", "推荐"),
            ("toplist", "排行榜"),
            ("1003449727", "厂牌"),
            ("1000001635", "经典老歌"),
            ("1000587679", "减压"),
            ("1000001809", "嘻哈"),
            ("1001076096", "网络热歌"),
            ("1000001675", "中国风"),
            ("1000001766", "英语"),
            ("1000001762", "国语"),
        ]),
        all: vec![
            FilterCategory {
                category: "热门".to_string(),
                filters: filters(&[
                    ("1001076096", "网络热歌"),
                    ("1000001672", "流行"),
                    ("1000001635", "经典老歌"),
                    ("1003449976", "官方"),
                    ("1000001675", "国风"),
                    ("1000001775", "民谣"),
                    ("1000001827", "KTV金曲"),
                    ("1000001777", "欧美"),
                    ("1000001780", "儿歌"),
                    ("1003449727", "厂牌"),
                    ("1000001795", "伤感"),
                    ("1001076080", "电影"),
                    ("1000001696", "怀旧"),
                    ("1000001831", "翻唱"),
                ]),
            },
            FilterCategory {
                category: "风格".to_string(),
                filters: filters(&[
                    ("1000001672", "流行"),
                    ("1000001682", "电子"),
                    ("1000001675", "国风"),
                    ("1000001681", "爵士"),
                    ("1000001683", "乡村"),
                    ("1000001851", "蓝调"),
                    ("1000001775", "民谣"),
                    ("1000001807", "纯音乐"),
                    ("1000001783", "古典"),
                    ("1000001674", "摇滚"),
                    ("1000001809", "嘻哈"),
                    ("1000001808", "R&B"),
                ]),
            },
            FilterCategory {
                category: "语种".to_string(),
                filters: filters(&[
                    ("1000001762", "国语"),
                    ("1000001763", "粤语"),
                    ("1000001766", "英语"),
                    ("1000001767", "日语"),
                    ("1000001599", "韩语"),
                    ("1003449724", "小语种"),
                ]),
            },
            FilterCategory {
                category: "场景".to_string(),
                filters: filters(&[
                    ("1003449209", "运动"),
                    ("1000587692", "瑜伽"),
                    ("1003450093", "学习"),
                    ("1000587688", "睡前安眠"),
                    ("1000093923", "驾车"),
                    ("1000001749", "旅行"),
                    ("1000587686", "夜店"),
                    ("1002600606", "派对"),
                    ("1003449723", "下午茶"),
                    ("1003449726", "读书"),
                ]),
            },
            FilterCategory {
                category: "主题".to_string(),
                filters: filters(&[
                    ("1001076083", "综艺"),
                    ("1000001827", "KTV金曲"),
                    ("1000001698", "爱情"),
                    ("1000001635", "经典老歌"),
                    ("1001076096", "网络热歌"),
                    ("1000001780", "儿歌"),
                    ("1000587717", "70后"),
                    ("1000587718", "80后"),
                    ("1000587726", "90后"),
                    ("1000587702", "广场舞"),
                    ("1000001670", "红歌"),
                    ("1000587698", "游戏"),
                    ("1000001675", "国风"),
                    ("1000001796", "安静"),
                    ("1000587712", "青春校园"),
                    ("1000587673", "小清新"),
                    ("1000093902", "DJ舞曲"),
                    ("1001076078", "电视剧"),
                    ("1001076080", "电影"),
                    ("1000001831", "翻唱"),
                ]),
            },
            FilterCategory {
                category: "心情".to_string(),
                filters: filters(&[
                    ("1000587677", "幸福"),
                    ("1000587710", "治愈"),
                    ("1000001703", "思念"),
                    ("1000001700", "励志"),
                    ("1000001694", "欢快"),
                    ("1002600588", "叛逆"),
                    ("1002600585", "宣泄"),
                    ("1000001696", "怀旧"),
                    ("1000587679", "减压"),
                    ("1000001699", "寂寞"),
                    ("1002600579", "忧郁"),
                    ("1000001795", "伤感"),
                    ("1000001796", "安静"),
                ]),
            },
        ],
    }
}

fn board_info(list_id: u64) -> Option<(&'static str, &'static str, &'static str)> {
    match list_id {
        27553319 => Some(("尖叫新歌榜", "jianjiao_newsong", "/20/02/36/20020512065402_360x360_2997.png")),
        27186466 => Some(("尖叫热歌榜", "jianjiao_hotsong", "/20/04/99/200408163640868_360x360_6587.png")),
        27553408 => Some(("尖叫原创榜", "jianjiao_original", "/20/04/99/200408163702795_360x360_1614.png")),
        1 => Some(("音乐榜", "migumusic", "/20/05/136/200515161733982_360x360_1523.png")),
        2 => Some(("影视榜", "movies", "/20/05/136/200515161848938_360x360_673.png")),
        23189399 => Some(("内地榜", "mainland", "/20/08/231/200818095104122_327x327_4971.png")),
        23189800 => Some(("港台榜", "hktw", "/20/08/231/200818095125191_327x327_2382.png")),
        19190036 => Some(("欧美榜", "eur_usa", "/20/08/231/200818095229556_327x327_1383.png")),
        23189813 => Some(("日韩榜", "jpn_kor", "/20/08/231/200818095259569_327x327_4628.png")),
        23190126 => Some(("彩铃榜", "coloring", "/20/08/231/200818095356693_327x327_7955.png")),
        15140045 => Some(("KTV榜", "ktv", "/20/08/231/200818095414420_327x327_4992.png")),
        15140034 => Some(("网络榜", "network", "/20/08/231/200818095442606_327x327_1298.png")),
        23218151 => Some(("新专辑榜", "newalbum", "/20/08/231/200818095603246_327x327_7480.png")),
        33683712 => Some((
            "数字专辑畅销榜",
            "",
            "https://d.musicapp.migu.cn/prod/file-service/file-down/bcb5ddaf77828caee4eddc172edaa105/2297b53efa678bbc8a5b83064622c4c8/ebfe5bff9fd9981b5ae1c043f743bfb3",
        )),
        23217754 => Some(("MV榜", "mv", "/20/08/231/200818095656365_327x327_8344.png")),
        21958042 => Some(("美国iTunes榜", "itunes", "/20/08/231/200818095755771_327x327_9250.png")),
        21975570 => Some(("美国billboard榜", "billboard", "/20/08/231/20081809581365_327x327_4636.png")),
        22272815 => Some(("Hito中文榜", "hito", "/20/08/231/200818095834912_327x327_5042.png")),
        22272943 => Some(("韩国Melon榜", "mnet", "/20/08/231/200818095926828_327x327_3277.png")),
        22273437 => Some(("英国UK榜", "uk", "/20/08/231/200818095950791_327x327_8293.png")),
        _ => None,
    }
}

async fn get_toplist(http: &Http, raw_id: &str) -> Result<Playlist> {
    let list_id: u64 = raw_id.parse().unwrap_or(0);
    let (board_name, board_url, board_img) =
        board_info(list_id).unwrap_or(("咪咕排行榜", "", ""));

    let cover_img_url = if list_id == 33683712 || board_img.starts_with("http") {
        board_img.to_string()
    } else if !board_img.is_empty() {
        format!("https://cdnmusic.migu.cn/tycms_picture{board_img}")
    } else {
        String::new()
    };

    let source_url = format!("https://music.migu.cn/v3/music/top/{board_url}");
    let mut info = PlaylistInfo {
        id: format!("mgtoplist_{raw_id}"),
        cover_img_url,
        title: board_name.to_string(),
        source_url: source_url.clone(),
        ..Default::default()
    };

    let mut tracks = Vec::new();

    // 1. 先按原版逻辑尝试从网页 `<script>` 标签中提取SSR数据
    if !board_url.is_empty() {
        if let Ok(html) = http.get_text(&source_url).await {
            if let Ok(re) = Regex::new(r"(?s)<script[^>]*>(.*?)</script>") {
                let mut scripts = re.captures_iter(&html);
                // 原版取 list_elements[1].innerText.split('=').slice(1).join('=')
                if let Some(cap) = scripts.nth(1) {
                    let inner = cap.get(1).map(|m| m.as_str()).unwrap_or_default().trim();
                    if let Some((_, json_part)) = inner.split_once('=') {
                        if let Ok(result) = serde_json::from_str::<Value>(json_part.trim()) {
                            if let Some(items) = result["songs"]["items"].as_array() {
                                for song in items {
                                    let album_obj = &song["album"];
                                    let album_id_num = album_obj["albumId"].as_i64().unwrap_or(1);
                                    let album_id_str = val_str(&album_obj["albumId"]);
                                    let (song_album, song_album_id) = if !album_obj.is_null()
                                        && album_id_num != 1
                                        && album_id_str != "1"
                                        && !album_id_str.is_empty()
                                    {
                                        (
                                            val_str(&album_obj["albumName"]),
                                            format!("mgalbum_{album_id_str}"),
                                        )
                                    } else {
                                        (String::new(), "mgalbum_".to_string())
                                    };

                                    let copyright_id = val_str(&song["copyrightId"]);
                                    let (artist, artist_id) = song["singers"]
                                        .get(0)
                                        .map(|s| {
                                            (val_str(&s["name"]), format!("mgartist_{}", val_str(&s["id"])))
                                        })
                                        .unwrap_or_else(|| (String::new(), "mgartist_".to_string()));

                                    let medium_pic = val_str(&song["mediumPic"]);
                                    let img_url = if medium_pic.starts_with("http") {
                                        medium_pic
                                    } else if !medium_pic.is_empty() {
                                        format!("https:{medium_pic}")
                                    } else {
                                        String::new()
                                    };

                                    let quality = if song["bit24"].as_bool().unwrap_or(false) {
                                        "111111"
                                    } else if song["sq"].as_bool().unwrap_or(false) {
                                        "111100"
                                    } else {
                                        "110000"
                                    };

                                    let mut track = Track {
                                        id: format!("mgtrack_{copyright_id}"),
                                        title: val_str(&song["name"]),
                                        artist,
                                        artist_id,
                                        album: song_album,
                                        album_id: song_album_id,
                                        source: "migu".to_string(),
                                        source_url: format!(
                                            "https://music.migu.cn/v3/music/song/{copyright_id}"
                                        ),
                                        img_url,
                                        disabled: false,
                                        ..Default::default()
                                    };
                                    track.set_extra("lyric_url", "null");
                                    track.set_extra("tlyric_url", "");
                                    track.set_extra("song_id", val_str(&song["id"]));
                                    track.set_extra("quality", quality);
                                    tracks.push(track);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. 咪咕官网已改版为纯前端 SPA（https://music.migu.cn/v3/music/top/... 不再内嵌歌曲 JSON），
    //    若网页提取为空，则回退使用咪咕移动端排行榜 API `querycontentbyId.do` 获取榜单数据
    if tracks.is_empty() {
        let api_url = format!(
            "https://app.c.nf.migu.cn/MIGUM2.0/v1.0/content/querycontentbyId.do?needAll=0&columnId={raw_id}"
        );
        if let Ok(resp) = http.get_json::<Value>(&api_url).await {
            let col_title = val_str(&resp["columnInfo"]["columnTitle"]);
            if !col_title.is_empty() && info.title == "咪咕排行榜" {
                info.title = col_title;
            }
            if let Some(contents) = resp["columnInfo"]["contents"].as_array() {
                for item in contents {
                    let obj = &item["objectInfo"];
                    if !obj.is_null() {
                        tracks.push(convert_song(obj));
                    }
                }
            }
        }
    }

    Ok(Playlist { info, tracks })
}

async fn get_migu_playlist(http: &Http, list_id: &str) -> Result<Playlist> {
    let info_url = format!(
        "https://app.c.nf.migu.cn/MIGUM2.0/v1.0/content/resourceinfo.do?needSimple=00&resourceType=2021&resourceId={list_id}"
    );
    let resp: Value = http.get_json(&info_url).await?;
    let res0 = &resp["resource"][0];
    let info = PlaylistInfo {
        id: format!("mgplaylist_{list_id}"),
        cover_img_url: val_str(&res0["imgItem"]["img"]),
        title: val_str(&res0["title"]),
        source_url: format!("https://music.migu.cn/v3/music/playlist/{list_id}"),
        ..Default::default()
    };

    let total = val_u64(&res0["musicNum"]).unwrap_or(0);
    let pages = ((total as f64) / 50.0).ceil() as u32;
    let mut tracks = Vec::new();
    for p in 1..=pages {
        if let Ok(mut page_tracks) = render_tracks(http, "mgplaylist", list_id, p).await {
            tracks.append(&mut page_tracks);
        }
    }

    Ok(Playlist { info, tracks })
}

async fn get_album(http: &Http, album_id: &str) -> Result<Playlist> {
    let info_url = format!(
        "https://app.c.nf.migu.cn/MIGUM2.0/v1.0/content/resourceinfo.do?needSimple=00&resourceType=2003&resourceId={album_id}"
    );
    let resp: Value = http.get_json(&info_url).await?;
    let res0 = &resp["resource"][0];
    let cover_img_url = res0["imgItems"]
        .get(1)
        .or_else(|| res0["imgItems"].get(0))
        .map(|v| val_str(&v["img"]))
        .unwrap_or_default();

    let info = PlaylistInfo {
        id: format!("mgalbum_{album_id}"),
        cover_img_url,
        title: val_str(&res0["title"]),
        source_url: format!("https://music.migu.cn/v3/music/album/{album_id}"),
        ..Default::default()
    };

    let total = val_u64(&res0["totalCount"]).unwrap_or(0);
    let pages = ((total as f64) / 50.0).ceil() as u32;
    let mut tracks = Vec::new();
    for p in 1..=pages {
        if let Ok(mut page_tracks) = render_tracks(http, "mgalbum", album_id, p).await {
            tracks.append(&mut page_tracks);
        }
    }

    Ok(Playlist { info, tracks })
}

async fn get_artist(http: &Http, artist_id: &str) -> Result<Playlist> {
    let page_size = 50;
    let page = 1;
    let target_url = format!(
        "https://app.c.nf.migu.cn/MIGUM2.0/v1.0/content/singer_songs.do?pageNo={page}&pageSize={page_size}&resourceType=2&singerId={artist_id}"
    );
    let data: Value = http.get_json(&target_url).await?;
    let cover_img_url = data["singer"]["imgs"]
        .get(1)
        .or_else(|| data["singer"]["imgs"].get(0))
        .map(|v| val_str(&v["img"]))
        .unwrap_or_default();

    let info = PlaylistInfo {
        id: format!("mgartist_{artist_id}"),
        cover_img_url,
        title: val_str(&data["singer"]["singer"]),
        source_url: format!("https://music.migu.cn/v3/music/artist/{artist_id}/song"),
        ..Default::default()
    };

    let tracks = data["songlist"]
        .as_array()
        .map(|arr| arr.iter().map(convert_song).collect())
        .unwrap_or_default();

    Ok(Playlist { info, tracks })
}

pub async fn get_playlist(http: &Http, list_id: &str) -> Result<Playlist> {
    let (prefix, raw_id) = list_id
        .split_once('_')
        .ok_or_else(|| anyhow!("无效的咪咕歌单 id: {list_id}"))?;
    match prefix {
        "mgplaylist" => get_migu_playlist(http, raw_id).await,
        "mgalbum" => get_album(http, raw_id).await,
        "mgartist" => get_artist(http, raw_id).await,
        "mgtoplist" => get_toplist(http, raw_id).await,
        _ => bail!("未知的咪咕歌单类型: {prefix}"),
    }
}

pub async fn bootstrap_track(http: &Http, track: &Track) -> Result<Sound> {
    let content_id = track.extra_str("content_id").unwrap_or_default();
    let copyright_id = track.id.strip_prefix("mgtrack_").unwrap_or(&track.id);
    let quality = track.extra_str("quality").unwrap_or_default();

    let tone_flag = match quality {
        "110000" => "HQ",
        "111100" => "SQ",
        "111111" => "ZQ",
        _ => "PQ",
    };

    let target_url = format!(
        "https://app.c.nf.migu.cn/MIGUM3.0/strategy/pc/listen/v1.0?scene=&netType=01&resourceType=2&copyrightId={copyright_id}&contentId={content_id}&toneFlag={tone_flag}"
    );

    let text = http
        .get(&target_url)
        .header("channel", "0146951")
        .header("uid", "1234")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let resp: Value = crate::http::parse_json(&text)?;

    let mut play_url = val_str(&resp["data"]["url"]);
    if play_url.is_empty() {
        bail!("咪咕无法获取播放地址");
    }
    if play_url.starts_with("//") {
        play_url = format!("https:{play_url}");
    }
    let url = play_url.replace('+', "%2B");

    let bitrate = match tone_flag {
        "HQ" => "320kbps",
        "SQ" | "ZQ" => "999kbps",
        _ => "128kbps",
    }
    .to_string();

    Ok(Sound {
        url,
        bitrate,
        platform: "migu".to_string(),
    })
}

pub async fn search(http: &Http, keyword: &str, page: u32, kind: SearchType) -> Result<SearchResult> {
    let kw = urlencode(keyword);
    match kind {
        SearchType::Track => {
            let target_url = format!(
                "https://app.u.nf.migu.cn/pc/resource/song/item/search/v1.0?text={kw}&pageNo={page}&pageSize=20"
            );
            let data: Value = http.get_json(&target_url).await?;
            let tracks: Vec<Track> = data
                .as_array()
                .map(|arr| arr.iter().map(convert_song2).collect())
                .unwrap_or_default();
            let total = if tracks.is_empty() && page == 1 { 0 } else { 1000 };
            Ok(SearchResult {
                tracks,
                playlists: Vec::new(),
                total,
            })
        }
        SearchType::Playlist => {
            let target_url = format!(
                "https://app.u.nf.migu.cn/pc/v1.0/content/search_all.do?text={kw}&pageNo={page}&pageSize=20&searchSwitch={{%22songlist%22:+1}}"
            );
            let data: Value = http.get_json(&target_url).await?;
            let res_data = &data["songListResultData"];
            let playlists: Vec<PlaylistInfo> = res_data["result"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|item| {
                            let id = val_str(&item["id"]);
                            let cover_img_url = item["imgItems"]
                                .get(0)
                                .map(|v| val_str(&v["img"]))
                                .unwrap_or_default();
                            PlaylistInfo {
                                id: format!("mgplaylist_{id}"),
                                title: val_str(&item["name"]),
                                source_url: format!("https://music.migu.cn/v3/music/playlist/{id}"),
                                cover_img_url,
                                author: val_str(&item["userName"]),
                                count: val_u64(&item["musicNum"]),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            let total = val_u64(&res_data["totalCount"]).unwrap_or(0);
            Ok(SearchResult {
                tracks: Vec::new(),
                playlists,
                total,
            })
        }
    }
}

/// 对应原版 `mg_generate_translation`
fn generate_translation(plain: &str, translation: &str) -> Lyric {
    if translation.is_empty() {
        return Lyric {
            lyric: plain.to_string(),
            tlyric: String::new(),
        };
    }

    let arr_plain: Vec<&str> = plain.split('\n').collect();
    let mut arr_translation: Vec<&str> = translation.split('\n').collect();

    // 歌词和翻译顶部信息不一定都有，会导致行列对不齐，所以删掉
    let reg_head = Regex::new(r"\[(ti|ar|al|by|offset|kana|high):").ok();
    let mut plain_head_line = 0;
    let mut trans_head_line = 0;
    for i in 0..7 {
        if let Some(line) = arr_plain.get(i) {
            if reg_head.as_ref().map(|r| r.is_match(line)).unwrap_or(false) {
                plain_head_line += 1;
            }
        }
        if let Some(line) = arr_translation.get(i) {
            if reg_head.as_ref().map(|r| r.is_match(line)).unwrap_or(false) {
                trans_head_line += 1;
            }
        }
    }
    let _ = plain_head_line; // 原版 arr_plain.splice(0, plain_head_line) 后仍返回未修改的 plain 字符串
    if trans_head_line <= arr_translation.len() {
        arr_translation.drain(0..trans_head_line);
    }

    // 删除翻译与原歌词重复的歌曲名，歌手、作曲、作词等信息
    let reg_info = Regex::new(r"(作|编)(词|曲)|歌(手|曲)名|Written by").ok();
    let mut trans_info_line = 0;
    for i in 0..6 {
        if let Some(line) = arr_translation.get(i) {
            if reg_info.as_ref().map(|r| r.is_match(line)).unwrap_or(false) {
                trans_info_line += 1;
            }
        }
    }
    if trans_info_line <= arr_translation.len() {
        arr_translation.drain(0..trans_info_line);
    }

    let tlrc = arr_translation.join("\r\n");
    Lyric {
        lyric: plain.to_string(),
        tlyric: tlrc,
    }
}

pub async fn lyric(http: &Http, track: &Track) -> Result<Lyric> {
    let lyric_url = track.extra_str("lyric_url").unwrap_or_default();
    let tlyric_url = track.extra_str("tlyric_url").unwrap_or_default();

    if lyric_url != "null" {
        let lrc_fut = async {
            if !lyric_url.is_empty() {
                http.get_text(lyric_url)
                    .await
                    .unwrap_or_else(|_| "[00:00.00]暂无歌词\r\n[00:02.00]\r\n".to_string())
            } else {
                "[00:00.00]暂无歌词\r\n[00:02.00]\r\n".to_string()
            }
        };
        let tlrc_fut = async {
            if !tlyric_url.is_empty() {
                http.get_text(tlyric_url).await.unwrap_or_default()
            } else {
                String::new()
            }
        };
        let (plain, trans) = tokio::join!(lrc_fut, tlrc_fut);
        Ok(generate_translation(&plain, &trans))
    } else {
        let song_id = track.id.strip_prefix("mgtrack_").unwrap_or(&track.id);
        let target_url =
            format!("https://music.migu.cn/v3/api/music/audioPlayer/getLyric?copyrightId={song_id}");
        if let Ok(resp) = http.get_json::<Value>(&target_url).await {
            let plain = val_str(&resp["lyric"]);
            let trans = val_str(&resp["translatedLyric"]);
            if !plain.is_empty() {
                return Ok(generate_translation(&plain, &trans));
            }
        }
        // 回退：若 web 接口 getLyric 已返回 SPA HTML，则通过移动端 resourceinfo.do 查询 lrcUrl/trcUrl
        let info_url = format!(
            "https://app.c.nf.migu.cn/MIGUM2.0/v1.0/content/resourceinfo.do?copyrightId={song_id}&resourceType=2"
        );
        if let Ok(resp) = http.get_json::<Value>(&info_url).await {
            let res0 = &resp["resource"][0];
            let lrc_u = val_str(&res0["lrcUrl"]);
            let trc_u = val_str(&res0["trcUrl"]);
            let plain = if !lrc_u.is_empty() {
                http.get_text(&lrc_u)
                    .await
                    .unwrap_or_else(|_| "[00:00.00]暂无歌词\r\n[00:02.00]\r\n".to_string())
            } else {
                "[00:00.00]暂无歌词\r\n[00:02.00]\r\n".to_string()
            };
            let trans = if !trc_u.is_empty() {
                http.get_text(&trc_u).await.unwrap_or_default()
            } else {
                String::new()
            };
            return Ok(generate_translation(&plain, &trans));
        }
        Ok(Lyric::default())
    }
}

pub async fn parse_url(_http: &Http, url: &str) -> Option<String> {
    let normalized = url.replace(
        "music.migu.cn/v3/my/playlist/",
        "music.migu.cn/v3/music/playlist/",
    );
    let re = Regex::new(r"//music\.migu\.cn/v3/music/playlist/([0-9]+)").ok()?;
    let caps = re.captures(&normalized)?;
    let id = caps.get(1)?.as_str();
    Some(format!("mgplaylist_{id}"))
}
