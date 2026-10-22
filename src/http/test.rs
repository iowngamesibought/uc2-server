use super::*;

async fn request(assets: Assets, static_dir: Option<PathBuf>, raw: String) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_on(listener, assets, static_dir));
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(raw.as_bytes()).await.unwrap();
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).await.unwrap();
    String::from_utf8_lossy(&resp).into_owned()
}

async fn get(assets: Assets, static_dir: Option<PathBuf>, path: &str) -> String {
    request(
        assets,
        static_dir,
        format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"),
    )
    .await
}

/// Generated assets come from memory, with "Last-Modified" spelled exactly as the client expects (see
/// module docs) and dated from their content.
#[tokio::test]
async fn serves_generated_assets_with_title_case_last_modified() {
    let asset = Asset::new(b"abc".to_vec());
    let date = httpdate::fmt_http_date(asset.last_modified);
    let text = get(
        Assets::from([("a/f.crypt".to_string(), asset)]),
        None,
        "/./a/f.crypt",
    )
    .await;
    assert!(text.starts_with("HTTP/1.1 200"), "{text}");
    assert!(
        text.contains(&format!("\r\nLast-Modified: {date}\r\n")),
        "{text}"
    );
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
    let motd = get(
        assets.clone(),
        Some(dir.clone()),
        "/languages/english/motd.jpg",
    )
    .await;
    let net = get(assets.clone(), Some(dir.clone()), "/net.crypt").await;
    let missing = get(assets, None, "/languages/english/motd.jpg").await;
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        motd.starts_with("HTTP/1.1 200") && motd.ends_with("jpg"),
        "{motd}"
    );
    assert!(net.ends_with("fresh"), "{net}");
    assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
}

/// Dates come from the content, so only an exact If-Modified-Since match gets a 304.
#[tokio::test]
async fn not_modified_only_on_an_exact_date() {
    let asset = Asset::new(b"abc".to_vec());
    let date = asset.last_modified;
    let assets = Assets::from([("f.crypt".to_string(), asset)]);
    let ask = |since: SystemTime| {
        format!(
            "GET /f.crypt HTTP/1.1\r\nHost: x\r\nIf-Modified-Since: {}\r\nConnection: close\r\n\r\n",
            httpdate::fmt_http_date(since)
        )
    };
    let same = request(assets.clone(), None, ask(date)).await;
    let later = request(assets, None, ask(date + Duration::from_secs(86_400))).await;
    assert!(same.starts_with("HTTP/1.1 304"), "{same}");
    assert!(later.starts_with("HTTP/1.1 200"), "{later}");
}

#[test]
fn connections_per_address_are_capped() {
    let conns = Connections::default();
    let (a, b): (IpAddr, IpAddr) = ("10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap());
    let held: Vec<Slot> = (0..MAX_CONNECTIONS_PER_IP)
        .map(|_| conns.open(a).unwrap())
        .collect();
    assert!(conns.open(a).is_none(), "one over the cap is refused");
    assert!(conns.open(b).is_some(), "other addresses are unaffected");
    drop(held);
    assert!(
        conns.open(a).is_some(),
        "closed connections give their places back"
    );
    assert!(
        conns.0.lock().unwrap().is_empty(),
        "addresses with no open connection are forgotten"
    );
}

#[test]
fn content_date_is_stable_per_content() {
    assert_eq!(content_date(b"same"), content_date(b"same"));
    assert_ne!(content_date(b"one"), content_date(b"two"));
    let secs = content_date(b"x")
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        secs >= 978_307_200,
        "dates are never near 0, which the client's cache check treats specially"
    );
}

#[test]
fn normalizes_game_paths() {
    assert_eq!(
        normalize("/./net.bin.psarc.crypt").as_deref(),
        Some("net.bin.psarc.crypt")
    );
    assert_eq!(
        normalize("/net-patch9.bin.psarc.crypt").as_deref(),
        Some("net-patch9.bin.psarc.crypt")
    );
    assert_eq!(normalize("/a/./b.jpg").as_deref(), Some("a/b.jpg"));
    assert_eq!(normalize("/../etc/passwd"), None);
    assert_eq!(normalize("/"), None);
    assert_eq!(normalize("/c:/x"), None);
}
