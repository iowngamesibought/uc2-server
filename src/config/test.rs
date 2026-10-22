use super::{Config, check_public_host};

/// The shipped example parses, and its written-out values equal the defaults.
#[test]
fn example_config_parses() {
    let text = include_str!("../../uc2-server.example.toml").replace("CHANGE-ME", "192.168.1.50");
    let cfg: Config = toml::from_str(&text).unwrap();
    assert_eq!(
        cfg.netbin_config().unwrap().config_a.stats_queue,
        "/uc2-stats"
    );
    assert_eq!(cfg.presence.heartbeat_interval_secs, 30);
    assert_eq!(cfg.presence.session_timeout_secs, 120);
    assert!(cfg.text_overrides.is_empty() && cfg.data_dir.is_none() && cfg.netbin.json.is_none());
}

/// The config module is read from the base game's script archive (root entry) and from an
/// update's archive (full path) alike.
#[test]
fn net_module_is_read_at_any_path() {
    let dir = std::env::temp_dir().join(format!("uc2-netmodule-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let read = |entries: &[(&str, &[u8])]| {
        let path = dir.join("archive");
        std::fs::write(&path, crate::netbin::build_archive(entries)).unwrap();
        let cfg: Config = toml::from_str(&format!(
            "public_host = \"192.168.1.50\"\nexecutable = \"x\"\nscript_archive = {:?}\n\
             [http]\nbind = \"0.0.0.0:80\"\n[auth]\nbind = \"0.0.0.0:3233\"\n[presence]\nbind = \"0.0.0.0:3232\"\n",
            path.display().to_string()
        ))
        .unwrap();
        cfg.base_net_module()
    };
    let base = read(&[("net-beta.bin", b"beta"), ("net.bin", b"base")]);
    let update = read(&[("build/main/dc1/net.bin", b"update")]);
    let neither = read(&[("build/main/text1/2.common", b"text")]);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(base.unwrap().unwrap(), b"base");
    assert_eq!(update.unwrap().unwrap(), b"update");
    assert!(
        neither
            .unwrap_err()
            .to_string()
            .contains("no online config module")
    );
}

/// The minimal config (only the required keys) gets the documented defaults.
#[test]
fn minimal_config_uses_defaults() {
    let cfg: Config = toml::from_str(
        r#"
        public_host = "192.168.1.50"
        executable = "game/a"
        script_archive = "game/b"
        text_archive = "game/c"
        [http]
        bind = "0.0.0.0:80"
        [auth]
        bind = "0.0.0.0:3233"
        [presence]
        bind = "0.0.0.0:3232"
        "#,
    )
    .unwrap();
    assert_eq!(
        cfg.netbin_config().unwrap().config_a.stats_queue,
        "/uc2-stats"
    );
    assert_eq!(cfg.presence.heartbeat_interval_secs, 30);
    assert_eq!(cfg.presence.session_timeout_secs, 120);
    assert_eq!(cfg.presence.daily_reset_utc_offset_hours, 0);
    assert_eq!(
        (
            cfg.presence.default_location.lat,
            cfg.presence.default_location.lon
        ),
        (0.0, 0.0)
    );
    assert!(cfg.text_overrides.is_empty() && cfg.data_dir.is_none());
}

/// A data_dir that holds a game file is refused; one beside the game folder is fine.
#[test]
fn data_dir_must_not_hold_the_game_files() {
    let dir = std::env::temp_dir().join(format!("uc2-config-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("game")).unwrap();
    std::fs::create_dir_all(dir.join("static")).unwrap();
    std::fs::write(dir.join("game/exe"), b"x").unwrap();
    let load = |data_dir: &str| {
        let text = format!(
            "public_host = \"192.168.1.50\"\nexecutable = \"game/exe\"\ndata_dir = \"{data_dir}\"\n\
                            [http]\nbind = \"0.0.0.0:80\"\n[auth]\nbind = \"0.0.0.0:3233\"\n\
                            [presence]\nbind = \"0.0.0.0:3232\"\n"
        );
        std::fs::write(dir.join("uc2-server.toml"), text).unwrap();
        Config::load(&dir.join("uc2-server.toml"))
    };
    let (root, game, separate) = (load("."), load("game"), load("static"));
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        root.is_err() && game.is_err(),
        "a folder holding the executable is refused"
    );
    assert!(separate.is_ok(), "{:?}", separate.err());
}

#[test]
fn public_host_must_be_reachable() {
    assert!(check_public_host("192.168.1.50").is_ok());
    assert!(check_public_host("server.example.lan").is_ok());
    for bad in ["", "0.0.0.0", "::", "CHANGE-ME"] {
        assert!(check_public_host(bad).is_err(), "{bad:?} must be rejected");
    }
}
