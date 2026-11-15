//! Typed feature preferences. `None` means no valid explicit override.
use crate::config;
use crate::layout::{
    IslandStyle, Material, ProgressStyle, StartMode, TIMER_PRESETS, TaskLayout, WaveStyle,
};
use crate::presentation::{IndicatorStyle, MediaLayout, PresentationPreferences};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Appearance {
    pub island_style: Option<IslandStyle>,
    pub material: Option<Material>,
    pub reduced_motion: Option<bool>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemIndicators {
    pub indicator_style: Option<IndicatorStyle>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MusicPlayer {
    pub media_layout: Option<MediaLayout>,
    pub sound_wave: Option<bool>,
    pub wave_style: Option<WaveStyle>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activities {
    pub task_layout: Option<TaskLayout>,
    pub progress_style: Option<ProgressStyle>,
    pub timer_presets: Option<[u64; TIMER_PRESETS.len()]>,
    pub start_mode: Option<StartMode>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemNotifications {
    pub notices_off: Option<Vec<String>>,
    pub windows_notifications: Option<bool>,
    pub focus_hides_notifications: Option<bool>,
    pub muted_apps: Option<Vec<String>>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuickControls {
    pub timeout_seconds: Option<u64>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct General {
    pub autostart: Option<bool>,
    pub monitor: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub appearance: Appearance,
    pub indicators: SystemIndicators,
    pub music: MusicPlayer,
    pub activities: Activities,
    pub notifications: SystemNotifications,
    pub quick_controls: QuickControls,
    pub general: General,
    // Legacy is a fallback, not an explicit feature override.
    legacy_presentation: PresentationPreferences,
}
fn boolean(text: &str, key: &str) -> Option<bool> {
    match config::read_value(text, key)?.as_str() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}
fn list(text: &str, key: &str) -> Option<Vec<String>> {
    // An empty list is a valid explicit choice.
    config::contains_setting(text, key).then(|| {
        config::read_value(text, key)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    })
}
impl Settings {
    pub fn read(text: &str) -> Self {
        let indicator = config::read_value(text, "indicator_style");
        let media = config::read_value(text, "media_layout");
        let wave = config::read_value(text, "wave_style");
        let task = config::read_value(text, "task_layout");
        let progress = config::read_value(text, "progress_style");
        Self {
            appearance: Appearance {
                island_style: config::read_island_style(text),
                material: config::read_material(text),
                reduced_motion: boolean(text, "reduced_motion"),
            },
            indicators: SystemIndicators {
                indicator_style: IndicatorStyle::ALL
                    .into_iter()
                    .find(|v| indicator.as_deref() == Some(v.setting())),
            },
            music: MusicPlayer {
                media_layout: MediaLayout::ALL
                    .into_iter()
                    .find(|v| media.as_deref() == Some(v.setting())),
                sound_wave: boolean(text, "sound_wave"),
                wave_style: WaveStyle::ALL
                    .into_iter()
                    .find(|v| wave.as_deref() == Some(v.setting())),
            },
            activities: Activities {
                task_layout: TaskLayout::ALL
                    .into_iter()
                    .find(|v| task.as_deref() == Some(v.setting())),
                progress_style: ProgressStyle::ALL
                    .into_iter()
                    .find(|v| progress.as_deref() == Some(v.setting())),
                timer_presets: config::read_presets(text),
                start_mode: config::read_start_mode(text),
            },
            notifications: SystemNotifications {
                notices_off: list(text, "notices_off"),
                windows_notifications: boolean(text, "windows_notifications"),
                focus_hides_notifications: boolean(text, "focus_hides_notifications"),
                muted_apps: list(text, "muted_apps"),
            },
            quick_controls: QuickControls {
                timeout_seconds: config::read_value(text, "quick_controls_timeout")
                    .and_then(|v| v.parse().ok())
                    .filter(|v| config::QUICK_TIMEOUTS.contains(v)),
            },
            general: General {
                autostart: config::read_autostart(text),
                monitor: config::read_value(text, "monitor"),
            },
            legacy_presentation: config::read_presentation_preferences(&config::remove_settings(
                text,
                &["indicator_style", "media_layout"],
            )),
        }
    }
    pub fn presentation(&self) -> PresentationPreferences {
        PresentationPreferences {
            indicator_style: self
                .indicators
                .indicator_style
                .unwrap_or(self.legacy_presentation.indicator_style),
            media_layout: self
                .music
                .media_layout
                .unwrap_or(self.legacy_presentation.media_layout),
        }
    }
}

impl Appearance {
    pub fn island_style(&self) -> IslandStyle {
        self.island_style.unwrap_or_default()
    }
    pub fn material(&self) -> Material {
        self.material.unwrap_or_default()
    }
    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion.unwrap_or(false)
    }
}
impl MusicPlayer {
    pub fn sound_wave(&self) -> bool {
        self.sound_wave.unwrap_or(true)
    }
    pub fn wave_style(&self) -> WaveStyle {
        self.wave_style.unwrap_or_default()
    }
}
impl Activities {
    pub fn task_layout(&self) -> TaskLayout {
        self.task_layout.unwrap_or_default()
    }
    pub fn progress_style(&self) -> ProgressStyle {
        self.progress_style.unwrap_or_default()
    }
    pub fn timer_presets(&self) -> [u64; TIMER_PRESETS.len()] {
        self.timer_presets.unwrap_or(TIMER_PRESETS)
    }
    pub fn start_mode(&self) -> StartMode {
        self.start_mode.unwrap_or_default()
    }
}
impl SystemNotifications {
    pub fn windows_notifications(&self) -> bool {
        self.windows_notifications.unwrap_or(false)
    }
    pub fn focus_hides_notifications(&self) -> bool {
        self.focus_hides_notifications.unwrap_or(true)
    }
}
impl QuickControls {
    pub fn timeout_seconds(&self) -> u64 {
        self.timeout_seconds
            .unwrap_or(crate::quick_controls::DEFAULT_INACTIVITY_MS / 1000)
    }
}
/// Why a preference is temporarily constrained. The preference itself remains unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Constraint {
    EnergySaver,
    ReducedMotion,
    TransparencyDisabled,
    WaveDisabled,
}

/// Availability is separate from the effective value: a temporary motion
/// constraint does not prevent the user from choosing a future wave style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Available,
    TemporarilyUnavailable(&'static str),
    ParentDisabled(&'static str),
    Unsupported(&'static str),
}

impl MusicPlayer {
    pub fn wave_style_availability(&self) -> Availability {
        if self.sound_wave() {
            Availability::Available
        } else {
            Availability::ParentDisabled("Enable Sound Wave to customize this setting.")
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveBehavior {
    Hidden,
    Still,
    Animated,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effective<T> {
    pub value: T,
    pub constraint: Option<Constraint>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeContext {
    pub energy_saver: bool,
    pub reduced_motion: bool,
    pub force_wave_still: bool,
    pub transparency_disabled: bool,
}
impl Settings {
    pub fn wave(&self, context: RuntimeContext) -> Effective<WaveBehavior> {
        if context.force_wave_still
            || context.reduced_motion
            || self.appearance.reduced_motion.unwrap_or(false)
        {
            Effective {
                value: WaveBehavior::Still,
                constraint: Some(Constraint::ReducedMotion),
            }
        } else if !self.music.sound_wave.unwrap_or(true) {
            Effective {
                value: WaveBehavior::Hidden,
                constraint: Some(Constraint::WaveDisabled),
            }
        } else if context.energy_saver {
            Effective {
                value: WaveBehavior::Hidden,
                constraint: Some(Constraint::EnergySaver),
            }
        } else {
            Effective {
                value: WaveBehavior::Animated,
                constraint: None,
            }
        }
    }
    pub fn material(&self, context: RuntimeContext) -> Effective<Material> {
        let chosen = self.appearance.material.unwrap_or_default();
        let constraint = if chosen != Material::Glass {
            None
        } else if context.energy_saver {
            Some(Constraint::EnergySaver)
        } else if context.transparency_disabled {
            Some(Constraint::TransparencyDisabled)
        } else {
            None
        };
        Effective {
            value: if constraint.is_some() {
                Material::Black
            } else {
                chosen
            },
            constraint,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    Appearance,
    SystemIndicators,
    MusicPlayer,
    Activities,
    SystemNotifications,
    QuickControls,
    General,
}
impl Feature {
    pub const ALL: [Self; 7] = [
        Self::Appearance,
        Self::SystemIndicators,
        Self::MusicPlayer,
        Self::Activities,
        Self::SystemNotifications,
        Self::QuickControls,
        Self::General,
    ];
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            Self::Appearance => &["island_style", "material", "reduced_motion"],
            Self::SystemIndicators => &["indicator_style"],
            Self::MusicPlayer => &["media_layout", "sound_wave", "wave_style"],
            Self::Activities => &[
                "task_layout",
                "progress_style",
                "timer_presets",
                "start_mode",
            ],
            Self::SystemNotifications => &[
                "notices_off",
                "windows_notifications",
                "focus_hides_notifications",
                "muted_apps",
            ],
            Self::QuickControls => &["quick_controls_timeout"],
            Self::General => &["autostart", "monitor"],
        }
    }
}
/// Remove this feature's overrides while retaining legacy compatibility, calendar
/// credentials, and unknown keys. Legacy density can still provide a fallback.
pub fn reset_feature(text: &str, feature: Feature) -> String {
    config::remove_settings(text, feature.keys())
}
pub fn reset_all(text: &str) -> String {
    Feature::ALL
        .into_iter()
        .fold(text.to_owned(), |s, feature| reset_feature(&s, feature))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_wave_retains_its_independent_style() {
        let settings = Settings::read(
            "sound_wave = \"off\"\nwave_style = \"lively\"\nmedia_layout = \"compact\"\n",
        );
        assert_eq!(settings.music.wave_style(), WaveStyle::Lively);
        assert!(matches!(
            settings.music.wave_style_availability(),
            Availability::ParentDisabled(_)
        ));
        let mut enabled = settings.clone();
        enabled.music.sound_wave = Some(true);
        assert_eq!(
            enabled.music.wave_style_availability(),
            Availability::Available
        );
        assert_eq!(enabled.music.wave_style(), WaveStyle::Lively);
        assert_eq!(enabled.music.media_layout, Some(MediaLayout::Compact));
    }

    #[test]
    fn quick_controls_reset_does_not_touch_activities_or_secrets() {
        let text = "# comment\nquick_controls_timeout = \"0\"\ntimer_presets = \"3, 7, 20, 30\"\ncalendar_ics_url = \"https://example.test/?secret=1\"\nunknown = \"keep\"\n";
        let reset = reset_feature(text, Feature::QuickControls);
        assert_eq!(Settings::read(text).quick_controls.timeout_seconds(), 0);
        assert_eq!(Settings::read(&reset).quick_controls.timeout_seconds(), 8);
        assert!(reset.contains("timer_presets = \"3, 7, 20, 30\""));
        assert!(reset.contains("calendar_ics_url = \"https://example.test/?secret=1\""));
        assert!(reset.contains("# comment\n"));
        assert!(reset.contains("unknown = \"keep\""));
    }

    #[test]
    fn indicator_reset_preserves_music_and_legacy_fallback() {
        let text = "ui_density = \"minimal\"\nindicator_style = \"context\"\nmedia_layout = \"informative\"\nsound_wave = \"off\"\n";
        let result = reset_feature(text, Feature::SystemIndicators);
        let settings = Settings::read(&result);
        assert_eq!(settings.indicators.indicator_style, None);
        assert_eq!(
            settings.presentation().indicator_style,
            IndicatorStyle::Ring
        );
        assert_eq!(
            settings.presentation().media_layout,
            MediaLayout::Informative
        );
        assert_eq!(settings.music.sound_wave, Some(false));
        assert!(result.contains("ui_density = \"minimal\""));
    }
    #[test]
    fn overrides_and_legacy_are_independent() {
        let settings = Settings::read(
            "ui_density = \"detailed\"\nindicator_style = \"ring\"\nmedia_layout = \"bad\"\nquick_controls_timeout = \"999\"",
        );
        assert_eq!(
            settings.indicators.indicator_style,
            Some(IndicatorStyle::Ring)
        );
        assert_eq!(settings.music.media_layout, None);
        assert_eq!(
            settings.presentation().media_layout,
            MediaLayout::Informative
        );
        assert_eq!(settings.quick_controls.timeout_seconds, None);
        assert_eq!(
            Settings::read("").presentation(),
            PresentationPreferences::default()
        );
    }
    #[test]
    fn constraints_do_not_change_preferences() {
        let settings = Settings::read("sound_wave = \"on\"\nmaterial = \"glass\"");
        let saver = RuntimeContext {
            energy_saver: true,
            ..Default::default()
        };
        assert_eq!(
            settings.wave(saver).constraint,
            Some(Constraint::EnergySaver)
        );
        assert_eq!(settings.material(saver).value, Material::Black);
        assert_eq!(
            settings.wave(RuntimeContext::default()).value,
            WaveBehavior::Animated
        );
        assert_eq!(
            settings.material(RuntimeContext::default()).value,
            Material::Glass
        );
        assert_eq!(settings.music.sound_wave, Some(true));
    }
    #[test]
    fn resets_keep_secrets_other_features_comments_and_duplicates_safe() {
        let text = "# header\r\ncalendar_ics_url = \"https://example/?token=secret#x\"\r\nui_density = \"detailed\"\r\nmedia_layout = \"compact\" # chosen\r\nmedia_layout = \"informative\"\r\nindicator_style = \"ring\"\r\nunknown = \"keep\"\r\n";
        let reset = reset_feature(text, Feature::MusicPlayer);
        let settings = Settings::read(&reset);
        assert_eq!(settings.music.media_layout, None);
        assert_eq!(
            settings.presentation().media_layout,
            MediaLayout::Informative
        );
        assert_eq!(
            settings.presentation().indicator_style,
            IndicatorStyle::Ring
        );
        assert!(reset.contains("# chosen"));
        assert!(reset.contains("token=secret#x"));
        assert!(reset.contains("unknown = \"keep\""));
        assert_eq!(
            config::read_value(&reset, "calendar_ics_url").unwrap(),
            "https://example/?token=secret#x"
        );
        let all = reset_all(text);
        let all_settings = Settings::read(&all);
        assert_eq!(all_settings.music.media_layout, None);
        assert_eq!(all_settings.indicators.indicator_style, None);
        assert_eq!(
            all_settings.presentation().media_layout,
            MediaLayout::Informative
        );
        assert!(all.contains("token=secret#x"));
    }
}
