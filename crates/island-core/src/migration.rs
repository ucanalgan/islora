//! Pure preparation of the version 1 to version 2 configuration migration.
//!
//! This module does not read or write files. The caller retains the original
//! input for fallback and is responsible for any eventual persistence.

use crate::presentation::{IndicatorStyle, MediaLayout, PresentationPreferences};

pub const MAX_CONFIG_BYTES: usize = 1024 * 1024;
pub const CURRENT_VERSION: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CriticalKey {
    ConfigVersion,
    UiDensity,
    IndicatorStyle,
    MediaLayout,
}

impl CriticalKey {
    pub fn name(self) -> &'static str {
        match self {
            Self::ConfigVersion => "config_version",
            Self::UiDensity => "ui_density",
            Self::IndicatorStyle => "indicator_style",
            Self::MediaLayout => "media_layout",
        }
    }
    fn parse(name: &str) -> Option<Self> {
        [
            Self::ConfigVersion,
            Self::UiDensity,
            Self::IndicatorStyle,
            Self::MediaLayout,
        ]
        .into_iter()
        .find(|key| key.name() == name)
    }
    fn index(self) -> usize {
        match self {
            Self::ConfigVersion => 0,
            Self::UiDensity => 1,
            Self::IndicatorStyle => 2,
            Self::MediaLayout => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationError {
    InputTooLarge,
    OutputTooLarge,
    DuplicateCriticalKey(CriticalKey),
    MalformedCriticalSetting(CriticalKey),
    InvalidSchemaVersion,
    InvalidLegacyValue,
    InvalidModernValue(CriticalKey),
    UnsupportedFormat { line: usize },
    InvalidOutput,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// The caller may persist this single, complete version 2 document.
    Prepared { text: String },
    /// The original document must be kept byte-for-byte.
    AlreadyCurrent,
    /// A newer version must be treated as read-only by the caller.
    Future { version: u64 },
}

#[derive(Clone, Debug)]
struct Entry<'a> {
    line: usize,
    value: &'a str,
    comment: Option<&'a str>,
}

/// Parse only the simple critical values supported by the existing reader.
/// Unknown settings are never interpreted or unescaped by migration.
fn critical_value(raw: &str, key: CriticalKey) -> Result<(&str, Option<&str>), MigrationError> {
    let raw = raw.trim_start();
    let malformed = || MigrationError::MalformedCriticalSetting(key);
    if let Some(quoted) = raw.strip_prefix('"') {
        let end = quoted.find('"').ok_or_else(malformed)?;
        let value = &quoted[..end];
        if value.contains('\\') {
            return Err(malformed());
        }
        let original_tail = &quoted[end + 1..];
        let tail = original_tail.trim_start();
        if tail.starts_with('#') && original_tail.len() == tail.len() {
            return Err(malformed());
        }
        if !tail.is_empty() && !tail.starts_with('#') {
            return Err(malformed());
        }
        Ok((value.trim(), (!tail.is_empty()).then_some(tail)))
    } else {
        let comment = raw.char_indices().find_map(|(i, ch)| {
            (ch == '#' && (i == 0 || raw[..i].ends_with(char::is_whitespace))).then_some(i)
        });
        let value = raw[..comment.unwrap_or(raw.len())].trim();
        if value.contains(['"', '\'', '=', '#', '\\']) || value.chars().any(char::is_whitespace) {
            return Err(malformed());
        }
        Ok((value, comment.map(|i| &raw[i..])))
    }
}

fn entries<'a>(text: &'a str) -> Result<[Option<Entry<'a>>; 4], MigrationError> {
    // Conflicts take precedence even if one of the duplicate values is invalid.
    let mut seen = [false; 4];
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let line = if index == 0 {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, _)) = line.split_once('=')
            && let Some(key) = CriticalKey::parse(name.trim())
        {
            if seen[key.index()] {
                return Err(MigrationError::DuplicateCriticalKey(key));
            }
            seen[key.index()] = true;
        }
    }
    let mut found: [Option<Entry<'a>>; 4] = [None, None, None, None];
    for (line, original) in text.split_inclusive('\n').enumerate() {
        let body = original
            .strip_suffix('\n')
            .unwrap_or(original)
            .strip_suffix('\r')
            .unwrap_or(original.strip_suffix('\n').unwrap_or(original));
        let body = if line == 0 {
            body.strip_prefix('\u{feff}').unwrap_or(body)
        } else {
            body
        };
        let trimmed = body.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((name, raw)) = body.split_once('=') else {
            if let Some(key) = [
                CriticalKey::ConfigVersion,
                CriticalKey::UiDensity,
                CriticalKey::IndicatorStyle,
                CriticalKey::MediaLayout,
            ]
            .into_iter()
            .find(|key| {
                trimmed.starts_with(key.name())
                    && (trimmed.len() == key.name().len()
                        || trimmed[key.name().len()..]
                            .starts_with(|c: char| c.is_whitespace() || c == ':'))
            }) {
                return Err(MigrationError::MalformedCriticalSetting(key));
            }
            return Err(MigrationError::UnsupportedFormat { line: line + 1 });
        };
        let name = name.trim();
        if name.is_empty() || name.starts_with('[') || name.contains(char::is_whitespace) {
            return Err(MigrationError::UnsupportedFormat { line: line + 1 });
        }
        if let Some(key) = CriticalKey::parse(name) {
            if found[key.index()].is_some() {
                return Err(MigrationError::DuplicateCriticalKey(key));
            }
            let (value, comment) = critical_value(raw, key)?;
            found[key.index()] = Some(Entry {
                line,
                value,
                comment,
            });
        }
    }
    Ok(found)
}

fn indicator(value: &str) -> Option<IndicatorStyle> {
    IndicatorStyle::ALL
        .into_iter()
        .find(|v| v.setting() == value)
}
fn media(value: &str) -> Option<MediaLayout> {
    MediaLayout::ALL.into_iter().find(|v| v.setting() == value)
}
fn legacy(value: &str) -> Option<PresentationPreferences> {
    let (indicator_style, media_layout) = match value {
        "minimal" => (IndicatorStyle::Ring, MediaLayout::Compact),
        "balanced" => (IndicatorStyle::Edge, MediaLayout::Standard),
        "detailed" => (IndicatorStyle::Context, MediaLayout::Informative),
        _ => return None,
    };
    Some(PresentationPreferences {
        indicator_style,
        media_layout,
    })
}

fn ending(line: &str) -> &'static str {
    if line.ends_with("\r\n") {
        "\r\n"
    } else if line.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

/// Classify and, only for a valid legacy file, prepare one complete update.
///
/// Both input and output are limited to `MAX_CONFIG_BYTES`. Existing lines
/// retain their endings; inserted lines use predominant CRLF or LF (LF on a
/// tie). An input without a final newline yields an output without one.
pub fn prepare(text: &str) -> Result<MigrationOutcome, MigrationError> {
    if text.len() > MAX_CONFIG_BYTES {
        return Err(MigrationError::InputTooLarge);
    }
    let found = entries(text)?;
    let get = |key: CriticalKey| found[key.index()].as_ref();
    let version = match get(CriticalKey::ConfigVersion) {
        None => 1,
        Some(entry) => {
            if entry.value.is_empty() || !entry.value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(MigrationError::InvalidSchemaVersion);
            }
            entry
                .value
                .parse::<u64>()
                .ok()
                .filter(|v| *v > 0)
                .ok_or(MigrationError::InvalidSchemaVersion)?
        }
    };
    if version > CURRENT_VERSION {
        return Ok(MigrationOutcome::Future { version });
    }
    let indicator_entry = get(CriticalKey::IndicatorStyle);
    let media_entry = get(CriticalKey::MediaLayout);
    let chosen_indicator = indicator_entry.and_then(|entry| indicator(entry.value));
    let chosen_media = media_entry.and_then(|entry| media(entry.value));
    if version == CURRENT_VERSION {
        if indicator_entry.is_some() && chosen_indicator.is_none() {
            return Err(MigrationError::InvalidModernValue(
                CriticalKey::IndicatorStyle,
            ));
        }
        if media_entry.is_some() && chosen_media.is_none() {
            return Err(MigrationError::InvalidModernValue(CriticalKey::MediaLayout));
        }
        return Ok(MigrationOutcome::AlreadyCurrent);
    }
    let density_entry = get(CriticalKey::UiDensity);
    let fallback = density_entry.and_then(|entry| legacy(entry.value));
    if density_entry.is_some()
        && fallback.is_none()
        && (chosen_indicator.is_none() || chosen_media.is_none())
    {
        return Err(MigrationError::InvalidLegacyValue);
    }
    if fallback.is_none() && indicator_entry.is_some() && chosen_indicator.is_none() {
        return Err(MigrationError::InvalidModernValue(
            CriticalKey::IndicatorStyle,
        ));
    }
    if fallback.is_none() && media_entry.is_some() && chosen_media.is_none() {
        return Err(MigrationError::InvalidModernValue(CriticalKey::MediaLayout));
    }
    let resolved = PresentationPreferences {
        indicator_style: chosen_indicator
            .or_else(|| fallback.map(|v| v.indicator_style))
            .unwrap_or_default(),
        media_layout: chosen_media
            .or_else(|| fallback.map(|v| v.media_layout))
            .unwrap_or_default(),
    };

    let mut replacements: [Option<&str>; 4] = [None; 4];
    if chosen_indicator.is_none() && fallback.is_some() {
        replacements[CriticalKey::IndicatorStyle.index()] =
            Some(resolved.indicator_style.setting());
    }
    if chosen_media.is_none() && fallback.is_some() {
        replacements[CriticalKey::MediaLayout.index()] = Some(resolved.media_layout.setting());
    }
    replacements[CriticalKey::ConfigVersion.index()] = Some("2");

    let bom = text.starts_with('\u{feff}');
    let source = text.strip_prefix('\u{feff}').unwrap_or(text);
    let crlf = source
        .split_inclusive('\n')
        .filter(|line| line.ends_with("\r\n"))
        .count();
    let lf = source.bytes().filter(|b| *b == b'\n').count() - crlf;
    let newline = if crlf > lf { "\r\n" } else { "\n" };
    let terminal_newline = source.ends_with('\n');
    let mut output = String::with_capacity(text.len());
    if bom {
        output.push('\u{feff}');
    }
    for (index, line) in source.split_inclusive('\n').enumerate() {
        if let Some(entry) = get(CriticalKey::UiDensity).filter(|entry| entry.line == index) {
            if let Some(comment) = entry.comment {
                let indent = &line[..line.len() - line.trim_start().len()];
                output.push_str(indent);
                output.push_str(comment);
                output.push_str(ending(line));
            }
            continue;
        }
        let key = [
            CriticalKey::ConfigVersion,
            CriticalKey::IndicatorStyle,
            CriticalKey::MediaLayout,
        ]
        .into_iter()
        .find(|key| {
            get(*key).is_some_and(|entry| entry.line == index)
                && replacements[key.index()].is_some()
        });
        if let Some(key) = key {
            let (prefix, _) = line.split_once('=').ok_or(MigrationError::InvalidOutput)?;
            output.push_str(prefix);
            output.push_str("= \"");
            output.push_str(replacements[key.index()].unwrap_or_default());
            output.push('"');
            if let Some(comment) = get(key).and_then(|entry| entry.comment) {
                output.push(' ');
                output.push_str(comment);
            }
            output.push_str(ending(line));
        } else {
            output.push_str(line);
        }
    }
    let mut additions = Vec::new();
    for key in [
        CriticalKey::IndicatorStyle,
        CriticalKey::MediaLayout,
        CriticalKey::ConfigVersion,
    ] {
        if get(key).is_none()
            && let Some(value) = replacements[key.index()]
        {
            additions.push(format!("{} = \"{}\"", key.name(), value));
        }
    }
    if !additions.is_empty() {
        if !output.is_empty() && output != "\u{feff}" && !output.ends_with('\n') {
            output.push_str(newline);
        }
        output.push_str(&additions.join(newline));
        if terminal_newline {
            output.push_str(newline);
        }
    } else if !terminal_newline && output.ends_with('\n') {
        // Removing a final, unterminated legacy assignment can expose the
        // previous separator as a terminal newline. Preserve EOF semantics.
        output.pop();
        if output.ends_with('\r') {
            output.pop();
        }
    }
    if output.len() > MAX_CONFIG_BYTES {
        return Err(MigrationError::OutputTooLarge);
    }
    let final_entries = entries(&output)?;
    if final_entries[CriticalKey::ConfigVersion.index()]
        .as_ref()
        .map(|e| e.value)
        != Some("2")
        || final_entries[CriticalKey::UiDensity.index()].is_some()
        || crate::settings::Settings::read(&output).presentation() != resolved
    {
        return Err(MigrationError::InvalidOutput);
    }
    Ok(MigrationOutcome::Prepared { text: output })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    fn migrated(input: &str) -> String {
        match prepare(input).unwrap() {
            MigrationOutcome::Prepared { text } => text,
            other => panic!("expected prepared migration, got {other:?}"),
        }
    }

    #[test]
    fn legacy_values_map_to_the_existing_feature_preferences() {
        for (density, indicator, media) in [
            ("minimal", IndicatorStyle::Ring, MediaLayout::Compact),
            ("balanced", IndicatorStyle::Edge, MediaLayout::Standard),
            (
                "detailed",
                IndicatorStyle::Context,
                MediaLayout::Informative,
            ),
        ] {
            let source = format!("ui_density = \"{density}\"\n");
            let output = migrated(&source);
            assert_eq!(
                Settings::read(&output).presentation(),
                PresentationPreferences {
                    indicator_style: indicator,
                    media_layout: media
                }
            );
            assert!(!crate::config::contains_setting(&output, "ui_density"));
            assert_eq!(
                crate::config::read_value(&output, "config_version").as_deref(),
                Some("2")
            );
            assert_eq!(prepare(&output), Ok(MigrationOutcome::AlreadyCurrent));
        }
    }

    #[test]
    fn explicit_modern_preferences_are_independent_and_win() {
        for (modern, expected_indicator, expected_media) in [
            ("", IndicatorStyle::Ring, MediaLayout::Compact),
            (
                "indicator_style = \"context\"\n",
                IndicatorStyle::Context,
                MediaLayout::Compact,
            ),
            (
                "media_layout = \"informative\"\n",
                IndicatorStyle::Ring,
                MediaLayout::Informative,
            ),
            (
                "indicator_style = \"edge\"\nmedia_layout = \"standard\"\n",
                IndicatorStyle::Edge,
                MediaLayout::Standard,
            ),
            (
                "indicator_style = \"invalid\"\nmedia_layout = \"informative\"\n",
                IndicatorStyle::Ring,
                MediaLayout::Informative,
            ),
            (
                "indicator_style = \"context\"\nmedia_layout = \"invalid\"\n",
                IndicatorStyle::Context,
                MediaLayout::Compact,
            ),
        ] {
            let input = format!("ui_density = \"minimal\"\n{modern}");
            let output = migrated(&input);
            assert_eq!(
                Settings::read(&output).presentation(),
                PresentationPreferences {
                    indicator_style: expected_indicator,
                    media_layout: expected_media
                }
            );
            assert_eq!(
                Settings::read(&input).presentation(),
                Settings::read(&output).presentation()
            );
        }
    }

    #[test]
    fn legacy_and_modern_invalid_values_are_not_guessed() {
        assert_eq!(
            prepare("ui_density = \"unknown\"\n"),
            Err(MigrationError::InvalidLegacyValue)
        );
        assert_eq!(
            prepare("ui_density = \"\"\nindicator_style = \"edge\""),
            Err(MigrationError::InvalidLegacyValue)
        );
        assert_eq!(
            prepare("media_layout = \"wrong\""),
            Err(MigrationError::InvalidModernValue(CriticalKey::MediaLayout))
        );
        let source = "ui_density = \"unknown\" # obsolete\nindicator_style = \"context\"\nmedia_layout = \"compact\"\n";
        let output = migrated(source);
        assert!(output.contains("# obsolete\n"));
        assert_eq!(
            Settings::read(&output).presentation(),
            Settings::read(source).presentation()
        );
    }

    #[test]
    fn versions_are_classified_without_rewriting_current_or_future() {
        assert!(matches!(prepare(""), Ok(MigrationOutcome::Prepared { .. })));
        assert!(matches!(
            prepare("# Turkish comment\n"),
            Ok(MigrationOutcome::Prepared { .. })
        ));
        assert!(matches!(
            prepare("config_version = \"1\"\n"),
            Ok(MigrationOutcome::Prepared { .. })
        ));
        assert!(matches!(
            prepare("custom = \"value\""),
            Ok(MigrationOutcome::Prepared { .. })
        ));
        let current =
            "# preserve exactly\r\nconfig_version = \"2\"\r\nui_density = \"minimal\"\r\n";
        assert_eq!(prepare(current), Ok(MigrationOutcome::AlreadyCurrent));
        assert_eq!(
            prepare("config_version = \"8\"\nui_density = \"other\""),
            Ok(MigrationOutcome::Future { version: 8 })
        );
        for invalid in ["", "0", "-1", "1.5", "x", "18446744073709551616"] {
            assert_eq!(
                prepare(&format!("config_version = \"{invalid}\"")),
                Err(MigrationError::InvalidSchemaVersion)
            );
        }
    }

    #[test]
    fn duplicate_critical_keys_are_conflicts_even_when_values_match() {
        for key in [
            CriticalKey::ConfigVersion,
            CriticalKey::UiDensity,
            CriticalKey::IndicatorStyle,
            CriticalKey::MediaLayout,
        ] {
            let line = format!("{} = \"same\"\n", key.name());
            assert_eq!(
                prepare(&(line.clone() + &line)),
                Err(MigrationError::DuplicateCriticalKey(key))
            );
        }
        assert!(matches!(
            prepare(
                "# ui_density = \"minimal\"\nui_density = \"balanced\"\ncustom = \"x\"\ncustom = \"y\""
            ),
            Ok(MigrationOutcome::Prepared { .. })
        ));
        assert_eq!(
            prepare("ui_density = \"broken\nui_density = \"minimal\""),
            Err(MigrationError::DuplicateCriticalKey(CriticalKey::UiDensity))
        );
    }

    #[test]
    fn rejects_malformed_critical_values_and_unknown_bare_lines() {
        for input in [
            "ui_density",
            "ui_density = \"minimal",
            "ui_density = \"minimal\" trailing",
            "ui_density = minimal=other",
            "ui_density = \"minimal\"#bad",
            "ui_density : minimal",
            "ui_density = \"minimal\\\"x\"",
        ] {
            assert_eq!(
                prepare(input),
                Err(MigrationError::MalformedCriticalSetting(
                    CriticalKey::UiDensity
                )),
                "{input}"
            );
        }
        assert!(matches!(
            prepare("unparseable bare line"),
            Err(MigrationError::UnsupportedFormat { line: 1 })
        ));
        assert!(matches!(
            prepare("[unexpected]"),
            Err(MigrationError::UnsupportedFormat { .. })
        ));
    }

    #[test]
    fn unrelated_values_comments_and_sensitive_data_survive() {
        let source = "# ayarlar = burada\ncalendar_ics_url = \"https://example.invalid/a?x=1#fragment\"\ncustom = \"odd=\\\"value#text\"\n\n ui_density = \"minimal\"  # seçilen görünüm = eski\nindicator_style = \"context\" # keep\n";
        let output = migrated(source);
        assert!(output.contains("# ayarlar = burada\ncalendar_ics_url = \"https://example.invalid/a?x=1#fragment\"\ncustom = \"odd=\\\"value#text\"\n\n"));
        assert!(output.contains(" # seçilen görünüm = eski\n"));
        assert!(output.contains("indicator_style = \"context\" # keep\n"));
        assert_eq!(
            Settings::read(&output).presentation(),
            Settings::read(source).presentation()
        );
    }

    #[test]
    fn bom_line_endings_and_missing_terminal_newline_are_preserved() {
        for source in [
            "\u{feff}ui_density = \"minimal\"\r\n# açıklama\r\n",
            "\u{feff}indicator_style = \"context\"\nui_density = \"minimal\"",
            "# one\r\n# two\nui_density = \"detailed\"\r\n",
            "# header\r\nui_density = \"minimal\" # note",
        ] {
            let output = migrated(source);
            assert_eq!(
                output.starts_with('\u{feff}'),
                source.starts_with('\u{feff}')
            );
            assert_eq!(output.ends_with('\n'), source.ends_with('\n'));
            assert_eq!(
                Settings::read(&output).presentation(),
                Settings::read(source).presentation()
            );
            assert_eq!(prepare(&output), Ok(MigrationOutcome::AlreadyCurrent));
        }
        let crlf = migrated("# header\r\nui_density = \"minimal\"\r\n");
        assert!(!crlf.replace("\r\n", "").contains('\n'));
        let tie = migrated("# first\r\n# second\n");
        assert!(tie.ends_with("config_version = \"2\"\n"));
        let mixed = migrated("# first\r\nui_density = \"minimal\" # keep\nunknown = x\r\n");
        assert_eq!(
            mixed,
            "# first\r\n# keep\nunknown = x\r\nindicator_style = \"ring\"\r\nmedia_layout = \"compact\"\r\nconfig_version = \"2\"\r\n"
        );
        let without_newline = migrated("ui_density = \"minimal\"");
        assert!(!without_newline.ends_with('\n'));
        let only_removal = migrated(
            "config_version = \"1\"\r\nindicator_style = \"ring\"\r\nmedia_layout = \"compact\"\r\nui_density = \"minimal\"",
        );
        assert!(!only_removal.ends_with('\n'));
        assert!(only_removal.contains("config_version = \"2\"\r\n"));
    }

    #[test]
    fn bom_does_not_prevent_later_writes_from_preserving_the_marker() {
        let source = "\u{feff}indicator_style = \"ring\"\n";
        assert_eq!(
            crate::config::read_value(source, "indicator_style").as_deref(),
            Some("ring")
        );
        assert!(
            crate::config::write_value(source, "indicator_style", "edge").starts_with('\u{feff}')
        );
        assert!(
            crate::config::remove_settings(source, &["indicator_style"]).starts_with('\u{feff}')
        );
    }

    #[test]
    fn preparation_is_deterministic_bounded_and_idempotent() {
        let input = "# sakla\nui_density = detailed\nunknown = a=b#c\n";
        let first = prepare(input).unwrap();
        assert_eq!(first, prepare(input).unwrap());
        let MigrationOutcome::Prepared { text } = first else {
            panic!("migration expected")
        };
        assert_eq!(prepare(&text), Ok(MigrationOutcome::AlreadyCurrent));
        assert_eq!(
            prepare(&"x".repeat(MAX_CONFIG_BYTES + 1)),
            Err(MigrationError::InputTooLarge)
        );
        let maximum = format!("#{}\n", "x".repeat(MAX_CONFIG_BYTES - 2));
        assert_eq!(maximum.len(), MAX_CONFIG_BYTES);
        assert_eq!(prepare(&maximum), Err(MigrationError::OutputTooLarge));
    }

    #[test]
    fn absent_legacy_keeps_missing_feature_overrides_absent() {
        let input = "# keep\nindicator_style = \"context\"\n";
        assert_eq!(
            migrated(input),
            "# keep\nindicator_style = \"context\"\nconfig_version = \"2\"\n"
        );
        assert_eq!(
            prepare("config_version = \"2\"\nmedia_layout = \"invalid\""),
            Err(MigrationError::InvalidModernValue(CriticalKey::MediaLayout))
        );
    }
}
