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
    response::Response,
    routing::get,
    Router,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct StreamQuery {
    pub url: String,
}

pub async fn spawn_stream_server(http: Arc<Http>) -> u16 {
    let app = Router::new()
        .route("/stream", get(handle_stream))
        .route("/img", get(handle_stream))
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
        "content-type",
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

    let stream = upstream.bytes_stream();
    let body = Body::from_stream(stream);
    builder.body(body).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
