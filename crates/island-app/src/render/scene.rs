//! Turns a frame into what gets painted. A `Scene` is plain data, so comparing
//! two of them tells whether the pixels would change at all.

use island_core::presentation::{IndicatorStyle, MediaLayout, PresentationPreferences};
use std::sync::Arc;

use island_core::MediaSnapshot;
use island_core::convert::Target;
use island_core::drop::{
    CONVERT_NOTE, DropView, HEADER, Rect, SHELF_BACK, SHELF_CLEAR, SHELF_FORWARD, SHELF_TILES,
    TRANSFER_CODE, TRANSFER_DIRECT, TRANSFER_STOP, TRANSFER_TEXT_LEFT, Zone, remove_button,
    thumbnail, tile_name,
};
use island_core::layout::{
    COMPACT_MAX_WIDTH, COMPACT_MIN_WIDTH, EVENT_HIDE_HEIGHT, EVENT_HIDE_WIDTH, HOME_SHORTCUT_SIZE,
    HOME_SHORTCUT_X, HOME_SHORTCUT_Y, MEDIA_BUTTON_Y, MODE_TAB_HEIGHT, MODE_TAB_Y, MediaButton,
    PRESET_HEIGHT, PRESET_WIDTH, PRESET_Y, PROGRESS_HEIGHT, PROGRESS_WIDTH, PROGRESS_X, PROGRESS_Y,
    QUICK_SHORTCUT, SETTINGS_SHORTCUT_SIZE, SETTINGS_SHORTCUT_X, SETTINGS_SHORTCUT_Y, STRIP_HEIGHT,
    StartMode, TIMER_BUTTON_SIZE, TIMER_PRESETS, TIMER_RING_RADIUS, TIMER_RING_WIDTH, TIMER_RING_X,
    TIMER_RING_Y, TimerButton, preset_center_x,
};
use island_core::media::{RepeatMode, Sides, format_time};
use island_core::quick_controls::{
    BACK_BOUNDS, CALENDAR_SETUP_BOUNDS, Control as QuickControl, MUTE_BOUNDS, Page as QuickPage,
    RadioState, SETTINGS_BOUNDS, SLIDER_BOUNDS, Target as QuickTarget,
};
use island_core::scheduler::QuickControlsView;
use island_core::shelf::ShelfView;
use island_core::transfer::{Direct, Shown, TransferView};
use island_core::{
    Bubble, CalendarState, ConvertView, Frame, HomeView, Icon, Kind, Millis, PanelView, Pill,
    Presentation, ProgressStyle, TimerView,
};

use super::text::{
    Align, BarRun, Bounds, CodeRun, Font, ImageRun, OutlineRun, Painting, RingRun, ShapeRun,
    TextRun,
};

const ICON_NOTE: &str = "\u{E8D6}";
const ICON_PLAY: &str = "\u{E768}";
const ICON_PAUSE: &str = "\u{E769}";
const ICON_PREVIOUS: &str = "\u{E892}";
const ICON_NEXT: &str = "\u{E893}";
const ICON_CANCEL: &str = "\u{E711}";
const ICON_STOPWATCH: &str = "\u{E916}";
const ICON_SETTINGS: &str = "\u{E713}";
const ICON_SHUFFLE: &str = "\u{E8B1}";
const ICON_REPEAT_ALL: &str = "\u{E8EE}";
const ICON_REPEAT_ONE: &str = "\u{E8ED}";
const ICON_VIDEO: &str = "\u{E714}";
const ICON_BACK_10: &str = "\u{ED3C}";
const ICON_FORWARD_30: &str = "\u{ED3D}";
/// A button the player does not take, shown so the row keeps its shape.
const UNAVAILABLE_OPACITY: f32 = 0.35;
const ICON_DOCUMENT: &str = "\u{E8A5}";
const ICON_FOLDER: &str = "\u{E8B7}";
const ICON_CLOSE: &str = "\u{E8BB}";
const ICON_BACK: &str = "\u{E76B}";
const ICON_FORWARD: &str = "\u{E76C}";

const SHELF_COLOR: [f32; 3] = [0.33, 0.58, 1.0];
const PHONE_COLOR: [f32; 3] = [0.25, 0.82, 0.48];
const CONVERT_COLOR: [f32; 3] = [0.68, 0.49, 1.0];
const WARNING_COLOR: [f32; 3] = [1.0, 0.76, 0.28];

fn zone_color(zone: Zone) -> [f32; 3] {
    match zone {
        Zone::Shelf => SHELF_COLOR,
        Zone::Phone => PHONE_COLOR,
        Zone::Convert => CONVERT_COLOR,
    }
}

fn zone_icon(zone: Zone) -> Icon {
    match zone {
        Zone::Shelf => Icon::Shelf,
        Zone::Phone => Icon::Phone,
        Zone::Convert => Icon::Convert,
    }
}

fn with_alpha([r, g, b]: [f32; 3], alpha: f32) -> [f32; 4] {
    [r, g, b, alpha]
}

fn glyph(icon: Icon) -> &'static str {
    match icon {
        Icon::Volume => "\u{E767}",
        Icon::Muted => "\u{E74F}",
        Icon::Brightness => "\u{E706}",
        Icon::Keyboard => "\u{E765}",
        Icon::Charging => "\u{E83E}",
        Icon::Battery => "\u{E83F}",
        Icon::BatteryLow => "\u{E851}",
        Icon::Bluetooth => "\u{E702}",
        Icon::Timer => ICON_STOPWATCH,
        Icon::Focus => "\u{E708}",
        Icon::Message => "\u{E8F2}",
        Icon::Calendar => "\u{E787}",
        Icon::Music => ICON_NOTE,
        Icon::Speaker => "\u{E7F5}",
        Icon::Microphone => "\u{E720}",
        Icon::MicrophoneOff => "\u{EC54}",
        Icon::Camera => "\u{E722}",
        Icon::Internet => "\u{E774}",
        Icon::InternetOff => "\u{F384}",
        Icon::Wifi => "\u{E701}",
        Icon::Vpn => "\u{E705}",
        Icon::Airplane => "\u{E709}",
        Icon::Usb => "\u{E88E}",
        Icon::Monitor => "\u{E7F4}",
        Icon::Clipboard => "\u{E8C8}",
        Icon::Phone => "\u{E8EA}",
        Icon::Shelf => "\u{E7B8}",
        Icon::Convert => "\u{E8AB}",
        Icon::Check => "\u{E73E}",
        Icon::Warning => "\u{E7BA}",
    }
}

/// Right edge of the compact pill reserved for the sound wave, in DIPs.
pub const WAVE_RESERVED: f32 = 42.0;
/// Right edge of the music pill kept for a task beside it, which a click opens.
pub const TASK_RESERVED: f32 = 40.0;

const ALERT_ART: f32 = 46.0;
const ALERT_ART_RADIUS: f32 = 12.0;
const ALERT_ART_TEXT_GAP: f32 = 12.0;
const COMPACT_ART: f32 = 24.0;
/// Where text starts beside the cover, which is as far in as the cover is high.
const COMPACT_ART_TEXT_X: f32 = 38.0;
const COMPACT_TEXT_SIZE: f32 = 13.0;
const COMPACT_CLOCK_SIZE: f32 = 14.0;
const COMPACT_CLOCK_PADDING: f32 = 20.0;
const COMPACT_TEXT_START: f32 = 18.0;
const COMPACT_ICON_TEXT_START: f32 = 40.0;
const COMPACT_TEXT_END: f32 = 16.0;
const COMPACT_TEXT_GAP: f32 = 12.0;
/// Text measures a little narrower than it may draw, and a cut last letter is ugly.
const MEASURE_SPARE: f32 = 2.0;
const SMALLEST_ART_RADIUS: f32 = 5.0;
const COUNTDOWN_SIZE: f32 = 34.0;
/// Past an hour the countdown has to fit two more digits inside the same ring.
const LONG_COUNTDOWN_SIZE: f32 = 26.0;
const LONG_COUNTDOWN: usize = "24:13".len();
const COUNTDOWN_LABEL_WIDTH: f32 = 96.0;

/// Digits differ in width, so a countdown is measured as if all were zeros, or
/// the pill would twitch every second.
fn all_zeros(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_ascii_digit() {
                '0'
            } else {
                character
            }
        })
        .collect()
}

/// How the compact pill fits what it shows, in DIPs.
#[derive(Clone, Copy, PartialEq)]
pub struct CompactFit {
    pub width: f32,
    /// What the value on the right takes up.
    pub trailing: f32,
}

/// The time of day, as it reads on the island.
pub struct ClockText {
    /// Such as `19:30`.
    pub time: String,
    /// Such as `Cumartesi, 31 Ekim`.
    pub date: String,
}

#[derive(Clone, PartialEq)]
pub struct MediaScene {
    pub artwork: Option<Arc<[u8]>>,
    pub playing: bool,
    pub shuffle: Option<bool>,
    pub repeat: Option<RepeatMode>,
    pub sides: Sides,
    pub is_music: bool,
    pub can_previous: bool,
    pub can_next: bool,
    /// Only tracked where it is drawn, so the compact view never repaints for it.
    pub timing: Option<Timing>,
}

#[derive(Clone, PartialEq)]
pub struct Timing {
    pub elapsed: String,
    pub remaining: String,
    pub permille: u16,
    pub scrubbing: bool,
}

/// What shares the pill with the activity it shows.
#[derive(Clone, PartialEq)]
pub enum Companion {
    /// A task on the right of the music pill, by how far it got.
    Task(Shown),
    /// A running focus session or timer beside the music title.
    Timed(Bubble),
    /// Music along the bottom of the open code.
    Strip { media: MediaScene, text: String },
}

#[derive(Clone, PartialEq)]
pub struct Scene {
    pub preferences: PresentationPreferences,
    pub presentation: Presentation,
    pub wave_visible: bool,
    pub clock: Option<String>,
    pub leading: String,
    pub trailing: String,
    pub title: String,
    pub detail: String,
    pub media: Option<MediaScene>,
    pub icon: Option<Icon>,
    pub level: Option<u8>,
    pub indicator_alert: bool,
    pub indicator_context: Option<island_core::activity::IndicatorContext>,
    /// For an open timer view: whether it runs, and whether it has buttons.
    pub timer: Option<TimerView>,
    pub home: Option<HomeScene>,
    /// An event, which its open view offers to hide.
    pub hideable: bool,
    pub fit: CompactFit,
    pub preview: Option<Arc<[u8]>>,
    /// A view a drop opened, or the panel for files being dragged.
    pub panel: Option<PanelView>,
    pub transfer: Option<TransferView>,
    /// The alert's file is shown in its folder, which a button on it offers.
    pub reveal: bool,
    pub companion: Option<Companion>,
}

#[derive(Clone, PartialEq)]
pub struct HomeEvent {
    pub title: String,
    pub when: String,
    /// `when` with the place added, for when there is room for it.
    pub detail: String,
}

#[derive(Clone, PartialEq)]
pub struct HomeScene {
    pub today: String,
    pub events: Vec<HomeEvent>,
    pub calendar: CalendarState,
    pub mode: StartMode,
    pub modes_offered: bool,
    pub presets: [u64; TIMER_PRESETS.len()],
}

const DEFAULT_FIT: CompactFit = CompactFit {
    width: Pill::COMPACT.width,
    trailing: 0.0,
};

/// The space kept after the title: the wave only takes room while it is
/// there, and a task beside the music takes it instead.
fn compact_media_end(media: &MediaScene, companion: Option<&Companion>, wave_visible: bool) -> f32 {
    match companion {
        Some(Companion::Task(_) | Companion::Timed(_)) => TASK_RESERVED,
        _ if media.playing && wave_visible => WAVE_RESERVED,
        _ => COMPACT_TEXT_END,
    }
}

fn media_scene(media: &MediaSnapshot, timing: Option<Timing>) -> MediaScene {
    MediaScene {
        artwork: media.artwork.clone(),
        playing: media.playing,
        shuffle: media.shuffle,
        repeat: media.repeat,
        sides: media.sides(),
        is_music: media.is_music,
        can_previous: media.can_previous,
        can_next: media.can_next,
        timing,
    }
}

impl HomeScene {
    fn new(home: &HomeView, clock: &ClockText) -> Self {
        let events = home.coming.iter().map(|event| HomeEvent {
            title: event.title.clone(),
            when: event.when.clone(),
            detail: match &event.location {
                Some(location) => format!("{} · {location}", event.when),
                None => event.when.clone(),
            },
        });
        Self {
            today: format!("{} · {}", clock.date, clock.time),
            events: events.collect(),
            calendar: home.calendar,
            mode: home.mode,
            modes_offered: home.modes_offered,
            presets: home.presets,
        }
    }
}

impl Scene {
    /// `scrub` is where the user is dragging the position bar, from 0 to 1.
    pub fn new(
        frame: &Frame,
        clock: &ClockText,
        now: Millis,
        scrub: Option<f32>,
        wave_visible: bool,
    ) -> Self {
        let presentation = frame.presentation;
        let Some(activity) = &frame.activity else {
            let home = frame.home.as_ref().map(|home| HomeScene::new(home, clock));
            return Self {
                preferences: PresentationPreferences::default(),
                presentation,
                wave_visible,
                clock: (home.is_none() && frame.panel.is_none()).then(|| clock.time.clone()),
                leading: String::new(),
                trailing: String::new(),
                title: String::new(),
                detail: String::new(),
                media: None,
                icon: None,
                level: None,
                indicator_alert: false,
                indicator_context: None,
                timer: None,
                home,
                hideable: false,
                fit: DEFAULT_FIT,
                preview: None,
                panel: frame.panel.clone(),
                transfer: None,
                reveal: false,
                companion: None,
            };
        };
        let media = activity.media.as_ref().map(|media| {
            let timing = (presentation == Presentation::Expanded)
                .then(|| {
                    media.progress_at(now).map(|progress| {
                        let progress = scrub.unwrap_or(progress);
                        let position = (media.duration_ms as f32 * progress) as u64;
                        Timing {
                            elapsed: format_time(position),
                            remaining: format!("-{}", format_time(media.duration_ms - position)),
                            permille: (progress * 1000.0) as u16,
                            scrubbing: scrub.is_some(),
                        }
                    })
                })
                .flatten();
            media_scene(media, timing)
        });
        let companion = frame
            .companion
            .as_ref()
            .and_then(|other| match presentation {
                Presentation::Compact => other
                    .transfer
                    .as_ref()
                    .map(|view| Companion::Task(view.shown.clone()))
                    .or_else(|| {
                        matches!(
                            other.kind,
                            Kind::Focus | Kind::TimerRunning | Kind::ClockTimer
                        )
                        .then(|| Companion::Timed(other.bubble()))
                    }),
                Presentation::Expanded => other.media.as_ref().map(|media| Companion::Strip {
                    media: media_scene(media, None),
                    text: if other.detail.is_empty() {
                        other.title.clone()
                    } else {
                        format!("{} · {}", other.title, other.detail)
                    },
                }),
                _ => None,
            });
        Self {
            preferences: PresentationPreferences::default(),
            presentation,
            wave_visible,
            clock: None,
            leading: if media.is_some() && activity.leading.trim().is_empty() {
                "Media".to_owned()
            } else {
                activity.leading.clone()
            },
            trailing: activity.trailing.clone(),
            title: if media.is_some() && activity.title.trim().is_empty() {
                "Media".to_owned()
            } else {
                activity.title.clone()
            },
            detail: activity.indicator_percent.map_or_else(
                || activity.detail.clone(),
                |value| format!("%{}", value.min(100)),
            ),
            media,
            icon: activity.icon,
            level: activity.level.or(activity.indicator_percent),
            indicator_alert: activity.indicator_percent.is_some(),
            indicator_context: activity.indicator_context.clone(),
            timer: activity.timer,
            home: None,
            hideable: activity.kind == Kind::Upcoming,
            fit: DEFAULT_FIT,
            preview: activity.preview.clone(),
            panel: None,
            transfer: activity.transfer.clone(),
            reveal: activity.reveal && activity.file.is_some(),
            companion,
        }
    }

    /// Music along the bottom of the open code, which makes the pill taller.
    pub fn has_strip(&self) -> bool {
        matches!(self.companion, Some(Companion::Strip { .. }))
    }

    /// A task on the right of the music pill.
    pub fn has_task_beside(&self) -> bool {
        matches!(
            self.companion,
            Some(Companion::Task(_) | Companion::Timed(_))
        )
    }

    /// Everything the fit depends on, so it is only measured when that changes.
    pub fn fit_key(&self) -> String {
        let cover = self.media.as_ref().map(|media| media.playing);
        format!(
            "{cover:?}|{}|{}|{}|{}|{}|{}|{:?}|{}",
            self.wave_visible,
            self.has_task_beside(),
            self.icon.is_some(),
            self.clock.as_deref().map(all_zeros).unwrap_or_default(),
            self.leading,
            all_zeros(&self.trailing),
            self.preferences.media_layout.setting(),
            if self.preferences.media_layout != MediaLayout::Compact && self.media.is_some() {
                &self.detail
            } else {
                ""
            },
        )
    }

    /// Works out how wide the compact pill should be. `measure` takes a text,
    /// its size and whether it is bold.
    pub fn fit(&self, measure: impl Fn(&str, f32, bool) -> f32) -> CompactFit {
        let text =
            |value: &str, bold| measure(value, COMPACT_TEXT_SIZE, bold).ceil() + MEASURE_SPARE;
        let trailing = if self.trailing.is_empty() {
            0.0
        } else {
            text(&all_zeros(&self.trailing), false)
        };
        let wanted = if let Some(clock) = &self.clock {
            measure(&all_zeros(clock), COMPACT_CLOCK_SIZE, true).ceil()
                + COMPACT_CLOCK_PADDING * 2.0
        } else if let Some(media) = &self.media {
            let title = text(&self.leading, true);
            let artist = if self.preferences.media_layout != MediaLayout::Compact
                && !self.detail.is_empty()
            {
                measure(&self.detail, 10.0, false).ceil() + MEASURE_SPARE
            } else {
                0.0
            };
            COMPACT_ART_TEXT_X
                + title.max(artist)
                + compact_media_end(media, self.companion.as_ref(), self.wave_visible)
        } else {
            let gap = if trailing > 0.0 {
                COMPACT_TEXT_GAP
            } else {
                0.0
            };
            self.text_start() + text(&self.leading, true) + gap + trailing + COMPACT_TEXT_END
        };
        CompactFit {
            width: wanted.clamp(COMPACT_MIN_WIDTH, COMPACT_MAX_WIDTH),
            trailing,
        }
    }

    fn text_start(&self) -> f32 {
        if self.icon.is_some() {
            COMPACT_ICON_TEXT_START
        } else {
            COMPACT_TEXT_START
        }
    }

    /// Same content apart from a value that ticks, such as the running time or
    /// a volume step. Those update in place, as a fade each time would flicker.
    pub fn differs_only_in_value(&self, other: &Scene) -> bool {
        let without_value = |scene: &Scene| {
            let mut scene = scene.clone();
            if let Some(media) = &mut scene.media {
                media.timing = None;
                media.shuffle = media.shuffle.map(|_| false);
                media.repeat = media.repeat.map(|_| RepeatMode::Off);
            }
            if scene.level.is_some() {
                scene.level = Some(0);
                scene.detail.clear();
            }
            if let Some(timer) = &mut scene.timer {
                timer.running = false;
                timer.left = None;
                scene.title.clear();
                scene.detail.clear();
            }
            if let Some(clock) = &mut scene.clock {
                clock.clear();
            }
            if let Some(home) = &mut scene.home {
                home.today.clear();
                home.mode = StartMode::default();
            }
            let without_progress = |shown: &Shown| match shown {
                Shown::Waiting { .. } => Shown::Waiting {
                    left: String::new(),
                },
                Shown::Sending { .. } => Shown::Sending {
                    permille: 0,
                    amount: String::new(),
                },
                Shown::Done => Shown::Done,
            };
            if let Some(transfer) = &mut scene.transfer {
                transfer.shown = without_progress(&transfer.shown);
            }
            if let Some(Companion::Task(shown)) = &mut scene.companion {
                *shown = without_progress(shown);
            }
            if let Some(Companion::Timed(Bubble::Count { value, ring, .. })) = &mut scene.companion
            {
                value.clear();
                *ring = None;
            }
            match &mut scene.panel {
                Some(PanelView::Drop(view)) => view.hovered = None,
                Some(PanelView::Shelf(view)) => {
                    view.items.iter_mut().for_each(|item| item.thumbnail = None)
                }
                Some(PanelView::Convert(view)) => view.busy = false,
                Some(PanelView::QuickControls(view)) => {
                    // Level and keyboard-focus updates repaint the current panel
                    // in place instead of fading the entire panel on every step.
                    view.focused = None;
                    view.volume = view.volume.map(|_| (0, false));
                    view.brightness = view.brightness.map(|_| 0);
                    view.wifi = RadioState::Unknown;
                    view.bluetooth = RadioState::Unknown;
                }
                None => {}
            }
            scene.trailing.clear();
            scene
        };
        without_value(self) == without_value(other)
    }

    pub fn shows_wave(&self) -> bool {
        self.wave_visible
            && self.presentation == Presentation::Compact
            && self.media.as_ref().is_some_and(|media| media.playing)
            && !self.has_task_beside()
    }

    /// `corner` is how round the pill's corners look, in DIPs.
    /// `notch` says the top edge joins the screen's.
    pub fn painting(&self, pill: Pill, scale: f32, corner: f32, notch: bool) -> Painting<'_> {
        let canvas = Canvas {
            scale,
            width: pill.width / scale,
            height: pill.height / scale,
            corner,
            notch,
        };
        if let Some(clock) = &self.clock {
            let mut painting = Painting::default();
            let bounds = canvas.bounds(0.0, 0.0, canvas.width, canvas.height);
            painting.texts.push(canvas.text(
                clock,
                bounds,
                COMPACT_CLOCK_SIZE,
                true,
                1.0,
                Align::Center,
            ));
            return painting;
        }
        if let Some(panel) = &self.panel {
            return match panel {
                PanelView::Drop(view) => drop_panel(view, &canvas),
                PanelView::Shelf(view) => shelf_panel(view, &canvas),
                PanelView::Convert(view) => convert_panel(view, &canvas),
                PanelView::QuickControls(view) => quick_controls_panel(view, &canvas),
            };
        }
        if let Some(home) = &self.home {
            return self.home_view(home, &canvas);
        }
        if let (Presentation::Expanded, Some(transfer)) = (self.presentation, &self.transfer) {
            return self.expanded_transfer(transfer, &canvas);
        }
        if let (Presentation::Expanded, Some(timer)) = (self.presentation, self.timer) {
            return self.expanded_timer(timer, &canvas);
        }
        if self.presentation == Presentation::Alert && self.indicator_alert {
            return self.level_bar(&canvas);
        }
        match (self.presentation, &self.media) {
            (Presentation::Compact, Some(media)) => self.compact_media(media, &canvas),
            (Presentation::Expanded, Some(media)) => self.expanded_media(media, &canvas),
            (Presentation::Compact, None) => self.compact_text(&canvas),
            (Presentation::Level, _) => self.level_bar(&canvas),
            (Presentation::Alert, _) => self.alert(&canvas),
            (Presentation::Expanded, None) => self.expanded_text(&canvas),
            (Presentation::Hidden, _) => Painting::default(),
        }
    }

    fn compact_text(&self, canvas: &Canvas) -> Painting<'_> {
        let mut painting = Painting::default();
        let height = canvas.height;
        let left = self.text_start();
        let sending = self
            .transfer
            .as_ref()
            .and_then(|transfer| match transfer.shown {
                Shown::Sending { permille, .. } => Some(permille),
                _ => None,
            });
        if let Some(permille) = sending {
            painting.rings.push(RingRun {
                center_x: 24.0 * canvas.scale,
                center_y: height / 2.0 * canvas.scale,
                radius: 7.0 * canvas.scale,
                width: 2.2 * canvas.scale,
                fraction: f32::from(permille) / 1000.0,
                opacity: 1.0,
                track_color: None,
            });
        } else if let Some(icon) = self.icon {
            let run = canvas.icon(glyph(icon), canvas.bounds(12.0, 0.0, 24.0, height), 15.0);
            if icon == Icon::Check {
                painting.colored.push((run, PHONE_COLOR));
            } else {
                painting.texts.push(run);
            }
        }
        let trailing = self.fit.trailing;
        let gap = if trailing > 0.0 {
            COMPACT_TEXT_GAP
        } else {
            0.0
        };
        let first = canvas.width - left - gap - trailing - COMPACT_TEXT_END;
        let leading = canvas.bounds(left, 0.0, first, height);
        painting.texts.push(canvas.text(
            &self.leading,
            leading,
            COMPACT_TEXT_SIZE,
            true,
            1.0,
            Align::Leading,
        ));
        let value = canvas.bounds(left + first + gap, 0.0, trailing, height);
        painting.texts.push(canvas.text(
            &self.trailing,
            value,
            COMPACT_TEXT_SIZE,
            false,
            0.75,
            Align::Trailing,
        ));
        painting
    }

    fn compact_media<'a>(&'a self, media: &'a MediaScene, canvas: &Canvas) -> Painting<'a> {
        let mut painting = Painting::default();
        let inset = (canvas.height - COMPACT_ART) / 2.0;
        // Corners inside corners only look right when they share a centre.
        let radius = (canvas.corner - inset).clamp(SMALLEST_ART_RADIUS, COMPACT_ART / 2.0);
        canvas.artwork(&mut painting, media, inset, inset, COMPACT_ART, radius);

        let width = canvas.width
            - COMPACT_ART_TEXT_X
            - compact_media_end(media, self.companion.as_ref(), self.wave_visible);
        if self.preferences.media_layout != MediaLayout::Compact && !self.detail.is_empty() {
            painting.texts.push(canvas.text(
                &self.leading,
                canvas.bounds(COMPACT_ART_TEXT_X, 2.0, width, 17.0),
                11.5,
                true,
                1.0,
                Align::Leading,
            ));
            painting.texts.push(canvas.text(
                &self.detail,
                canvas.bounds(COMPACT_ART_TEXT_X, 18.0, width, 15.0),
                10.0,
                false,
                0.7,
                Align::Leading,
            ));
        } else {
            let bounds = canvas.bounds(COMPACT_ART_TEXT_X, 0.0, width, canvas.height);
            painting.texts.push(canvas.text(
                &self.leading,
                bounds,
                COMPACT_TEXT_SIZE,
                true,
                1.0,
                Align::Leading,
            ));
        }
        if let Some(Companion::Task(shown)) = &self.companion {
            // As far in from the end as the cover is from the start.
            let center_x = canvas.width - inset - COMPACT_ART / 2.0;
            task_mark(&mut painting, shown, canvas, center_x, canvas.height / 2.0);
        } else if let Some(Companion::Timed(badge)) = &self.companion {
            let center_x = canvas.width - inset - COMPACT_ART / 2.0;
            timed_mark(&mut painting, badge, canvas, center_x, canvas.height / 2.0);
        }
        painting
    }

    fn level_bar(&self, canvas: &Canvas) -> Painting<'_> {
        let mut painting = Painting::default();
        let layout = self.preferences.indicator_style.level_layout();
        let icon_bounds = canvas.rect(layout.icon);
        if self.preferences.indicator_style != IndicatorStyle::Ring {
            painting.shapes.push(ShapeRun {
                bounds: icon_bounds,
                radius: 11.0 * canvas.scale,
                fill: with_alpha(super::tokens::SURFACE, 1.0),
                border: None,
            });
        }
        if let Some(icon) = self.icon {
            painting
                .texts
                .push(canvas.icon(glyph(icon), icon_bounds, 18.0));
        }
        let value = self.level.unwrap_or(0).min(100);
        painting.texts.push(canvas.text(
            &self.detail,
            canvas.rect(layout.value),
            15.0,
            true,
            1.0,
            Align::Center,
        ));
        if self.preferences.indicator_style == IndicatorStyle::Ring && self.level.is_some() {
            let ring = layout.progress;
            painting.rings.push(RingRun {
                center_x: (ring.x + ring.width / 2.0) * canvas.scale,
                center_y: (ring.y + ring.height / 2.0) * canvas.scale,
                radius: 9.0 * canvas.scale,
                width: 3.0 * canvas.scale,
                fraction: f32::from(value) / 100.0,
                opacity: 1.0,
                track_color: Some(super::tokens::INDICATOR_TRACK),
            });
        } else if self.preferences.indicator_style != IndicatorStyle::Ring {
            let label = if self.title.is_empty() {
                &self.leading
            } else {
                &self.title
            };
            painting.texts.push(canvas.text(
                label,
                canvas.rect(layout.title),
                14.0,
                true,
                1.0,
                Align::Leading,
            ));
            let fallback = match self.icon {
                Some(Icon::Muted) => "Sessize alındı",
                Some(Icon::Volume) => "Sistem sesi",
                Some(Icon::Brightness) => "Ekran parlaklığı",
                Some(Icon::Charging) => "Harici güç",
                Some(Icon::Battery) => "Pil gücü",
                Some(Icon::BatteryLow) => "Pil seviyesi düşük",
                _ => "",
            };
            let context = self
                .indicator_context
                .as_ref()
                .and_then(|context| context.subtitle.as_deref())
                .unwrap_or(fallback);
            painting.texts.push(canvas.text(
                context,
                canvas.rect(layout.detail),
                11.0,
                false,
                0.65,
                Align::Leading,
            ));
            let track = layout.progress;
            painting.shapes.push(ShapeRun {
                bounds: canvas.rect(track),
                radius: track.height * canvas.scale / 2.0,
                fill: with_alpha(super::tokens::INDICATOR_TRACK, 1.0),
                border: None,
            });
            if value > 0 {
                let width = track.width * f32::from(value) / 100.0;
                painting.shapes.push(ShapeRun {
                    bounds: canvas.bounds(track.x, track.y, width, track.height),
                    radius: (width.min(track.height) / 2.0) * canvas.scale,
                    fill: with_alpha(super::tokens::PRIMARY, 1.0),
                    border: None,
                });
            }
            if self.preferences.indicator_style == IndicatorStyle::Context {
                let chips = self
                    .indicator_context
                    .as_ref()
                    .map_or(&[][..], |context| context.chips.as_slice());
                for (label, rect) in chips
                    .iter()
                    .zip(island_core::layout::indicator_chip_bounds(chips))
                {
                    let bounds = canvas.rect(rect);
                    painting.shapes.push(ShapeRun {
                        bounds,
                        radius: 7.0 * canvas.scale,
                        fill: with_alpha(super::tokens::SURFACE, 1.0),
                        border: None,
                    });
                    painting.texts.push(canvas.text(
                        label,
                        bounds,
                        9.0,
                        false,
                        0.85,
                        Align::Center,
                    ));
                }
            }
        }
        painting
    }

    fn alert(&self, canvas: &Canvas) -> Painting<'_> {
        let mut painting = Painting::default();
        let art = ALERT_ART;
        let inset = (canvas.height - art) / 2.0;
        let left = match (&self.preview, self.icon) {
            (Some(_), _) => inset + art + ALERT_ART_TEXT_GAP,
            (None, Some(_)) => 62.0,
            (None, None) => 28.0,
        };
        if let Some(encoded) = &self.preview {
            painting.images.push(ImageRun {
                encoded,
                x: inset * canvas.scale,
                y: inset * canvas.scale,
                size: art * canvas.scale,
                radius: ALERT_ART_RADIUS * canvas.scale,
            });
        } else if let Some(icon) = self.icon {
            painting.texts.push(canvas.icon(
                glyph(icon),
                canvas.bounds(20.0, 0.0, 34.0, canvas.height),
                22.0,
            ));
        }
        const REVEAL_WIDTH: f32 = 64.0;
        let button = if self.reveal {
            REVEAL_WIDTH + 10.0
        } else {
            0.0
        };
        let inner = canvas.width - left - 28.0 - button;
        painting.texts.push(canvas.text(
            &self.title,
            canvas.bounds(left, 12.0, inner, 22.0),
            14.0,
            true,
            1.0,
            Align::Leading,
        ));
        painting.texts.push(canvas.text(
            &self.detail,
            canvas.bounds(left, 33.0, inner, 20.0),
            12.5,
            false,
            0.7,
            Align::Leading,
        ));
        if self.reveal {
            let bounds = canvas.bounds(
                canvas.width - 22.0 - REVEAL_WIDTH,
                (canvas.height - 28.0) / 2.0,
                REVEAL_WIDTH,
                28.0,
            );
            painting.bars.push(BarRun {
                bounds,
                opacity: 0.16,
            });
            painting
                .texts
                .push(canvas.text("Göster", bounds, 12.5, true, 1.0, Align::Center));
        }
        painting
    }

    fn expanded_transfer<'a>(
        &'a self,
        transfer: &'a TransferView,
        canvas: &Canvas,
    ) -> Painting<'a> {
        let mut painting = Painting::default();
        let code = canvas.rect(TRANSFER_CODE);
        let middle_x = TRANSFER_CODE.x + TRANSFER_CODE.width / 2.0;
        let middle_y = TRANSFER_CODE.y + TRANSFER_CODE.height / 2.0;
        let title = match &transfer.shown {
            Shown::Waiting { left } => {
                // The time left shares the row under the name with the network.
                let time = canvas.bounds(
                    TRANSFER_TEXT_LEFT,
                    70.0,
                    canvas.width - TRANSFER_TEXT_LEFT - 26.0,
                    18.0,
                );
                painting
                    .texts
                    .push(canvas.text(left, time, 12.0, false, 0.6, Align::Trailing));
                match &transfer.direct {
                    Some(Direct::Preparing) => {
                        let icon = canvas.bounds(
                            TRANSFER_CODE.x,
                            middle_y - 30.0,
                            TRANSFER_CODE.width,
                            40.0,
                        );
                        painting
                            .texts
                            .push(canvas.icon(glyph(Icon::Wifi), icon, 30.0));
                        let note = canvas.bounds(
                            TRANSFER_CODE.x,
                            middle_y + 14.0,
                            TRANSFER_CODE.width,
                            20.0,
                        );
                        painting.texts.push(canvas.text(
                            "Ağ kuruluyor…",
                            note,
                            12.0,
                            false,
                            0.7,
                            Align::Center,
                        ));
                        "Doğrudan bağlan"
                    }
                    Some(Direct::Ready { code: join, .. }) => {
                        painting.codes.push(CodeRun {
                            bounds: code,
                            code: join,
                        });
                        "Telefonu bağla"
                    }
                    // No code without a network: Wi-Fi in its place, dimmed.
                    None if transfer.offline => {
                        let icon = canvas.bounds(
                            TRANSFER_CODE.x,
                            middle_y - 30.0,
                            TRANSFER_CODE.width,
                            40.0,
                        );
                        let mut run = canvas.icon(glyph(Icon::Wifi), icon, 30.0);
                        run.opacity = 0.45;
                        painting.texts.push(run);
                        let note = canvas.bounds(
                            TRANSFER_CODE.x,
                            middle_y + 14.0,
                            TRANSFER_CODE.width,
                            20.0,
                        );
                        painting.texts.push(canvas.text(
                            "Wi-Fi kapalı",
                            note,
                            12.0,
                            false,
                            0.7,
                            Align::Center,
                        ));
                        "Telefona gönder"
                    }
                    _ => {
                        painting.codes.push(CodeRun {
                            bounds: code,
                            code: &transfer.code,
                        });
                        "Telefona gönder"
                    }
                }
            }
            Shown::Sending { permille, .. } => {
                let fraction = f32::from(*permille) / 1000.0;
                // Labels made while painting are sized by their height.
                let percent =
                    canvas.bounds(TRANSFER_CODE.x, middle_y - 47.0, TRANSFER_CODE.width, 94.0);
                match transfer.style {
                    ProgressStyle::Ring => {
                        painting.rings.push(RingRun {
                            center_x: middle_x * canvas.scale,
                            center_y: middle_y * canvas.scale,
                            radius: 54.0 * canvas.scale,
                            width: 7.0 * canvas.scale,
                            fraction,
                            opacity: 1.0,
                            track_color: None,
                        });
                        painting.owned_labels.push((
                            format!("%{}", permille / 10),
                            canvas.bounds(
                                TRANSFER_CODE.x,
                                middle_y - 36.0,
                                TRANSFER_CODE.width,
                                72.0,
                            ),
                        ));
                    }
                    ProgressStyle::Edge => {
                        let whole = canvas.bounds(0.0, 0.0, canvas.width, canvas.height);
                        let radius = canvas.corner * canvas.scale;
                        painting.outlines.push(OutlineRun {
                            bounds: whole,
                            radius,
                            width: 7.0 * canvas.scale,
                            color: with_alpha(PHONE_COLOR, 0.22),
                            fraction,
                            dash: None,
                            open_top: canvas.notch,
                        });
                        painting.outlines.push(OutlineRun {
                            bounds: whole,
                            radius,
                            width: 2.5 * canvas.scale,
                            color: with_alpha(PHONE_COLOR, 1.0),
                            fraction,
                            dash: None,
                            open_top: canvas.notch,
                        });
                        painting
                            .owned_labels
                            .push((format!("%{}", permille / 10), percent));
                    }
                    ProgressStyle::Fill => {
                        let reach = canvas.width * fraction;
                        let filled = canvas.bounds(0.0, 0.0, reach, canvas.height);
                        painting.shapes.push(ShapeRun {
                            bounds: filled,
                            radius: 0.0,
                            fill: with_alpha(PHONE_COLOR, 0.26),
                            border: None,
                        });
                        // A brighter front edge, so it reads as moving rather than as a colour.
                        let front = canvas.bounds(reach - 2.0, 0.0, 2.0, canvas.height);
                        painting.shapes.push(ShapeRun {
                            bounds: front,
                            radius: 0.0,
                            fill: with_alpha(PHONE_COLOR, 0.8),
                            border: None,
                        });
                        painting
                            .owned_labels
                            .push((format!("%{}", permille / 10), percent));
                    }
                }
                "Gönderiliyor"
            }
            Shown::Done => {
                let circle = canvas.bounds(middle_x - 40.0, middle_y - 40.0, 80.0, 80.0);
                painting.shapes.push(ShapeRun {
                    bounds: circle,
                    radius: 40.0 * canvas.scale,
                    fill: with_alpha(PHONE_COLOR, 1.0),
                    border: None,
                });
                painting
                    .texts
                    .push(canvas.icon(glyph(Icon::Check), circle, 38.0));
                "Gönderildi"
            }
        };

        let left = TRANSFER_TEXT_LEFT;
        let width = canvas.width - left - 26.0;
        painting.texts.push(canvas.text(
            title,
            canvas.bounds(left, 22.0, width, 24.0),
            16.0,
            true,
            1.0,
            Align::Leading,
        ));
        painting.texts.push(canvas.text(
            &transfer.summary,
            canvas.bounds(left, 47.0, width, 20.0),
            13.0,
            false,
            0.75,
            Align::Leading,
        ));
        let line = canvas.bounds(left, 70.0, width, 18.0);
        // Room on the right of the row for the time left.
        let line_beside_time = canvas.bounds(left, 70.0, width - 44.0, 18.0);
        let note = canvas.bounds(left, 89.0, width, 18.0);
        match &transfer.shown {
            Shown::Waiting { .. } => match &transfer.direct {
                None if transfer.offline => {
                    painting.colored.push((
                        canvas.text(
                            "Wi-Fi kapalı",
                            line_beside_time,
                            12.0,
                            false,
                            1.0,
                            Align::Leading,
                        ),
                        WARNING_COLOR,
                    ));
                    painting.texts.push(canvas.text(
                        "Wi-Fi açılınca kod hazırlanır",
                        note,
                        12.0,
                        false,
                        0.6,
                        Align::Leading,
                    ));
                }
                None => {
                    painting.texts.push(canvas.text(
                        &transfer.network,
                        line_beside_time,
                        12.0,
                        false,
                        0.6,
                        Align::Leading,
                    ));
                    match &transfer.warning {
                        Some(warning) => painting.colored.push((
                            canvas.text(warning, note, 12.0, false, 1.0, Align::Leading),
                            WARNING_COLOR,
                        )),
                        // Some networks keep phones away from computers, which
                        // looks like nobody coming at all.
                        None if transfer.nudge => painting.texts.push(canvas.text(
                            "Açılmazsa Doğrudan bağlan'ı dene",
                            note,
                            12.0,
                            false,
                            0.9,
                            Align::Leading,
                        )),
                        None => painting.texts.push(canvas.text(
                            "Telefonun kamerasıyla okut",
                            note,
                            12.0,
                            false,
                            0.6,
                            Align::Leading,
                        )),
                    }
                    let other = canvas.rect(TRANSFER_DIRECT);
                    painting.bars.push(BarRun {
                        bounds: other,
                        opacity: if transfer.nudge { 0.3 } else { 0.14 },
                    });
                    painting.texts.push(canvas.text(
                        "Doğrudan bağlan",
                        other,
                        11.5,
                        true,
                        1.0,
                        Align::Center,
                    ));
                }
                Some(Direct::Hotspot { network }) => {
                    painting.texts.push(canvas.text(
                        network,
                        line_beside_time,
                        12.0,
                        true,
                        0.9,
                        Align::Leading,
                    ));
                    painting.texts.push(canvas.text(
                        "Telefon etkin noktadaysa okut",
                        note,
                        12.0,
                        false,
                        0.6,
                        Align::Leading,
                    ));
                }
                Some(Direct::Preparing) => {
                    painting.texts.push(canvas.text(
                        "Islora kendi ağını kuruyor",
                        line_beside_time,
                        12.0,
                        false,
                        0.6,
                        Align::Leading,
                    ));
                    painting.texts.push(canvas.text(
                        "Telefon başka bir ağdaysa",
                        note,
                        12.0,
                        false,
                        0.6,
                        Align::Leading,
                    ));
                }
                Some(Direct::Ready { network, .. }) => {
                    painting.texts.push(canvas.text(
                        network,
                        line_beside_time,
                        12.0,
                        true,
                        0.9,
                        Align::Leading,
                    ));
                    painting.texts.push(canvas.text(
                        "Kamerayla okut, bu ağa katıl",
                        note,
                        12.0,
                        false,
                        0.6,
                        Align::Leading,
                    ));
                }
                Some(Direct::Joined { network }) => {
                    painting.texts.push(canvas.text(
                        network,
                        line_beside_time,
                        12.0,
                        false,
                        0.6,
                        Align::Leading,
                    ));
                    painting.colored.push((
                        canvas.text(
                            "Bağlandı, şimdi bu kodu okut",
                            note,
                            12.0,
                            false,
                            1.0,
                            Align::Leading,
                        ),
                        PHONE_COLOR,
                    ));
                }
            },
            Shown::Sending { amount, .. } => {
                painting
                    .texts
                    .push(canvas.text(amount, line, 12.0, false, 0.6, Align::Leading));
            }
            Shown::Done => {
                painting.texts.push(canvas.text(
                    &transfer.network,
                    line,
                    12.0,
                    false,
                    0.6,
                    Align::Leading,
                ));
            }
        }
        if transfer.shown != Shown::Done {
            let stop = canvas.rect(TRANSFER_STOP);
            painting.bars.push(BarRun {
                bounds: stop,
                opacity: 0.14,
            });
            painting
                .texts
                .push(canvas.text("Durdur", stop, 12.5, true, 1.0, Align::Center));
        }
        if let Some(Companion::Strip { media, text }) = &self.companion {
            music_strip(&mut painting, media, text, canvas);
        }
        painting
    }

    fn expanded_text(&self, canvas: &Canvas) -> Painting<'_> {
        let mut painting = Painting::default();
        canvas.home_shortcut(&mut painting);
        let inner = canvas.width - 64.0 - HOME_SHORTCUT_SIZE;
        painting.texts.push(canvas.text(
            &self.title,
            canvas.bounds(32.0, 30.0, inner, 30.0),
            19.0,
            true,
            1.0,
            Align::Leading,
        ));
        painting.texts.push(canvas.text(
            &self.detail,
            canvas.bounds(32.0, 62.0, inner, 24.0),
            14.0,
            false,
            0.7,
            Align::Leading,
        ));
        if self.hideable {
            let left = (canvas.width - EVENT_HIDE_WIDTH) / 2.0;
            let bounds = canvas.bounds(
                left,
                MEDIA_BUTTON_Y - EVENT_HIDE_HEIGHT / 2.0,
                EVENT_HIDE_WIDTH,
                EVENT_HIDE_HEIGHT,
            );
            painting.bars.push(BarRun {
                bounds,
                opacity: 0.14,
            });
            painting
                .texts
                .push(canvas.text("Gizle", bounds, 13.0, true, 1.0, Align::Center));
        }
        painting
    }

    fn expanded_timer(&self, timer: TimerView, canvas: &Canvas) -> Painting<'_> {
        let mut painting = Painting::default();
        canvas.home_shortcut(&mut painting);
        let opacity = 1.0;
        let header_icon = self.icon.unwrap_or(Icon::Timer);
        let header = if header_icon == Icon::Focus {
            "Focus"
        } else {
            "Timer"
        };
        {
            painting.texts.push(canvas.icon(
                glyph(header_icon),
                canvas.bounds(18.0, 14.0, 20.0, 20.0),
                15.0,
            ));
            painting.texts.push(canvas.text(
                header,
                canvas.bounds(42.0, 14.0, 74.0, 20.0),
                13.0,
                true,
                1.0,
                Align::Leading,
            ));
        }
        painting.rings.push(RingRun {
            center_x: TIMER_RING_X * canvas.scale,
            center_y: TIMER_RING_Y * canvas.scale,
            radius: TIMER_RING_RADIUS * canvas.scale,
            width: TIMER_RING_WIDTH * canvas.scale,
            fraction: timer.left.map_or(0.0, |left| left as f32 / 1000.0),
            opacity,
            track_color: None,
        });

        let size = if self.title.len() > LONG_COUNTDOWN {
            LONG_COUNTDOWN_SIZE
        } else {
            COUNTDOWN_SIZE
        };
        let inside = (TIMER_RING_RADIUS - TIMER_RING_WIDTH / 2.0) * 2.0;
        let countdown = canvas.bounds(
            TIMER_RING_X - inside / 2.0,
            TIMER_RING_Y - 28.0,
            inside,
            44.0,
        );
        painting.texts.push(TextRun {
            even_digits: true,
            ..canvas.text(&self.title, countdown, size, true, opacity, Align::Center)
        });
        let label = canvas.bounds(
            TIMER_RING_X - COUNTDOWN_LABEL_WIDTH / 2.0,
            TIMER_RING_Y + 18.0,
            COUNTDOWN_LABEL_WIDTH,
            18.0,
        );
        let state = if header_icon == Icon::Timer && timer.has_controls {
            if timer.running { "Running" } else { "Paused" }
        } else {
            &self.detail
        };
        painting
            .texts
            .push(canvas.text(state, label, 11.5, false, 0.6, Align::Center));

        let buttons: &[TimerButton] = if timer.has_controls {
            &TimerButton::ALL
        } else {
            &[]
        };
        for &button in buttons {
            let glyph = match button {
                TimerButton::Toggle if timer.running => ICON_PAUSE,
                TimerButton::Toggle => ICON_PLAY,
                TimerButton::Cancel => ICON_CANCEL,
            };
            let half = TIMER_BUTTON_SIZE / 2.0;
            let bounds = canvas.bounds(
                button.center_x() - half,
                TIMER_RING_Y - half,
                TIMER_BUTTON_SIZE,
                TIMER_BUTTON_SIZE,
            );
            painting.shapes.push(ShapeRun {
                bounds,
                radius: 12.0 * canvas.scale,
                fill: with_alpha(super::tokens::SURFACE, 1.0),
                border: None,
            });
            let top = TIMER_RING_Y - half;
            painting.texts.push(canvas.icon(
                glyph,
                canvas.bounds(button.center_x() - half, top + 6.0, TIMER_BUTTON_SIZE, 21.0),
                16.0,
            ));
            let label = match button {
                TimerButton::Toggle if timer.running => "Pause",
                TimerButton::Toggle => "Resume",
                TimerButton::Cancel => "Stop",
            };
            painting.texts.push(canvas.text(
                label,
                canvas.bounds(
                    button.center_x() - half,
                    top + 28.0,
                    TIMER_BUTTON_SIZE,
                    16.0,
                ),
                10.0,
                false,
                1.0,
                Align::Center,
            ));
        }
        painting
    }

    fn home_view<'a>(&'a self, home: &'a HomeScene, canvas: &Canvas) -> Painting<'a> {
        let mut painting = Painting::default();
        const MARGIN: f32 = 30.0;
        let inner = canvas.width - MARGIN * 2.0;
        let label = canvas.bounds(MARGIN, 14.0, inner, 16.0);
        painting
            .texts
            .push(canvas.text(&home.today, label, 11.5, false, 0.6, Align::Trailing));
        match home.events.as_slice() {
            [] => {
                let (title, detail) = match home.calendar {
                    CalendarState::Ready => ("Yaklaşan etkinlik yok", ""),
                    CalendarState::NotSet => {
                        ("Takvim bağlı değil", "Ayarlar'dan takvim adresini ekle")
                    }
                    CalendarState::Unreachable => {
                        ("Takvime ulaşılamadı", "Bağlantı gelince yeniden denenecek")
                    }
                };
                painting.texts.push(canvas.text(
                    title,
                    canvas.bounds(MARGIN, 30.0, inner, 24.0),
                    14.0,
                    false,
                    0.8,
                    Align::Leading,
                ));
                painting.texts.push(canvas.text(
                    detail,
                    canvas.bounds(MARGIN, 54.0, inner, 20.0),
                    12.5,
                    false,
                    0.6,
                    Align::Leading,
                ));
            }
            [only] => {
                painting.texts.push(canvas.text(
                    "Sıradaki",
                    label,
                    11.5,
                    false,
                    0.6,
                    Align::Leading,
                ));
                painting.texts.push(canvas.text(
                    &only.title,
                    canvas.bounds(MARGIN, 30.0, inner, 24.0),
                    15.0,
                    true,
                    1.0,
                    Align::Leading,
                ));
                painting.texts.push(canvas.text(
                    &only.detail,
                    canvas.bounds(MARGIN, 54.0, inner, 20.0),
                    12.5,
                    false,
                    0.7,
                    Align::Leading,
                ));
            }
            several => {
                const WHEN_WIDTH: f32 = 96.0;
                painting.texts.push(canvas.text(
                    "Sıradaki",
                    label,
                    11.5,
                    false,
                    0.6,
                    Align::Leading,
                ));
                for (index, event) in several.iter().enumerate() {
                    let top = 32.0 + index as f32 * 23.0;
                    let title = canvas.bounds(MARGIN, top, inner - WHEN_WIDTH - 8.0, 22.0);
                    let when = canvas.bounds(MARGIN + inner - WHEN_WIDTH, top, WHEN_WIDTH, 22.0);
                    painting.texts.push(canvas.text(
                        &event.title,
                        title,
                        13.5,
                        true,
                        1.0,
                        Align::Leading,
                    ));
                    painting.texts.push(canvas.text(
                        &event.when,
                        when,
                        12.0,
                        false,
                        0.7,
                        Align::Trailing,
                    ));
                }
            }
        }
        if home.modes_offered {
            for mode in StartMode::ALL {
                let (left, width) = mode.tab();
                let bounds = canvas.bounds(
                    left,
                    MODE_TAB_Y - MODE_TAB_HEIGHT / 2.0,
                    width,
                    MODE_TAB_HEIGHT,
                );
                let chosen = mode == home.mode;
                if chosen {
                    painting.bars.push(BarRun {
                        bounds,
                        opacity: 0.16,
                    });
                }
                let opacity = if chosen { 1.0 } else { 0.6 };
                painting.texts.push(canvas.text(
                    mode.label(),
                    bounds,
                    11.5,
                    chosen,
                    opacity,
                    Align::Center,
                ));
            }
        } else {
            let bounds = canvas.bounds(MARGIN, MODE_TAB_Y - 8.0, inner, 16.0);
            painting.texts.push(canvas.text(
                "Zamanlayıcı başlat",
                bounds,
                11.5,
                false,
                0.6,
                Align::Leading,
            ));
        }
        for (index, minutes) in home.presets.iter().enumerate() {
            let center = preset_center_x(index);
            let bounds = canvas.bounds(
                center - PRESET_WIDTH / 2.0,
                PRESET_Y - PRESET_HEIGHT / 2.0,
                PRESET_WIDTH,
                PRESET_HEIGHT,
            );
            painting.bars.push(BarRun {
                bounds,
                opacity: 0.14,
            });
            painting
                .owned_labels
                .push((format!("{minutes} dk"), bounds));
        }
        let half = SETTINGS_SHORTCUT_SIZE / 2.0;
        let gear = canvas.bounds(
            SETTINGS_SHORTCUT_X - half,
            SETTINGS_SHORTCUT_Y - half,
            SETTINGS_SHORTCUT_SIZE,
            SETTINGS_SHORTCUT_SIZE,
        );
        painting.texts.push(canvas.icon(ICON_SETTINGS, gear, 15.0));
        let quick = canvas.rect(QUICK_SHORTCUT);
        painting.bars.push(BarRun {
            bounds: quick,
            opacity: 0.16,
        });
        painting
            .texts
            .push(canvas.text("Quick Controls", quick, 10.5, true, 1.0, Align::Center));
        painting
    }

    fn expanded_media<'a>(&'a self, media: &'a MediaScene, canvas: &Canvas) -> Painting<'a> {
        let mut painting = Painting::default();
        const MARGIN: f32 = 24.0;
        const ART: f32 = 72.0;
        canvas.artwork(&mut painting, media, MARGIN, 20.0, ART, 14.0);

        let text_x = MARGIN + ART + 14.0;
        let text_width = canvas.width - text_x - MARGIN - HOME_SHORTCUT_SIZE;
        canvas.home_shortcut(&mut painting);
        painting.texts.push(canvas.text(
            &self.title,
            canvas.bounds(text_x, 26.0, text_width, 28.0),
            16.0,
            true,
            1.0,
            Align::Leading,
        ));
        if self.preferences.media_layout != MediaLayout::Compact && !self.detail.is_empty() {
            painting.texts.push(canvas.text(
                &self.detail,
                canvas.bounds(text_x, 53.0, text_width, 22.0),
                13.0,
                false,
                0.7,
                Align::Leading,
            ));
        }
        if self.preferences.media_layout == MediaLayout::Informative {
            painting.texts.push(canvas.text(
                if media.playing { "Playing" } else { "Paused" },
                canvas.bounds(text_x, 76.0, text_width, 16.0),
                10.0,
                false,
                0.65,
                Align::Leading,
            ));
        }

        if let Some(timing) = &media.timing {
            // Thicker with a handle while dragging, so it is clear what moves.
            const KNOB: f32 = 14.0;
            let height = if timing.scrubbing {
                PROGRESS_HEIGHT + 2.0
            } else {
                PROGRESS_HEIGHT
            };
            let top = PROGRESS_Y + (PROGRESS_HEIGHT - height) / 2.0;
            let filled = PROGRESS_WIDTH * timing.permille as f32 / 1000.0;
            painting.bars.push(BarRun {
                bounds: canvas.bounds(PROGRESS_X, top, PROGRESS_WIDTH, height),
                opacity: 0.2,
            });
            painting.bars.push(BarRun {
                bounds: canvas.bounds(PROGRESS_X, top, filled.max(height), height),
                opacity: 1.0,
            });
            if timing.scrubbing {
                let middle = PROGRESS_Y + PROGRESS_HEIGHT / 2.0;
                let bounds = canvas.bounds(
                    PROGRESS_X + filled - KNOB / 2.0,
                    middle - KNOB / 2.0,
                    KNOB,
                    KNOB,
                );
                painting.bars.push(BarRun {
                    bounds,
                    opacity: 1.0,
                });
            }

            let label_y = MEDIA_BUTTON_Y - 9.0;
            painting.texts.push(canvas.text(
                &timing.elapsed,
                canvas.bounds(MARGIN, label_y, 72.0, 18.0),
                11.0,
                false,
                0.6,
                Align::Leading,
            ));
            painting.texts.push(canvas.text(
                &timing.remaining,
                canvas.bounds(canvas.width - MARGIN - 72.0, label_y, 72.0, 18.0),
                11.0,
                false,
                0.6,
                Align::Trailing,
            ));
        }

        for button in MediaButton::TRANSPORT {
            let (glyph, size, opacity) = match (button, media.sides) {
                (MediaButton::PlayPause, _) if media.playing => (ICON_PAUSE, 25.0, 1.0),
                (MediaButton::PlayPause, _) => (ICON_PLAY, 25.0, 1.0),
                (MediaButton::Previous, Sides::Jumps) => (ICON_BACK_10, 21.0, 1.0),
                (MediaButton::Previous, Sides::Tracks) if media.can_previous => {
                    (ICON_PREVIOUS, 19.0, 1.0)
                }
                (MediaButton::Previous, _) => (ICON_PREVIOUS, 19.0, UNAVAILABLE_OPACITY),
                (_, Sides::Jumps) => (ICON_FORWARD_30, 21.0, 1.0),
                (_, Sides::Tracks) if media.can_next => (ICON_NEXT, 19.0, 1.0),
                _ => (ICON_NEXT, 19.0, UNAVAILABLE_OPACITY),
            };
            let half = 21.0;
            let bounds = canvas.bounds(
                button.center_x() - half,
                MEDIA_BUTTON_Y - half,
                half * 2.0,
                half * 2.0,
            );
            painting.texts.push(TextRun {
                opacity,
                ..canvas.icon(glyph, bounds, size)
            });
        }
        if let Some(on) = media.shuffle {
            canvas.mode_button(&mut painting, MediaButton::Shuffle, ICON_SHUFFLE, on);
        }
        if let Some(repeat) = media.repeat {
            let glyph = if repeat == RepeatMode::One {
                ICON_REPEAT_ONE
            } else {
                ICON_REPEAT_ALL
            };
            canvas.mode_button(
                &mut painting,
                MediaButton::Repeat,
                glyph,
                repeat != RepeatMode::Off,
            );
        }
        painting
    }
}

/// A task's state in a small space: the phone while the code waits, a ring
/// as the files go, and a green tick once they are there.
fn task_mark(
    painting: &mut Painting<'_>,
    shown: &Shown,
    canvas: &Canvas,
    center_x: f32,
    center_y: f32,
) {
    const SIZE: f32 = 24.0;
    let bounds = canvas.bounds(center_x - SIZE / 2.0, center_y - SIZE / 2.0, SIZE, SIZE);
    match shown {
        Shown::Waiting { .. } => painting
            .texts
            .push(canvas.icon(glyph(Icon::Phone), bounds, 14.0)),
        Shown::Sending { permille, .. } => painting.rings.push(RingRun {
            center_x: center_x * canvas.scale,
            center_y: center_y * canvas.scale,
            radius: 8.0 * canvas.scale,
            width: 2.2 * canvas.scale,
            fraction: f32::from(*permille) / 1000.0,
            opacity: 1.0,
            track_color: None,
        }),
        Shown::Done => painting
            .colored
            .push((canvas.icon(glyph(Icon::Check), bounds, 15.0), PHONE_COLOR)),
    }
}

fn timed_mark<'a>(
    painting: &mut Painting<'a>,
    badge: &'a Bubble,
    canvas: &Canvas,
    center_x: f32,
    center_y: f32,
) {
    const SIZE: f32 = 28.0;
    let bounds = canvas.bounds(center_x - SIZE / 2.0, center_y - SIZE / 2.0, SIZE, SIZE);
    match badge {
        Bubble::Count { value, ring, .. } => {
            if let Some(percent) = ring {
                painting.rings.push(RingRun {
                    center_x: center_x * canvas.scale,
                    center_y: center_y * canvas.scale,
                    radius: 13.0 * canvas.scale,
                    width: 2.0 * canvas.scale,
                    fraction: *percent as f32 / 100.0,
                    opacity: 1.0,
                    track_color: None,
                });
            }
            painting
                .texts
                .push(canvas.text(value, bounds, 11.0, true, 1.0, Align::Center));
        }
        Bubble::Symbol(icon) => painting.texts.push(canvas.icon(glyph(*icon), bounds, 15.0)),
        Bubble::Artwork(_) => {}
    }
}

/// Music along the bottom of the open code: its cover and what plays.
fn music_strip<'a>(
    painting: &mut Painting<'a>,
    media: &'a MediaScene,
    text: &'a str,
    canvas: &Canvas,
) {
    const ART: f32 = 14.0;
    // Far enough in that the rounded bottom corners never cut the cover.
    const LEFT: f32 = 40.0;
    let top = Pill::EXPANDED.height;
    painting.bars.push(BarRun {
        bounds: canvas.bounds(24.0, top - 1.0, canvas.width - 48.0, 1.0),
        opacity: 0.12,
    });
    let middle = top + (STRIP_HEIGHT - 4.0) / 2.0;
    canvas.artwork(painting, media, LEFT, middle - ART / 2.0, ART, 4.0);
    let text_left = LEFT + ART + 8.0;
    let bounds = canvas.bounds(
        text_left,
        middle - 9.0,
        canvas.width - text_left - LEFT,
        18.0,
    );
    painting
        .texts
        .push(canvas.text(text, bounds, 11.5, false, 0.75, Align::Leading));
}

/// What the small bubble beside the pill shows.
pub fn bubble_painting(bubble: &Bubble, size: f32, scale: f32) -> Painting<'_> {
    let canvas = Canvas {
        scale,
        width: size / scale,
        height: size / scale,
        corner: size / scale / 2.0,
        notch: false,
    };
    let middle = canvas.width / 2.0;
    let mut painting = Painting::default();
    match bubble {
        Bubble::Artwork(encoded) => painting.images.push(ImageRun {
            encoded,
            x: 0.0,
            y: 0.0,
            size,
            radius: size / 2.0,
        }),
        Bubble::Symbol(icon) => {
            let bounds = canvas.bounds(0.0, 0.0, canvas.width, canvas.height);
            let run = canvas.icon(glyph(*icon), bounds, 16.0);
            // A finished task ticks green wherever it shows.
            if *icon == Icon::Check {
                painting.colored.push((run, PHONE_COLOR));
            } else {
                painting.texts.push(run);
            }
        }
        Bubble::Count { value, icon, ring } => {
            if let Some(percent) = ring {
                painting.rings.push(RingRun {
                    center_x: middle * scale,
                    center_y: middle * scale,
                    radius: (middle - 3.0) * scale,
                    width: 2.0 * scale,
                    fraction: *percent as f32 / 100.0,
                    opacity: 1.0,
                    track_color: None,
                });
            }
            let (text_top, text_height) = match icon {
                Some(icon) => {
                    painting.texts.push(canvas.icon(
                        glyph(*icon),
                        canvas.bounds(0.0, 5.0, canvas.width, 12.0),
                        10.0,
                    ));
                    (16.0, 15.0)
                }
                None => (0.0, canvas.height),
            };
            let bounds = canvas.bounds(0.0, text_top, canvas.width, text_height);
            painting
                .texts
                .push(canvas.text(value, bounds, 12.0, true, 1.0, Align::Center));
        }
    }
    painting
}

fn quick_controls_panel<'a>(view: &'a QuickControlsView, canvas: &Canvas) -> Painting<'a> {
    let mut painting = Painting::default();
    painting.shapes.push(ShapeRun {
        bounds: canvas.bounds(0.0, 0.0, canvas.width, canvas.height),
        radius: canvas.corner * canvas.scale,
        fill: [0.0, 0.0, 0.0, 1.0],
        border: None,
    });
    let title = match view.page {
        QuickPage::Main => "Quick Controls",
        QuickPage::Timer => "Timer",
        QuickPage::Calendar => "Calendar",
        QuickPage::Volume => "Volume",
        QuickPage::Brightness => "Brightness",
    };
    let title_x = if view.page == QuickPage::Main {
        22.0
    } else {
        60.0
    };
    painting.texts.push(canvas.text(
        title,
        canvas.bounds(title_x, 14.0, 270.0, 32.0),
        17.0,
        true,
        1.0,
        Align::Leading,
    ));
    if view.page != QuickPage::Main {
        let back = canvas.rect(BACK_BOUNDS);
        painting.texts.push(canvas.icon("\u{E72B}", back, 17.0));
        if view.focused == Some(QuickTarget::Back) {
            quick_focus(&mut painting, canvas, back);
        }
    }
    let settings = canvas.rect(SETTINGS_BOUNDS);
    painting
        .texts
        .push(canvas.icon(ICON_SETTINGS, settings, 18.0));
    if view.focused == Some(QuickTarget::Settings) {
        painting.outlines.push(OutlineRun {
            bounds: settings,
            radius: 8.0 * canvas.scale,
            width: 2.0 * canvas.scale,
            color: [1.0, 1.0, 1.0, 1.0],
            fraction: 1.0,
            dash: None,
            open_top: false,
        });
    }
    if view.page == QuickPage::Calendar {
        if view.calendar.coming.is_empty() {
            let label = match view.calendar.state {
                CalendarState::Ready => "No upcoming events",
                CalendarState::NotSet => "Calendar is not connected",
                CalendarState::Unreachable => "Calendar is unavailable",
            };
            painting.texts.push(canvas.text(
                label,
                canvas.bounds(36.0, 62.0, 328.0, 24.0),
                14.0,
                false,
                0.8,
                Align::Center,
            ));
            if view.calendar.state == CalendarState::NotSet {
                let bounds = canvas.rect(CALENDAR_SETUP_BOUNDS);
                painting.bars.push(BarRun {
                    bounds,
                    opacity: 0.18,
                });
                painting.texts.push(canvas.text(
                    "Set up calendar",
                    bounds,
                    12.0,
                    true,
                    1.0,
                    Align::Center,
                ));
                if view.focused == Some(QuickTarget::CalendarSetup) {
                    quick_focus(&mut painting, canvas, bounds);
                }
            }
        } else {
            for (index, event) in view.calendar.coming.iter().enumerate() {
                let y = 60.0 + index as f32 * 42.0;
                painting.texts.push(canvas.text(
                    &event.title,
                    canvas.bounds(32.0, y, 220.0, 24.0),
                    14.0,
                    true,
                    1.0,
                    Align::Leading,
                ));
                painting.texts.push(canvas.text(
                    &event.when,
                    canvas.bounds(252.0, y, 116.0, 24.0),
                    12.0,
                    false,
                    0.7,
                    Align::Trailing,
                ));
                if let Some(location) = event
                    .location
                    .as_deref()
                    .filter(|location| !location.is_empty())
                {
                    painting.texts.push(canvas.text(
                        location,
                        canvas.bounds(32.0, y + 23.0, 336.0, 16.0),
                        10.0,
                        false,
                        0.7,
                        Align::Leading,
                    ));
                }
            }
        }
        return painting;
    }
    if view.page == QuickPage::Timer {
        for mode in StartMode::ALL {
            let (left, width) = mode.tab();
            let bounds = canvas.bounds(
                left,
                MODE_TAB_Y - MODE_TAB_HEIGHT / 2.0,
                width,
                MODE_TAB_HEIGHT,
            );
            if view.modes_offered {
                if view.mode == mode {
                    painting.bars.push(BarRun {
                        bounds,
                        opacity: 0.16,
                    });
                }
                painting.texts.push(canvas.text(
                    mode.label(),
                    bounds,
                    11.5,
                    view.mode == mode,
                    1.0,
                    Align::Center,
                ));
                if view.focused == Some(QuickTarget::TimerMode(mode)) {
                    quick_focus(&mut painting, canvas, bounds);
                }
            }
        }
        for (index, minutes) in view.presets.iter().enumerate() {
            let bounds = canvas.bounds(
                preset_center_x(index) - PRESET_WIDTH / 2.0,
                PRESET_Y - PRESET_HEIGHT / 2.0,
                PRESET_WIDTH,
                PRESET_HEIGHT,
            );
            painting.bars.push(BarRun {
                bounds,
                opacity: 0.14,
            });
            painting
                .owned_labels
                .push((format!("{minutes} dk"), bounds));
            if view.focused == Some(QuickTarget::TimerPreset(index as u8)) {
                quick_focus(&mut painting, canvas, bounds);
            }
        }
        return painting;
    }
    if matches!(view.page, QuickPage::Volume | QuickPage::Brightness) {
        let value = if view.page == QuickPage::Volume {
            view.volume.map(|(level, _)| level)
        } else {
            view.brightness
        };
        if let Some(value) = value {
            let bounds = canvas.rect(SLIDER_BOUNDS);
            painting.shapes.push(ShapeRun {
                bounds,
                radius: 12.0 * canvas.scale,
                fill: [0.16, 0.16, 0.18, 1.0],
                border: None,
            });
            let fill = canvas.bounds(
                SLIDER_BOUNDS.x,
                SLIDER_BOUNDS.y,
                SLIDER_BOUNDS.width * f32::from(value) / 100.0,
                SLIDER_BOUNDS.height,
            );
            painting.shapes.push(ShapeRun {
                bounds: fill,
                radius: 12.0 * canvas.scale,
                fill: [0.6, 0.6, 0.65, 1.0],
                border: None,
            });
            painting.owned_labels.push((format!("{value}%"), bounds));
        } else {
            painting.texts.push(canvas.text(
                "Use Windows settings",
                canvas.rect(SLIDER_BOUNDS),
                13.0,
                false,
                0.8,
                Align::Center,
            ));
        }
        if view.page == QuickPage::Volume
            && let Some((_, muted)) = view.volume
        {
            let bounds = canvas.rect(MUTE_BOUNDS);
            painting.bars.push(BarRun {
                bounds,
                opacity: 0.16,
            });
            painting.texts.push(canvas.text(
                if muted { "Unmute" } else { "Mute" },
                bounds,
                12.0,
                true,
                1.0,
                Align::Center,
            ));
            if view.focused == Some(QuickTarget::Mute) {
                quick_focus(&mut painting, canvas, bounds);
            }
        }
        if view.focused == Some(QuickTarget::Slider) {
            quick_focus(&mut painting, canvas, canvas.rect(SLIDER_BOUNDS));
        }
        return painting;
    }
    for control in QuickControl::ALL {
        let area = control.bounds();
        let bounds = canvas.rect(area);
        painting.shapes.push(ShapeRun {
            bounds,
            radius: 12.0 * canvas.scale,
            fill: [0.16, 0.16, 0.18, 1.0],
            border: None,
        });
        let (icon, label) = match control {
            QuickControl::Wifi => (Icon::Wifi, "Wi-Fi"),
            QuickControl::Bluetooth => (Icon::Bluetooth, "Bluetooth"),
            QuickControl::Volume => (Icon::Volume, "Volume"),
            QuickControl::Brightness => (Icon::Brightness, "Brightness"),
            QuickControl::Timer => (Icon::Timer, "Timer"),
            QuickControl::Calendar => (Icon::Calendar, "Calendar"),
        };
        painting.texts.push(canvas.icon(glyph(icon), bounds, 22.0));
        painting.texts.push(canvas.text(
            label,
            canvas.bounds(
                area.x - 4.0,
                area.y + area.height + 5.0,
                area.width + 8.0,
                18.0,
            ),
            10.0,
            false,
            0.9,
            Align::Center,
        ));
        let radio = match control {
            QuickControl::Wifi => Some(view.wifi),
            QuickControl::Bluetooth => Some(view.bluetooth),
            _ => None,
        };
        if let Some(state) = radio {
            let label = match state {
                RadioState::On => "On",
                RadioState::Off => "Off",
                RadioState::Unknown => "—",
            };
            painting.texts.push(canvas.text(
                label,
                canvas.bounds(area.x, area.y + area.height + 22.0, area.width, 14.0),
                9.0,
                false,
                0.6,
                Align::Center,
            ));
        }
        let value = match control {
            QuickControl::Volume => view.volume.map(|(level, muted)| {
                if muted {
                    "Muted".to_owned()
                } else {
                    format!("{level}%")
                }
            }),
            QuickControl::Brightness => view.brightness.map(|level| format!("{level}%")),
            _ => None,
        };
        if let Some(value) = value {
            painting.owned_labels.push((
                value,
                canvas.bounds(area.x, area.y + area.height + 22.0, area.width, 14.0),
            ));
        }
        if view.focused == Some(QuickTarget::Control(control)) {
            painting.outlines.push(OutlineRun {
                bounds,
                radius: 12.0 * canvas.scale,
                width: 2.0 * canvas.scale,
                color: [1.0, 1.0, 1.0, 1.0],
                fraction: 1.0,
                dash: None,
                open_top: false,
            });
        }
    }
    painting
}

fn quick_focus(painting: &mut Painting<'_>, canvas: &Canvas, bounds: Bounds) {
    painting.outlines.push(OutlineRun {
        bounds,
        radius: 8.0 * canvas.scale,
        width: 2.0 * canvas.scale,
        color: [1.0, 1.0, 1.0, 1.0],
        fraction: 1.0,
        dash: None,
        open_top: false,
    });
}

fn drop_panel<'a>(view: &'a DropView, canvas: &Canvas) -> Painting<'a> {
    let mut painting = Painting::default();
    painting.texts.push(canvas.text(
        &view.header,
        canvas.rect(HEADER),
        13.0,
        true,
        0.9,
        Align::Center,
    ));
    for zone in Zone::ALL {
        let area = zone.bounds();
        let bounds = canvas.rect(area);
        let color = zone_color(zone);
        let refusal = view.refusal(zone);
        let lit = view.hovered == Some(zone) && refusal.is_none();
        let radius = 16.0 * canvas.scale;
        if lit {
            let glow = canvas.bounds(
                area.x - 3.0,
                area.y - 3.0,
                area.width + 6.0,
                area.height + 6.0,
            );
            painting.shapes.push(ShapeRun {
                bounds: glow,
                radius: radius + 3.0 * canvas.scale,
                fill: with_alpha(color, 0.16),
                border: None,
            });
        }
        let fill = match (refusal, lit) {
            (Some(_), _) => 0.05,
            (None, true) => 0.34,
            (None, false) => 0.14,
        };
        let edge = match (refusal, lit) {
            (Some(_), _) => 0.25,
            (None, true) => 1.0,
            (None, false) => 0.6,
        };
        painting.shapes.push(ShapeRun {
            bounds,
            radius,
            fill: with_alpha(color, fill),
            border: None,
        });
        let width = 1.5 * canvas.scale;
        // The shelf only holds on to where files are, which a dashed edge suggests.
        let dash = (zone == Zone::Shelf).then_some(4.0);
        painting.outlines.push(OutlineRun {
            bounds,
            radius,
            width,
            color: with_alpha(color, edge),
            fraction: 1.0,
            dash,
            open_top: false,
        });

        let opacity = if refusal.is_some() { 0.4 } else { 1.0 };
        let icon = canvas.bounds(area.x, area.y + 16.0, area.width, 34.0);
        painting.texts.push(TextRun {
            opacity,
            ..canvas.icon(glyph(zone_icon(zone)), icon, 22.0)
        });
        let label = canvas.bounds(area.x + 6.0, area.y + 56.0, area.width - 12.0, 20.0);
        painting
            .texts
            .push(canvas.text(zone.label(), label, 13.0, true, opacity, Align::Center));
        let hint = refusal.unwrap_or(match zone {
            Zone::Shelf => "Sonra sürükle",
            Zone::Phone => "QR kod ile",
            Zone::Convert => "JPG, PNG, küçült",
        });
        let note = canvas.bounds(area.x + 6.0, area.y + 78.0, area.width - 12.0, 18.0);
        painting
            .texts
            .push(canvas.text(hint, note, 11.0, false, 0.6 * opacity, Align::Center));
    }
    painting
}

/// The files kept on the shelf, a few at a time.
fn shelf_panel<'a>(view: &'a ShelfView, canvas: &Canvas) -> Painting<'a> {
    let mut painting = Painting::default();
    let title = canvas.bounds(24.0, SHELF_CLEAR.y, 220.0, SHELF_CLEAR.height);
    painting
        .texts
        .push(canvas.text(&view.title, title, 14.0, true, 1.0, Align::Leading));
    let clear = canvas.rect(SHELF_CLEAR);
    painting.bars.push(BarRun {
        bounds: clear,
        opacity: 0.14,
    });
    painting
        .texts
        .push(canvas.text("Temizle", clear, 12.0, true, 1.0, Align::Center));

    for (slot, item) in view.items.iter().enumerate() {
        let picture = thumbnail(slot);
        match &item.thumbnail {
            Some(encoded) => painting.images.push(ImageRun {
                encoded,
                x: picture.x * canvas.scale,
                y: picture.y * canvas.scale,
                size: picture.width * canvas.scale,
                radius: 10.0 * canvas.scale,
            }),
            None => {
                let bounds = canvas.rect(picture);
                painting.shapes.push(ShapeRun {
                    bounds,
                    radius: 10.0 * canvas.scale,
                    fill: [1.0, 1.0, 1.0, 0.1],
                    border: None,
                });
                let folder = !item.name.contains('.');
                painting.texts.push(canvas.icon(
                    if folder { ICON_FOLDER } else { ICON_DOCUMENT },
                    bounds,
                    24.0,
                ));
            }
        }
        painting.texts.push(canvas.text(
            &item.name,
            canvas.rect(tile_name(slot)),
            11.0,
            false,
            0.8,
            Align::Center,
        ));
        let cross = canvas.rect(remove_button(slot));
        painting.shapes.push(ShapeRun {
            bounds: cross,
            radius: cross.width / 2.0,
            fill: [0.24, 0.24, 0.26, 1.0],
            border: Some(([0.0, 0.0, 0.0, 1.0], canvas.scale)),
        });
        painting.texts.push(canvas.icon(ICON_CLOSE, cross, 8.0));
    }

    if view.total > SHELF_TILES {
        let back = view.first > 0;
        let forward = view.first + SHELF_TILES < view.total;
        painting.texts.push(TextRun {
            opacity: if back { 1.0 } else { 0.3 },
            ..canvas.icon(ICON_BACK, canvas.rect(SHELF_BACK), 12.0)
        });
        painting.texts.push(TextRun {
            opacity: if forward { 1.0 } else { 0.3 },
            ..canvas.icon(ICON_FORWARD, canvas.rect(SHELF_FORWARD), 12.0)
        });
        let place = canvas.bounds(
            SHELF_BACK.x + SHELF_BACK.width,
            SHELF_BACK.y,
            SHELF_FORWARD.x - SHELF_BACK.x - SHELF_BACK.width,
            SHELF_BACK.height,
        );
        painting
            .texts
            .push(canvas.text(&view.page, place, 11.5, false, 0.6, Align::Center));
    }
    painting
}

/// The formats pictures can be saved in.
fn convert_panel<'a>(view: &'a ConvertView, canvas: &Canvas) -> Painting<'a> {
    let mut painting = Painting::default();
    painting.texts.push(canvas.text(
        &view.header,
        canvas.rect(HEADER),
        13.0,
        true,
        0.9,
        Align::Center,
    ));
    let opacity = if view.busy { 0.4 } else { 1.0 };
    for target in Target::ALL {
        let area = target.bounds();
        let bounds = canvas.rect(area);
        painting.shapes.push(ShapeRun {
            bounds,
            radius: 14.0 * canvas.scale,
            fill: with_alpha(CONVERT_COLOR, 0.16 * opacity),
            border: None,
        });
        let label = canvas.bounds(area.x, area.y + 16.0, area.width, 26.0);
        painting
            .texts
            .push(canvas.text(target.label(), label, 17.0, true, opacity, Align::Center));
        let hint = canvas.bounds(area.x + 4.0, area.y + 46.0, area.width - 8.0, 18.0);
        painting.texts.push(canvas.text(
            target.hint(),
            hint,
            11.0,
            false,
            0.6 * opacity,
            Align::Center,
        ));
    }
    let note = if view.busy {
        "Dönüştürülüyor…"
    } else {
        "Yeni dosya aslının yanına kaydedilir"
    };
    painting.texts.push(canvas.text(
        note,
        canvas.rect(CONVERT_NOTE),
        11.5,
        false,
        0.6,
        Align::Center,
    ));
    painting
}

/// Converts layout written in DIPs to the pixels the surface is drawn in.
struct Canvas {
    scale: f32,
    width: f32,
    height: f32,
    corner: f32,
    notch: bool,
}

impl Canvas {
    fn rect(&self, rect: Rect) -> Bounds {
        self.bounds(rect.x, rect.y, rect.width, rect.height)
    }

    fn bounds(&self, x: f32, y: f32, width: f32, height: f32) -> Bounds {
        let scale = self.scale;
        Bounds {
            x: x * scale,
            y: y * scale,
            width: width * scale,
            height: height * scale,
        }
    }

    fn text<'a>(
        &self,
        text: &'a str,
        bounds: Bounds,
        size: f32,
        bold: bool,
        opacity: f32,
        align: Align,
    ) -> TextRun<'a> {
        TextRun {
            text,
            bounds,
            size: size * self.scale,
            bold,
            opacity,
            align,
            font: Font::Text,
            even_digits: false,
        }
    }

    fn icon<'a>(&self, glyph: &'a str, bounds: Bounds, size: f32) -> TextRun<'a> {
        TextRun {
            text: glyph,
            bounds,
            size: size * self.scale,
            bold: false,
            opacity: 1.0,
            align: Align::Center,
            font: Font::Icons,
            even_digits: false,
        }
    }

    /// A switch beside the transport buttons, with a dot under it while on.
    fn mode_button(
        &self,
        painting: &mut Painting<'_>,
        button: MediaButton,
        glyph: &'static str,
        on: bool,
    ) {
        const DOT: f32 = 4.0;
        let (center, half) = (button.center_x(), 21.0);
        let bounds = self.bounds(center - half, MEDIA_BUTTON_Y - half, half * 2.0, half * 2.0);
        let opacity = if on { 1.0 } else { 0.5 };
        painting.texts.push(TextRun {
            opacity,
            ..self.icon(glyph, bounds, 16.0)
        });
        if on {
            let bounds = self.bounds(center - DOT / 2.0, MEDIA_BUTTON_Y + 13.0, DOT, DOT);
            painting.bars.push(BarRun {
                bounds,
                opacity: 1.0,
            });
        }
    }

    /// The Quick Controls shortcut, in the same spot on expanded activity views.
    fn home_shortcut(&self, painting: &mut Painting<'_>) {
        for (offset, knob) in [(-6.0, -3.0), (0.0, 3.0), (6.0, -2.0)] {
            painting.shapes.push(ShapeRun {
                bounds: self.bounds(
                    HOME_SHORTCUT_X - 8.0,
                    HOME_SHORTCUT_Y + offset - 0.75,
                    16.0,
                    1.5,
                ),
                radius: self.scale,
                fill: [0.75, 0.75, 0.75, 1.0],
                border: None,
            });
            painting.shapes.push(ShapeRun {
                bounds: self.bounds(
                    HOME_SHORTCUT_X + knob - 2.0,
                    HOME_SHORTCUT_Y + offset - 2.0,
                    4.0,
                    4.0,
                ),
                radius: 2.0 * self.scale,
                fill: [1.0, 1.0, 1.0, 1.0],
                border: None,
            });
        }
    }

    fn artwork<'a>(
        &self,
        painting: &mut Painting<'a>,
        media: &'a MediaScene,
        x: f32,
        y: f32,
        size: f32,
        radius: f32,
    ) {
        match &media.artwork {
            Some(encoded) => painting.images.push(ImageRun {
                encoded,
                x: x * self.scale,
                y: y * self.scale,
                size: size * self.scale,
                radius: radius * self.scale,
            }),
            None => {
                let glyph = if media.is_music {
                    ICON_NOTE
                } else {
                    ICON_VIDEO
                };
                painting
                    .texts
                    .push(self.icon(glyph, self.bounds(x, y, size, size), size * 0.62));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_controls_paint_six_buttons_inside_the_panel_at_supported_scales() {
        let view = island_core::scheduler::QuickControlsView {
            page: QuickPage::Main,
            focused: None,
            calendar: island_core::scheduler::QuickCalendarView {
                state: CalendarState::NotSet,
                coming: Vec::new(),
            },
            presets: TIMER_PRESETS,
            mode: StartMode::Focus,
            modes_offered: true,
            volume: None,
            brightness: None,
            wifi: RadioState::Unknown,
            bluetooth: RadioState::Unknown,
        };
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let canvas = Canvas {
                scale,
                width: 400.0,
                height: 170.0,
                corner: 42.0,
                notch: false,
            };
            let painting = quick_controls_panel(&view, &canvas);
            assert_eq!(painting.shapes.len(), 7);
            assert_eq!(painting.texts.len(), 16);
            for shape in painting.shapes.iter().skip(1) {
                assert!(
                    shape.bounds.x >= 0.0
                        && shape.bounds.x + shape.bounds.width <= canvas.width * scale
                );
                assert!(
                    shape.bounds.y >= 0.0
                        && shape.bounds.y + shape.bounds.height <= canvas.height * scale
                );
            }
            for text in &painting.texts {
                assert!(
                    text.bounds.x >= 0.0
                        && text.bounds.x + text.bounds.width <= canvas.width * scale
                );
            }
        }
    }

    #[test]
    fn quick_controls_keep_confirmed_levels_at_each_scale() {
        let view = QuickControlsView {
            page: QuickPage::Main,
            focused: Some(QuickTarget::Control(QuickControl::Volume)),
            calendar: island_core::scheduler::QuickCalendarView {
                state: CalendarState::NotSet,
                coming: Vec::new(),
            },
            presets: TIMER_PRESETS,
            mode: StartMode::Timer,
            modes_offered: false,
            volume: Some((42, false)),
            brightness: Some(75),
            wifi: RadioState::On,
            bluetooth: RadioState::Unknown,
        };
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let canvas = Canvas {
                scale,
                width: 400.0,
                height: 170.0,
                corner: 42.0,
                notch: false,
            };
            let painting = quick_controls_panel(&view, &canvas);
            assert_eq!(painting.outlines.len(), 1);
            assert_eq!(painting.owned_labels.len(), 2);
            for (_, bounds) in &painting.owned_labels {
                assert!(bounds.x >= 0.0 && bounds.x + bounds.width <= canvas.width * scale);
                assert!(bounds.y >= 0.0 && bounds.y + bounds.height <= canvas.height * scale);
            }
        }
    }

    fn playing_music(wave_visible: bool) -> Scene {
        Scene {
            preferences: PresentationPreferences::default(),
            presentation: Presentation::Compact,
            wave_visible,
            clock: None,
            leading: "Example song title".into(),
            trailing: String::new(),
            title: String::new(),
            detail: String::new(),
            media: Some(MediaScene {
                artwork: None,
                playing: true,
                shuffle: None,
                repeat: None,
                sides: Sides::Tracks,
                is_music: true,
                can_previous: true,
                can_next: true,
                timing: None,
            }),
            icon: None,
            level: None,
            indicator_alert: false,
            indicator_context: None,
            timer: None,
            home: None,
            hideable: false,
            fit: DEFAULT_FIT,
            preview: None,
            panel: None,
            transfer: None,
            reveal: false,
            companion: None,
        }
    }

    fn text_geometry(scene: &Scene, pill: Pill) -> Vec<(String, u32, u32, u32, u32)> {
        scene
            .painting(pill, 1.0, pill.radius, false)
            .texts
            .iter()
            .map(|run| {
                (
                    run.text.to_owned(),
                    run.bounds.x.to_bits(),
                    run.bounds.y.to_bits(),
                    run.bounds.width.to_bits(),
                    run.bounds.height.to_bits(),
                )
            })
            .collect()
    }

    #[test]
    fn feature_preferences_do_not_change_other_feature_paintings() {
        let mut scene = playing_music(true);
        scene.detail = "Artist".into();
        let media = text_geometry(&scene, Pill::COMPACT);
        for style in IndicatorStyle::ALL {
            scene.preferences.indicator_style = style;
            assert_eq!(text_geometry(&scene, Pill::COMPACT), media);
        }
        scene.presentation = Presentation::Level;
        scene.media = None;
        scene.level = Some(42);
        scene.preferences.indicator_style = IndicatorStyle::Edge;
        let pill = IndicatorStyle::Edge.level_layout().pill;
        let indicator = text_geometry(&scene, pill);
        for layout in MediaLayout::ALL {
            scene.preferences.media_layout = layout;
            assert_eq!(text_geometry(&scene, pill), indicator);
        }
        scene.presentation = Presentation::Expanded;
        scene.timer = Some(TimerView {
            running: true,
            has_controls: true,
            left: Some(500),
        });
        scene.title = "05:00".into();
        let timer = text_geometry(&scene, Pill::EXPANDED);
        scene.timer = None;
        scene.home = Some(HomeScene {
            today: "Today".into(),
            events: Vec::new(),
            calendar: CalendarState::NotSet,
            mode: StartMode::Timer,
            modes_offered: true,
            presets: TIMER_PRESETS,
        });
        let home = text_geometry(&scene, Pill::EXPANDED);
        for style in IndicatorStyle::ALL {
            for layout in MediaLayout::ALL {
                scene.preferences = PresentationPreferences {
                    indicator_style: style,
                    media_layout: layout,
                };
                assert_eq!(text_geometry(&scene, Pill::EXPANDED), home);
                scene.home = None;
                scene.timer = Some(TimerView {
                    running: true,
                    has_controls: true,
                    left: Some(500),
                });
                assert_eq!(text_geometry(&scene, Pill::EXPANDED), timer);
                scene.timer = None;
                scene.home = Some(HomeScene {
                    today: "Today".into(),
                    events: Vec::new(),
                    calendar: CalendarState::NotSet,
                    mode: StartMode::Timer,
                    modes_offered: true,
                    presets: TIMER_PRESETS,
                });
            }
        }
    }

    #[test]
    fn media_layouts_show_only_known_information_and_keep_controls() {
        for layout in MediaLayout::ALL {
            for playing in [false, true] {
                let mut scene = playing_music(true);
                scene.preferences.media_layout = layout;
                scene.title = "Track".into();
                scene.detail = "Artist".into();
                scene.media.as_mut().unwrap().playing = playing;
                let compact = scene.painting(Pill::COMPACT, 1.0, Pill::COMPACT.radius, false);
                assert_eq!(
                    compact.texts.iter().any(|run| run.text == "Artist"),
                    layout != MediaLayout::Compact
                );
                assert!(compact.images.is_empty()); // Missing art uses the existing music glyph.
                assert_eq!(scene.shows_wave(), playing);
                scene.presentation = Presentation::Expanded;
                let expanded = scene.painting(Pill::EXPANDED, 1.0, Pill::EXPANDED.radius, false);
                assert_eq!(
                    expanded.texts.iter().any(|run| run.text == "Artist"),
                    layout != MediaLayout::Compact
                );
                assert_eq!(
                    expanded
                        .texts
                        .iter()
                        .any(|run| run.text == if playing { "Playing" } else { "Paused" }),
                    layout == MediaLayout::Informative
                );
                assert!(
                    expanded
                        .texts
                        .iter()
                        .any(|run| run.text == if playing { ICON_PAUSE } else { ICON_PLAY })
                );
                scene.detail.clear();
                let expanded = scene.painting(Pill::EXPANDED, 1.0, Pill::EXPANDED.radius, false);
                assert!(!expanded.texts.iter().any(|run| run.text == "Artist"));
            }
        }
    }

    #[test]
    fn empty_media_metadata_uses_a_safe_label_in_both_island_states() {
        let snapshot = MediaSnapshot {
            new_position: false,
            shuffle: None,
            repeat: None,
            title: String::new(),
            artist: String::new(),
            playing: true,
            position_ms: 0,
            duration_ms: 0,
            rate: 1.0,
            sampled_at: 0,
            can_seek: false,
            is_music: true,
            can_previous: false,
            can_next: false,
            artwork: None,
        };
        let clock = ClockText {
            time: String::new(),
            date: String::new(),
        };
        for presentation in [Presentation::Compact, Presentation::Expanded] {
            let frame = Frame {
                presentation,
                activity: Some(snapshot.clone().into_activity(0)),
                secondary: None,
                companion: None,
                home: None,
                panel: None,
                chime: false,
                wake_at: None,
            };
            let mut scene = Scene::new(&frame, &clock, 0, None, true);
            for layout in MediaLayout::ALL {
                scene.preferences.media_layout = layout;
                let pill = if presentation == Presentation::Compact {
                    Pill::COMPACT
                } else {
                    Pill::EXPANDED
                };
                let painting = scene.painting(pill, 1.0, pill.radius, false);
                assert!(painting.texts.iter().any(|run| run.text == "Media"));
                assert!(painting.images.is_empty());
            }
        }
    }

    #[test]
    fn unknown_indicator_percentage_does_not_draw_a_filled_ring() {
        let mut scene = playing_music(false);
        scene.presentation = Presentation::Level;
        scene.media = None;
        scene.level = None;
        scene.detail.clear();
        for style in IndicatorStyle::ALL {
            scene.preferences.indicator_style = style;
            let pill = style.level_layout().pill;
            let painting = scene.painting(pill, 1.0, pill.radius, false);
            assert!(painting.rings.is_empty());
            assert!(!painting.texts.iter().any(|run| run.text.contains('%')));
        }
    }

    #[test]
    fn detailed_music_and_timer_expose_existing_information() {
        let mut music = playing_music(false);
        music.detail = "Artist".into();
        music.preferences.media_layout = MediaLayout::Compact;
        let compact = music.painting(Pill::COMPACT, 1.0, Pill::COMPACT.radius, false);
        assert!(!compact.texts.iter().any(|text| text.text == "Artist"));
        music.preferences.media_layout = MediaLayout::Informative;
        let detailed = music.painting(Pill::COMPACT, 1.0, Pill::COMPACT.radius, false);
        assert!(detailed.texts.iter().any(|text| text.text == "Artist"));

        music.presentation = Presentation::Expanded;
        music.media = None;
        music.timer = Some(TimerView {
            running: true,
            has_controls: true,
            left: Some(500),
        });
        music.title = "05:00".into();
        music.icon = Some(Icon::Timer);
        let painting = music.painting(Pill::EXPANDED, 1.0, Pill::EXPANDED.radius, false);
        assert!(painting.texts.iter().any(|text| text.text == "Timer"));
        assert!(painting.texts.iter().any(|text| text.text == "Pause"));
    }

    #[test]
    fn compact_media_reserves_space_for_the_wave_or_its_companion_at_each_scale() {
        for density in MediaLayout::ALL {
            for title in [
                "Song",
                "A very long track title that must be clipped safely inside the island",
            ] {
                for scale in [1.0, 1.25, 1.5, 2.0] {
                    let mut scene = playing_music(true);
                    scene.preferences.media_layout = density;
                    scene.leading = title.into();
                    scene.detail = "A much longer artist name".into();
                    scene.fit =
                        scene.fit(|text, size, _| text.chars().count() as f32 * size * 0.55);
                    let pill = Pill {
                        width: scene.fit.width,
                        ..Pill::COMPACT
                    }
                    .scaled(scale);
                    let painting = scene.painting(pill, scale, Pill::COMPACT.radius, false);
                    assert!(scene.shows_wave());
                    let title_run = painting.texts.iter().find(|run| run.text == title).unwrap();
                    assert!(title_run.bounds.x >= COMPACT_ART_TEXT_X * scale);
                    assert!(
                        title_run.bounds.x + title_run.bounds.width
                            <= pill.width - WAVE_RESERVED * scale + 0.01
                    );
                    scene.companion = Some(Companion::Timed(Bubble::Count {
                        value: "3".into(),
                        icon: None,
                        ring: Some(50),
                    }));
                    assert!(!scene.shows_wave());
                    scene.fit =
                        scene.fit(|text, size, _| text.chars().count() as f32 * size * 0.55);
                    let pill = Pill {
                        width: scene.fit.width,
                        ..Pill::COMPACT
                    }
                    .scaled(scale);
                    let painting = scene.painting(pill, scale, Pill::COMPACT.radius, false);
                    let title_run = painting.texts.iter().find(|run| run.text == title).unwrap();
                    assert!(
                        title_run.bounds.x + title_run.bounds.width
                            <= pill.width - TASK_RESERVED * scale + 0.01
                    );
                }
            }
        }
    }

    #[test]
    fn detailed_artist_participates_in_compact_width_and_cache_key() {
        let mut scene = playing_music(false);
        scene.preferences.media_layout = MediaLayout::Informative;
        scene.leading = "A".into();
        let measure = |text: &str, size: f32, _: bool| text.len() as f32 * size * 0.5;
        let before = scene.fit(measure);
        let key = scene.fit_key();
        scene.detail = "An artist name much longer than the title".into();
        assert_ne!(scene.fit_key(), key);
        assert!(scene.fit(measure).width > before.width);
    }

    #[test]
    fn wave_only_runs_for_playing_compact_media_without_a_companion() {
        let mut scene = playing_music(true);
        assert!(scene.shows_wave());
        scene.presentation = Presentation::Expanded;
        assert!(!scene.shows_wave());
        scene.presentation = Presentation::Hidden;
        assert!(!scene.shows_wave());
        scene.presentation = Presentation::Compact;
        scene.media.as_mut().unwrap().playing = false;
        assert!(!scene.shows_wave());
        scene.media.as_mut().unwrap().playing = true;
        scene.companion = Some(Companion::Timed(Bubble::Count {
            value: "1".into(),
            icon: None,
            ring: None,
        }));
        assert!(!scene.shows_wave());
        scene.companion = None;
        scene.wave_visible = false;
        assert!(!scene.shows_wave());
    }

    #[test]
    fn detailed_system_level_keeps_the_value_and_adds_its_name() {
        let mut scene = playing_music(false);
        scene.presentation = Presentation::Level;
        scene.media = None;
        scene.leading = "Volume".into();
        scene.detail = "42%".into();
        scene.level = Some(42);
        for density in IndicatorStyle::ALL {
            scene.preferences.indicator_style = density;
            let pill = density.level_layout().pill;
            let painting = scene.painting(pill, 1.0, pill.radius, false);
            assert!(painting.texts.iter().any(|text| text.text == "42%"));
            assert_eq!(
                painting.texts.iter().any(|text| text.text == "Volume"),
                density != IndicatorStyle::Ring
            );
            assert_eq!(
                painting.rings.len(),
                usize::from(density == IndicatorStyle::Ring)
            );
            assert_eq!(
                painting.shapes.len(),
                if density == IndicatorStyle::Ring {
                    0
                } else {
                    3
                }
            );
        }
    }

    #[test]
    fn indicator_levels_render_in_bounds_for_all_densities_and_scales() {
        let mut scene = playing_music(false);
        scene.presentation = Presentation::Level;
        scene.media = None;
        scene.title = "Ses".into();
        scene.detail = "%75".into();
        scene.icon = Some(Icon::Volume);
        scene.indicator_context = Some(island_core::activity::IndicatorContext {
            subtitle: None,
            chips: vec!["Ses açık".into()],
        });
        for density in IndicatorStyle::ALL {
            scene.preferences.indicator_style = density;
            for value in [0, 1, 25, 50, 75, 99, 100, 255] {
                scene.level = Some(value);
                for scale in [1.0, 1.25, 1.5, 2.0] {
                    let layout = density.level_layout();
                    let pill = layout.pill.scaled(scale);
                    let painting = scene.painting(pill, scale, layout.pill.radius, false);
                    if density == IndicatorStyle::Ring {
                        assert_eq!(painting.rings.len(), 1);
                        assert_eq!(
                            painting.rings[0].fraction,
                            f32::from(value.min(100)) / 100.0
                        );
                    } else {
                        for shape in &painting.shapes {
                            assert!(shape.bounds.x >= 0.0 && shape.bounds.y >= 0.0);
                            assert!(shape.bounds.x + shape.bounds.width <= pill.width + 0.01);
                            assert!(shape.bounds.y + shape.bounds.height <= pill.height + 0.01);
                        }
                        let chip = usize::from(density == IndicatorStyle::Context);
                        assert_eq!(
                            painting.shapes.len(),
                            if value == 0 { 2 + chip } else { 3 + chip }
                        );
                        if value > 0 {
                            let filled = painting.shapes[2].bounds.width;
                            let expected =
                                layout.progress.width * scale * f32::from(value.min(100)) / 100.0;
                            assert!((filled - expected).abs() < 0.01);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn detailed_indicator_clips_long_context_and_limits_chips() {
        let mut scene = playing_music(false);
        scene.presentation = Presentation::Level;
        scene.media = None;
        scene.preferences.indicator_style = IndicatorStyle::Context;
        scene.title = "Ses".into();
        scene.detail = "%42".into();
        scene.level = Some(42);
        let long_name = "A long output device name that cannot fit in the indicator panel";
        scene.indicator_context = Some(island_core::activity::IndicatorContext {
            subtitle: Some(long_name.into()),
            chips: vec![
                "first very long context label".into(),
                "second long label".into(),
                "not displayed".into(),
            ],
        });
        let layout = IndicatorStyle::Context.level_layout();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let pill = layout.pill.scaled(scale);
            let painting = scene.painting(pill, scale, layout.pill.radius, false);
            let subtitle = painting
                .texts
                .iter()
                .find(|run| run.text == long_name)
                .unwrap();
            assert!(subtitle.bounds.x + subtitle.bounds.width <= layout.value.x * scale);
            assert!(!painting.texts.iter().any(|run| run.text == "not displayed"));
            let chips: Vec<_> = painting
                .texts
                .iter()
                .filter(|run| run.text.contains("label"))
                .collect();
            assert_eq!(chips.len(), 2);
            assert!(chips[0].bounds.x + chips[0].bounds.width < chips[1].bounds.x);
            assert!(chips[1].bounds.x + chips[1].bounds.width <= pill.width);
        }
    }

    #[test]
    fn quick_control_level_and_focus_updates_do_not_crossfade_the_panel() {
        let mut before = playing_music(false);
        before.presentation = Presentation::Expanded;
        before.panel = Some(PanelView::QuickControls(QuickControlsView {
            page: QuickPage::Volume,
            focused: Some(QuickTarget::Slider),
            calendar: island_core::scheduler::QuickCalendarView {
                state: CalendarState::NotSet,
                coming: Vec::new(),
            },
            presets: TIMER_PRESETS,
            mode: StartMode::Focus,
            modes_offered: true,
            volume: Some((30, false)),
            brightness: Some(50),
            wifi: RadioState::On,
            bluetooth: RadioState::Unknown,
        }));
        let mut after = before.clone();
        let Some(PanelView::QuickControls(view)) = &mut after.panel else {
            panic!("quick controls missing")
        };
        view.volume = Some((45, true));
        view.brightness = Some(65);
        view.focused = Some(QuickTarget::Mute);
        assert!(before.differs_only_in_value(&after));
        let Some(PanelView::QuickControls(view)) = &mut after.panel else {
            unreachable!()
        };
        view.page = QuickPage::Brightness;
        assert!(!before.differs_only_in_value(&after));
    }

    #[test]
    fn hiding_the_wave_gives_its_space_back_to_the_music_pill() {
        let shown = playing_music(true);
        let hidden = playing_music(false);
        let measure = |text: &str, _size: f32, _bold: bool| text.len() as f32 * 7.0;
        assert_eq!(
            shown.fit(measure).width - hidden.fit(measure).width,
            WAVE_RESERVED - COMPACT_TEXT_END
        );
        assert_ne!(shown.fit_key(), hidden.fit_key());
        assert!(shown.shows_wave());
        assert!(!hidden.shows_wave());
    }
}
