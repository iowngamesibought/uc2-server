//! Live-data HTTP server. Answers for any Host, so DNS can point the client's live-data host names
//! here (see the README).
//!
//! Files generated from the user's game data (the net config module, the networking texts) are held
//! in memory as [`Asset`]s and never written to disk. An optional static directory serves extra files
//! the operator adds (e.g. languages/english/motd.jpg); generated paths take precedence over it.
//!
//! Last-Modified: the client compares it for equality with its cached copy's mtime to decide whether
//! to download again. An asset's date is therefore derived from its content ([`content_date`]):
//! stable across restarts while the bytes are unchanged, different when they change.
//!
//! Header names are sent in Title-Case, as the original servers did: the client looks the header up with a
//! case-sensitive "Last-Modified". With hyper's default lowercase names it read time 0, stamped its
//! cached copy with mtime 0, and then treated that copy as current on every later start, so changes
//! to the served file never arrived.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use md5::{Digest, Md5};
use tracing::{debug, info, warn};

/// A file served from memory.
#[derive(Clone, Debug)]
pub struct Asset {
    bytes: Bytes,
    last_modified: SystemTime,
}

impl Asset {
    pub fn new(bytes: Vec<u8>) -> Self {
        let last_modified = content_date(&bytes);
        Self { bytes: Bytes::from(bytes), last_modified }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

/// Generated files by normalized request path ("build/main/text1/2.networking.crypt").
pub type Assets = HashMap<String, Asset>;

/// A deterministic date for some bytes: 2001-01-01 plus an MD5-derived offset of up to ~20 years,
/// at whole seconds (HTTP dates have 1 s resolution).
pub fn content_date(bytes: &[u8]) -> SystemTime {
    const START: u64 = 978_307_200; // 2001-01-01T00:00:00Z
    const SPAN: u64 = 20 * 365 * 86_400;
    let digest = Md5::digest(bytes);
    let n = u64::from_be_bytes(digest[..8].try_into().unwrap());
    UNIX_EPOCH + Duration::from_secs(START + n % SPAN)
}

#[derive(Clone)]
struct Ctx {
    assets: Arc<Assets>,
    static_dir: Option<Arc<PathBuf>>,
}

pub async fn serve(bind: std::net::SocketAddr, assets: Assets, static_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!(%bind, generated = assets.len(), static_dir = ?static_dir, "live-data HTTP listening");
    serve_on(listener, assets, static_dir).await
}

async fn serve_on(listener: tokio::net::TcpListener, assets: Assets, static_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let ctx = Ctx { assets: Arc::new(assets), static_dir: static_dir.map(Arc::new) };
    let app = Router::new().fallback(handle).with_state(ctx);
    loop {
        let (stream, peer) = listener.accept().await?;
        let service = hyper_util::service::TowerToHyperService::new(app.clone());
        tokio::spawn(async move {
            let conn = hyper::server::conn::http1::Builder::new()
                .title_case_headers(true)
                .serve_connection(hyper_util::rt::TokioIo::new(stream), service);
            if let Err(e) = conn.await {
                debug!(%peer, error = %e, "http connection ended with an error");
            }
        });
    }
}

/// Normalizes a request path to an asset key: drops empty and "." segments (the older build's URL
/// contains a literal "/./"), rejects "..", backslashes and drive letters. None for the root or a
/// rejected path.
pub fn normalize(uri_path: &str) -> Option<String> {
    let mut segs = Vec::new();
    for seg in uri_path.split('/') {
        match seg {
            "" | "." => {}
            ".." => return None,
            s if s.contains('\\') || s.contains(':') => return None,
            s => segs.push(s),
        }
    }
    (!segs.is_empty()).then(|| segs.join("/"))
}

async fn handle(State(ctx): State<Ctx>, req: Request) -> Response {
    let method = req.method().clone();
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("-").to_string();
    let path = req.uri().path().to_string();
    let response = respond(&ctx, &method, &path, req.headers()).await;
    info!(%method, %host, %path, status = response.status().as_u16(), "http");
    response
}

async fn respond(ctx: &Ctx, method: &Method, path: &str, headers: &HeaderMap) -> Response {
    if method != Method::GET && method != Method::HEAD {
        // TODO(stats queue): match-report POSTs would be routed from here; see config::STATS_QUEUE.
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let Some(key) = normalize(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if let Some(asset) = ctx.assets.get(&key) {
        return reply(method, headers, asset.bytes.clone(), Some(asset.last_modified));
    }
    let Some(dir) = &ctx.static_dir else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let file = key.split('/').fold(dir.to_path_buf(), |p, s| p.join(s));
    match tokio::fs::metadata(&file).await {
        Ok(m) if m.is_file() => match tokio::fs::read(&file).await {
            Ok(bytes) => reply(method, headers, Bytes::from(bytes), m.modified().ok()),
            Err(e) => {
                warn!(file = %file.display(), error = %e, "read failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        },
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn reply(method: &Method, headers: &HeaderMap, bytes: Bytes, modified: Option<SystemTime>) -> Response {
    let last_modified = modified.map(httpdate::fmt_http_date);
    if let (Some(m), Some(ims)) = (modified, headers.get(header::IF_MODIFIED_SINCE)) {
        if let Some(since) = ims.to_str().ok().and_then(|s| httpdate::parse_http_date(s).ok()) {
            // HTTP dates have 1 s resolution: compare at that resolution.
            let secs = |t: SystemTime| t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            if secs(m) <= secs(since) {
                return with_lm(StatusCode::NOT_MODIFIED.into_response(), &last_modified);
            }
        }
    }
    let len = bytes.len();
    let mut resp = Response::new(if method == Method::HEAD { Body::empty() } else { Body::from(bytes) });
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    with_lm(resp, &last_modified)
}

fn with_lm(mut resp: Response, lm: &Option<String>) -> Response {
    if let Some(v) = lm.as_deref().and_then(|s| HeaderValue::from_str(s).ok()) {
        resp.headers_mut().insert(header::LAST_MODIFIED, v);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn get(assets: Assets, static_dir: Option<PathBuf>, path: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on(listener, assets, static_dir));
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut resp = Vec::new();
        s.read_to_end(&mut resp).await.unwrap();
        String::from_utf8_lossy(&resp).into_owned()
    }

    /// Generated assets come from memory, with "Last-Modified" spelled exactly as the client expects (see
    /// module docs) and dated from their content.
    #[tokio::test]
    async fn serves_generated_assets_with_title_case_last_modified() {
        let asset = Asset::new(b"abc".to_vec());
        let date = httpdate::fmt_http_date(asset.last_modified);
        let text = get(Assets::from([("a/f.crypt".to_string(), asset)]), None, "/./a/f.crypt").await;
        assert!(text.starts_with("HTTP/1.1 200"), "{text}");
        assert!(text.contains(&format!("\r\nLast-Modified: {date}\r\n")), "{text}");
        assert!(text.contains("\r\nContent-Length: 3\r\n"), "{text}");
        assert!(text.ends_with("abc"), "{text}");
    }

    /// Paths that are not generated fall back to the optional static directory; generated ones win.
    #[tokio::test]
    async fn static_dir_is_a_fallback() {
        let dir = std::env::temp_dir().join(format!("uc2-http-static-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("languages/english")).unwrap();
        std::fs::write(dir.join("languages/english/motd.jpg"), b"jpg").unwrap();
        std::fs::write(dir.join("net.crypt"), b"stale file").unwrap();
        let assets = Assets::from([("net.crypt".to_string(), Asset::new(b"fresh".to_vec()))]);
        let motd = get(assets.clone(), Some(dir.clone()), "/languages/english/motd.jpg").await;
        let net = get(assets.clone(), Some(dir.clone()), "/net.crypt").await;
        let missing = get(assets, None, "/languages/english/motd.jpg").await;
        std::fs::remove_dir_all(&dir).ok();
        assert!(motd.starts_with("HTTP/1.1 200") && motd.ends_with("jpg"), "{motd}");
        assert!(net.ends_with("fresh"), "{net}");
        assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
    }

    #[test]
    fn content_date_is_stable_per_content() {
        assert_eq!(content_date(b"same"), content_date(b"same"));
        assert_ne!(content_date(b"one"), content_date(b"two"));
        let secs = content_date(b"x").duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert!(secs >= 978_307_200, "dates are never near 0, which the client's cache check treats specially");
    }

    #[test]
    fn normalizes_game_paths() {
        assert_eq!(normalize("/./net.bin.psarc.crypt").as_deref(), Some("net.bin.psarc.crypt"));
        assert_eq!(normalize("/net-patch9.bin.psarc.crypt").as_deref(), Some("net-patch9.bin.psarc.crypt"));
        assert_eq!(normalize("/a/./b.jpg").as_deref(), Some("a/b.jpg"));
        assert_eq!(normalize("/../etc/passwd"), None);
        assert_eq!(normalize("/"), None);
        assert_eq!(normalize("/c:/x"), None);
    }
}
