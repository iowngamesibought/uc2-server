use super::*;

#[test]
fn daily_players_count_unique_ids_and_reset_on_a_new_day() {
    let mut d = DailyPlayers::default();
    assert_eq!(d.record(100, "a"), 1);
    assert_eq!(d.record(100, "b"), 2);
    assert_eq!(d.record(100, "a"), 2, "a repeat login is not counted twice");
    assert_eq!(
        d.record(101, "a"),
        1,
        "the set is cleared on a new day before inserting"
    );
    assert_eq!(d.record(101, "c"), 2);
}

#[test]
fn day_boundary_follows_the_utc_offset() {
    let t = |s: u64| UNIX_EPOCH + Duration::from_secs(s);
    // 1970-01-01 15:59:59 UTC is 23:59:59 at UTC+8; one second later is the next day there.
    assert_eq!(day_number(t(57_599), 8), 0);
    assert_eq!(day_number(t(57_600), 8), 1);
    assert_eq!(day_number(t(57_600), 0), 0);
    assert_eq!(day_number(t(0), -1), -1);
}

#[test]
fn old_tickets_are_pruned_unless_a_session_is_live_there() {
    let r = Registry::new(0);
    let (live, gone): (IpAddr, IpAddr) = ("10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap());
    r.record_ticket(live);
    r.record_ticket(gone);
    r.touch("a", SocketAddr::new(live, 3232), |_| {});
    std::thread::sleep(Duration::from_millis(5));
    r.prune_tickets(Duration::from_secs(60));
    assert!(
        r.has_ticket(live) && r.has_ticket(gone),
        "recent tickets are kept"
    );
    r.prune_tickets(Duration::from_millis(1));
    assert!(
        r.has_ticket(live),
        "kept while its address has a live session"
    );
    assert!(!r.has_ticket(gone));
}

#[test]
fn throttle_limits_lines_per_minute_and_counts_the_rest() {
    let mut t = Throttle::new(2);
    let start = t.window;
    assert_eq!(t.allow_at(start), Some(0));
    assert_eq!(t.allow_at(start), Some(0));
    assert_eq!(t.allow_at(start), None);
    assert_eq!(t.allow_at(start + Duration::from_secs(59)), None);
    assert_eq!(
        t.allow_at(start + Duration::from_secs(60)),
        Some(2),
        "the next minute's first line reports the dropped ones"
    );
    assert_eq!(t.allow_at(start + Duration::from_secs(60)), Some(0));
}
