//! The subscription quota and the thermostat that reacts to it.
//!
//! Claude Code gives its rate limits (5-hour and 7-day windows) only to the status
//! line command; Vario's status line saves them in `<data>/quota.json`, and the hooks
//! and the interface read them from there.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::files;

const FILE: &str = "quota.json";

/// Thresholds, in percent of the most used window.
pub const REINFORCED: f64 = 50.0;
pub const ALERT: f64 = 80.0;
pub const LIMIT: f64 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub used_percentage: f64,
    /// Unix seconds when the window resets.
    pub resets_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Quota {
    /// Unix seconds when Vario last saw the limits.
    pub captured_at: i64,
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}

/// What the thermostat advises, from the most used window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Normal,
    /// Delegate everything an economical model can do.
    Reinforced,
    /// Warn at each prompt; avoid costly work in the main session.
    Alert,
    /// The limit is reached: switch agents, after saving a handoff.
    Limit,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

impl Quota {
    /// The limits in a status line input (`rate_limits.five_hour`, `.seven_day`);
    /// `None` when Claude Code sent none (API users, or before the first answer).
    pub fn from_status_line(input: &Value, now: i64) -> Option<Quota> {
        let window = |name: &str| {
            let window = input.get("rate_limits")?.get(name)?;
            Some(Window {
                used_percentage: window.get("used_percentage")?.as_f64()?,
                resets_at: window.get("resets_at")?.as_i64()?,
            })
        };
        let quota = Quota {
            captured_at: now,
            five_hour: window("five_hour"),
            seven_day: window("seven_day"),
        };
        (quota.five_hour.is_some() || quota.seven_day.is_some()).then_some(quota)
    }

    pub fn load(data_dir: &Path) -> Option<Quota> {
        let text = std::fs::read_to_string(path(data_dir)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        let json = serde_json::to_string(self).expect("a quota serializes");
        files::atomic_write(&path(data_dir), &(json + "\n"))
    }

    /// The windows still current at `now`: a window past its reset time says nothing
    /// about the present.
    pub fn current(&self, now: i64) -> (Option<Window>, Option<Window>) {
        let live = |window: Option<Window>| window.filter(|w| w.resets_at > now);
        (live(self.five_hour), live(self.seven_day))
    }

    /// The most used current window, as (percent, reset time).
    pub fn highest(&self, now: i64) -> Option<Window> {
        let (five, seven) = self.current(now);
        [five, seven]
            .into_iter()
            .flatten()
            .max_by(|a, b| a.used_percentage.total_cmp(&b.used_percentage))
    }

    pub fn level(&self, now: i64) -> Level {
        match self.highest(now).map(|w| w.used_percentage) {
            Some(used) if used >= LIMIT => Level::Limit,
            Some(used) if used >= ALERT => Level::Alert,
            Some(used) if used >= REINFORCED => Level::Reinforced,
            _ => Level::Normal,
        }
    }
}

/// "1 h 20", "45 min": time left until `resets_at`.
pub fn time_left(resets_at: i64, now: i64) -> String {
    let minutes = ((resets_at - now).max(0) + 59) / 60;
    let (hours, minutes) = (minutes / 60, minutes % 60);
    match (hours, minutes) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes:02}"),
    }
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn quota(five: f64, seven: f64) -> Quota {
        Quota {
            captured_at: NOW,
            five_hour: Some(Window {
                used_percentage: five,
                resets_at: NOW + 3600,
            }),
            seven_day: Some(Window {
                used_percentage: seven,
                resets_at: NOW + 86_400,
            }),
        }
    }

    #[test]
    fn reads_the_status_line_input() {
        let input = json!({"rate_limits": {
            "five_hour": {"used_percentage": 23.5, "resets_at": NOW + 100},
            "seven_day": {"used_percentage": 41.2, "resets_at": NOW + 200}
        }});
        let quota = Quota::from_status_line(&input, NOW).unwrap();
        assert_eq!(quota.five_hour.unwrap().used_percentage, 23.5);
        assert_eq!(quota.seven_day.unwrap().resets_at, NOW + 200);
        assert_eq!(Quota::from_status_line(&json!({"model": {}}), NOW), None);
    }

    #[test]
    fn the_thermostat_follows_the_most_used_window() {
        assert_eq!(quota(10.0, 30.0).level(NOW), Level::Normal);
        assert_eq!(quota(55.0, 30.0).level(NOW), Level::Reinforced);
        assert_eq!(quota(20.0, 85.0).level(NOW), Level::Alert);
        assert_eq!(quota(100.0, 30.0).level(NOW), Level::Limit);
        assert_eq!(
            quota(20.0, 85.0).highest(NOW).unwrap().used_percentage,
            85.0
        );
    }

    #[test]
    fn a_window_past_its_reset_is_ignored() {
        let quota = quota(95.0, 30.0);
        let later = NOW + 3601;
        assert_eq!(quota.level(later), Level::Normal);
        assert_eq!(quota.current(later).0, None);
        assert_eq!(quota.level(NOW + 90_000), Level::Normal, "both expired");
    }

    #[test]
    fn save_then_load() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Quota::load(dir.path()), None);
        let quota = quota(12.0, 34.0);
        quota.save(dir.path()).unwrap();
        assert_eq!(Quota::load(dir.path()), Some(quota));
    }

    #[test]
    fn time_left_reads_naturally() {
        assert_eq!(time_left(NOW + 45 * 60, NOW), "45 min");
        assert_eq!(time_left(NOW + 80 * 60, NOW), "1 h 20");
        assert_eq!(time_left(NOW + 120 * 60, NOW), "2 h");
        assert_eq!(time_left(NOW - 10, NOW), "0 min");
    }
}
