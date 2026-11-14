//! The settings file: one `key = "value"` per line, `#` for comments.

use crate::layout::{
    IslandStyle, Material, ProgressStyle, StartMode, TIMER_PRESETS, TaskLayout, WaveStyle,
};
use crate::presentation::{IndicatorStyle, MediaLayout, PresentationPreferences};

const LONGEST_PRESET_MINUTES: u64 = 720;

fn is_setting(line: &str, key: &str) -> bool {
    let line = line.trim();
    !line.starts_with('#')
        && line
            .split_once('=')
            .is_some_and(|(name, _)| name.trim() == key)
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
    let minutes: Vec<u64> = value
        .split(',')
        .map(|part| part.trim().parse().ok())
        .collect::<Option<_>>()?;
    let presets: [u64; TIMER_PRESETS.len()] = minutes.try_into().ok()?;
    presets
        .iter()
        .all(|minutes| (1..=LONGEST_PRESET_MINUTES).contains(minutes))
        .then_some(presets)
}

/// The notices the user turned off, such as `wifi, usb`.
pub fn read_notices_off(text: &str) -> Vec<String> {
    read_value(text, "notices_off").map_or_else(Vec::new, |value| {
        value
            .split(',')
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect()
    })
}

/// Only an explicit `off` stills the wave, which runs by default.
pub fn read_sound_wave(text: &str) -> bool {
    read_value(text, "sound_wave").as_deref() != Some("off")
}

pub const QUICK_TIMEOUTS: [u64; 6] = [0, 5, 8, 15, 30, 60];

/// Zero disables automatic collapse; malformed settings use eight seconds.
pub fn read_quick_timeout(text: &str) -> u64 {
    read_value(text, "quick_controls_timeout")
        .and_then(|value| value.parse().ok())
        .filter(|value| QUICK_TIMEOUTS.contains(value))
        .unwrap_or(8)
}

pub fn read_reduced_motion(text: &str) -> bool {
    read_value(text, "reduced_motion").as_deref() == Some("on")
}

/// Temporary compatibility only; Step 3 will migrate legacy settings explicitly.
pub fn read_presentation_preferences(text: &str) -> PresentationPreferences {
    let legacy = match read_value(text, "ui_density").as_deref() {
        Some("minimal") => PresentationPreferences {
            indicator_style: IndicatorStyle::Ring,
            media_layout: MediaLayout::Compact,
        },
        Some("detailed") => PresentationPreferences {
            indicator_style: IndicatorStyle::Context,
            media_layout: MediaLayout::Informative,
        },
        _ => PresentationPreferences::default(),
    };
    let indicator = read_value(text, "indicator_style");
    let media = read_value(text, "media_layout");
    PresentationPreferences {
        indicator_style: IndicatorStyle::ALL
            .into_iter()
            .find(|v| indicator.as_deref() == Some(v.setting()))
            .unwrap_or(legacy.indicator_style),
        media_layout: MediaLayout::ALL
            .into_iter()
            .find(|v| media.as_deref() == Some(v.setting()))
            .unwrap_or(legacy.media_layout),
    }
}

pub fn read_wave_style(text: &str) -> WaveStyle {
    let value = read_value(text, "wave_style");
    WaveStyle::ALL
        .into_iter()
        .find(|style| value.as_deref() == Some(style.setting()))
        .unwrap_or_default()
}

pub fn read_start_mode(text: &str) -> Option<StartMode> {
    match read_value(text, "start_mode")?.as_str() {
        "focus" => Some(StartMode::Focus),
        "timer" => Some(StartMode::Timer),
        _ => None,
    }
}

pub fn read_material(text: &str) -> Option<Material> {
    match read_value(text, "material")?.as_str() {
        "black" => Some(Material::Black),
        "glass" => Some(Material::Glass),
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

/// The ring is the default, so anything else unknown reads as the ring.
pub fn read_progress_style(text: &str) -> ProgressStyle {
    let value = read_value(text, "progress_style");
    ProgressStyle::ALL
        .into_iter()
        .find(|style| value.as_deref() == Some(style.setting()))
        .unwrap_or_default()
}

/// Whether the user chose to start with Windows or not. `None` until they
/// chose, which is when a new copy turns it on by itself.
pub fn read_autostart(text: &str) -> Option<bool> {
    match read_value(text, "autostart")?.as_str() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

/// The bubble is the default, so anything else unknown reads as the bubble.
pub fn read_task_layout(text: &str) -> TaskLayout {
    let value = read_value(text, "task_layout");
    TaskLayout::ALL
        .into_iter()
        .find(|layout| value.as_deref() == Some(layout.setting()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_preferences_preserve_legacy_and_unrelated_settings() {
        let old = "task_layout = \"shared\"\ntimer_presets = \"3, 7, 20, 30\"\n";
        for (value, indicator_style, media_layout) in [
            ("minimal", IndicatorStyle::Ring, MediaLayout::Compact),
            ("balanced", IndicatorStyle::Edge, MediaLayout::Standard),
            (
                "detailed",
                IndicatorStyle::Context,
                MediaLayout::Informative,
            ),
            ("invalid", IndicatorStyle::Edge, MediaLayout::Standard),
        ] {
            let updated = write_value(old, "ui_density", value);
            assert_eq!(
                read_presentation_preferences(&updated),
                PresentationPreferences {
                    indicator_style,
                    media_layout
                }
            );
            assert_eq!(read_task_layout(&updated), TaskLayout::Shared);
            assert_eq!(read_presets(&updated), Some([3, 7, 20, 30]));
        }
        assert_eq!(
            read_presentation_preferences(old),
            PresentationPreferences::default()
        );
        assert_eq!(
            read_presentation_preferences(
                "ui_density = \"detailed\"\nindicator_style = \"ring\"\nmedia_layout = \"invalid\""
            ),
            PresentationPreferences {
                indicator_style: IndicatorStyle::Ring,
                media_layout: MediaLayout::Informative
            }
        );
        assert_eq!(
            read_presentation_preferences("ui_density = \"minimal\"\nmedia_layout = \"standard\""),
            PresentationPreferences {
                indicator_style: IndicatorStyle::Ring,
                media_layout: MediaLayout::Standard
            }
        );
        for style in IndicatorStyle::ALL {
            for layout in MediaLayout::ALL {
                let text = format!(
                    "ui_density = \"minimal\"\nindicator_style = \"{}\"\nmedia_layout = \"{}\"",
                    style.setting(),
                    layout.setting()
                );
                assert_eq!(
                    read_presentation_preferences(&text),
                    PresentationPreferences {
                        indicator_style: style,
                        media_layout: layout
                    }
                );
            }
        }
    }

    #[test]
    fn starting_with_windows_is_undecided_until_chosen() {
        assert_eq!(read_autostart("autostart = \"on\""), Some(true));
        assert_eq!(read_autostart("autostart = \"off\""), Some(false));
        assert_eq!(read_autostart("autostart = \"maybe\""), None);
        assert_eq!(read_autostart("# autostart = \"off\""), None);
        assert_eq!(read_autostart(""), None);
    }

    #[test]
    fn the_task_layout_is_read_by_its_name() {
        assert_eq!(
            read_task_layout("task_layout = \"shared\""),
            TaskLayout::Shared
        );
        assert_eq!(
            read_task_layout("task_layout = \"front\""),
            TaskLayout::Front
        );
        assert_eq!(
            read_task_layout("task_layout = \"bubble\""),
            TaskLayout::Bubble
        );
        assert_eq!(
            read_task_layout("task_layout = \"stack\""),
            TaskLayout::Bubble
        );
        assert_eq!(read_task_layout(""), TaskLayout::Bubble);
    }

    #[test]
    fn the_progress_style_is_read_by_its_name() {
        assert_eq!(
            read_progress_style("progress_style = \"edge\""),
            ProgressStyle::Edge
        );
        assert_eq!(
            read_progress_style("progress_style = \"fill\""),
            ProgressStyle::Fill
        );
        assert_eq!(
            read_progress_style("progress_style = \"ring\""),
            ProgressStyle::Ring
        );
        assert_eq!(
            read_progress_style("progress_style = \"spiral\""),
            ProgressStyle::Ring
        );
        assert_eq!(read_progress_style(""), ProgressStyle::Ring);
    }

    #[test]
    fn turned_off_notices_are_listed_by_name() {
        assert_eq!(
            read_notices_off("notices_off = \"wifi, usb,\""),
            vec!["wifi", "usb"]
        );
        assert_eq!(read_notices_off("notices_off = \"\""), Vec::<String>::new());
        assert_eq!(read_notices_off(""), Vec::<String>::new());
    }

    #[test]
    fn the_sound_wave_stays_on_unless_turned_off() {
        assert!(read_sound_wave(""));
        assert!(read_sound_wave("sound_wave = \"on\""));
        assert!(!read_sound_wave("sound_wave = \"off\""));
    }

    #[test]
    fn quick_controls_timeout_and_motion_settings_survive_parsing() {
        assert_eq!(read_quick_timeout(""), 8);
        assert_eq!(read_quick_timeout("quick_controls_timeout = \"0\""), 0);
        assert_eq!(read_quick_timeout("quick_controls_timeout = \"30\""), 30);
        assert_eq!(read_quick_timeout("quick_controls_timeout = \"999\""), 8);
        assert!(!read_reduced_motion(""));
        assert!(read_reduced_motion("reduced_motion = \"on\""));
        assert!(!read_reduced_motion("reduced_motion = \"off\""));
    }

    #[test]
    fn the_wave_style_defaults_to_the_existing_motion() {
        assert_eq!(read_wave_style(""), WaveStyle::Normal);
        assert_eq!(read_wave_style("wave_style = \"calm\""), WaveStyle::Calm);
        assert_eq!(
            read_wave_style("wave_style = \"lively\""),
            WaveStyle::Lively
        );
        assert_eq!(
            read_wave_style("wave_style = \"unknown\""),
            WaveStyle::Normal
        );
    }

    #[test]
    fn the_material_is_read_by_its_name() {
        assert_eq!(read_material("material = \"glass\""), Some(Material::Glass));
        assert_eq!(read_material("material = \"black\""), Some(Material::Black));
        assert_eq!(read_material("material = \"tahta\""), None);
    }

    #[test]
    fn the_style_is_read_by_its_name() {
        assert_eq!(
            read_island_style("island_style = \"notch\""),
            Some(IslandStyle::Notch)
        );
        assert_eq!(
            read_island_style("island_style = \"island\""),
            Some(IslandStyle::Island)
        );
        assert_eq!(read_island_style("island_style = \"baska\""), None);
        assert_eq!(read_island_style(""), None);
    }

    #[test]
    fn finds_a_value_with_or_without_quotes() {
        assert_eq!(
            read_value("a = \"bir\"\nb = iki\n", "a").as_deref(),
            Some("bir")
        );
        assert_eq!(
            read_value("a = \"bir\"\nb = iki\n", "b").as_deref(),
            Some("iki")
        );
    }

    #[test]
    fn keeps_equals_signs_inside_the_value() {
        let text = "url = \"https://example.com/a.ics?key=1\"";
        assert_eq!(
            read_value(text, "url").as_deref(),
            Some("https://example.com/a.ics?key=1")
        );
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
        assert_eq!(
            write_value(text, "a", "yeni"),
            "# not = \"this\"\r\na = \"yeni\"\r\nb = \"iki\"\r\n"
        );
    }

    #[test]
    fn writing_a_new_setting_adds_a_line() {
        assert_eq!(
            write_value("a = \"bir\"\n", "b", "iki"),
            "a = \"bir\"\nb = \"iki\"\n"
        );
        assert_eq!(
            write_value("a = \"bir\"", "b", "iki"),
            "a = \"bir\"\nb = \"iki\"\n"
        );
        assert_eq!(write_value("", "b", "iki"), "b = \"iki\"\n");
    }

    #[test]
    fn writing_ignores_comments_about_the_setting() {
        let text = "# a = \"eski\"\n";
        assert_eq!(
            write_value(text, "a", "yeni"),
            "# a = \"eski\"\na = \"yeni\"\n"
        );
    }

    #[test]
    fn what_was_written_can_be_read_back() {
        let text = write_value("url = \"gizli\"\n", "start_mode", "timer");
        assert_eq!(read_start_mode(&text), Some(StartMode::Timer));
        assert_eq!(read_value(&text, "url").as_deref(), Some("gizli"));
    }

    #[test]
    fn presets_are_four_sensible_lengths() {
        assert_eq!(
            read_presets("timer_presets = \"10, 20, 30, 90\""),
            Some([10, 20, 30, 90])
        );
        assert_eq!(read_presets("timer_presets = \"10, 20, 30\""), None);
        assert_eq!(read_presets("timer_presets = \"10, 20, 30, 40, 50\""), None);
        assert_eq!(read_presets("timer_presets = \"0, 20, 30, 40\""), None);
        assert_eq!(read_presets("timer_presets = \"a, b, c, d\""), None);
        assert_eq!(read_presets(""), None);
    }

    #[test]
    fn an_unknown_start_mode_is_ignored() {
        assert_eq!(
            read_start_mode("start_mode = \"focus\""),
            Some(StartMode::Focus)
        );
        assert_eq!(read_start_mode("start_mode = \"baska\""), None);
    }
}
