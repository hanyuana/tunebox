//! 本地音频与图片流代理服务（监听 127.0.0.1 随机空闲端口）
//!
//! 解决三个问题：
//! 1. B站 (`*.bilivideo.com`)、网易、QQ 等音频 CDN 校验 `Referer` / `User-Agent`，WebView `<audio>` 直接请求会 403；
//! 2. YouTube 音频 (`*.googlevideo.com`) 与缩略图 (`i.ytimg.com`) 需要走海外代理（梯子），而 WebView 本身不需全局挂代理；
//! 3. 完整透传 `Range` / `Content-Range` / `Accept-Ranges` / `Content-Length`，支持拖动进度条。
use crate::http::Http;
use axum::{
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct StreamQuery {
    pub url: String,
}

const YT_BRIDGE_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8" />
<meta name="referrer" content="strict-origin-when-cross-origin" />
<style>html,body,#player{margin:0;padding:0;width:100%;height:100%;background:#000;overflow:hidden}</style>
</head>
<body>
<div id="player"></div>
<script>
let player = null;
let ready = false;
let pendingVid = null;
let timer = null;

function notify(msg) {
  window.parent.postMessage(msg, "*");
}

window.onYouTubeIframeAPIReady = function() {
  player = new YT.Player('player', {
    width: '200',
    height: '200',
    playerVars: { autoplay: 1, controls: 0, rel: 0, playsinline: 1 },
    events: {
      onReady: function() {
        ready = true;
        if (pendingVid) {
          player.loadVideoById(pendingVid);
          pendingVid = null;
        }
      },
      onStateChange: function(e) {
        notify({ type: 'yt-state', state: e.data });
      },
      onError: function(e) {
        notify({ type: 'yt-error', code: e.data });
      }
    }
  });
  clearInterval(timer);
  timer = setInterval(function() {
    if (ready && player && typeof player.getCurrentTime === 'function') {
      const st = player.getPlayerState ? player.getPlayerState() : -1;
      if (st === 1 || st === 2) {
        notify({
          type: 'yt-time',
          currentTime: player.getCurrentTime() || 0,
          duration: player.getDuration() || 0
        });
      }
    }
  }, 400);
};

window.addEventListener('message', function(ev) {
  const d = ev.data || {};
  if (d.cmd === 'load') {
    if (ready && player && player.loadVideoById) {
      player.loadVideoById(d.vid);
    } else {
      pendingVid = d.vid;
    }
  } else if (d.cmd === 'play' && ready && player) {
    player.playVideo();
  } else if (d.cmd === 'pause' && ready && player) {
    player.pauseVideo();
  } else if (d.cmd === 'stop' && ready && player) {
    player.stopVideo();
  } else if (d.cmd === 'seek' && ready && player) {
    player.seekTo(d.time || 0, true);
  } else if (d.cmd === 'volume' && ready && player) {
    player.setVolume(d.val ?? 80);
  }
});

const tag = document.createElement('script');
tag.src = "https://www.youtube.com/iframe_api";
document.head.appendChild(tag);
</script>
</body>
</html>"#;

pub async fn spawn_stream_server(http: Arc<Http>) -> u16 {
    let app = Router::new()
        .route("/stream", get(handle_stream))
        .route("/img", get(handle_stream))
        .route("/yt_bridge", get(|| async { Html(YT_BRIDGE_HTML) }))
        .with_state(http);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地流代理端口失败");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    port
}

async fn handle_stream(
    State(http): State<Arc<Http>>,
    Query(q): Query<StreamQuery>,
    req_headers: HeaderMap,
) -> Result<Response, StatusCode> {
    if !q.url.starts_with("http://") && !q.url.starts_with("https://") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut rb = http.stream_request(&q.url);
    if let Some(range) = req_headers.get(axum::http::header::RANGE) {
        rb = rb.header(reqwest::header::RANGE, range.as_bytes());
    }

    let upstream = rb.send().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::OK);

    let mut builder = Response::builder()
        .status(status)
        .header("Access-Control-Allow-Origin", "*");

    for name in [
        "content-length",
        "content-range",
        "accept-ranges",
        "cache-control",
    ] {
        if let Some(val) = upstream.headers().get(name) {
            if let Ok(hv) = HeaderValue::from_bytes(val.as_bytes()) {
                builder = builder.header(name, hv);
            }
        }
    }

    // 修正常见 CDN 返回 application/octet-stream 或 video/mp4 导致 WebView <audio> 拒绝解码的问题
    let raw_ct = upstream
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let fixed_ct = if raw_ct.is_empty()
        || raw_ct.contains("octet-stream")
        || raw_ct.starts_with("video/mp4")
    {
        if q.url.contains(".mp3") {
            "audio/mpeg"
        } else if q.url.contains(".flac") {
            "audio/flac"
        } else {
            "audio/mp4"
        }
    } else {
        raw_ct
    };
    if let Ok(hv) = HeaderValue::from_str(fixed_ct) {
        builder = builder.header("content-type", hv);
    }

    let stream = upstream.bytes_stream();
    let body = Body::from_stream(stream);
    builder.body(body).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
