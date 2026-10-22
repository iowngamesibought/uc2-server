//! Shared in-memory state: when each client IP last sent a login ticket, presence sessions by
//! online id, and the unique players seen today. Also the log throttle the UDP services share.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Warnings about rejected datagrams each UDP service logs per minute; see [`Throttle`].
pub const REJECTED_LOGS_PER_MINUTE: u32 = 10;

/// Caps a repeated warning at `per_minute` lines a minute, so a flood of junk datagrams can neither
/// fill the log nor slow the service loop down writing it.
pub struct Throttle {
    per_minute: u32,
    window: Instant,
    shown: u32,
    suppressed: u64,
}

impl Throttle {
    pub fn new(per_minute: u32) -> Self {
        Self {
            per_minute,
            window: Instant::now(),
            shown: 0,
            suppressed: 0,
        }
    }

    /// Some(lines suppressed since the last one shown) when this line may be logged.
    pub fn allow(&mut self) -> Option<u64> {
        self.allow_at(Instant::now())
    }

    fn allow_at(&mut self, now: Instant) -> Option<u64> {
        if now.duration_since(self.window) >= Duration::from_secs(60) {
            self.window = now;
            self.shown = 0;
        }
        if self.shown < self.per_minute {
            self.shown += 1;
            Some(std::mem::take(&mut self.suppressed))
        } else {
            self.suppressed += 1;
            None
        }
    }
}

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
    tickets: Mutex<HashMap<IpAddr, Instant>>,
    sessions: Mutex<HashMap<String, Session>>,
    daily: Mutex<DailyPlayers>,
    utc_offset_hours: i32,
}

impl Registry {
    pub fn new(utc_offset_hours: i32) -> Self {
        Self {
            utc_offset_hours,
            ..Self::default()
        }
    }

    /// Records a player for today's unique count (called on login and on every presence message).
    pub fn record_player(&self, online_id: &str) -> usize {
        let day = day_number(SystemTime::now(), self.utc_offset_hours);
        self.daily.lock().unwrap().record(day, online_id)
    }

    pub fn record_ticket(&self, ip: IpAddr) {
        self.tickets.lock().unwrap().insert(ip, Instant::now());
    }

    /// Drops tickets older than `timeout` unless a presence session is still live at that address.
    pub fn prune_tickets(&self, timeout: Duration) {
        let live: HashSet<IpAddr> = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .map(|s| s.addr.ip())
            .collect();
        self.tickets
            .lock()
            .unwrap()
            .retain(|ip, at| at.elapsed() <= timeout || live.contains(ip));
    }

    pub fn has_ticket(&self, ip: IpAddr) -> bool {
        self.tickets.lock().unwrap().contains_key(&ip)
    }

    /// Creates or refreshes a session; returns true when it is new.
    pub fn touch(
        &self,
        online_id: &str,
        addr: SocketAddr,
        update: impl FnOnce(&mut Session),
    ) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        let is_new = !sessions.contains_key(online_id);
        let s = sessions
            .entry(online_id.to_string())
            .or_insert_with(|| Session {
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
        let expired: Vec<String> = sessions
            .iter()
            .filter(|(_, s)| s.last_seen.elapsed() > timeout)
            .map(|(k, _)| k.clone())
            .collect();
        for k in &expired {
            sessions.remove(k);
        }
        expired
    }
}

#[cfg(test)]
mod test;
