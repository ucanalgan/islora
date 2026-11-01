//! The settings file: one `key = "value"` per line, `#` for comments.

use crate::layout::{IslandStyle, StartMode, TIMER_PRESETS};

const LONGEST_PRESET_MINUTES: u64 = 720;

fn is_setting(line: &str, key: &str) -> bool {
    let line = line.trim();
    !line.starts_with('#') && line.split_once('=').is_some_and(|(name, _)| name.trim() == key)
}

/// Finds a setting. Blank values count as missing.
pub fn read_value(text: &str, key: &str) -> Option<String> {
    let line = text.lines().find(|line| is_setting(line, key))?;
    let (_, value) = line.split_once('=')?;
    let value = value.trim().trim_matches('"').trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// The file with one setting changed and every other line left as it was.
pub fn write_value(text: &str, key: &str, value: &str) -> String {
    let setting = format!("{key} = \"{value}\"");
    let mut replaced = false;
    let mut result = String::with_capacity(text.len() + setting.len() + 1);
    for line in text.split_inclusive('\n') {
        if !replaced && is_setting(line, key) {
            replaced = true;
            result.push_str(&setting);
            result.push_str(if line.ends_with("\r\n") { "\r\n" } else { "\n" });
        } else {
            result.push_str(line);
        }
    }
    if !replaced {
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(&setting);
        result.push('\n');
    }
    result
}

/// Reads four lengths in minutes, such as `5, 15, 25, 45`.
pub fn read_presets(text: &str) -> Option<[u64; TIMER_PRESETS.len()]> {
    let value = read_value(text, "timer_presets")?;
    let minutes: Vec<u64> = value.split(',').map(|part| part.trim().parse().ok()).collect::<Option<_>>()?;
    let presets: [u64; TIMER_PRESETS.len()] = minutes.try_into().ok()?;
    presets.iter().all(|minutes| (1..=LONGEST_PRESET_MINUTES).contains(minutes)).then_some(presets)
}

/// The notices the user turned off, such as `wifi, usb`.
pub fn read_notices_off(text: &str) -> Vec<String> {
    read_value(text, "notices_off").map_or_else(Vec::new, |value| {
        value.split(',').map(|name| name.trim().to_owned()).filter(|name| !name.is_empty()).collect()
    })
}

pub fn read_start_mode(text: &str) -> Option<StartMode> {
    match read_value(text, "start_mode")?.as_str() {
        "focus" => Some(StartMode::Focus),
        "timer" => Some(StartMode::Timer),
        _ => None,
    }
}

pub fn read_island_style(text: &str) -> Option<IslandStyle> {
    match read_value(text, "island_style")?.as_str() {
        "island" => Some(IslandStyle::Island),
        "notch" => Some(IslandStyle::Notch),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turned_off_notices_are_listed_by_name() {
        assert_eq!(read_notices_off("notices_off = \"wifi, usb,\""), vec!["wifi", "usb"]);
        assert_eq!(read_notices_off("notices_off = \"\""), Vec::<String>::new());
        assert_eq!(read_notices_off(""), Vec::<String>::new());
    }

    #[test]
    fn the_style_is_read_by_its_name() {
        assert_eq!(read_island_style("island_style = \"notch\""), Some(IslandStyle::Notch));
        assert_eq!(read_island_style("island_style = \"island\""), Some(IslandStyle::Island));
        assert_eq!(read_island_style("island_style = \"baska\""), None);
        assert_eq!(read_island_style(""), None);
    }

    #[test]
    fn finds_a_value_with_or_without_quotes() {
        assert_eq!(read_value("a = \"bir\"\nb = iki\n", "a").as_deref(), Some("bir"));
        assert_eq!(read_value("a = \"bir\"\nb = iki\n", "b").as_deref(), Some("iki"));
    }

    #[test]
    fn keeps_equals_signs_inside_the_value() {
        let text = "url = \"https://example.com/a.ics?key=1\"";
        assert_eq!(read_value(text, "url").as_deref(), Some("https://example.com/a.ics?key=1"));
    }

    #[test]
    fn ignores_comments_and_blanks() {
        assert_eq!(read_value("# url = \"x\"\nurl = \"\"\n", "url"), None);
        assert_eq!(read_value("", "url"), None);
    }

    #[test]
    fn a_longer_key_is_not_a_match() {
        assert_eq!(read_value("calendar_url = \"x\"", "url"), None);
    }

    #[test]
    fn writing_changes_only_that_setting() {
        let text = "# not = \"this\"\r\na = \"bir\"\r\nb = \"iki\"\r\n";
        assert_eq!(write_value(text, "a", "yeni"), "# not = \"this\"\r\na = \"yeni\"\r\nb = \"iki\"\r\n");
    }

    #[test]
    fn writing_a_new_setting_adds_a_line() {
        assert_eq!(write_value("a = \"bir\"\n", "b", "iki"), "a = \"bir\"\nb = \"iki\"\n");
        assert_eq!(write_value("a = \"bir\"", "b", "iki"), "a = \"bir\"\nb = \"iki\"\n");
        assert_eq!(write_value("", "b", "iki"), "b = \"iki\"\n");
    }

    #[test]
    fn writing_ignores_comments_about_the_setting() {
        let text = "# a = \"eski\"\n";
        assert_eq!(write_value(text, "a", "yeni"), "# a = \"eski\"\na = \"yeni\"\n");
    }

    #[test]
    fn what_was_written_can_be_read_back() {
        let text = write_value("url = \"gizli\"\n", "start_mode", "timer");
        assert_eq!(read_start_mode(&text), Some(StartMode::Timer));
        assert_eq!(read_value(&text, "url").as_deref(), Some("gizli"));
    }

    #[test]
    fn presets_are_four_sensible_lengths() {
        assert_eq!(read_presets("timer_presets = \"10, 20, 30, 90\""), Some([10, 20, 30, 90]));
        assert_eq!(read_presets("timer_presets = \"10, 20, 30\""), None);
        assert_eq!(read_presets("timer_presets = \"10, 20, 30, 40, 50\""), None);
        assert_eq!(read_presets("timer_presets = \"0, 20, 30, 40\""), None);
        assert_eq!(read_presets("timer_presets = \"a, b, c, d\""), None);
        assert_eq!(read_presets(""), None);
    }

    #[test]
    fn an_unknown_start_mode_is_ignored() {
        assert_eq!(read_start_mode("start_mode = \"focus\""), Some(StartMode::Focus));
        assert_eq!(read_start_mode("start_mode = \"baska\""), None);
    }
}
