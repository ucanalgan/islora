use crate::activity::{Activity, Bubble, Icon, Kind, Millis};

/// A coming event appears as a quiet bubble this long before it starts, which
/// matches Google Calendar's default reminder.
pub const EVENT_LEAD_MS: Millis = 30 * 60_000;
/// From here on it takes the pill.
pub const EVENT_URGENT_MS: Millis = 5 * 60_000;
const STARTING_ALERT_MS: Millis = 8_000;
/// Nobody wants to hear that something started hours ago after waking the laptop.
const STARTING_ALERT_LATEST_MS: Millis = 60_000;

/// An event on the island's own clock, with its display text already worked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upcoming {
    pub start: Millis,
    /// The same moment in Unix time. Unlike `start` it stays the same every time
    /// the calendar is reloaded, so it can tell which event this is.
    pub wall_start: i64,
    pub title: String,
    pub location: Option<String>,
    /// Such as `19:30`.
    pub time: String,
    /// Such as `Yarın 09:00`.
    pub when: String,
}

impl Upcoming {
    fn place(&self, prefix: &str) -> String {
        match &self.location {
            Some(location) => format!("{prefix} · {location}"),
            None => prefix.to_owned(),
        }
    }

    pub fn is_near(&self, now: Millis) -> bool {
        self.start.saturating_sub(now) <= EVENT_LEAD_MS
    }

    pub fn activity(&self, now: Millis) -> Activity {
        let minutes = self.start.saturating_sub(now).div_ceil(60_000);
        Activity {
            leading: self.title.clone(),
            trailing: format!("{minutes} dk"),
            title: self.title.clone(),
            detail: self.place(&self.time),
            urgent_at: Some(self.start.saturating_sub(EVENT_URGENT_MS)),
            icon: Some(Icon::Calendar),
            badge: Some(Bubble::Count {
                value: minutes.to_string(),
                icon: Some(Icon::Calendar),
                ring: None,
            }),
            ..Activity::new(Kind::Upcoming)
        }
    }

    pub fn just_started(&self, now: Millis) -> bool {
        now.saturating_sub(self.start) <= STARTING_ALERT_LATEST_MS
    }

    /// `others` is how many more events start with this one. The count comes
    /// before the place, as a long place gets cut off at the end.
    pub fn starting_alert(&self, now: Millis, others: usize) -> Activity {
        let prefix = match others {
            0 => "Başlıyor".to_owned(),
            others => format!("Başlıyor · +{others} etkinlik"),
        };
        Activity {
            title: self.title.clone(),
            detail: self.place(&prefix),
            expires_at: Some(now + STARTING_ALERT_MS),
            icon: Some(Icon::Calendar),
            ..Activity::new(Kind::Notification)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(start: Millis) -> Upcoming {
        Upcoming {
            start,
            wall_start: start as i64,
            title: "Ders".into(),
            location: Some("A-12".into()),
            time: "19:30".into(),
            when: "Bugün 19:30".into(),
        }
    }

    #[test]
    fn counts_whole_minutes_rounded_up() {
        let activity = event(10 * 60_000).activity(30_000);
        assert_eq!(activity.trailing, "10 dk");
    }

    #[test]
    fn shows_time_and_place() {
        assert_eq!(event(0).activity(0).detail, "19:30 · A-12");
        let without = Upcoming {
            location: None,
            ..event(0)
        };
        assert_eq!(without.activity(0).detail, "19:30");
    }

    #[test]
    fn is_near_only_inside_the_lead_time() {
        let start = EVENT_LEAD_MS + 1_000;
        assert!(!event(start).is_near(0));
        assert!(event(start).is_near(1_000));
    }

    #[test]
    fn a_late_start_is_not_announced() {
        assert!(event(0).just_started(1_000));
        assert!(!event(0).just_started(STARTING_ALERT_LATEST_MS + 1));
    }

    #[test]
    fn an_alert_counts_what_else_starts_before_saying_where() {
        assert_eq!(event(0).starting_alert(0, 0).detail, "Başlıyor · A-12");
        assert_eq!(
            event(0).starting_alert(0, 1).detail,
            "Başlıyor · +1 etkinlik · A-12"
        );
    }
}
