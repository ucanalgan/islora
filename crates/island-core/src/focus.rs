//! Works out what a Windows focus session is doing.
//!
//! Windows itself only says whether a session is active, and that turns false
//! while paused. The Clock app answers with its state but rounds the time left,
//! while a settings record holds the exact end but has no documented layout. So
//! the three are combined, and losing any of them only loses precision.

/// The tag in front of the end time.
const END_TAG: [u8; 2] = [0xC6, 0x1E];
/// The tag in front of when the record was written, in Unix seconds.
const WRITTEN_TAG: [u8; 2] = [0x2A, 0x06];

const FILETIME_TICKS_PER_MS: u64 = 10_000;
/// Milliseconds between the Windows epoch (1601) and the Unix epoch (1970).
const EPOCH_GAP_MS: u64 = 11_644_473_600_000;

/// Sanity limits, from 2020 to 2100, to reject bytes that only look like a time.
pub(crate) const EARLIEST_MS: i64 = 1_577_836_800_000;
pub(crate) const LATEST_MS: i64 = 4_102_444_800_000;

/// How long the Clock app gets to start a session. It may have to launch first.
pub const START_WAIT_MS: i64 = 15_000;
/// How long it gets to pause, resume or end one.
pub const CHANGE_WAIT_MS: i64 = 4_000;

/// The exact times from the settings record, in Unix milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusRecord {
    pub started: i64,
    pub ends: i64,
}

/// What the Clock app reports about its focus timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockFocus {
    pub in_session: bool,
    pub paused: bool,
    pub on_break: bool,
    pub remaining_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusStatus {
    Idle,
    /// Active, but no source can say for how long.
    Open,
    Running { started: i64, ends: i64, on_break: bool },
    Paused { remaining_ms: u64, on_break: bool },
}

fn varint(bytes: &[u8]) -> Option<u64> {
    let mut value = 0_u64;
    for (index, byte) in bytes.iter().take(10).enumerate() {
        value |= u64::from(byte & 0x7F) << (index * 7);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn value_after(record: &[u8], tag: [u8; 2], last: bool) -> Option<u64> {
    let mut places = record.windows(2).enumerate().filter(|(_, pair)| *pair == tag);
    let (position, _) = if last { places.next_back()? } else { places.next()? };
    varint(record.get(position + 2..)?)
}

pub fn parse_record(record: &[u8]) -> Option<FocusRecord> {
    let ticks = value_after(record, END_TAG, true)?;
    let ends = (ticks / FILETIME_TICKS_PER_MS).checked_sub(EPOCH_GAP_MS)? as i64;
    let started = value_after(record, WRITTEN_TAG, false)? as i64 * 1000;
    let plausible = (EARLIEST_MS..LATEST_MS).contains(&ends)
        && (EARLIEST_MS..LATEST_MS).contains(&started)
        && started < ends;
    plausible.then_some(FocusRecord { started, ends })
}

fn json_field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let rest = &json[json.find(&format!("\"{key}\":"))? + key.len() + 3..];
    let end = rest.find([',', '}'])?;
    Some(rest[..end].trim().trim_matches('"'))
}

/// Reads the Clock app's reply. Its unit is translated, such as `dk` or `min`.
pub fn parse_clock_state(json: &str) -> Option<ClockFocus> {
    let number = |key| json_field(json, key)?.parse::<u64>().ok();
    let amount = number("remainingTimeText")?;
    let unit = json_field(json, "remainingTimeUnit")?.to_lowercase();
    let unit_ms = match unit.chars().next()? {
        's' if unit.starts_with("sa") || unit == "st" => 3_600_000,
        'h' => 3_600_000,
        'd' | 'm' => 60_000,
        _ => 1_000,
    };
    Some(ClockFocus {
        // Zero means a session is on. One means the timer is at rest.
        in_session: number("sessionState")? == 0,
        paused: number("isPaused")? != 0,
        on_break: number("isBreak")? != 0,
        remaining_ms: amount * unit_ms,
    })
}

/// Combines what is known. `clock` is the most trusted about the state, and the
/// record about the time. `now` is Unix milliseconds.
pub fn combine(active: bool, record: Option<FocusRecord>, clock: Option<ClockFocus>, now: i64) -> FocusStatus {
    let record = record.filter(|record| record.ends > now);
    match clock {
        Some(clock) if !clock.in_session => FocusStatus::Idle,
        Some(clock) if clock.paused => {
            FocusStatus::Paused { remaining_ms: clock.remaining_ms, on_break: clock.on_break }
        }
        Some(clock) => match record {
            Some(record) => FocusStatus::Running {
                started: record.started,
                ends: record.ends,
                on_break: clock.on_break,
            },
            None if clock.remaining_ms > 0 => FocusStatus::Running {
                started: now,
                ends: now + clock.remaining_ms as i64,
                on_break: clock.on_break,
            },
            None => FocusStatus::Open,
        },
        None => match (active, record) {
            (true, Some(record)) => {
                FocusStatus::Running { started: record.started, ends: record.ends, on_break: false }
            }
            (true, None) => FocusStatus::Open,
            (false, _) => FocusStatus::Idle,
        },
    }
}

/// What a report from Windows meant for the island.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusReport {
    /// The status may have changed.
    Taken,
    /// Windows has not caught up with a command yet, so the report says nothing.
    TooEarly,
    /// The session asked for never began. This much of it was left.
    NeverStarted { remaining_ms: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Command {
    deadline: i64,
    starts_session: bool,
}

/// What the island believes about the session. A command to the Clock app shows
/// at once, then Windows has until a deadline to agree, since reports sent in
/// between still describe how things were.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusTracker {
    status: FocusStatus,
    command: Option<Command>,
}

impl Default for FocusTracker {
    fn default() -> Self {
        Self { status: FocusStatus::Idle, command: None }
    }
}

impl FocusTracker {
    pub fn status(&self) -> FocusStatus {
        self.status
    }

    /// A session was asked for. Length and `now` are in milliseconds.
    pub fn expect_start(&mut self, length: i64, now: i64) {
        self.status = FocusStatus::Running { started: now, ends: now + length, on_break: false };
        self.command = Some(Command { deadline: now + START_WAIT_MS, starts_session: true });
    }

    /// The running session was paused, resumed or ended.
    pub fn expect_change(&mut self, status: FocusStatus, now: i64) {
        self.status = status;
        self.command = Some(Command { deadline: now + CHANGE_WAIT_MS, starts_session: false });
    }

    pub fn report(&mut self, report: FocusStatus, now: i64) -> FocusReport {
        let previous = self.status;
        if let Some(command) = self.command {
            // Starting runs through a session of the wrong length on the way, see
            // `start_session` in the app, which is not the one that was asked for.
            let agrees = match (previous, report) {
                (FocusStatus::Idle, FocusStatus::Idle) => true,
                (FocusStatus::Paused { .. }, FocusStatus::Paused { .. }) => true,
                (FocusStatus::Running { ends: wanted, .. }, FocusStatus::Running { ends, .. }) => {
                    !command.starts_session || (ends - wanted).abs() <= START_WAIT_MS
                }
                (FocusStatus::Running { .. }, FocusStatus::Open) => !command.starts_session,
                _ => false,
            };
            if !agrees && now < command.deadline {
                return FocusReport::TooEarly;
            }
            self.command = None;
            if let (true, false, FocusStatus::Running { ends, .. }, FocusStatus::Idle) =
                (command.starts_session, agrees, previous, report)
            {
                self.status = FocusStatus::Idle;
                return FocusReport::NeverStarted { remaining_ms: (ends - now).max(0) as u64 };
            }
            // An end from the island beats not knowing how long it runs for.
            if let (FocusStatus::Running { .. }, FocusStatus::Open) = (previous, report) {
                return FocusReport::Taken;
            }
        }
        self.status = match (report, previous) {
            // The Clock app rounds a paused session to the minute, while the end
            // it was heading for gives the exact time left.
            (FocusStatus::Paused { on_break, .. }, FocusStatus::Running { ends, .. }) => {
                FocusStatus::Paused { remaining_ms: (ends - now).max(0) as u64, on_break }
            }
            (FocusStatus::Paused { on_break, .. }, FocusStatus::Paused { remaining_ms, .. }) => {
                FocusStatus::Paused { remaining_ms, on_break }
            }
            _ => report,
        };
        FocusReport::Taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record written by Windows for a 15 minute session.
    const RECORD: &str = "43 42 01 00 0A 00 2A 06 98 A9 93 D7 06 2A 2B 0E 66 43 42 01 00 CA 0A 05 E5 A6 86 D8 01 24 A5 4B 44 F1 9F 01 66 B5 95 84 AE A5 C4 A6 FE 8F 01 00 CB 14 0A 01 CA 0A 05 CE DE 94 9C 0D 24 B1 FE 01 44 EE 95 01 66 9F B3 F4 99 B0 9E 95 83 37 00 CA 14 05 E5 A6 86 D8 01 24 A5 4B 44 F1 9F 01 66 B5 95 84 AE A5 C4 A6 FE 8F 01 00 C6 1E A1 C9 AE CD EA 92 DA EE 01 00 00 00 00 00";
    const RUNNING: &str = r#"{"sessionState":0,"isPaused":0,"isBreak":0,"remainingTimeText":"2","remainingTimeUnit":"dk","sessionDurationSettingInMinutes":5.0,"upNext":""}"#;
    const PAUSED: &str = r#"{"sessionState":0,"isPaused":1,"isBreak":0,"remainingTimeText":"2","remainingTimeUnit":"dk","sessionDurationSettingInMinutes":5.0,"upNext":""}"#;
    const AT_REST: &str = r#"{"sessionState":1,"isPaused":0,"isBreak":0,"remainingTimeText":"0","remainingTimeUnit":"sn","sessionDurationSettingInMinutes":5.0,"upNext":""}"#;

    const NOW: i64 = 1_793_381_600_000;

    fn bytes(text: &str) -> Vec<u8> {
        text.split(' ').map(|byte| u8::from_str_radix(byte, 16).unwrap()).collect()
    }

    fn record() -> Option<FocusRecord> {
        parse_record(&bytes(RECORD))
    }

    #[test]
    fn reads_the_end_of_a_real_session() {
        let session = record().unwrap();
        assert_eq!(session.started, 1_793_381_528_000);
        assert_eq!(session.ends, 1_793_382_419_196);
    }

    #[test]
    fn a_record_without_an_end_time_gives_nothing() {
        let finished = bytes("43 42 01 00 0A 00 2A 06 98 A9 93 D7 06 00 00 00");
        assert_eq!(parse_record(&finished), None);
    }

    #[test]
    fn nonsense_is_rejected_rather_than_trusted() {
        assert_eq!(parse_record(&[]), None);
        assert_eq!(parse_record(&bytes("C6 1E 01 2A 06 01")), None);
        assert_eq!(parse_record(&bytes("C6 1E FF FF FF FF FF FF FF FF FF FF FF")), None);
    }

    #[test]
    fn an_end_before_the_start_is_rejected() {
        let backwards = bytes("2A 06 98 A9 93 D7 06 C6 1E 80 80 80 80 80 80 80 EA 01");
        assert_eq!(parse_record(&backwards), None);
    }

    #[test]
    fn reads_the_clock_reply() {
        let state = parse_clock_state(RUNNING).unwrap();
        assert_eq!(state, ClockFocus { in_session: true, paused: false, on_break: false, remaining_ms: 120_000 });
        assert!(parse_clock_state(PAUSED).unwrap().paused);
        assert!(!parse_clock_state(AT_REST).unwrap().in_session);
    }

    #[test]
    fn units_are_understood_in_either_language() {
        let with_unit = |unit: &str| {
            let json = format!(r#"{{"sessionState":0,"isPaused":0,"isBreak":0,"remainingTimeText":"3","remainingTimeUnit":"{unit}"}}"#);
            parse_clock_state(&json).unwrap().remaining_ms
        };
        assert_eq!(with_unit("sn"), 3_000);
        assert_eq!(with_unit("sec"), 3_000);
        assert_eq!(with_unit("dk"), 180_000);
        assert_eq!(with_unit("min"), 180_000);
        assert_eq!(with_unit("sa"), 10_800_000);
        assert_eq!(with_unit("hr"), 10_800_000);
    }

    #[test]
    fn an_unexpected_reply_gives_nothing() {
        assert_eq!(parse_clock_state("{}"), None);
        assert_eq!(parse_clock_state("hata"), None);
    }

    #[test]
    fn the_exact_time_is_used_while_running() {
        let clock = parse_clock_state(RUNNING);
        let expected = FocusStatus::Running { started: 1_793_381_528_000, ends: 1_793_382_419_196, on_break: false };
        assert_eq!(combine(true, record(), clock, NOW), expected);
    }

    #[test]
    fn a_paused_session_stays_although_windows_calls_it_inactive() {
        let clock = parse_clock_state(PAUSED);
        let expected = FocusStatus::Paused { remaining_ms: 120_000, on_break: false };
        assert_eq!(combine(false, record(), clock, NOW), expected);
    }

    #[test]
    fn the_clock_saying_it_stopped_beats_an_old_record() {
        assert_eq!(combine(false, record(), parse_clock_state(AT_REST), NOW), FocusStatus::Idle);
    }

    #[test]
    fn without_the_record_the_clocks_rounded_time_still_works() {
        let expected = FocusStatus::Running { started: NOW, ends: NOW + 120_000, on_break: false };
        assert_eq!(combine(true, None, parse_clock_state(RUNNING), NOW), expected);
    }

    #[test]
    fn without_the_clock_the_record_and_windows_still_work() {
        assert!(matches!(combine(true, record(), None, NOW), FocusStatus::Running { .. }));
        assert_eq!(combine(false, record(), None, NOW), FocusStatus::Idle);
    }

    #[test]
    fn with_only_windows_left_the_session_still_shows() {
        assert_eq!(combine(true, None, None, NOW), FocusStatus::Open);
        assert_eq!(combine(false, None, None, NOW), FocusStatus::Idle);
    }

    #[test]
    fn a_record_in_the_past_is_ignored() {
        let later = 1_793_382_419_196 + 1;
        assert_eq!(combine(true, record(), None, later), FocusStatus::Open);
    }

    const LENGTH: i64 = 300_000;

    fn running(ends: i64) -> FocusStatus {
        FocusStatus::Running { started: NOW, ends, on_break: false }
    }

    fn started() -> FocusTracker {
        let mut tracker = FocusTracker::default();
        tracker.expect_start(LENGTH, NOW);
        tracker
    }

    #[test]
    fn a_started_session_shows_before_windows_confirms_it() {
        assert_eq!(started().status(), running(NOW + LENGTH));
    }

    #[test]
    fn reports_of_the_old_state_do_not_undo_a_start() {
        let mut tracker = started();
        assert_eq!(tracker.report(FocusStatus::Idle, NOW + 1_000), FocusReport::TooEarly);
        assert_eq!(tracker.status(), running(NOW + LENGTH));
    }

    #[test]
    fn the_exact_times_from_windows_replace_the_guess() {
        let mut tracker = started();
        assert_eq!(tracker.report(running(NOW + LENGTH + 700), NOW + 2_000), FocusReport::Taken);
        assert_eq!(tracker.status(), running(NOW + LENGTH + 700));
    }

    #[test]
    fn a_session_windows_never_started_is_noticed() {
        let mut tracker = started();
        let later = NOW + START_WAIT_MS;
        assert_eq!(
            tracker.report(FocusStatus::Idle, later),
            FocusReport::NeverStarted { remaining_ms: (LENGTH - START_WAIT_MS) as u64 }
        );
        assert_eq!(tracker.status(), FocusStatus::Idle);
    }

    #[test]
    fn a_known_length_beats_a_session_of_unknown_length() {
        let mut tracker = started();
        assert_eq!(tracker.report(FocusStatus::Open, NOW + 1_000), FocusReport::TooEarly);
        assert_eq!(tracker.status(), running(NOW + LENGTH));
        assert_eq!(tracker.report(FocusStatus::Open, NOW + START_WAIT_MS), FocusReport::Taken);
        assert_eq!(tracker.status(), running(NOW + LENGTH));
        assert_eq!(tracker.report(FocusStatus::Idle, NOW + START_WAIT_MS + 1), FocusReport::Taken);
        assert_eq!(tracker.status(), FocusStatus::Idle);
    }

    #[test]
    fn the_session_of_the_wrong_length_on_the_way_is_not_shown() {
        let mut tracker = started();
        let wrong = running(NOW + 90 * 60_000);
        assert_eq!(tracker.report(wrong, NOW + 300), FocusReport::TooEarly);
        assert_eq!(tracker.report(FocusStatus::Idle, NOW + 600), FocusReport::TooEarly);
        assert_eq!(tracker.report(running(NOW + LENGTH + 900), NOW + 900), FocusReport::Taken);
        assert_eq!(tracker.status(), running(NOW + LENGTH + 900));
    }

    #[test]
    fn a_wrong_length_that_stays_is_shown_as_it_is() {
        let mut tracker = started();
        let wrong = running(NOW + 90 * 60_000);
        assert_eq!(tracker.report(wrong, NOW + START_WAIT_MS), FocusReport::Taken);
        assert_eq!(tracker.status(), wrong);
    }

    #[test]
    fn once_settled_every_report_counts() {
        let mut tracker = FocusTracker::default();
        assert_eq!(tracker.report(running(NOW + LENGTH), NOW), FocusReport::Taken);
        assert_eq!(tracker.report(FocusStatus::Idle, NOW + 1), FocusReport::Taken);
        assert_eq!(tracker.status(), FocusStatus::Idle);
    }

    #[test]
    fn ending_from_the_island_holds_until_windows_agrees() {
        let mut tracker = FocusTracker::default();
        tracker.report(running(NOW + LENGTH), NOW);
        tracker.expect_change(FocusStatus::Idle, NOW);
        assert_eq!(tracker.report(running(NOW + LENGTH), NOW + 500), FocusReport::TooEarly);
        assert_eq!(tracker.status(), FocusStatus::Idle);
        assert_eq!(tracker.report(FocusStatus::Idle, NOW + 900), FocusReport::Taken);
    }

    #[test]
    fn a_command_windows_ignored_is_undone_at_the_deadline() {
        let mut tracker = FocusTracker::default();
        tracker.report(running(NOW + LENGTH), NOW);
        tracker.expect_change(FocusStatus::Idle, NOW);
        let report = tracker.report(running(NOW + LENGTH), NOW + CHANGE_WAIT_MS);
        assert_eq!(report, FocusReport::Taken);
        assert_eq!(tracker.status(), running(NOW + LENGTH));
    }

    #[test]
    fn an_ignored_resume_is_not_mistaken_for_a_failed_start() {
        let mut tracker = FocusTracker::default();
        tracker.expect_change(running(NOW + LENGTH), NOW);
        let report = tracker.report(FocusStatus::Idle, NOW + CHANGE_WAIT_MS);
        assert_eq!(report, FocusReport::Taken);
        assert_eq!(tracker.status(), FocusStatus::Idle);
    }

    #[test]
    fn pausing_keeps_the_exact_time_left() {
        let mut tracker = FocusTracker::default();
        tracker.report(running(NOW + LENGTH), NOW);
        tracker.report(FocusStatus::Paused { remaining_ms: 300_000, on_break: false }, NOW + 28_500);
        assert_eq!(tracker.status(), FocusStatus::Paused { remaining_ms: 271_500, on_break: false });
    }

    #[test]
    fn the_rounded_report_does_not_replace_a_pause_from_the_island() {
        let mut tracker = FocusTracker::default();
        let exact = FocusStatus::Paused { remaining_ms: 271_500, on_break: false };
        tracker.expect_change(exact, NOW);
        tracker.report(FocusStatus::Paused { remaining_ms: 300_000, on_break: false }, NOW + 500);
        assert_eq!(tracker.status(), exact);
    }

    #[test]
    fn a_pause_found_at_startup_uses_what_the_clock_says() {
        let mut tracker = FocusTracker::default();
        let paused = FocusStatus::Paused { remaining_ms: 120_000, on_break: false };
        tracker.report(paused, NOW);
        assert_eq!(tracker.status(), paused);
    }
}
