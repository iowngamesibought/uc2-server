//! Shared in-memory state: verified tickets by client IP, presence sessions by online id, and the
//! unique players seen today.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ticket::TicketInfo;

#[derive(Debug, Clone)]
pub struct Session {
    pub addr: SocketAddr,
    pub last_seen: Instant,
    pub state: String,
    pub headset: bool,
    pub party: bool,
    pub match_mode: i64,
    pub game_type: i64,
}

/// Unique online ids seen on one day; the set is cleared when the day changes.
#[derive(Debug, Default)]
pub struct DailyPlayers {
    day: i64,
    ids: HashSet<String>,
}

impl DailyPlayers {
    /// Records a player on `day`, clearing the set first when `day` is a new day. Returns today's count.
    pub fn record(&mut self, day: i64, online_id: &str) -> usize {
        if day != self.day {
            self.day = day;
            self.ids.clear();
        }
        self.ids.insert(online_id.to_owned());
        self.ids.len()
    }
}

/// Day number of the system time, with days starting at 00:00 in UTC + `utc_offset_hours`.
pub fn day_number(now: SystemTime, utc_offset_hours: i32) -> i64 {
    let secs = match now.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    (secs + i64::from(utc_offset_hours) * 3600).div_euclid(86_400)
}

#[derive(Default)]
pub struct Registry {
    tickets: Mutex<HashMap<IpAddr, (Instant, Option<TicketInfo>)>>,
    sessions: Mutex<HashMap<String, Session>>,
    daily: Mutex<DailyPlayers>,
    utc_offset_hours: i32,
}

impl Registry {
    pub fn new(utc_offset_hours: i32) -> Self {
        Self { utc_offset_hours, ..Self::default() }
    }

    /// Records a player for today's unique count (called on login and on every presence message).
    pub fn record_player(&self, online_id: &str) -> usize {
        let day = day_number(SystemTime::now(), self.utc_offset_hours);
        self.daily.lock().unwrap().record(day, online_id)
    }

    pub fn record_ticket(&self, ip: IpAddr, info: Option<TicketInfo>) {
        self.tickets.lock().unwrap().insert(ip, (Instant::now(), info));
    }

    pub fn has_ticket(&self, ip: IpAddr) -> bool {
        self.tickets.lock().unwrap().contains_key(&ip)
    }

    /// Creates or refreshes a session; returns true when it is new.
    pub fn touch(&self, online_id: &str, addr: SocketAddr, update: impl FnOnce(&mut Session)) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        let is_new = !sessions.contains_key(online_id);
        let s = sessions.entry(online_id.to_string()).or_insert_with(|| Session {
            addr,
            last_seen: Instant::now(),
            state: "unknown".into(),
            headset: false,
            party: false,
            match_mode: 0,
            game_type: -1,
        });
        s.addr = addr;
        s.last_seen = Instant::now();
        update(s);
        is_new
    }

    pub fn prune(&self, timeout: Duration) -> Vec<String> {
        let mut sessions = self.sessions.lock().unwrap();
        let expired: Vec<String> =
            sessions.iter().filter(|(_, s)| s.last_seen.elapsed() > timeout).map(|(k, _)| k.clone()).collect();
        for k in &expired {
            sessions.remove(k);
        }
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_players_count_unique_ids_and_reset_on_a_new_day() {
        let mut d = DailyPlayers::default();
        assert_eq!(d.record(100, "a"), 1);
        assert_eq!(d.record(100, "b"), 2);
        assert_eq!(d.record(100, "a"), 2, "a repeat login is not counted twice");
        assert_eq!(d.record(101, "a"), 1, "the set is cleared on a new day before inserting");
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
}
