use super::*;
use crate::config::Location;

fn cfg() -> PresenceConfig {
    PresenceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        heartbeat_interval_secs: 45,
        session_timeout_secs: 120,
        default_location: Location {
            lat: 34.0542,
            lon: -118.2411,
        },
        daily_reset_utc_offset_hours: 0,
    }
}

#[test]
fn parses_client_formats() {
    // Note the double space after the state in heartbeat, as the client sends it.
    assert_eq!(
        parse("Player1 heartbeat playing  headset 1 party 0 match_mode 3 gametype 7"),
        Some(Request::Heartbeat {
            id: "Player1",
            state: "playing",
            headset: true,
            party: false,
            match_mode: 3,
            game_type: 7
        })
    );
    assert_eq!(
        parse("Player1 playercount"),
        Some(Request::PlayerCount { id: "Player1" })
    );
    assert_eq!(
        parse("Player1 get-location disc XXXX00000 hdsd 1 up 1200 down 8000 wired 1"),
        Some(Request::GetLocation {
            id: "Player1",
            disc: "XXXX00000",
            hd: true,
            up: 1200,
            down: 8000,
            wired: true
        })
    );
    assert_eq!(parse("Player1 bogus"), None);
}

/// An online id that equals a keyword does not shadow that keyword's value; a first token that
/// is not an online id gets no reply.
#[test]
fn keywords_follow_the_command_and_ids_are_checked() {
    assert_eq!(
        parse("party heartbeat menu  headset 0 party 1 match_mode 0 gametype -1"),
        Some(Request::Heartbeat {
            id: "party",
            state: "menu",
            headset: false,
            party: true,
            match_mode: 0,
            game_type: -1
        })
    );
    assert_eq!(
        parse("down get-location disc XXXX00000 hdsd 0 up 900 down 4000 wired 0"),
        Some(Request::GetLocation {
            id: "down",
            disc: "XXXX00000",
            hd: false,
            up: 900,
            down: 4000,
            wired: false
        })
    );
    for bad in [
        "x playercount",
        "Player.1 playercount",
        "Player1234567890XY heartbeat menu",
    ] {
        assert_eq!(parse(bad), None, "{bad:?}");
    }
}

#[test]
fn replies_match_client_parser() {
    let c = cfg();
    assert_eq!(
        reply(&Request::PlayerCount { id: "x" }, &c, 5),
        "playercount 4"
    );
    assert_eq!(
        reply(
            &Request::Heartbeat {
                id: "x",
                state: "menu",
                headset: false,
                party: false,
                match_mode: 0,
                game_type: -1
            },
            &c,
            1
        ),
        "heartbeat next 45"
    );
    assert_eq!(
        reply(
            &Request::GetLocation {
                id: "x",
                disc: "",
                hd: false,
                up: 0,
                down: 0,
                wired: false
            },
            &c,
            1
        ),
        "location 34.0542 -118.2411"
    );
}
