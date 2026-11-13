//! HTTP 客户端封装。
//!
//! 原版 Electron 通过 `webRequest.onBeforeSendHeaders` 给各平台请求统一改写 Referer / Origin / UA，
//! 这里在 Rust 侧发请求时按 URL 自动注入同样的请求头（见 [`apply_platform_headers`]），
//! 所以各个 provider 只需像原版 axios 那样直接发请求即可。
//!
//! 国内平台与海外平台（YouTube）使用两个独立的客户端，可以分别配置代理。
use crate::settings::ProxyMode;
use reqwest::header::{HeaderMap, HeaderValue, ORIGIN, REFERER, USER_AGENT};
use reqwest::{cookie::Jar, Client, Method, RequestBuilder};
use serde::de::DeserializeOwned;
use std::sync::{Arc, RwLock};
use std::time::Duration;

pub const DESKTOP_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
pub const MOBILE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 14_3 like Mac OS X) AppleWebKit/534.30 (KHTML, like Gecko) Version/4.0 Mobile Safari/534.30";

/// 走「海外代理」的域名
const OVERSEAS_HOSTS: &[&str] = &[
    "youtube.com",
    "youtu.be",
    "googlevideo.com",
    "ytimg.com",
    "ggpht.com",
    "googleapis.com",
    "google.com",
];

pub struct Http {
    pub jar: Arc<Jar>,
    domestic: RwLock<Client>,
    overseas: RwLock<Client>,
    stream_domestic: RwLock<Client>,
    stream_overseas: RwLock<Client>,
}

fn build_client(jar: Arc<Jar>, proxy: &ProxyMode) -> Client {
    let mut b = Client::builder()
        .cookie_provider(jar)
        .user_agent(DESKTOP_UA)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(60));
    match proxy {
        ProxyMode::System => {}
        ProxyMode::Direct => b = b.no_proxy(),
        ProxyMode::Custom(url) => match reqwest::Proxy::all(url.trim()) {
            Ok(p) => b = b.proxy(p),
            Err(e) => eprintln!("无效的代理地址 {url}: {e}"),
        },
    }
    b.build().unwrap_or_else(|_| Client::new())
}

/// 用于音频流的客户端不设置整体超时（长音频需要持续下载）
fn build_stream_client(jar: Arc<Jar>, proxy: &ProxyMode) -> Client {
    let mut b = Client::builder()
        .cookie_provider(jar)
        .user_agent(DESKTOP_UA)
        .connect_timeout(Duration::from_secs(15));
    match proxy {
        ProxyMode::System => {}
        ProxyMode::Direct => b = b.no_proxy(),
        ProxyMode::Custom(url) => {
            if let Ok(p) = reqwest::Proxy::all(url.trim()) {
                b = b.proxy(p)
            }
        }
    }
    b.build().unwrap_or_else(|_| Client::new())
}

pub fn is_overseas(url: &str) -> bool {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
        .unwrap_or_default();
    OVERSEAS_HOSTS
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

impl Http {
    pub fn new(domestic: &ProxyMode, overseas: &ProxyMode) -> Self {
        let jar = Arc::new(Jar::default());
        Self {
            domestic: RwLock::new(build_client(jar.clone(), domestic)),
            overseas: RwLock::new(build_client(jar.clone(), overseas)),
            stream_domestic: RwLock::new(build_stream_client(jar.clone(), domestic)),
            stream_overseas: RwLock::new(build_stream_client(jar.clone(), overseas)),
            jar,
        }
    }

    /// 修改代理设置后重建客户端（cookie 保留）
    pub fn reconfigure(&self, domestic: &ProxyMode, overseas: &ProxyMode) {
        *self.domestic.write().unwrap() = build_client(self.jar.clone(), domestic);
        *self.overseas.write().unwrap() = build_client(self.jar.clone(), overseas);
        *self.stream_domestic.write().unwrap() = build_stream_client(self.jar.clone(), domestic);
        *self.stream_overseas.write().unwrap() = build_stream_client(self.jar.clone(), overseas);
    }

    pub fn client_for(&self, url: &str) -> Client {
        if is_overseas(url) {
            self.overseas.read().unwrap().clone()
        } else {
            self.domestic.read().unwrap().clone()
        }
    }

    /// 创建请求，并按平台自动加上 Referer / Origin / UA
    pub fn request(&self, method: Method, url: &str) -> RequestBuilder {
        let rb = self.client_for(url).request(method, url);
        rb.headers(platform_headers(url))
    }

    pub fn get(&self, url: &str) -> RequestBuilder {
        self.request(Method::GET, url)
    }

    pub fn post(&self, url: &str) -> RequestBuilder {
        self.request(Method::POST, url)
    }

    /// 设置 cookie，例如 `http.set_cookie("https://music.163.com", "os=pc")`
    pub fn set_cookie(&self, url: &str, cookie: &str) {
        if let Ok(u) = url.parse::<url::Url>() {
            let domain = u.host_str().unwrap_or_default().to_string();
            let c = format!("{cookie}; Domain={domain}; Path=/");
            self.jar.add_cookie_str(&c, &u);
        }
    }

    /// 读取某个 URL 下的 cookie 值
    pub fn get_cookie(&self, url: &str, name: &str) -> Option<String> {
        use reqwest::cookie::CookieStore;
        let u: url::Url = url.parse().ok()?;
        let header = self.jar.cookies(&u)?;
        let s = header.to_str().ok()?;
        s.split(';').find_map(|kv| {
            let (k, v) = kv.trim().split_once('=')?;
            (k == name).then(|| v.to_string())
        })
    }

    pub async fn get_text(&self, url: &str) -> anyhow::Result<String> {
        Ok(self.get(url).send().await?.error_for_status()?.text().await?)
    }

    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> anyhow::Result<T> {
        let text = self.get_text(url).await?;
        parse_json(&text)
    }

    /// POST application/x-www-form-urlencoded，返回 JSON
    pub async fn post_form_json<T: DeserializeOwned>(
        &self,
        url: &str,
        form: &[(&str, String)],
    ) -> anyhow::Result<T> {
        let text = self
            .post(url)
            .form(form)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        parse_json(&text)
    }

    /// 音频流请求（无整体超时）
    pub fn stream_request(&self, url: &str) -> RequestBuilder {
        let client = if is_overseas(url) {
            self.stream_overseas.read().unwrap().clone()
        } else {
            self.stream_domestic.read().unwrap().clone()
        };
        client.get(url).headers(platform_headers(url))
    }
}

/// 解析 JSON，兼容 JSONP（`callback({...})`）以及前后有杂字符的情况
pub fn parse_json<T: DeserializeOwned>(text: &str) -> anyhow::Result<T> {
    let t = text.trim().trim_start_matches('\u{feff}');
    if let Ok(v) = serde_json::from_str(t) {
        return Ok(v);
    }
    // JSONP：截取第一个 '(' 与最后一个 ')' 之间
    if let (Some(l), Some(r)) = (t.find('('), t.rfind(')')) {
        if l < r {
            if let Ok(v) = serde_json::from_str(&t[l + 1..r]) {
                return Ok(v);
            }
        }
    }
    // 截取第一个 '{' 与最后一个 '}' 之间
    if let (Some(l), Some(r)) = (t.find('{'), t.rfind('}')) {
        if l < r {
            return Ok(serde_json::from_str(&t[l..=r])?);
        }
    }
    anyhow::bail!("无法解析 JSON: {}", &t[..t.len().min(200)])
}

/// 移植自原版 main.js 的 `hack_referer_header`
pub fn platform_headers(url: &str) -> HeaderMap {
    let mut referer = "";
    let mut origin = "";
    let mut add_origin = true;
    let mut ua = "";

    if url.contains("://music.163.com/") || url.contains("://interface3.music.163.com/") || url.contains("://interface.music.163.com/") {
        referer = "https://music.163.com/";
    }
    if url.contains("://gist.githubusercontent.com/") {
        referer = "https://gist.githubusercontent.com/";
    }
    if url.contains("c.y.qq.com/") {
        referer = "https://y.qq.com/";
        origin = "https://y.qq.com";
    }
    if url.contains("y.qq.com/") && !url.contains("c.y.qq.com/")
        || url.contains("qqmusic.qq.com/")
        || url.contains("music.qq.com/")
        || url.contains("imgcache.qq.com/")
    {
        referer = "https://y.qq.com/";
    }
    if url.contains(".kugou.com/") {
        referer = "https://www.kugou.com/";
        ua = MOBILE_UA;
    }
    if url.contains("m.kugou.com/") {
        ua = MOBILE_UA;
    }
    if url.contains(".kuwo.cn/") {
        referer = "https://www.kuwo.cn/";
    }
    if url.contains(".bilibili.com/") || url.contains(".bilivideo.com/") {
        referer = "https://www.bilibili.com/";
        add_origin = false;
    }
    if url.contains(".bilivideo.cn") {
        referer = "https://www.bilibili.com/";
        origin = "https://www.bilibili.com";
        add_origin = true;
    }
    if url.contains(".migu.cn") {
        referer = "https://music.migu.cn/v3/music/player/audio?from=migu";
    }
    if url.contains("m.music.migu.cn") {
        referer = "https://m.music.migu.cn/";
    }
    if url.contains(".googlevideo.com/") {
        ua = crate::providers::youtube::ua_for_stream(url);
    }

    let mut h = HeaderMap::new();
    if !referer.is_empty() {
        if origin.is_empty() {
            origin = referer.trim_end_matches('/');
        }
        if let Ok(v) = HeaderValue::from_str(referer) {
            h.insert(REFERER, v);
        }
        if add_origin {
            if let Ok(v) = HeaderValue::from_str(origin) {
                h.insert(ORIGIN, v);
            }
        }
    }
    if !ua.is_empty() {
        if let Ok(v) = HeaderValue::from_str(ua) {
            h.insert(USER_AGENT, v);
        }
    }
    h
}

/// 读取 URL 查询参数
pub fn query_param(url: &str, name: &str) -> Option<String> {
    let u = url::Url::parse(url)
        .or_else(|_| url::Url::parse(&url.replacen("#/", "", 1)))
        .ok()?;
    let found = u.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());
    if found.is_some() {
        return found;
    }
    // hash 路由里的参数，例如 https://music.163.com/#/playlist?id=1
    let frag = u.fragment()?;
    let q = frag.split_once('?')?.1;
    url::form_urlencoded::parse(q.as_bytes())
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

pub fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
