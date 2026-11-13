use crate::http::{urlencode, Http};
use crate::models::*;
use anyhow::{anyhow, bail, Result};
use futures_util::future::join_all;
use regex_lite::Regex;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PREFIX: &str = "kg";

fn val_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn val_u64(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn is_status_ok(v: &Value) -> bool {
    v.as_i64().map(|n| n != 0).unwrap_or(false) || v.as_bool().unwrap_or(false)
}

fn convert_song(song: &Value) -> Track {
    let file_hash = val_str(&song["FileHash"]);
    let album_id = val_str(&song["AlbumID"]);

    let (singer_id, singer_name) = if let Some(arr) = song["SingerId"].as_array() {
        let id = arr.first().map(val_str).unwrap_or_default();
        let raw_name = val_str(&song["SingerName"]);
        let name = raw_name.split('、').next().unwrap_or_default().to_string();
        (id, name)
    } else {
        (val_str(&song["SingerId"]), val_str(&song["SingerName"]))
    };

    let mut track = Track {
        id: format!("kgtrack_{file_hash}"),
        title: val_str(&song["SongName"]),
        artist: singer_name,
        artist_id: format!("kgartist_{singer_id}"),
        album: val_str(&song["AlbumName"]),
        album_id: format!("kgalbum_{album_id}"),
        source: "kugou".to_string(),
        source_url: format!("https://www.kugou.com/song/#hash={file_hash}&album_id={album_id}"),
        img_url: String::new(),
        ..Default::default()
    };
    track.set_extra("lyric_url", file_hash);
    track
}

async fn render_search_result_item(http: &Http, item: &Value) -> Track {
    let mut track = convert_song(item);
    let lyric_url = track.extra_str("lyric_url").unwrap_or_default().to_string();
    if !lyric_url.is_empty() {
        let url = format!("https://www.kugou.com/yy/index.php?r=play/getdata&hash={lyric_url}");
        if let Ok(res) = http.get_json::<Value>(&url).await {
            track.img_url = val_str(&res["data"]["img"]);
        }
    }
    track
}

pub async fn search(
    http: &Http,
    keyword: &str,
    page: u32,
    kind: SearchType,
) -> Result<SearchResult> {
    let encoded_kw = urlencode(keyword);
    match kind {
        SearchType::Playlist => {
            let target_url = format!(
                "http://mobilecdnbj.kugou.com/api/v3/search/special?keyword={encoded_kw}&pagesize=20&filter=0&page={page}"
            );
            let Ok(data) = http.get_json::<Value>(&target_url).await else {
                return Ok(SearchResult::default());
            };
            let playlists = data["data"]["info"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .map(|item| {
                            let specialid = val_str(&item["specialid"]);
                            let imgurl = val_str(&item["imgurl"]);
                            PlaylistInfo {
                                id: format!("kgplaylist_{specialid}"),
                                title: val_str(&item["specialname"]),
                                cover_img_url: if imgurl.is_empty() {
                                    String::new()
                                } else {
                                    imgurl.replace("{size}", "400")
                                },
                                source_url: format!(
                                    "https://www.kugou.com/yy/special/single/{specialid}.html"
                                ),
                                author: val_str(&item["nickname"]),
                                count: val_u64(&item["songcount"]),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            let total = val_u64(&data["data"]["total"]).unwrap_or(0);
            Ok(SearchResult {
                playlists,
                total,
                ..Default::default()
            })
        }
        SearchType::Track => {
            let target_url = format!(
                "https://songsearch.kugou.com/song_search_v2?keyword={encoded_kw}&page={page}"
            );
            let Ok(data) = http.get_json::<Value>(&target_url).await else {
                return Ok(SearchResult::default());
            };
            let tracks = if let Some(lists) = data["data"]["lists"].as_array() {
                let futs = lists.iter().map(|item| render_search_result_item(http, item));
                join_all(futs).await
            } else {
                Vec::new()
            };
            let total = val_u64(&data["data"]["total"]).unwrap_or(0);
            Ok(SearchResult {
                tracks,
                total,
                ..Default::default()
            })
        }
    }
}

async fn render_playlist_result_item(http: &Http, item: &Value) -> Track {
    let hash = val_str(&item["hash"]);
    let album_id = val_str(&item["album_id"]);

    let mut track = Track {
        id: format!("kgtrack_{hash}"),
        title: String::new(),
        artist: String::new(),
        artist_id: String::new(),
        album: String::new(),
        album_id: format!("kgalbum_{album_id}"),
        source: "kugou".to_string(),
        source_url: format!("https://www.kugou.com/song/#hash={hash}&album_id={album_id}"),
        img_url: String::new(),
        ..Default::default()
    };
    track.set_extra("lyric_url", hash.clone());

    let song_info_url =
        format!("https://m.kugou.com/app/i/getSongInfo.php?cmd=playInfo&hash={hash}");
    if let Ok(data) = http.get_json::<Value>(&song_info_url).await {
        track.title = val_str(&data["songName"]);
        let singer_id = val_str(&data["singerId"]);
        track.artist = if singer_id == "0" || singer_id.is_empty() {
            "未知".to_string()
        } else {
            val_str(&data["singerName"])
        };
        track.artist_id = format!("kgartist_{singer_id}");
        if let Some(album_img) = data.get("album_img").and_then(|v| v.as_str()) {
            track.img_url = album_img.replace("{size}", "400");
        }
    }

    let album_info_url =
        format!("http://mobilecdnbj.kugou.com/api/v3/album/info?albumid={album_id}");
    if let Ok(res_data) = http.get_json::<Value>(&album_info_url).await {
        if is_status_ok(&res_data["status"]) && !res_data["data"].is_null() {
            track.album = val_str(&res_data["data"]["albumname"]);
        }
    }

    track
}

async fn kg_get_playlist(http: &Http, raw_id: &str) -> Result<Playlist> {
    let target_url = format!("https://m.kugou.com/plist/list/{raw_id}?json=true");
    let data: Value = http.get_json(&target_url).await?;

    let list_info = &data["info"]["list"];
    let specialid = {
        let s = val_str(&list_info["specialid"]);
        if s.is_empty() {
            raw_id.to_string()
        } else {
            s
        }
    };
    let imgurl = val_str(&list_info["imgurl"]);
    let info = PlaylistInfo {
        cover_img_url: if imgurl.is_empty() {
            String::new()
        } else {
            imgurl.replace("{size}", "400")
        },
        title: val_str(&list_info["specialname"]),
        id: format!("kgplaylist_{specialid}"),
        source_url: format!("https://www.kugou.com/yy/special/single/{specialid}.html"),
        ..Default::default()
    };

    let tracks = if let Some(arr) = data["list"]["list"]["info"].as_array() {
        let futs = arr.iter().map(|item| render_playlist_result_item(http, item));
        join_all(futs).await
    } else {
        Vec::new()
    };

    Ok(Playlist { info, tracks })
}

async fn render_album_result_item(
    http: &Http,
    item: &Value,
    album_title: &str,
    album_id: &str,
) -> Track {
    let hash = val_str(&item["hash"]);
    let mut track = Track {
        id: format!("kgtrack_{hash}"),
        title: String::new(),
        artist: String::new(),
        artist_id: String::new(),
        album: album_title.to_string(),
        album_id: format!("kgalbum_{album_id}"),
        source: "kugou".to_string(),
        source_url: format!("https://www.kugou.com/song/#hash={hash}&album_id={album_id}"),
        img_url: String::new(),
        ..Default::default()
    };
    track.set_extra("lyric_url", hash.clone());

    let target_url = format!("https://m.kugou.com/app/i/getSongInfo.php?cmd=playInfo&hash={hash}");
    if let Ok(data) = http.get_json::<Value>(&target_url).await {
        track.title = val_str(&data["songName"]);
        let singer_id = val_str(&data["singerId"]);
        track.artist = if singer_id == "0" || singer_id.is_empty() {
            "未知".to_string()
        } else {
            val_str(&data["singerName"])
        };
        track.artist_id = format!("kgartist_{singer_id}");
        track.img_url = val_str(&data["imgUrl"]).replace("{size}", "400");
    }

    track
}

async fn kg_album(http: &Http, album_id: &str) -> Result<Playlist> {
    let info_url = format!("http://mobilecdnbj.kugou.com/api/v3/album/info?albumid={album_id}");
    let data: Value = http.get_json(&info_url).await?;

    let albumid = {
        let s = val_str(&data["data"]["albumid"]);
        if s.is_empty() {
            album_id.to_string()
        } else {
            s
        }
    };
    let info = PlaylistInfo {
        cover_img_url: val_str(&data["data"]["imgurl"]).replace("{size}", "400"),
        title: val_str(&data["data"]["albumname"]),
        id: format!("kgalbum_{albumid}"),
        source_url: format!("https://www.kugou.com/album/{albumid}.html"),
        ..Default::default()
    };

    let song_url = format!(
        "http://mobilecdnbj.kugou.com/api/v3/album/song?albumid={album_id}&page=1&pagesize=-1"
    );
    let res: Value = http.get_json(&song_url).await?;

    let tracks = if let Some(arr) = res["data"]["info"].as_array() {
        let futs = arr
            .iter()
            .map(|item| render_album_result_item(http, item, &info.title, album_id));
        join_all(futs).await
    } else {
        Vec::new()
    };

    Ok(Playlist { info, tracks })
}

async fn render_artist_result_item(http: &Http, item: &Value, artist_id_full: &str) -> Track {
    let hash = val_str(&item["hash"]);
    let album_id = val_str(&item["album_id"]);
    let filename = val_str(&item["filename"]);

    let (artist, title) = match filename.split_once('-') {
        Some((a, t)) => (a.trim().to_string(), t.trim().to_string()),
        None => (String::new(), filename.trim().to_string()),
    };

    let mut track = Track {
        id: format!("kgtrack_{hash}"),
        title,
        artist,
        artist_id: artist_id_full.to_string(),
        album: String::new(),
        album_id: format!("kgalbum_{album_id}"),
        source: "kugou".to_string(),
        source_url: format!("https://www.kugou.com/song/#hash={hash}&album_id={album_id}"),
        img_url: String::new(),
        ..Default::default()
    };
    track.set_extra("lyric_url", hash.clone());

    let album_url = format!("http://mobilecdnbj.kugou.com/api/v3/album/info?albumid={album_id}");
    if let Ok(data) = http.get_json::<Value>(&album_url).await {
        if is_status_ok(&data["status"]) && !data["data"].is_null() {
            track.album = val_str(&data["data"]["albumname"]);
        }
    }

    let play_url = format!("https://www.kugou.com/yy/index.php?r=play/getdata&hash={hash}");
    if let Ok(res) = http.get_json::<Value>(&play_url).await {
        track.img_url = val_str(&res["data"]["img"]);
    }

    track
}

async fn kg_artist(http: &Http, artist_id: &str) -> Result<Playlist> {
    let info_url = format!("http://mobilecdnbj.kugou.com/api/v3/singer/info?singerid={artist_id}");
    let data: Value = http.get_json(&info_url).await?;

    let info = PlaylistInfo {
        cover_img_url: val_str(&data["data"]["imgurl"]).replace("{size}", "400"),
        title: val_str(&data["data"]["singername"]),
        id: format!("kgartist_{artist_id}"),
        source_url: format!("https://www.kugou.com/singer/{artist_id}.html"),
        ..Default::default()
    };

    let song_url = format!(
        "http://mobilecdnbj.kugou.com/api/v3/singer/song?singerid={artist_id}&page=1&pagesize=30"
    );
    let res: Value = http.get_json(&song_url).await?;

    let tracks = if let Some(arr) = res["data"]["info"].as_array() {
        let futs = arr
            .iter()
            .map(|item| render_artist_result_item(http, item, &info.id));
        join_all(futs).await
    } else {
        Vec::new()
    };

    Ok(Playlist { info, tracks })
}

pub async fn get_playlist(http: &Http, list_id: &str) -> Result<Playlist> {
    let (prefix, raw_id) = list_id
        .split_once('_')
        .ok_or_else(|| anyhow!("无效的酷狗歌单 id: {list_id}"))?;
    match prefix {
        "kgplaylist" => kg_get_playlist(http, raw_id).await,
        "kgalbum" => kg_album(http, raw_id).await,
        "kgartist" => kg_artist(http, raw_id).await,
        _ => bail!("未知的酷狗歌单类型: {list_id}"),
    }
}

pub async fn show_playlist(http: &Http, offset: u32, _filter_id: &str) -> Result<Vec<PlaylistInfo>> {
    let page = offset / 30 + 1;
    let target_url = format!("https://m.kugou.com/plist/index&json=true&page={page}");
    let data: Value = http.get_json(&target_url).await?;

    let result = data["plist"]["list"]["info"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|item| {
                    let specialid = val_str(&item["specialid"]);
                    let imgurl = val_str(&item["imgurl"]);
                    PlaylistInfo {
                        cover_img_url: if imgurl.is_empty() {
                            String::new()
                        } else {
                            imgurl.replace("{size}", "400")
                        },
                        title: val_str(&item["specialname"]),
                        id: format!("kgplaylist_{specialid}"),
                        source_url: format!(
                            "https://www.kugou.com/yy/special/single/{specialid}.html"
                        ),
                        ..Default::default()
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(result)
}

pub fn playlist_filters() -> Filters {
    Filters::default()
}

pub async fn bootstrap_track(http: &Http, track: &Track) -> Result<Sound> {
    let track_id = track.id.strip_prefix("kgtrack_").unwrap_or(&track.id);
    let target_url =
        format!("https://m.kugou.com/app/i/getSongInfo.php?cmd=playInfo&hash={track_id}");
    let info: Value = http.get_json(&target_url).await?;

    let url = val_str(&info["url"]);
    if url.is_empty() {
        bail!("酷狗无法获取音频地址: {}", track.id);
    }

    let bit_rate = val_str(&info["bitRate"]);
    let bitrate = if bit_rate.is_empty() {
        String::new()
    } else {
        format!("{bit_rate}kbps")
    };

    Ok(Sound {
        url,
        bitrate,
        platform: "kugou".to_string(),
    })
}

pub async fn lyric(http: &Http, track: &Track) -> Result<Lyric> {
    let track_id = track
        .extra_str("lyric_url")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| track.id.strip_prefix("kgtrack_").unwrap_or(&track.id));
    let album_id = track
        .album_id
        .strip_prefix("kgalbum_")
        .unwrap_or(&track.album_id);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let lyric_url = format!(
        "https://wwwapi.kugou.com/yy/index.php?r=play/getdata&callback=jQuery&mid=1&hash={track_id}&platid=4&album_id={album_id}&_={timestamp}"
    );
    let info: Value = http.get_json(&lyric_url).await?;

    Ok(Lyric {
        lyric: val_str(&info["data"]["lyrics"]),
        tlyric: String::new(),
    })
}

pub async fn parse_url(_http: &Http, url: &str) -> Option<String> {
    let re = Regex::new(r"//www\.kugou\.com/yy/special/single/([0-9]+)\.html").ok()?;
    let caps = re.captures(url)?;
    let playlist_id = caps.get(1)?.as_str();
    Some(format!("kgplaylist_{playlist_id}"))
}
