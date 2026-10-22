use super::archive_role;

#[test]
fn archive_roles_follow_entry_names() {
    let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(
        archive_role(&names(&["alpha.bin", "net.bin", "beta.bin"]))
            .starts_with("fits script_archive")
    );
    // An update's archive carries full paths.
    assert!(
        archive_role(&names(&[
            "build/main/dc1/net.bin",
            "build/main/text1/2.common"
        ]))
        .starts_with("fits script_archive")
    );
    assert!(archive_role(&names(&["net-beta.bin", "dc1/subnet.bin"])).starts_with("fits neither"));
    assert!(
        archive_role(&names(&["2.common", "2.networking", "3.subtitles"]))
            .starts_with("fits text_archive")
    );
    assert!(archive_role(&names(&["2.common", "x.networking"])).starts_with("fits neither"));
    assert!(archive_role(&names(&["map.pak"])).starts_with("fits neither"));
}
