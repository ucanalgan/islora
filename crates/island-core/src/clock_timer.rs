//! Reads the timers of the Windows Clock app.
//!
//! The Clock answers nobody about them, but keeps each one in its settings as a
//! packed list of named values. That layout is not documented, so anything that
//! does not look right is skipped rather than guessed at.

use crate::activity::Millis;
use crate::focus::{EARLIEST_MS, LATEST_MS};
use crate::timer::Timer;

const ENTRY_HEADER: usize = 12;
const ENTRY_ALIGNMENT: usize = 8;
const TYPE_INT64: u32 = 6;
const TYPE_STRING: u32 = 12;

const TICKS_PER_MS: i64 = 10_000;
/// Milliseconds from year 1601, where Windows counts from, to 1970.
const FILETIME_EPOCH_MS: i64 = 11_644_473_600_000;
/// Milliseconds from year 1, where .NET counts from, to 1970.
const DOTNET_EPOCH_MS: i64 = 62_135_596_800_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockTimerState {
    Stopped,
    /// `ends` is in Unix milliseconds.
    Running {
        ends: i64,
    },
    Paused {
        remaining_ms: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockTimer {
    /// As typed in the Clock app, such as `5 dk`.
    pub name: String,
    pub duration_ms: u64,
    pub state: ClockTimerState,
}

impl ClockTimer {
    /// Its countdown on the island's clock `now`, given the Unix time `wall_now`.
    pub fn countdown(&self, now: Millis, wall_now: i64) -> Option<Timer> {
        match self.state {
            ClockTimerState::Stopped => None,
            ClockTimerState::Paused { remaining_ms } => {
                Some(Timer::paused(remaining_ms, self.duration_ms))
            }
            ClockTimerState::Running { ends } => Some(Timer::ending_at(
                now + (ends - wall_now).max(0) as Millis,
                self.duration_ms,
            )),
        }
    }
}

/// The stored times, which only differ in where they count from.
fn unix_ms(stored: i64) -> Option<i64> {
    let since_year_zero = stored / TICKS_PER_MS;
    [FILETIME_EPOCH_MS, DOTNET_EPOCH_MS]
        .into_iter()
        .map(|epoch| since_year_zero - epoch)
        .find(|moment| (EARLIEST_MS..LATEST_MS).contains(moment))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn utf16(bytes: &[u8]) -> String {
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair));
    let text: Vec<u16> = units.take_while(|&unit| unit != 0).collect();
    String::from_utf16_lossy(&text)
}

/// One named value, as `(name, type, value bytes)`.
fn entry(record: &[u8], at: usize) -> Option<(String, u32, &[u8], usize)> {
    let size = u32_at(record, at)? as usize;
    let kind = u32_at(record, at + 4)?;
    let name_units = u32_at(record, at + 8)? as usize;
    let value_at = name_units
        .checked_add(1)?
        .checked_mul(2)?
        .checked_add(ENTRY_HEADER)?;
    if size < value_at {
        return None;
    }
    let body = record.get(at..at.checked_add(size)?)?;
    let name = utf16(&body[ENTRY_HEADER..value_at]);
    Some((
        name,
        kind,
        &body[value_at..],
        (at + size).next_multiple_of(ENTRY_ALIGNMENT),
    ))
}

/// Reads one timer. `None` for anything that is not a complete one.
pub fn parse_timer(record: &[u8]) -> Option<ClockTimer> {
    let (mut name, mut duration, mut start, mut remaining) = (None, None, None, None);
    let mut at = 0;
    while let Some((field, kind, value, next)) = entry(record, at) {
        match (field.as_str(), kind) {
            ("Name", TYPE_STRING) => name = Some(utf16(value)),
            ("Duration", TYPE_INT64) => duration = int64(value),
            ("StartTime", TYPE_INT64) => start = int64(value),
            ("RemainingTime", TYPE_INT64) => remaining = int64(value),
            _ => {}
        }
        at = next;
    }

    let duration_ms = u64::try_from(duration? / TICKS_PER_MS)
        .ok()
        .filter(|&ms| ms > 0)?;
    let left_ms = u64::try_from(remaining? / TICKS_PER_MS).ok()?;
    // A timer that was never paused may not have written what is left.
    let from_start = if left_ms > 0 { left_ms } else { duration_ms };
    let state = match unix_ms(start?) {
        Some(started) => ClockTimerState::Running {
            ends: started + from_start as i64,
        },
        None if left_ms > 0 => ClockTimerState::Paused {
            remaining_ms: left_ms,
        },
        None => ClockTimerState::Stopped,
    };
    Some(ClockTimer {
        name: name?,
        duration_ms,
        state,
    })
}

fn int64(value: &[u8]) -> Option<i64> {
    Some(i64::from_le_bytes(value.get(..8)?.try_into().ok()?))
}

/// The timer worth showing: the running one that ends first, or else one that
/// is paused. A timer past its end is ringing, which the Clock app handles.
pub fn current(timers: &[ClockTimer], now: i64) -> Option<&ClockTimer> {
    let running = timers.iter().filter_map(|timer| match timer.state {
        ClockTimerState::Running { ends } if ends > now => Some((ends, timer)),
        _ => None,
    });
    let paused = || {
        timers
            .iter()
            .find(|timer| matches!(timer.state, ClockTimerState::Paused { .. }))
    };
    running
        .min_by_key(|(ends, _)| *ends)
        .map(|(_, timer)| timer)
        .or_else(paused)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one minute timer at rest, as the Clock app stored it.
    const STOPPED: &str = "20 00 00 00 05 00 00 00 07 00 00 00 56 00 65 00 72 00 73 00 69 00 6f 00 6e 00 00 00 01 00 00 00 20 00 00 00 0c 00 00 00 04 00 00 00 4e 00 61 00 6d 00 65 00 00 00 31 00 20 00 64 00 6b 00 00 00 26 00 00 00 06 00 00 00 08 00 00 00 44 00 75 00 72 00 61 00 74 00 69 00 6f 00 6e 00 00 00 00 46 c3 23 00 00 00 00 67 00 28 00 00 00 06 00 00 00 09 00 00 00 53 00 74 00 61 00 72 00 74 00 54 00 69 00 6d 00 65 00 00 00 00 00 00 00 00 00 00 00 30 00 00 00 06 00 00 00 0d 00 00 00 52 00 65 00 6d 00 61 00 69 00 6e 00 69 00 6e 00 67 00 54 00 69 00 6d 00 65 00 00 00 00 00 00 00 00 00 00 00 28 00 00 00 06 00 00 00 09 00 00 00 5f 00 5f 00 43 00 72 00 65 00 61 00 74 00 65 00 64 00 00 00 73 09 bc ba 94 68 dd 01 28 00 00 00 06 00 00 00 09 00 00 00 5f 00 5f 00 55 00 70 00 64 00 61 00 74 00 65 00 64 00 00 00 73 09 bc ba 94 68 dd 01 73 09 bc ba 94 68 dd 01";

    /// Where the value of each time starts in the record above.
    const START_VALUE: usize = 0x88;
    const REMAINING_VALUE: usize = 0xb8;

    const NOW: i64 = 1_793_381_600_000;

    fn stored(start: i64, remaining_ms: i64) -> Vec<u8> {
        let mut record: Vec<u8> = STOPPED
            .split(' ')
            .map(|byte| u8::from_str_radix(byte, 16).unwrap())
            .collect();
        record[START_VALUE..START_VALUE + 8].copy_from_slice(&start.to_le_bytes());
        let remaining = remaining_ms * TICKS_PER_MS;
        record[REMAINING_VALUE..REMAINING_VALUE + 8].copy_from_slice(&remaining.to_le_bytes());
        record
    }

    fn filetime(unix: i64) -> i64 {
        (unix + FILETIME_EPOCH_MS) * TICKS_PER_MS
    }

    fn timer(state: ClockTimerState) -> ClockTimer {
        ClockTimer {
            name: "5 dk".into(),
            duration_ms: 300_000,
            state,
        }
    }

    #[test]
    fn reads_a_real_timer_at_rest() {
        let parsed = parse_timer(&stored(0, 0)).unwrap();
        assert_eq!(parsed.name, "1 dk");
        assert_eq!(parsed.duration_ms, 60_000);
        assert_eq!(parsed.state, ClockTimerState::Stopped);
    }

    #[test]
    fn a_running_timer_ends_after_what_was_left_when_it_started() {
        let parsed = parse_timer(&stored(filetime(NOW), 30_000)).unwrap();
        assert_eq!(
            parsed.state,
            ClockTimerState::Running { ends: NOW + 30_000 }
        );
    }

    #[test]
    fn a_timer_started_fresh_runs_for_its_whole_length() {
        let parsed = parse_timer(&stored(filetime(NOW), 0)).unwrap();
        assert_eq!(
            parsed.state,
            ClockTimerState::Running { ends: NOW + 60_000 }
        );
    }

    #[test]
    fn the_start_may_be_stored_in_dotnet_time_too() {
        let dotnet = (NOW + DOTNET_EPOCH_MS) * TICKS_PER_MS;
        let parsed = parse_timer(&stored(dotnet, 30_000)).unwrap();
        assert_eq!(
            parsed.state,
            ClockTimerState::Running { ends: NOW + 30_000 }
        );
    }

    #[test]
    fn time_left_without_a_start_is_a_paused_timer() {
        let parsed = parse_timer(&stored(0, 42_000)).unwrap();
        assert_eq!(
            parsed.state,
            ClockTimerState::Paused {
                remaining_ms: 42_000
            }
        );
    }

    #[test]
    fn broken_records_give_nothing() {
        assert_eq!(parse_timer(&[]), None);
        assert_eq!(parse_timer(&[0xFF; 64]), None);
        let record = stored(0, 0);
        assert_eq!(parse_timer(&record[..0x50]), None);
    }

    #[test]
    fn the_running_timer_that_ends_first_is_shown() {
        let timers = [
            timer(ClockTimerState::Paused {
                remaining_ms: 1_000,
            }),
            timer(ClockTimerState::Running { ends: NOW + 90_000 }),
            timer(ClockTimerState::Running { ends: NOW + 30_000 }),
        ];
        assert_eq!(current(&timers, NOW), Some(&timers[2]));
    }

    #[test]
    fn a_paused_timer_shows_when_nothing_runs() {
        let timers = [
            timer(ClockTimerState::Stopped),
            timer(ClockTimerState::Running { ends: NOW - 1 }),
            timer(ClockTimerState::Paused {
                remaining_ms: 1_000,
            }),
        ];
        assert_eq!(current(&timers, NOW), Some(&timers[2]));
    }

    #[test]
    fn a_countdown_moves_to_the_islands_clock() {
        let running = timer(ClockTimerState::Running { ends: NOW + 30_000 });
        assert_eq!(
            running.countdown(5_000, NOW).unwrap().ends_at(),
            Some(35_000)
        );
        let paused = timer(ClockTimerState::Paused {
            remaining_ms: 7_000,
        });
        assert_eq!(
            paused.countdown(5_000, NOW).unwrap().remaining(9_000),
            7_000
        );
        assert_eq!(timer(ClockTimerState::Stopped).countdown(5_000, NOW), None);
    }

    #[test]
    fn stopped_timers_show_nothing() {
        assert_eq!(current(&[timer(ClockTimerState::Stopped)], NOW), None);
        assert_eq!(current(&[], NOW), None);
    }
}
