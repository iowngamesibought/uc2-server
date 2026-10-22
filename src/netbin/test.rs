use super::*;

fn example() -> NetBinConfig {
    let server = r#"{
        "hostA": "uc2.example.lan", "portA": 3232, "hostB": "uc2.example.lan", "portB": 3232,
        "authHost": "uc2.example.lan", "authPort": 3233, "authHost2": "uc2.example.lan",
        "statsPrefix": null, "statsQueue": "/uc2-stats", "statsQueues": ["/uc2-stats"],
        "tusMirrorQueue": "/uc2-tus-mirror", "natQueue": "/uc2-nat"
    }"#;
    serde_json::from_str(&format!(r#"{{"configA": {server}, "configB": {server}}}"#)).unwrap()
}

/// Pins the exact module bytes for the example config. This output was checked against the
/// client's own module loader; any layout change must be re-verified before updating the digest.
#[test]
fn module_bytes_are_stable() {
    let m = build_module(&example()).unwrap();
    assert_eq!(m.len(), 448);
    let digest: String = Md5::digest(&m).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(digest, "a72bc91ea740fd2d9d04a4089d978ced");
}

/// Merging into an existing module keeps its other bytes, repoints both config symbols to
/// appended objects and keeps a sorted reloc table that covers the old and new pointers.
#[test]
fn merge_keeps_base_and_repoints_configs() {
    let cfg = example();
    let base = build_module(&cfg).unwrap();
    let mut other = cfg.clone();
    other.config_a.auth_port = 4444;
    let m = merge_module(&base, &other).unwrap();
    let be = |o: usize| u32::from_be_bytes(m[o..o + 4].try_into().unwrap()) as usize;
    let old_reloc = be32(&base, 8).unwrap();
    let (root, reloc) = (be(0x10), be(8));
    assert_eq!(
        m[0x14..old_reloc],
        base[0x14..old_reloc],
        "base data must be unchanged"
    );
    assert!(root >= old_reloc, "the new root table is appended");
    let relocs: Vec<usize> = (0..be(reloc)).map(|i| be(reloc + 4 + 4 * i)).collect();
    assert!(relocs.windows(2).all(|w| w[0] < w[1]));
    let syms: Vec<(u32, usize)> = (0..be(root + 4))
        .map(|i| (be(root + 8 + 8 * i) as u32, be(root + 12 + 8 * i)))
        .collect();
    assert!(
        syms.windows(2).all(|w| w[0].0 < w[1].0),
        "root table must stay sorted"
    );
    assert_eq!(
        syms.len(),
        3,
        "both configs plus the added empty 0x9f9bcb43 list"
    );
    for (i, &(sid, obj)) in syms.iter().enumerate() {
        assert!(
            relocs.contains(&(root + 12 + 8 * i)),
            "root pointer must be relocated"
        );
        if sid == SID_PATCH9_PAIRS {
            assert_eq!(be(obj), 0);
            continue;
        }
        assert!(
            relocs.contains(&(obj + 0x10)),
            "auth host pointer must be relocated"
        );
        let port = if sid == SID_SERVER_CONFIG_A {
            4444
        } else {
            cfg.config_b.auth_port as usize
        };
        assert_eq!(be(obj + 0x14), port);
    }
}

#[test]
fn archive_round_trip() {
    let blob = build_archive(&[("2.networking", b"hello"), ("2.common", &[7u8; 0x12345])]);
    assert_eq!(
        archive_manifest(&blob).unwrap(),
        ["2.networking", "2.common"]
    );
    assert_eq!(read_archive_entry(&blob, "2.networking").unwrap(), b"hello");
    assert_eq!(
        read_archive_entry(&blob, "2.common").unwrap(),
        vec![7u8; 0x12345]
    );
    assert!(read_archive_entry(&blob, "3.networking").is_err());
}

#[test]
fn module_structure() {
    let m = build_module(&example()).unwrap();
    let be = |o: usize| u32::from_be_bytes(m[o..o + 4].try_into().unwrap()) as usize;
    assert_eq!(&m[..4], b"DC00");
    assert_eq!(m.len() % 16, 0);
    let reloc = be(8);
    let count = be(reloc);
    let relocs: Vec<usize> = (0..count).map(|i| be(reloc + 4 + 4 * i)).collect();
    assert!(relocs.contains(&0x10));
    let root = be(0x10);
    assert_eq!(be(root + 4), 2);
    assert!(
        be(root + 8) < be(root + 16),
        "root entries must be sorted by sid"
    );
}
