use crate::presentation::IndicatorStyle;
use crate::scheduler::Presentation;

/// Up to two compact chips within the Detailed panel's inner width.
pub fn indicator_chip_bounds(labels: &[String]) -> Vec<crate::geometry::Rect> {
    use crate::geometry::Rect;
    let count = labels.len().min(2);
    if count == 0 {
        return Vec::new();
    }
    let left = 20.0;
    let available = IndicatorStyle::Context.level_layout().pill.width - left * 2.0;
    let gap = 8.0;
    let limit = (available - gap * (count - 1) as f32) / count as f32;
    let mut x = left;
    labels
        .iter()
        .take(count)
        .map(|label| {
            // An estimate keeps short chips compact; DirectWrite bounds long text.
            let width = (label.chars().count() as f32 * 5.3 + 20.0).clamp(52.0, limit);
            let rect = Rect {
                x,
                y: 79.0,
                width,
                height: 14.0,
            };
            x += width + gap;
            rect
        })
        .collect()
}

/// Level indicator geometry in logical pixels, shared by the compositor and painter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelLayout {
    pub pill: Pill,
    pub icon: crate::geometry::Rect,
    pub title: crate::geometry::Rect,
    pub detail: crate::geometry::Rect,
    pub value: crate::geometry::Rect,
    pub progress: crate::geometry::Rect,
}

impl IndicatorStyle {
    pub fn level_layout(self) -> LevelLayout {
        use crate::geometry::Rect;
        let rect = |x, y, width, height| Rect {
            x,
            y,
            width,
            height,
        };
        match self {
            Self::Ring => LevelLayout {
                pill: Pill {
                    width: 168.0,
                    height: 44.0,
                    radius: 22.0,
                },
                icon: rect(14.0, 8.0, 28.0, 28.0),
                title: rect(0.0, 0.0, 0.0, 0.0),
                detail: rect(0.0, 0.0, 0.0, 0.0),
                value: rect(50.0, 8.0, 60.0, 28.0),
                progress: rect(126.0, 11.0, 22.0, 22.0),
            },
            Self::Edge => LevelLayout {
                pill: Pill {
                    width: 296.0,
                    height: 70.0,
                    radius: 24.0,
                },
                icon: rect(16.0, 12.0, 38.0, 38.0),
                title: rect(66.0, 9.0, 130.0, 23.0),
                detail: rect(66.0, 32.0, 130.0, 18.0),
                value: rect(212.0, 12.0, 64.0, 32.0),
                progress: rect(20.0, 57.0, 256.0, 3.0),
            },
            Self::Context => LevelLayout {
                pill: Pill {
                    width: 320.0,
                    height: 96.0,
                    radius: 26.0,
                },
                icon: rect(16.0, 14.0, 42.0, 42.0),
                title: rect(70.0, 11.0, 160.0, 25.0),
                detail: rect(70.0, 37.0, 160.0, 19.0),
                value: rect(242.0, 16.0, 58.0, 30.0),
                progress: rect(20.0, 68.0, 280.0, 7.0),
            },
        }
    }
}

/// Size of the transparent window that hosts the pill, in DIPs. The pill can
/// overshoot its target while springing, so this is larger than `EXPANDED`.
pub const WINDOW_WIDTH: f32 = 460.0;
pub const WINDOW_HEIGHT: f32 = 220.0;

/// Gap between the screen edge and a floating pill.
pub const TOP_MARGIN: f32 = 8.0;

/// The compact pill fits what it shows, within these widths.
pub const COMPACT_MIN_WIDTH: f32 = 84.0;
pub const COMPACT_MAX_WIDTH: f32 = 260.0;

/// Size of the curve that joins a notch to the top of the screen.
pub const NOTCH_SHOULDER: f32 = 8.0;
/// Corners as round as a floating pill's would make a notch look like a blob.
const NOTCH_ROUNDING: f32 = 0.7;

/// Where the pill sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IslandStyle {
    /// Floating just below the top of the screen.
    #[default]
    Island,
    /// Grown out of the top of the screen.
    Notch,
}

impl IslandStyle {
    /// The name used for it in the settings file.
    pub fn setting(self) -> &'static str {
        match self {
            IslandStyle::Island => "island",
            IslandStyle::Notch => "notch",
        }
    }

    /// Gap between the top of the screen and the pill.
    pub fn margin(self) -> f32 {
        match self {
            IslandStyle::Island => TOP_MARGIN,
            IslandStyle::Notch => 0.0,
        }
    }

    /// Where the bubble beside `pill` starts, for pills scaled by `scale`. It
    /// floats level with a pill, and hangs from the top of the screen by a notch.
    pub fn bubble_top(self, scale: f32, pill: Pill, bubble: Pill) -> f32 {
        match self {
            IslandStyle::Island => TOP_MARGIN * scale + (pill.height - bubble.height) / 2.0,
            IslandStyle::Notch => 0.0,
        }
    }
}

/// Length of the invisible strip along the top edge that reveals the island.
pub const EDGE_TRIGGER_WIDTH: f32 = 160.0;

/// Vertical centre of the transport buttons inside the expanded pill.
pub const MEDIA_BUTTON_Y: f32 = 138.0;
/// Side length of each button's clickable area.
pub const MEDIA_BUTTON_SIZE: f32 = 42.0;

/// Gap between the pill and the bubble beside it.
pub const BUBBLE_GAP: f32 = 8.0;

/// Height of the strip under an open view that shows what else plays. The
/// window leaves no room for more, with the spring's overshoot.
pub const STRIP_HEIGHT: f32 = 22.0;

/// What the pill is made of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Material {
    #[default]
    Black,
    /// Shows a blurred view of what lies behind it.
    Glass,
}

/// How the compact music pill's bars move while music plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WaveStyle {
    Calm,
    #[default]
    Normal,
    Lively,
}

impl WaveStyle {
    pub const ALL: [Self; 3] = [Self::Calm, Self::Normal, Self::Lively];

    pub fn setting(self) -> &'static str {
        match self {
            Self::Calm => "calm",
            Self::Normal => "normal",
            Self::Lively => "lively",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Calm => "Sakin",
            Self::Normal => "Normal",
            Self::Lively => "Canlı",
        }
    }

    pub fn low(self) -> f32 {
        match self {
            Self::Calm => 0.45,
            Self::Normal => 0.38,
            Self::Lively => 0.15,
        }
    }

    pub fn high(self) -> f32 {
        match self {
            Self::Calm => 0.85,
            Self::Normal | Self::Lively => 1.0,
        }
    }

    pub fn period_ms(self, index: usize) -> u64 {
        let (base, offset) = match self {
            Self::Calm => (1050, 220),
            Self::Normal => (640, 170),
            Self::Lively => (390, 95),
        };
        base + index as u64 * offset
    }
}

impl Material {
    /// The name used for it in the settings file.
    pub fn setting(self) -> &'static str {
        match self {
            Material::Black => "black",
            Material::Glass => "glass",
        }
    }

    /// Glass asks more of the system, so it gives way when the user wants the
    /// battery to last or has turned transparency off for all of Windows.
    pub fn in_effect(self, transparency: bool, battery_saver: bool) -> Material {
        if transparency && !battery_saver {
            self
        } else {
            Material::Black
        }
    }
}

/// How the open view shows that files are on their way to a phone. The
/// compact pill always uses a small ring, as there is no room for more.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProgressStyle {
    /// The code turns into a ring that fills.
    #[default]
    Ring,
    /// Light runs along the edge of the island.
    Edge,
    /// The island fills from left to right.
    Fill,
}

impl ProgressStyle {
    pub const ALL: [ProgressStyle; 3] = [
        ProgressStyle::Ring,
        ProgressStyle::Edge,
        ProgressStyle::Fill,
    ];

    pub fn setting(self) -> &'static str {
        match self {
            ProgressStyle::Ring => "ring",
            ProgressStyle::Edge => "edge",
            ProgressStyle::Fill => "fill",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProgressStyle::Ring => "Halka",
            ProgressStyle::Edge => "Kenar ışığı",
            ProgressStyle::Fill => "Dolgu",
        }
    }
}

/// How focus, a timer or a file send shares the island with music.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TaskLayout {
    /// What lasts keeps the pill, and the task waits in the bubble beside it.
    #[default]
    Bubble,
    /// One pill shows both: what lasts on the left, the task on the right.
    Shared,
    /// The task takes the pill, and what lasts moves to the bubble.
    Front,
}

impl TaskLayout {
    pub const ALL: [TaskLayout; 3] = [TaskLayout::Bubble, TaskLayout::Shared, TaskLayout::Front];

    pub fn setting(self) -> &'static str {
        match self {
            TaskLayout::Bubble => "bubble",
            TaskLayout::Shared => "shared",
            TaskLayout::Front => "front",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TaskLayout::Bubble => "Baloncukta",
            TaskLayout::Shared => "Yan yana",
            TaskLayout::Front => "Öne çıkar",
        }
    }
}

/// The shortcut to the home view inside the expanded media view.
pub const HOME_SHORTCUT_X: f32 = 362.0;
pub const HOME_SHORTCUT_Y: f32 = 36.0;
pub const HOME_SHORTCUT_SIZE: f32 = 34.0;

pub fn on_home_shortcut(x: f32, y: f32) -> bool {
    (x - HOME_SHORTCUT_X).abs() <= HOME_SHORTCUT_SIZE / 2.0
        && (y - HOME_SHORTCUT_Y).abs() <= HOME_SHORTCUT_SIZE / 2.0
}

/// The settings button in the home view, on the row of the mode tabs.
pub const SETTINGS_SHORTCUT_X: f32 = HOME_SHORTCUT_X;
pub const SETTINGS_SHORTCUT_Y: f32 = MODE_TAB_Y;
pub const SETTINGS_SHORTCUT_SIZE: f32 = 28.0;

pub fn on_settings_shortcut(x: f32, y: f32) -> bool {
    (x - SETTINGS_SHORTCUT_X).abs() <= SETTINGS_SHORTCUT_SIZE / 2.0
        && (y - SETTINGS_SHORTCUT_Y).abs() <= SETTINGS_SHORTCUT_SIZE / 2.0
}

pub const QUICK_SHORTCUT: crate::geometry::Rect = crate::geometry::Rect {
    x: 225.0,
    y: 84.0,
    width: 112.0,
    height: 24.0,
};

/// What a preset in the home view starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StartMode {
    /// A real Windows focus session, which may also silence notifications.
    #[default]
    Focus,
    /// The island's own countdown, which Windows knows nothing about.
    Timer,
}

pub const MODE_TAB_Y: f32 = 96.0;
pub const MODE_TAB_HEIGHT: f32 = 24.0;

impl StartMode {
    pub const ALL: [StartMode; 2] = [StartMode::Focus, StartMode::Timer];

    /// Left edge and width of its tab inside the expanded pill.
    pub fn tab(self) -> (f32, f32) {
        match self {
            StartMode::Focus => (30.0, 58.0),
            StartMode::Timer => (92.0, 94.0),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StartMode::Focus => "Odak",
            StartMode::Timer => "Zamanlayıcı",
        }
    }

    /// The name used for it in the settings file.
    pub fn setting(self) -> &'static str {
        match self {
            StartMode::Focus => "focus",
            StartMode::Timer => "timer",
        }
    }

    pub fn at(x: f32, y: f32) -> Option<StartMode> {
        if (y - MODE_TAB_Y).abs() > MODE_TAB_HEIGHT / 2.0 {
            return None;
        }
        StartMode::ALL.into_iter().find(|mode| {
            let (left, width) = mode.tab();
            (left..=left + width).contains(&x)
        })
    }
}

/// Timer lengths offered in the home view by default, in minutes.
pub const TIMER_PRESETS: [u64; 4] = [5, 10, 15, 25];
pub const PRESET_Y: f32 = 130.0;
pub const PRESET_WIDTH: f32 = 76.0;
pub const PRESET_HEIGHT: f32 = 38.0;
const PRESET_GAP: f32 = 12.0;

pub fn preset_center_x(index: usize) -> f32 {
    let step = PRESET_WIDTH + PRESET_GAP;
    Pill::EXPANDED.width / 2.0 + (index as f32 - 1.5) * step
}

/// Which preset is under a point in DIPs relative to the expanded pill.
pub fn preset_at(x: f32, y: f32) -> Option<usize> {
    if (y - PRESET_Y).abs() > PRESET_HEIGHT / 2.0 {
        return None;
    }
    (0..TIMER_PRESETS.len()).find(|&index| (x - preset_center_x(index)).abs() <= PRESET_WIDTH / 2.0)
}

/// The ring of the expanded timer view, around the countdown.
pub const TIMER_RING_X: f32 = Pill::EXPANDED.width / 2.0;
pub const TIMER_RING_Y: f32 = Pill::EXPANDED.height / 2.0;
pub const TIMER_RING_RADIUS: f32 = 62.0;
pub const TIMER_RING_WIDTH: f32 = 6.0;

/// Round buttons on either side of the ring, level with its centre.
pub const TIMER_BUTTON_SIZE: f32 = 52.0;
const TIMER_BUTTON_REACH: f32 = 122.0;

/// Controls of the expanded timer view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerButton {
    Cancel,
    Toggle,
}

impl TimerButton {
    pub const ALL: [TimerButton; 2] = [TimerButton::Cancel, TimerButton::Toggle];

    pub fn center_x(self) -> f32 {
        match self {
            TimerButton::Cancel => TIMER_RING_X - TIMER_BUTTON_REACH,
            TimerButton::Toggle => TIMER_RING_X + TIMER_BUTTON_REACH,
        }
    }

    pub fn at(x: f32, y: f32) -> Option<TimerButton> {
        let half = TIMER_BUTTON_SIZE / 2.0;
        if (y - TIMER_RING_Y).abs() > half {
            return None;
        }
        TimerButton::ALL
            .into_iter()
            .find(|button| (x - button.center_x()).abs() <= half)
    }
}

/// The button that hides an event, in its expanded view.
pub const EVENT_HIDE_WIDTH: f32 = 96.0;
pub const EVENT_HIDE_HEIGHT: f32 = 34.0;

pub fn on_event_hide(x: f32, y: f32) -> bool {
    (x - Pill::EXPANDED.width / 2.0).abs() <= EVENT_HIDE_WIDTH / 2.0
        && (y - MEDIA_BUTTON_Y).abs() <= EVENT_HIDE_HEIGHT / 2.0
}

/// The position bar inside the expanded pill.
pub const PROGRESS_X: f32 = 24.0;
pub const PROGRESS_Y: f32 = 104.0;
pub const PROGRESS_WIDTH: f32 = Pill::EXPANDED.width - PROGRESS_X * 2.0;
pub const PROGRESS_HEIGHT: f32 = 4.0;
/// Clickable height. The bar alone would be a very small target.
const PROGRESS_HIT_HEIGHT: f32 = 22.0;
const PROGRESS_HIT_OVERHANG: f32 = 8.0;

/// Whether a point in DIPs relative to the expanded pill is on the position bar.
pub fn on_progress_bar(x: f32, y: f32) -> bool {
    let middle = PROGRESS_Y + PROGRESS_HEIGHT / 2.0;
    let across =
        PROGRESS_X - PROGRESS_HIT_OVERHANG..=PROGRESS_X + PROGRESS_WIDTH + PROGRESS_HIT_OVERHANG;
    (y - middle).abs() < PROGRESS_HIT_HEIGHT / 2.0 && across.contains(&x)
}

/// Where along the track a horizontal position falls, from 0 to 1.
pub fn progress_fraction(x: f32) -> f32 {
    ((x - PROGRESS_X) / PROGRESS_WIDTH).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaButton {
    Shuffle,
    Previous,
    PlayPause,
    Next,
    Repeat,
}

impl MediaButton {
    /// The buttons every player has.
    pub const TRANSPORT: [MediaButton; 3] = [
        MediaButton::Previous,
        MediaButton::PlayPause,
        MediaButton::Next,
    ];
    pub const ALL: [MediaButton; 5] = [
        MediaButton::Shuffle,
        MediaButton::Previous,
        MediaButton::PlayPause,
        MediaButton::Next,
        MediaButton::Repeat,
    ];

    /// Horizontal centre inside the expanded pill.
    pub fn center_x(self) -> f32 {
        let middle = Pill::EXPANDED.width / 2.0;
        match self {
            MediaButton::Shuffle => middle - 108.0,
            MediaButton::Previous => middle - 56.0,
            MediaButton::PlayPause => middle,
            MediaButton::Next => middle + 56.0,
            MediaButton::Repeat => middle + 108.0,
        }
    }

    /// The button under a point given in DIPs relative to the expanded pill.
    pub fn at(x: f32, y: f32) -> Option<MediaButton> {
        let half = MEDIA_BUTTON_SIZE / 2.0;
        if (y - MEDIA_BUTTON_Y).abs() > half {
            return None;
        }
        MediaButton::ALL
            .into_iter()
            .find(|button| (x - button.center_x()).abs() <= half)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pill {
    pub width: f32,
    pub height: f32,
    pub radius: f32,
}

impl Pill {
    /// Small sliver the pill collapses into before the window hides.
    pub const HIDDEN: Pill = Pill {
        width: 96.0,
        height: 8.0,
        radius: 4.0,
    };
    pub const BUBBLE: Pill = Pill {
        width: 36.0,
        height: 36.0,
        radius: 18.0,
    };
    pub const COMPACT: Pill = Pill {
        width: 210.0,
        height: 36.0,
        radius: 18.0,
    };
    pub const LEVEL: Pill = Pill {
        width: 280.0,
        height: 44.0,
        radius: 22.0,
    };
    pub const ALERT: Pill = Pill {
        width: 320.0,
        height: 66.0,
        radius: 33.0,
    };
    pub const EXPANDED: Pill = Pill {
        width: 400.0,
        height: 170.0,
        radius: 42.0,
    };
    /// The open view with a strip along its bottom, for music under the code.
    pub const EXPANDED_WITH_STRIP: Pill = Pill {
        height: Pill::EXPANDED.height + STRIP_HEIGHT,
        ..Pill::EXPANDED
    };

    pub fn for_presentation(presentation: Presentation) -> Pill {
        match presentation {
            Presentation::Hidden => Pill::HIDDEN,
            Presentation::Compact => Pill::COMPACT,
            Presentation::Level => Pill::LEVEL,
            Presentation::Alert => Pill::ALERT,
            Presentation::Expanded => Pill::EXPANDED,
        }
    }

    pub fn scaled(self, scale: f32) -> Pill {
        Pill {
            width: self.width * scale,
            height: self.height * scale,
            radius: self.radius * scale,
        }
    }

    /// This pill as a notch reaching `reach` down from the top of the screen.
    /// Returns where its top goes, which is above the screen by exactly its
    /// radius, so the upper corners are out of sight and the lower ones stay.
    pub fn as_notch(self, reach: f32) -> (f32, Pill) {
        let radius = self.radius * NOTCH_ROUNDING;
        (
            -radius,
            Pill {
                height: reach + radius,
                radius,
                ..self
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_chips_are_bounded_and_never_overlap() {
        assert!(indicator_chip_bounds(&[]).is_empty());
        for count in [1, 2, 3, 100] {
            let labels = vec!["Long context label to be clipped".to_owned(); count];
            let chips = indicator_chip_bounds(&labels);
            assert_eq!(chips.len(), count.min(2));
            let panel = IndicatorStyle::Context.level_layout();
            for scale in [1.0, 1.25, 1.5, 2.0] {
                for chip in &chips {
                    assert!(chip.x >= 0.0 && chip.y >= panel.progress.y + panel.progress.height);
                    assert!((chip.x + chip.width) * scale <= panel.pill.width * scale);
                    assert!((chip.y + chip.height) * scale <= panel.pill.height * scale);
                }
                if chips.len() == 2 {
                    assert!(chips[0].x + chips[0].width < chips[1].x);
                }
            }
        }
    }

    #[test]
    fn indicator_layouts_fit_and_keep_content_separate_at_common_scales() {
        assert_eq!(IndicatorStyle::default(), IndicatorStyle::Edge);
        for density in IndicatorStyle::ALL {
            let layout = density.level_layout();
            for scale in [1.0, 1.25, 1.5, 2.0] {
                for r in [
                    layout.icon,
                    layout.title,
                    layout.detail,
                    layout.value,
                    layout.progress,
                ] {
                    assert!(r.x >= 0.0 && r.y >= 0.0);
                    assert!((r.x + r.width) * scale <= layout.pill.width * scale);
                    assert!((r.y + r.height) * scale <= layout.pill.height * scale);
                }
                assert!((layout.pill.width * 1.1) <= WINDOW_WIDTH);
                assert!((layout.pill.height * 1.1 + TOP_MARGIN) <= WINDOW_HEIGHT);
            }
            assert!(layout.icon.x + layout.icon.width <= layout.value.x);
            if density != IndicatorStyle::Ring {
                assert!(layout.title.x + layout.title.width <= layout.value.x);
                assert!(layout.detail.x + layout.detail.width <= layout.value.x);
                assert!(layout.progress.y >= layout.detail.y + layout.detail.height);
            } else {
                assert!(layout.value.x + layout.value.width <= layout.progress.x);
            }
        }
    }

    #[test]
    fn wave_styles_keep_the_old_motion_and_change_its_range_and_pace() {
        assert_eq!(
            (WaveStyle::Normal.low(), WaveStyle::Normal.high()),
            (0.38, 1.0)
        );
        assert_eq!(
            (0..4)
                .map(|index| WaveStyle::Normal.period_ms(index))
                .collect::<Vec<_>>(),
            vec![640, 810, 980, 1150]
        );
        assert!(WaveStyle::Calm.low() > WaveStyle::Normal.low());
        assert!(WaveStyle::Calm.high() < WaveStyle::Normal.high());
        assert!(WaveStyle::Lively.low() < WaveStyle::Normal.low());
        for index in 0..4 {
            assert!(WaveStyle::Lively.period_ms(index) < WaveStyle::Normal.period_ms(index));
            assert!(WaveStyle::Normal.period_ms(index) < WaveStyle::Calm.period_ms(index));
        }
    }

    #[test]
    fn every_pill_fits_inside_the_window_with_room_to_overshoot() {
        let overshoot = 1.1;
        for pill in [
            Pill::HIDDEN,
            Pill::COMPACT,
            Pill::LEVEL,
            Pill::ALERT,
            Pill::EXPANDED,
            Pill::EXPANDED_WITH_STRIP,
        ] {
            assert!(pill.width * overshoot <= WINDOW_WIDTH, "{pill:?}");
            assert!(
                pill.height * overshoot + TOP_MARGIN <= WINDOW_HEIGHT,
                "{pill:?}"
            );
        }
    }

    #[test]
    fn radius_never_exceeds_half_the_height() {
        for pill in [
            Pill::HIDDEN,
            Pill::COMPACT,
            Pill::LEVEL,
            Pill::ALERT,
            Pill::EXPANDED,
        ] {
            assert!(pill.radius * 2.0 <= pill.height, "{pill:?}");
        }
    }

    #[test]
    fn every_pill_still_fits_with_a_bubble_beside_it() {
        for pill in [Pill::COMPACT, Pill::LEVEL, Pill::ALERT, Pill::EXPANDED] {
            assert!(
                pill.width + BUBBLE_GAP + Pill::BUBBLE.width <= WINDOW_WIDTH,
                "{pill:?}"
            );
        }
        const { assert!(COMPACT_MAX_WIDTH + BUBBLE_GAP + Pill::BUBBLE.width <= WINDOW_WIDTH) };
    }

    #[test]
    fn presets_are_found_by_position_and_fit_the_pill() {
        for index in 0..TIMER_PRESETS.len() {
            assert_eq!(preset_at(preset_center_x(index), PRESET_Y), Some(index));
        }
        let half = PRESET_WIDTH / 2.0;
        assert!(preset_center_x(0) - half >= 0.0);
        assert!(preset_center_x(3) + half <= Pill::EXPANDED.width);
        const { assert!(PRESET_Y + PRESET_HEIGHT / 2.0 <= Pill::EXPANDED.height) };
        assert_eq!(preset_at(preset_center_x(0), 40.0), None);
    }

    #[test]
    fn mode_tabs_are_found_by_position_and_clear_the_presets() {
        for mode in StartMode::ALL {
            let (left, width) = mode.tab();
            assert_eq!(StartMode::at(left + width / 2.0, MODE_TAB_Y), Some(mode));
            assert!(left + width <= Pill::EXPANDED.width);
        }
        assert_eq!(StartMode::at(StartMode::Focus.tab().0, 30.0), None);
        const { assert!(MODE_TAB_Y + MODE_TAB_HEIGHT / 2.0 < PRESET_Y - PRESET_HEIGHT / 2.0) };
        assert_eq!(preset_at(40.0, MODE_TAB_Y), None);
    }

    #[test]
    fn the_hide_button_is_found_and_fits_the_pill() {
        assert!(on_event_hide(Pill::EXPANDED.width / 2.0, MEDIA_BUTTON_Y));
        assert!(!on_event_hide(40.0, MEDIA_BUTTON_Y));
        assert!(!on_event_hide(Pill::EXPANDED.width / 2.0, 40.0));
        const { assert!(MEDIA_BUTTON_Y + EVENT_HIDE_HEIGHT / 2.0 <= Pill::EXPANDED.height) };
    }

    #[test]
    fn timer_buttons_are_found_by_position() {
        for button in TimerButton::ALL {
            assert_eq!(
                TimerButton::at(button.center_x(), TIMER_RING_Y),
                Some(button)
            );
        }
        assert_eq!(TimerButton::at(TIMER_RING_X, TIMER_RING_Y), None);
        assert_eq!(TimerButton::at(TimerButton::Cancel.center_x(), 10.0), None);
    }

    #[test]
    fn the_timer_ring_fits_the_pill_and_leaves_room_for_its_buttons() {
        let half = TIMER_BUTTON_SIZE / 2.0;
        let outer = TIMER_RING_RADIUS + TIMER_RING_WIDTH / 2.0;
        assert!(TIMER_RING_Y - outer >= 0.0);
        assert!(TIMER_RING_Y + outer <= Pill::EXPANDED.height);
        assert!(TimerButton::Cancel.center_x() + half < TIMER_RING_X - outer);
        assert!(TimerButton::Toggle.center_x() - half > TIMER_RING_X + outer);
        assert!(TimerButton::Cancel.center_x() - half >= 0.0);
        assert!(TimerButton::Toggle.center_x() + half <= Pill::EXPANDED.width);
    }

    #[test]
    fn timer_buttons_stay_clear_of_the_home_shortcut() {
        const {
            assert!(
                TIMER_RING_Y - TIMER_BUTTON_SIZE / 2.0 > HOME_SHORTCUT_Y + HOME_SHORTCUT_SIZE / 2.0
            )
        };
        for button in TimerButton::ALL {
            assert!(!on_home_shortcut(button.center_x(), TIMER_RING_Y));
        }
    }

    #[test]
    fn glass_gives_way_to_battery_saver_and_plain_windows() {
        assert_eq!(Material::Glass.in_effect(true, false), Material::Glass);
        assert_eq!(Material::Glass.in_effect(true, true), Material::Black);
        assert_eq!(Material::Glass.in_effect(false, false), Material::Black);
        assert_eq!(Material::Black.in_effect(true, false), Material::Black);
    }

    #[test]
    fn the_settings_shortcut_stays_clear_of_the_tabs_and_presets() {
        let half = SETTINGS_SHORTCUT_SIZE / 2.0;
        assert!(SETTINGS_SHORTCUT_X + half <= Pill::EXPANDED.width);
        assert!(SETTINGS_SHORTCUT_Y + half <= PRESET_Y - PRESET_HEIGHT / 2.0);
        for mode in StartMode::ALL {
            let (left, width) = mode.tab();
            assert!(left + width < SETTINGS_SHORTCUT_X - half);
        }
        assert!(on_settings_shortcut(
            SETTINGS_SHORTCUT_X,
            SETTINGS_SHORTCUT_Y
        ));
        assert!(!on_settings_shortcut(100.0, SETTINGS_SHORTCUT_Y));
    }

    #[test]
    fn the_home_shortcut_stays_inside_the_pill_and_off_the_bar() {
        let half = HOME_SHORTCUT_SIZE / 2.0;
        assert!(HOME_SHORTCUT_X + half <= Pill::EXPANDED.width);
        assert!(HOME_SHORTCUT_Y + half < PROGRESS_Y);
        assert!(on_home_shortcut(HOME_SHORTCUT_X, HOME_SHORTCUT_Y));
        assert!(!on_home_shortcut(100.0, HOME_SHORTCUT_Y));
    }

    #[test]
    fn the_position_bar_is_easier_to_hit_than_it_is_to_see() {
        let middle = PROGRESS_Y + PROGRESS_HEIGHT / 2.0;
        assert!(on_progress_bar(200.0, middle));
        assert!(on_progress_bar(200.0, middle + 9.0));
        assert!(on_progress_bar(PROGRESS_X - 4.0, middle));
        assert!(!on_progress_bar(200.0, 40.0));
        assert!(!on_progress_bar(4.0, middle));
    }

    #[test]
    fn the_position_bar_never_takes_a_click_meant_for_a_button() {
        for button in MediaButton::ALL {
            for y in [MEDIA_BUTTON_Y - MEDIA_BUTTON_SIZE / 2.0, MEDIA_BUTTON_Y] {
                assert!(!on_progress_bar(button.center_x(), y));
            }
        }
    }

    #[test]
    fn the_fraction_stays_on_the_track() {
        assert_eq!(progress_fraction(PROGRESS_X), 0.0);
        assert_eq!(progress_fraction(PROGRESS_X + PROGRESS_WIDTH / 2.0), 0.5);
        assert_eq!(progress_fraction(PROGRESS_X + PROGRESS_WIDTH), 1.0);
        assert_eq!(progress_fraction(-500.0), 0.0);
        assert_eq!(progress_fraction(5_000.0), 1.0);
    }

    #[test]
    fn media_buttons_are_found_by_position() {
        for button in MediaButton::ALL {
            assert_eq!(
                MediaButton::at(button.center_x(), MEDIA_BUTTON_Y),
                Some(button)
            );
        }
    }

    #[test]
    fn clicks_between_and_around_media_buttons_hit_nothing() {
        let between = (MediaButton::Previous.center_x() + MediaButton::PlayPause.center_x()) / 2.0;
        assert_eq!(MediaButton::at(between, MEDIA_BUTTON_Y), None);
        assert_eq!(
            MediaButton::at(MediaButton::PlayPause.center_x(), 40.0),
            None
        );
        assert_eq!(MediaButton::at(10.0, MEDIA_BUTTON_Y), None);
    }

    #[test]
    fn media_buttons_stay_inside_the_expanded_pill() {
        let half = MEDIA_BUTTON_SIZE / 2.0;
        assert!(MEDIA_BUTTON_Y + half <= Pill::EXPANDED.height);
        assert!(MediaButton::Shuffle.center_x() - half >= 0.0);
        assert!(MediaButton::Repeat.center_x() + half <= Pill::EXPANDED.width);
    }

    #[test]
    fn media_buttons_do_not_overlap() {
        for pair in MediaButton::ALL.windows(2) {
            assert!(pair[1].center_x() - pair[0].center_x() >= MEDIA_BUTTON_SIZE);
        }
    }

    #[test]
    fn a_notch_hides_its_upper_corners_and_ends_where_it_should() {
        let (top, notch) = Pill::COMPACT.as_notch(Pill::COMPACT.height);
        assert_eq!(top + notch.height, Pill::COMPACT.height);
        assert_eq!(top, -notch.radius);
        assert_eq!(notch.width, Pill::COMPACT.width);
        assert!(notch.radius < Pill::COMPACT.radius);
    }

    #[test]
    fn a_notch_starts_at_the_top_of_the_screen() {
        assert_eq!(IslandStyle::Notch.margin(), 0.0);
        assert_eq!(IslandStyle::Island.margin(), TOP_MARGIN);
    }

    #[test]
    fn the_bubble_floats_level_with_a_pill_and_hangs_with_a_notch() {
        let (pill, bubble) = (Pill::ALERT, Pill::BUBBLE);
        let level = TOP_MARGIN + (pill.height - bubble.height) / 2.0;
        assert_eq!(IslandStyle::Island.bubble_top(1.0, pill, bubble), level);
        assert_eq!(IslandStyle::Notch.bubble_top(1.0, pill, bubble), 0.0);
    }

    #[test]
    fn scaling_applies_to_every_dimension() {
        let scaled = Pill::COMPACT.scaled(1.5);
        assert_eq!(
            scaled,
            Pill {
                width: 315.0,
                height: 54.0,
                radius: 27.0
            }
        );
    }
}
