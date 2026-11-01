use crate::activity::{Activity, Bubble, Icon, Kind, Millis, TimerView};
use crate::media::format_time;

/// In its last minute a timer takes the pill, whatever else is going on.
pub const TIMER_URGENT_MS: Millis = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Progress {
    Running { ends_at: Millis },
    Paused { remaining: Millis },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timer {
    total: Millis,
    progress: Progress,
}

impl Timer {
    pub fn start(total: Millis, now: Millis) -> Self {
        Self { total, progress: Progress::Running { ends_at: now + total } }
    }

    /// One that is already under way, such as a Windows focus session.
    pub fn ending_at(ends_at: Millis, total: Millis) -> Self {
        Self { total, progress: Progress::Running { ends_at } }
    }

    pub fn paused(remaining: Millis, total: Millis) -> Self {
        Self { total, progress: Progress::Paused { remaining } }
    }

    pub fn remaining(&self, now: Millis) -> Millis {
        match self.progress {
            Progress::Running { ends_at } => ends_at.saturating_sub(now),
            Progress::Paused { remaining } => remaining,
        }
    }

    pub fn ends_at(&self) -> Option<Millis> {
        match self.progress {
            Progress::Running { ends_at } => Some(ends_at),
            Progress::Paused { .. } => None,
        }
    }

    pub fn is_finished(&self, now: Millis) -> bool {
        self.ends_at().is_some_and(|ends_at| ends_at <= now)
    }

    pub fn toggle(&mut self, now: Millis) {
        self.progress = match self.progress {
            Progress::Running { ends_at } => Progress::Paused { remaining: ends_at.saturating_sub(now) },
            Progress::Paused { remaining } => Progress::Running { ends_at: now + remaining },
        };
    }

    pub fn activity(&self, now: Millis) -> Activity {
        let remaining = self.remaining(now);
        let running = self.ends_at().is_some();
        let shown = remaining.div_ceil(1000) * 1000;
        let left = (remaining.min(self.total) * 1000 / self.total.max(1)) as u16;
        Activity {
            leading: "Zamanlayıcı".into(),
            trailing: format_time(shown),
            title: format_time(shown),
            detail: if running { "Zamanlayıcı".into() } else { "Duraklatıldı".into() },
            urgent_at: self.ends_at().map(|ends_at| ends_at.saturating_sub(TIMER_URGENT_MS)),
            icon: Some(Icon::Timer),
            badge: Some(Bubble::Count {
                value: remaining.div_ceil(60_000).to_string(),
                icon: None,
                ring: Some((left / 10) as u8),
            }),
            timer: Some(TimerView { running, has_controls: true, left: Some(left) }),
            ..Activity::new(Kind::TimerRunning)
        }
    }

    /// The same countdown as a Windows focus session.
    pub fn focus_activity(&self, now: Millis, on_break: bool) -> Activity {
        let activity = self.activity(now);
        let running = self.ends_at().is_some();
        let name = if on_break { "Mola" } else { "Odak" };
        Activity {
            kind: Kind::Focus,
            leading: name.into(),
            detail: if running { format!("{name} oturumu") } else { "Duraklatıldı".into() },
            icon: Some(Icon::Focus),
            ..activity
        }
    }

    /// The same countdown as a timer in the Windows Clock app. It can only be
    /// followed, as the Clock takes no commands for its timers.
    pub fn clock_activity(&self, now: Millis, name: &str) -> Activity {
        let activity = self.activity(now);
        let running = self.ends_at().is_some();
        Activity {
            kind: Kind::ClockTimer,
            detail: if running { format!("Saat · {name}") } else { "Duraklatıldı".into() },
            timer: activity.timer.map(|timer| TimerView { has_controls: false, ..timer }),
            ..activity
        }
    }

    /// When the shown value next changes: every second when it reads `24:13`,
    /// every minute as a bubble that reads `24`.
    pub fn next_tick(&self, now: Millis, shows_seconds: bool) -> Option<Millis> {
        self.ends_at()?;
        let step = if shows_seconds { 1_000 } else { 60_000 };
        let into_step = self.remaining(now) % step;
        Some(now + if into_step == 0 { step } else { into_step })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_down_while_running() {
        let timer = Timer::start(60_000, 1_000);
        assert_eq!(timer.remaining(1_000), 60_000);
        assert_eq!(timer.remaining(31_000), 30_000);
        assert!(!timer.is_finished(60_999));
        assert!(timer.is_finished(61_000));
    }

    #[test]
    fn pausing_holds_the_remaining_time() {
        let mut timer = Timer::start(60_000, 0);
        timer.toggle(10_000);
        assert_eq!(timer.remaining(500_000), 50_000);
        assert!(!timer.is_finished(9_999_999));
        timer.toggle(500_000);
        assert_eq!(timer.remaining(510_000), 40_000);
    }

    #[test]
    fn shows_rounded_up_so_it_starts_on_the_full_time() {
        let activity = Timer::start(1_500_000, 0).activity(10);
        assert_eq!(activity.trailing, "25:00");
        assert_eq!(activity.badge, Some(Bubble::Count { value: "25".into(), icon: None, ring: Some(99) }));
    }

    #[test]
    fn becomes_urgent_in_its_last_minute_unless_paused() {
        let mut timer = Timer::start(300_000, 0);
        assert_eq!(timer.activity(0).urgent_at, Some(240_000));
        timer.toggle(10_000);
        assert_eq!(timer.activity(10_000).urgent_at, None);
    }

    #[test]
    fn ticks_as_often_as_the_display_changes() {
        let timer = Timer::start(90_500, 0);
        assert_eq!(timer.next_tick(0, true), Some(500));
        assert_eq!(timer.next_tick(0, false), Some(30_500));
        let exact = Timer::start(120_000, 0);
        assert_eq!(exact.next_tick(0, false), Some(60_000));
    }

    #[test]
    fn a_clock_timer_shows_where_it_runs_and_offers_no_buttons() {
        let activity = Timer::ending_at(60_000, 60_000).clock_activity(0, "1 dk");
        assert_eq!(activity.kind, Kind::ClockTimer);
        assert_eq!(activity.detail, "Saat · 1 dk");
        assert!(!activity.timer.unwrap().has_controls);
    }

    #[test]
    fn says_how_much_of_its_time_is_left() {
        let left_at = |now| Timer::start(100_000, 0).activity(now).timer.unwrap().left;
        assert_eq!(left_at(0), Some(1000));
        assert_eq!(left_at(25_000), Some(750));
        assert_eq!(left_at(100_000), Some(0));
    }

    #[test]
    fn a_clock_timer_knows_how_much_is_left_too() {
        let activity = Timer::ending_at(30_000, 60_000).clock_activity(0, "1 dk");
        assert_eq!(activity.timer.unwrap().left, Some(500));
    }

    #[test]
    fn a_paused_timer_never_ticks() {
        let mut timer = Timer::start(60_000, 0);
        timer.toggle(1_000);
        assert_eq!(timer.next_tick(1_000, true), None);
    }
}
