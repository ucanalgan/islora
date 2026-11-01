//! Reads the iCalendar feed calendar services publish, and lists the concrete
//! occurrences in a time window. All-day events are left out: the island reminds
//! of things that start at a certain moment.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeZone, Utc};
use rrule::RRuleSet;

/// A safety limit for rules that never end.
const MAX_OCCURRENCES: u16 = 500;
const UNTITLED: &str = "Etkinlik";

/// One occurrence of an event, in Unix milliseconds.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub start: i64,
    pub end: i64,
    pub title: String,
    pub location: Option<String>,
}

#[derive(Default)]
struct RawEvent {
    uid: String,
    summary: String,
    location: String,
    cancelled: bool,
    start: Option<String>,
    end: Option<String>,
    rule: Option<String>,
    exceptions: Vec<String>,
    replaces: Option<String>,
}

/// Lists occurrences that overlap `from..to`, sorted by start.
pub fn parse(ics: &str, from: i64, to: i64) -> Vec<Event> {
    let raw = read_events(ics);

    // A moved or cancelled instance arrives as a separate event naming the
    // original time, which must then no longer come from the repeating rule.
    let mut replaced: HashMap<&str, HashSet<i64>> = HashMap::new();
    for event in &raw {
        if let Some(moment) = event.replaces.as_deref().and_then(moment_of) {
            replaced.entry(&event.uid).or_default().insert(moment);
        }
    }

    let mut events = Vec::new();
    for event in &raw {
        if event.cancelled {
            continue;
        }
        let Some(start_line) = &event.start else { continue };
        let Some(start) = moment_of(start_line) else { continue };
        let end = event.end.as_deref().and_then(moment_of).unwrap_or(start);
        let length = (end - start).max(0);

        let starts = match &event.rule {
            Some(rule) if event.replaces.is_none() => {
                let skip = replaced.get(event.uid.as_str());
                expand(start_line, rule, &event.exceptions, from - length, to)
                    .into_iter()
                    .filter(|moment| skip.is_none_or(|skip| !skip.contains(moment)))
                    .collect()
            }
            _ => vec![start],
        };

        for start in starts {
            if start < to && start + length > from {
                events.push(Event {
                    start,
                    end: start + length,
                    title: title_of(&event.summary),
                    location: short_location(&event.location),
                });
            }
        }
    }
    events.sort_by_key(|event| event.start);
    events
}

fn read_events(ics: &str) -> Vec<RawEvent> {
    // Long lines continue on the next one after a leading space or tab.
    let unfolded = ics.replace("\r\n", "\n").replace("\n ", "").replace("\n\t", "");

    let mut events = Vec::new();
    let mut current: Option<RawEvent> = None;
    for line in unfolded.lines() {
        match line {
            "BEGIN:VEVENT" => current = Some(RawEvent::default()),
            "END:VEVENT" => events.extend(current.take()),
            _ => {
                if let Some(event) = &mut current {
                    read_property(event, line);
                }
            }
        }
    }
    events
}

fn read_property(event: &mut RawEvent, line: &str) {
    let Some((head, value)) = line.split_once(':') else { return };
    let name = head.split(';').next().unwrap_or(head);
    match name {
        "UID" => event.uid = value.to_owned(),
        "SUMMARY" => event.summary = value.to_owned(),
        "LOCATION" => event.location = value.to_owned(),
        "STATUS" => event.cancelled = value == "CANCELLED",
        "DTSTART" => event.start = Some(line.to_owned()),
        "DTEND" => event.end = Some(line.to_owned()),
        "RRULE" => event.rule = Some(value.to_owned()),
        "EXDATE" => event.exceptions.push(line.to_owned()),
        "RECURRENCE-ID" => event.replaces = Some(line.to_owned()),
        _ => {}
    }
}

/// Reads a line such as `DTSTART;TZID=Europe/Istanbul:20261030T120000`.
/// Returns `None` for dates without a time, which mark all-day events.
fn moment_of(line: &str) -> Option<i64> {
    let (head, value) = line.split_once(':')?;
    if value.len() < 15 {
        return None;
    }
    if let Some(utc) = value.strip_suffix('Z') {
        let moment = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some(moment.and_utc().timestamp_millis());
    }
    let moment = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;
    let zone = head
        .split(';')
        .find_map(|part| part.strip_prefix("TZID="))
        .and_then(|name| name.trim_matches('"').parse::<chrono_tz::Tz>().ok());
    match zone {
        Some(zone) => Some(zone.from_local_datetime(&moment).earliest()?.timestamp_millis()),
        // No zone, or a name outside the IANA list: the user's own time is the
        // best remaining guess.
        None => Some(Local.from_local_datetime(&moment).earliest()?.timestamp_millis()),
    }
}

fn expand(start_line: &str, rule: &str, exceptions: &[String], from: i64, to: i64) -> Vec<i64> {
    let mut text = format!("{start_line}\nRRULE:{rule}");
    for exception in exceptions {
        text.push('\n');
        text.push_str(exception);
    }
    let Ok(set) = text.parse::<RRuleSet>() else { return Vec::new() };
    let (Some(after), Some(before)) = (utc_moment(from), utc_moment(to)) else { return Vec::new() };
    set.after(after)
        .before(before)
        .all(MAX_OCCURRENCES)
        .dates
        .iter()
        .map(DateTime::timestamp_millis)
        .collect()
}

fn utc_moment(milliseconds: i64) -> Option<DateTime<rrule::Tz>> {
    rrule::Tz::UTC.timestamp_millis_opt(milliseconds).single()
}

fn unescape(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }
        match characters.next() {
            Some('n' | 'N') => result.push(' '),
            Some(other) => result.push(other),
            None => {}
        }
    }
    result
}

/// An event may have no name, and an empty pill would explain nothing.
fn title_of(summary: &str) -> String {
    let title = unescape(summary);
    if title.trim().is_empty() { UNTITLED.to_owned() } else { title }
}

/// Addresses are long and the part that matters comes first, such as a room.
fn short_location(raw: &str) -> Option<String> {
    let text = unescape(raw);
    let first = text.split(',').next().unwrap_or("").trim();
    (!first.is_empty()).then(|| first.to_owned())
}

/// Describes when something starts, such as `Bugün 19:30` or `Pzt 09:00`.
pub fn describe_start<T: TimeZone>(start: &DateTime<T>, now: &DateTime<T>) -> String
where
    T::Offset: std::fmt::Display,
{
    const DAYS: [&str; 7] = ["Pzt", "Sal", "Çar", "Per", "Cum", "Cmt", "Paz"];
    const MONTHS: [&str; 12] =
        ["Oca", "Şub", "Mar", "Nis", "May", "Haz", "Tem", "Ağu", "Eyl", "Eki", "Kas", "Ara"];

    let time = start.format("%H:%M");
    let days_ahead = (start.date_naive() - now.date_naive()).num_days();
    match days_ahead {
        0 => format!("Bugün {time}"),
        1 => format!("Yarın {time}"),
        2..=6 => format!("{} {time}", DAYS[start.weekday().num_days_from_monday() as usize]),
        _ => format!("{} {} {time}", start.day(), MONTHS[start.month0() as usize]),
    }
}

fn local_time(milliseconds: i64) -> Option<DateTime<Local>> {
    Utc.timestamp_millis_opt(milliseconds).single().map(|moment| moment.with_timezone(&Local))
}

/// Text for an event in the user's time zone: `19:30` and `Yarın 19:30`.
pub fn local_labels(start: i64, now: i64) -> Option<(String, String)> {
    let start = local_time(start)?;
    let now = local_time(now)?;
    Some((start.format("%H:%M").to_string(), describe_start(&start, &now)))
}

/// A day in words, such as `Cumartesi, 31 Ekim`.
pub fn describe_day<T: TimeZone>(day: &DateTime<T>) -> String {
    const DAYS: [&str; 7] = ["Pazartesi", "Salı", "Çarşamba", "Perşembe", "Cuma", "Cumartesi", "Pazar"];
    const MONTHS: [&str; 12] = [
        "Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim",
        "Kasım", "Aralık",
    ];
    let weekday = DAYS[day.weekday().num_days_from_monday() as usize];
    format!("{weekday}, {} {}", day.day(), MONTHS[day.month0() as usize])
}

/// Today in the user's time zone, in words.
pub fn local_day(now: i64) -> Option<String> {
    local_time(now).map(|now| describe_day(&now))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    fn utc(text: &str) -> i64 {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap().and_utc().timestamp_millis()
    }

    fn calendar(events: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}END:VCALENDAR\r\n")
    }

    fn titles(events: &[Event]) -> Vec<&str> {
        events.iter().map(|event| event.title.as_str()).collect()
    }

    const WEEKLY: &str = "BEGIN:VEVENT\r\n\
        UID:lesson\r\n\
        DTSTART;TZID=Europe/Istanbul:20261102T090000\r\n\
        DTEND;TZID=Europe/Istanbul:20261102T120000\r\n\
        RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261231T205959Z\r\n\
        SUMMARY:Ders\r\n\
        END:VEVENT\r\n";

    #[test]
    fn a_plain_event_keeps_its_time_and_length() {
        let ics = calendar(
            "BEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261105T090000Z\r\nDTEND:20261105T120000Z\r\nSUMMARY:Sınav\r\nEND:VEVENT\r\n",
        );
        let events = parse(&ics, utc("2026-11-01 00:00"), utc("2026-11-10 00:00"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].start, utc("2026-11-05 09:00"));
        assert_eq!(events[0].end, utc("2026-11-05 12:00"));
        assert_eq!(events[0].title, "Sınav");
    }

    #[test]
    fn a_named_time_zone_is_respected() {
        let ics = calendar(
            "BEGIN:VEVENT\r\nUID:a\r\nDTSTART;TZID=Europe/Istanbul:20261105T120000\r\nDTEND;TZID=Europe/Istanbul:20261105T130000\r\nSUMMARY:X\r\nEND:VEVENT\r\n",
        );
        let events = parse(&ics, utc("2026-11-01 00:00"), utc("2026-11-10 00:00"));
        assert_eq!(events[0].start, utc("2026-11-05 09:00"));
    }

    #[test]
    fn weekly_events_repeat_inside_the_window_only() {
        let events = parse(&calendar(WEEKLY), utc("2026-11-03 00:00"), utc("2026-11-20 00:00"));
        let starts: Vec<i64> = events.iter().map(|event| event.start).collect();
        assert_eq!(starts, vec![utc("2026-11-09 06:00"), utc("2026-11-16 06:00")]);
    }

    #[test]
    fn repeats_keep_the_length_of_the_first() {
        let events = parse(&calendar(WEEKLY), utc("2026-11-03 00:00"), utc("2026-11-10 00:00"));
        assert_eq!(events[0].end - events[0].start, 3 * 3_600_000);
    }

    #[test]
    fn an_occurrence_already_under_way_is_included() {
        let events = parse(&calendar(WEEKLY), utc("2026-11-09 07:00"), utc("2026-11-09 08:00"));
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn a_moved_occurrence_replaces_the_original() {
        let moved = "BEGIN:VEVENT\r\n\
            UID:lesson\r\n\
            RECURRENCE-ID;TZID=Europe/Istanbul:20261109T090000\r\n\
            DTSTART;TZID=Europe/Istanbul:20261114T084500\r\n\
            DTEND;TZID=Europe/Istanbul:20261114T171500\r\n\
            SUMMARY:Ders taşındı\r\n\
            END:VEVENT\r\n";
        let ics = calendar(&format!("{WEEKLY}{moved}"));
        let events = parse(&ics, utc("2026-11-08 00:00"), utc("2026-11-17 00:00"));
        assert_eq!(titles(&events), vec!["Ders taşındı", "Ders"]);
        assert_eq!(events[0].start, utc("2026-11-14 05:45"));
        assert_eq!(events[1].start, utc("2026-11-16 06:00"));
    }

    #[test]
    fn a_cancelled_occurrence_disappears() {
        let cancelled = "BEGIN:VEVENT\r\n\
            UID:lesson\r\n\
            RECURRENCE-ID;TZID=Europe/Istanbul:20261109T090000\r\n\
            DTSTART;TZID=Europe/Istanbul:20261109T090000\r\n\
            DTEND;TZID=Europe/Istanbul:20261109T120000\r\n\
            STATUS:CANCELLED\r\n\
            SUMMARY:Ders\r\n\
            END:VEVENT\r\n";
        let ics = calendar(&format!("{WEEKLY}{cancelled}"));
        let events = parse(&ics, utc("2026-11-08 00:00"), utc("2026-11-17 00:00"));
        assert_eq!(events.iter().map(|event| event.start).collect::<Vec<_>>(), vec![utc("2026-11-16 06:00")]);
    }

    #[test]
    fn an_excluded_date_is_skipped() {
        let event = WEEKLY.replace(
            "SUMMARY:Ders",
            "EXDATE;TZID=Europe/Istanbul:20261109T090000\r\nSUMMARY:Ders",
        );
        let events = parse(&calendar(&event), utc("2026-11-08 00:00"), utc("2026-11-17 00:00"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].start, utc("2026-11-16 06:00"));
    }

    #[test]
    fn a_cancelled_event_is_left_out() {
        let ics = calendar(
            "BEGIN:VEVENT\r\nUID:a\r\nSTATUS:CANCELLED\r\nDTSTART:20261105T090000Z\r\nDTEND:20261105T100000Z\r\nSUMMARY:X\r\nEND:VEVENT\r\n",
        );
        assert!(parse(&ics, utc("2026-11-01 00:00"), utc("2026-11-10 00:00")).is_empty());
    }

    #[test]
    fn all_day_events_are_left_out() {
        let ics = calendar(
            "BEGIN:VEVENT\r\nUID:a\r\nDTSTART;VALUE=DATE:20261105\r\nDTEND;VALUE=DATE:20261106\r\nSUMMARY:Tatil\r\nEND:VEVENT\r\n",
        );
        assert!(parse(&ics, utc("2026-11-01 00:00"), utc("2026-11-10 00:00")).is_empty());
    }

    #[test]
    fn folded_lines_are_joined() {
        let ics = calendar(
            "BEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261105T090000Z\r\nDTEND:20261105T100000Z\r\nSUMMARY:Uzun bir \r\n başlık\r\nEND:VEVENT\r\n",
        );
        let events = parse(&ics, utc("2026-11-01 00:00"), utc("2026-11-10 00:00"));
        assert_eq!(events[0].title, "Uzun bir başlık");
    }

    #[test]
    fn locations_are_cut_to_their_first_part() {
        let ics = calendar(
            "BEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261105T090000Z\r\nDTEND:20261105T100000Z\r\nSUMMARY:X\r\nLOCATION:Kampüs A-12\\, Örnek Üniversitesi\\, İstanbul\r\nEND:VEVENT\r\n",
        );
        let events = parse(&ics, utc("2026-11-01 00:00"), utc("2026-11-10 00:00"));
        assert_eq!(events[0].location.as_deref(), Some("Kampüs A-12"));
    }

    #[test]
    fn an_event_without_a_name_still_reads_as_something() {
        let ics = calendar(
            "BEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261105T090000Z\r\nDTEND:20261105T100000Z\r\nEND:VEVENT\r\n",
        );
        let events = parse(&ics, utc("2026-11-01 00:00"), utc("2026-11-10 00:00"));
        assert_eq!(events[0].title, UNTITLED);
    }

    #[test]
    fn days_read_in_full() {
        let day = Utc.from_utc_datetime(&NaiveDateTime::parse_from_str("2026-10-31 12:00", "%Y-%m-%d %H:%M").unwrap());
        assert_eq!(describe_day(&day), "Cumartesi, 31 Ekim");
    }

    #[test]
    fn an_event_without_a_location_has_none() {
        let ics = calendar(
            "BEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261105T090000Z\r\nDTEND:20261105T100000Z\r\nSUMMARY:X\r\nEND:VEVENT\r\n",
        );
        let events = parse(&ics, utc("2026-11-01 00:00"), utc("2026-11-10 00:00"));
        assert_eq!(events[0].location, None);
    }

    #[test]
    fn escaped_text_is_restored() {
        assert_eq!(unescape("A\\, B\\; C\\\\D"), "A, B; C\\D");
    }

    #[test]
    fn events_come_sorted_by_start() {
        let late = "BEGIN:VEVENT\r\nUID:b\r\nDTSTART:20261108T090000Z\r\nDTEND:20261108T100000Z\r\nSUMMARY:Geç\r\nEND:VEVENT\r\n";
        let early = "BEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261105T090000Z\r\nDTEND:20261105T100000Z\r\nSUMMARY:Erken\r\nEND:VEVENT\r\n";
        let ics = calendar(&format!("{late}{early}"));
        let events = parse(&ics, utc("2026-11-01 00:00"), utc("2026-11-10 00:00"));
        assert_eq!(titles(&events), vec!["Erken", "Geç"]);
    }

    #[test]
    fn broken_input_gives_nothing_rather_than_failing() {
        assert!(parse("bu bir takvim değil", 0, DAY).is_empty());
        let ics = calendar("BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Zamansız\r\nEND:VEVENT\r\n");
        assert!(parse(&ics, 0, DAY).is_empty());
    }

    #[test]
    fn start_times_read_naturally() {
        let at = |text: &str| Utc.from_utc_datetime(&NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap());
        let now = at("2026-11-02 10:00");
        assert_eq!(describe_start(&at("2026-11-02 19:30"), &now), "Bugün 19:30");
        assert_eq!(describe_start(&at("2026-11-03 09:00"), &now), "Yarın 09:00");
        assert_eq!(describe_start(&at("2026-11-06 12:00"), &now), "Cum 12:00");
        assert_eq!(describe_start(&at("2026-11-20 08:45"), &now), "20 Kas 08:45");
    }
}
