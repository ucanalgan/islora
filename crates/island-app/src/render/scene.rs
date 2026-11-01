//! Turns a frame into what gets painted. A `Scene` is plain data, so comparing
//! two of them tells whether the pixels would change at all.

use std::sync::Arc;

use island_core::layout::{
    COMPACT_MAX_WIDTH, COMPACT_MIN_WIDTH, EVENT_HIDE_HEIGHT, EVENT_HIDE_WIDTH, HOME_SHORTCUT_SIZE,
    HOME_SHORTCUT_X, HOME_SHORTCUT_Y, MEDIA_BUTTON_Y, MODE_TAB_HEIGHT,
    MODE_TAB_Y, MediaButton, PRESET_HEIGHT, PRESET_WIDTH, PRESET_Y, PROGRESS_HEIGHT,
    PROGRESS_WIDTH, PROGRESS_X, PROGRESS_Y, StartMode, TIMER_PRESETS, TimerButton, preset_center_x,
};
use island_core::media::{RepeatMode, format_time};
use island_core::{
    Bubble, CalendarState, Frame, HomeView, Icon, Kind, Millis, Pill, Presentation, TimerView,
};

use super::text::{Align, BarRun, Bounds, Font, ImageRun, Painting, RingRun, TextRun};

const ICON_NOTE: &str = "\u{E8D6}";
const ICON_PLAY: &str = "\u{E768}";
const ICON_PAUSE: &str = "\u{E769}";
const ICON_PREVIOUS: &str = "\u{E892}";
const ICON_NEXT: &str = "\u{E893}";
const ICON_CANCEL: &str = "\u{E711}";
const ICON_STOPWATCH: &str = "\u{E916}";
const ICON_SHUFFLE: &str = "\u{E8B1}";
const ICON_REPEAT_ALL: &str = "\u{E8EE}";
const ICON_REPEAT_ONE: &str = "\u{E8ED}";

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
    }
}

/// Right edge of the compact pill reserved for the sound wave, in DIPs.
pub const WAVE_RESERVED: f32 = 42.0;

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

/// Digits differ in width, so a countdown is measured as if all were zeros, or
/// the pill would twitch every second.
fn all_zeros(text: &str) -> String {
    text.chars().map(|character| if character.is_ascii_digit() { '0' } else { character }).collect()
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

#[derive(Clone, PartialEq)]
pub struct Scene {
    pub presentation: Presentation,
    pub clock: Option<String>,
    pub leading: String,
    pub trailing: String,
    pub title: String,
    pub detail: String,
    pub media: Option<MediaScene>,
    pub icon: Option<Icon>,
    pub level: Option<u8>,
    /// For an open timer view: whether it runs, and whether it has buttons.
    pub timer: Option<TimerView>,
    pub home: Option<HomeScene>,
    /// An event, which its open view offers to hide.
    pub hideable: bool,
    pub fit: CompactFit,
    pub preview: Option<Arc<[u8]>>,
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

const DEFAULT_FIT: CompactFit = CompactFit { width: Pill::COMPACT.width, trailing: 0.0 };

/// The space kept after the title: the wave only takes room while it is there.
fn compact_media_end(media: &MediaScene) -> f32 {
    if media.playing { WAVE_RESERVED } else { COMPACT_TEXT_END }
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
    pub fn new(frame: &Frame, clock: &ClockText, now: Millis, scrub: Option<f32>) -> Self {
        let presentation = frame.presentation;
        let Some(activity) = &frame.activity else {
            let home = frame.home.as_ref().map(|home| HomeScene::new(home, clock));
            return Self {
                presentation,
                clock: home.is_none().then(|| clock.time.clone()),
                leading: String::new(),
                trailing: String::new(),
                title: String::new(),
                detail: String::new(),
                media: None,
                icon: None,
                level: None,
                timer: None,
                home,
                hideable: false,
                fit: DEFAULT_FIT,
                preview: None,
            };
        };
        let media = activity.media.as_ref().map(|media| MediaScene {
            artwork: media.artwork.clone(),
            playing: media.playing,
            shuffle: media.shuffle,
            repeat: media.repeat,
            timing: (presentation == Presentation::Expanded)
                .then(|| media.progress_at(now).map(|progress| {
                    let progress = scrub.unwrap_or(progress);
                    let position = (media.duration_ms as f32 * progress) as u64;
                    Timing {
                        elapsed: format_time(position),
                        remaining: format!("-{}", format_time(media.duration_ms - position)),
                        permille: (progress * 1000.0) as u16,
                        scrubbing: scrub.is_some(),
                    }
                }))
                .flatten(),
        });
        Self {
            presentation,
            clock: None,
            leading: activity.leading.clone(),
            trailing: activity.trailing.clone(),
            title: activity.title.clone(),
            detail: activity.detail.clone(),
            media,
            icon: activity.icon,
            level: activity.level,
            timer: activity.timer,
            home: None,
            hideable: activity.kind == Kind::Upcoming,
            fit: DEFAULT_FIT,
            preview: activity.preview.clone(),
        }
    }

    /// Everything the fit depends on, so it is only measured when that changes.
    pub fn fit_key(&self) -> String {
        let cover = self.media.as_ref().map(|media| media.playing);
        format!(
            "{cover:?}|{}|{}|{}|{}",
            self.icon.is_some(),
            self.clock.as_deref().map(all_zeros).unwrap_or_default(),
            self.leading,
            all_zeros(&self.trailing),
        )
    }

    /// Works out how wide the compact pill should be. `measure` takes a text,
    /// its size and whether it is bold.
    pub fn fit(&self, measure: impl Fn(&str, f32, bool) -> f32) -> CompactFit {
        let text = |value: &str, bold| measure(value, COMPACT_TEXT_SIZE, bold).ceil() + MEASURE_SPARE;
        let trailing = if self.trailing.is_empty() { 0.0 } else { text(&all_zeros(&self.trailing), false) };
        let wanted = if let Some(clock) = &self.clock {
            measure(&all_zeros(clock), COMPACT_CLOCK_SIZE, true).ceil() + COMPACT_CLOCK_PADDING * 2.0
        } else if let Some(media) = &self.media {
            COMPACT_ART_TEXT_X + text(&self.leading, true) + compact_media_end(media)
        } else {
            let gap = if trailing > 0.0 { COMPACT_TEXT_GAP } else { 0.0 };
            self.text_start() + text(&self.leading, true) + gap + trailing + COMPACT_TEXT_END
        };
        CompactFit { width: wanted.clamp(COMPACT_MIN_WIDTH, COMPACT_MAX_WIDTH), trailing }
    }

    fn text_start(&self) -> f32 {
        if self.icon.is_some() { COMPACT_ICON_TEXT_START } else { COMPACT_TEXT_START }
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
            if scene.timer.is_some() {
                scene.title.clear();
            }
            if let Some(clock) = &mut scene.clock {
                clock.clear();
            }
            if let Some(home) = &mut scene.home {
                home.today.clear();
                home.mode = StartMode::default();
            }
            scene.trailing.clear();
            scene
        };
        without_value(self) == without_value(other)
    }

    pub fn shows_wave(&self) -> bool {
        self.presentation == Presentation::Compact
            && self.media.as_ref().is_some_and(|media| media.playing)
    }

    /// `corner` is how round the pill's corners look, in DIPs.
    pub fn painting(&self, pill: Pill, scale: f32, corner: f32) -> Painting<'_> {
        let canvas = Canvas { scale, width: pill.width / scale, height: pill.height / scale, corner };
        if let Some(clock) = &self.clock {
            let mut painting = Painting::default();
            let bounds = canvas.bounds(0.0, 0.0, canvas.width, canvas.height);
            painting.texts.push(canvas.text(clock, bounds, COMPACT_CLOCK_SIZE, true, 1.0, Align::Center));
            return painting;
        }
        if let Some(home) = &self.home {
            return self.home_view(home, &canvas);
        }
        if let (Presentation::Expanded, Some(timer)) = (self.presentation, self.timer) {
            return self.expanded_timer(timer, &canvas);
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
        if let Some(icon) = self.icon {
            painting.texts.push(canvas.icon(glyph(icon), canvas.bounds(12.0, 0.0, 24.0, height), 15.0));
        }
        let trailing = self.fit.trailing;
        let gap = if trailing > 0.0 { COMPACT_TEXT_GAP } else { 0.0 };
        let first = canvas.width - left - gap - trailing - COMPACT_TEXT_END;
        let leading = canvas.bounds(left, 0.0, first, height);
        painting.texts.push(canvas.text(&self.leading, leading, COMPACT_TEXT_SIZE, true, 1.0, Align::Leading));
        let value = canvas.bounds(left + first + gap, 0.0, trailing, height);
        painting.texts.push(canvas.text(&self.trailing, value, COMPACT_TEXT_SIZE, false, 0.75, Align::Trailing));
        painting
    }

    fn compact_media<'a>(&'a self, media: &'a MediaScene, canvas: &Canvas) -> Painting<'a> {
        let mut painting = Painting::default();
        let inset = (canvas.height - COMPACT_ART) / 2.0;
        // Corners inside corners only look right when they share a centre.
        let radius = (canvas.corner - inset).clamp(SMALLEST_ART_RADIUS, COMPACT_ART / 2.0);
        canvas.artwork(&mut painting, media, inset, inset, COMPACT_ART, radius);

        let width = canvas.width - COMPACT_ART_TEXT_X - compact_media_end(media);
        let bounds = canvas.bounds(COMPACT_ART_TEXT_X, 0.0, width, canvas.height);
        painting.texts.push(canvas.text(&self.leading, bounds, COMPACT_TEXT_SIZE, true, 1.0, Align::Leading));
        painting
    }

    fn level_bar(&self, canvas: &Canvas) -> Painting<'_> {
        let mut painting = Painting::default();
        let height = canvas.height;
        if let Some(icon) = self.icon {
            painting.texts.push(canvas.icon(glyph(icon), canvas.bounds(14.0, 0.0, 30.0, height), 18.0));
        }

        const BAR_X: f32 = 52.0;
        const BAR_HEIGHT: f32 = 4.0;
        const LABEL_WIDTH: f32 = 42.0;
        let width = canvas.width - BAR_X - LABEL_WIDTH - 22.0;
        let filled = width * self.level.unwrap_or(0) as f32 / 100.0;
        let top = (height - BAR_HEIGHT) / 2.0;
        painting.bars.push(BarRun { bounds: canvas.bounds(BAR_X, top, width, BAR_HEIGHT), opacity: 0.25 });
        if filled > 0.0 {
            painting.bars.push(BarRun { bounds: canvas.bounds(BAR_X, top, filled.max(BAR_HEIGHT), BAR_HEIGHT), opacity: 1.0 });
        }

        let label_x = canvas.width - LABEL_WIDTH - 16.0;
        painting.texts.push(canvas.text(&self.detail, canvas.bounds(label_x, 0.0, LABEL_WIDTH, height), 12.5, false, 0.8, Align::Trailing));
        painting
    }

    fn alert(&self, canvas: &Canvas) -> Painting<'_> {
        let mut painting = Painting::default();
        let inset = (canvas.height - ALERT_ART) / 2.0;
        let left = match (&self.preview, self.icon) {
            (Some(_), _) => inset + ALERT_ART + ALERT_ART_TEXT_GAP,
            (None, Some(_)) => 62.0,
            (None, None) => 28.0,
        };
        if let Some(encoded) = &self.preview {
            painting.images.push(ImageRun {
                encoded,
                x: inset * canvas.scale,
                y: inset * canvas.scale,
                size: ALERT_ART * canvas.scale,
                radius: ALERT_ART_RADIUS * canvas.scale,
            });
        } else if let Some(icon) = self.icon {
            painting.texts.push(canvas.icon(glyph(icon), canvas.bounds(20.0, 0.0, 34.0, canvas.height), 22.0));
        }
        let inner = canvas.width - left - 28.0;
        painting.texts.push(canvas.text(&self.title, canvas.bounds(left, 12.0, inner, 22.0), 14.0, true, 1.0, Align::Leading));
        painting.texts.push(canvas.text(&self.detail, canvas.bounds(left, 33.0, inner, 20.0), 12.5, false, 0.7, Align::Leading));
        painting
    }

    fn expanded_text(&self, canvas: &Canvas) -> Painting<'_> {
        let mut painting = Painting::default();
        canvas.home_shortcut(&mut painting);
        let inner = canvas.width - 64.0 - HOME_SHORTCUT_SIZE;
        painting.texts.push(canvas.text(&self.title, canvas.bounds(32.0, 30.0, inner, 30.0), 19.0, true, 1.0, Align::Leading));
        painting.texts.push(canvas.text(&self.detail, canvas.bounds(32.0, 62.0, inner, 24.0), 14.0, false, 0.7, Align::Leading));
        if self.hideable {
            let left = (canvas.width - EVENT_HIDE_WIDTH) / 2.0;
            let bounds = canvas.bounds(left, MEDIA_BUTTON_Y - EVENT_HIDE_HEIGHT / 2.0, EVENT_HIDE_WIDTH, EVENT_HIDE_HEIGHT);
            painting.bars.push(BarRun { bounds, opacity: 0.14 });
            painting.texts.push(canvas.text("Gizle", bounds, 13.0, true, 1.0, Align::Center));
        }
        painting
    }

    fn expanded_timer(&self, timer: TimerView, canvas: &Canvas) -> Painting<'_> {
        let mut painting = Painting::default();
        canvas.home_shortcut(&mut painting);
        painting.texts.push(canvas.text(&self.title, canvas.bounds(0.0, 26.0, canvas.width, 56.0), 44.0, true, 1.0, Align::Center));
        painting.texts.push(canvas.text(&self.detail, canvas.bounds(0.0, 84.0, canvas.width, 22.0), 13.0, false, 0.7, Align::Center));
        let buttons: &[TimerButton] = if timer.has_controls { &TimerButton::ALL } else { &[] };
        for &button in buttons {
            let glyph = match button {
                TimerButton::Toggle if timer.running => ICON_PAUSE,
                TimerButton::Toggle => ICON_PLAY,
                TimerButton::Cancel => ICON_CANCEL,
            };
            let half = 21.0;
            let bounds = canvas.bounds(button.center_x() - half, MEDIA_BUTTON_Y - half, half * 2.0, half * 2.0);
            painting.texts.push(canvas.icon(glyph, bounds, 21.0));
        }
        painting
    }

    fn home_view<'a>(&'a self, home: &'a HomeScene, canvas: &Canvas) -> Painting<'a> {
        let mut painting = Painting::default();
        const MARGIN: f32 = 30.0;
        let inner = canvas.width - MARGIN * 2.0;
        let label = canvas.bounds(MARGIN, 14.0, inner, 16.0);
        painting.texts.push(canvas.text(&home.today, label, 11.5, false, 0.6, Align::Trailing));
        match home.events.as_slice() {
            [] => {
                let (title, detail) = match home.calendar {
                    CalendarState::Ready => ("Yaklaşan etkinlik yok", ""),
                    CalendarState::NotSet => ("Takvim bağlı değil", "Tepsi menüsünden Ayarları aç"),
                    CalendarState::Unreachable => ("Takvime ulaşılamadı", "Bağlantı gelince yeniden denenecek"),
                };
                painting.texts.push(canvas.text(title, canvas.bounds(MARGIN, 30.0, inner, 24.0), 14.0, false, 0.8, Align::Leading));
                painting.texts.push(canvas.text(detail, canvas.bounds(MARGIN, 54.0, inner, 20.0), 12.5, false, 0.6, Align::Leading));
            }
            [only] => {
                painting.texts.push(canvas.text("Sıradaki", label, 11.5, false, 0.6, Align::Leading));
                painting.texts.push(canvas.text(&only.title, canvas.bounds(MARGIN, 30.0, inner, 24.0), 15.0, true, 1.0, Align::Leading));
                painting.texts.push(canvas.text(&only.detail, canvas.bounds(MARGIN, 54.0, inner, 20.0), 12.5, false, 0.7, Align::Leading));
            }
            several => {
                const WHEN_WIDTH: f32 = 96.0;
                painting.texts.push(canvas.text("Sıradaki", label, 11.5, false, 0.6, Align::Leading));
                for (index, event) in several.iter().enumerate() {
                    let top = 32.0 + index as f32 * 23.0;
                    let title = canvas.bounds(MARGIN, top, inner - WHEN_WIDTH - 8.0, 22.0);
                    let when = canvas.bounds(MARGIN + inner - WHEN_WIDTH, top, WHEN_WIDTH, 22.0);
                    painting.texts.push(canvas.text(&event.title, title, 13.5, true, 1.0, Align::Leading));
                    painting.texts.push(canvas.text(&event.when, when, 12.0, false, 0.7, Align::Trailing));
                }
            }
        }
        if home.modes_offered {
            for mode in StartMode::ALL {
                let (left, width) = mode.tab();
                let bounds = canvas.bounds(left, MODE_TAB_Y - MODE_TAB_HEIGHT / 2.0, width, MODE_TAB_HEIGHT);
                let chosen = mode == home.mode;
                if chosen {
                    painting.bars.push(BarRun { bounds, opacity: 0.16 });
                }
                let opacity = if chosen { 1.0 } else { 0.6 };
                painting.texts.push(canvas.text(mode.label(), bounds, 11.5, chosen, opacity, Align::Center));
            }
        } else {
            let bounds = canvas.bounds(MARGIN, MODE_TAB_Y - 8.0, inner, 16.0);
            painting.texts.push(canvas.text("Zamanlayıcı başlat", bounds, 11.5, false, 0.6, Align::Leading));
        }
        for (index, minutes) in home.presets.iter().enumerate() {
            let center = preset_center_x(index);
            let bounds = canvas.bounds(center - PRESET_WIDTH / 2.0, PRESET_Y - PRESET_HEIGHT / 2.0, PRESET_WIDTH, PRESET_HEIGHT);
            painting.bars.push(BarRun { bounds, opacity: 0.14 });
            painting.owned_labels.push((format!("{minutes} dk"), bounds));
        }
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
        painting.texts.push(canvas.text(&self.title, canvas.bounds(text_x, 26.0, text_width, 28.0), 16.0, true, 1.0, Align::Leading));
        painting.texts.push(canvas.text(&self.detail, canvas.bounds(text_x, 53.0, text_width, 22.0), 13.0, false, 0.7, Align::Leading));

        if let Some(timing) = &media.timing {
            // Thicker with a handle while dragging, so it is clear what moves.
            const KNOB: f32 = 14.0;
            let height = if timing.scrubbing { PROGRESS_HEIGHT + 2.0 } else { PROGRESS_HEIGHT };
            let top = PROGRESS_Y + (PROGRESS_HEIGHT - height) / 2.0;
            let filled = PROGRESS_WIDTH * timing.permille as f32 / 1000.0;
            painting.bars.push(BarRun { bounds: canvas.bounds(PROGRESS_X, top, PROGRESS_WIDTH, height), opacity: 0.2 });
            painting.bars.push(BarRun { bounds: canvas.bounds(PROGRESS_X, top, filled.max(height), height), opacity: 1.0 });
            if timing.scrubbing {
                let middle = PROGRESS_Y + PROGRESS_HEIGHT / 2.0;
                let bounds = canvas.bounds(PROGRESS_X + filled - KNOB / 2.0, middle - KNOB / 2.0, KNOB, KNOB);
                painting.bars.push(BarRun { bounds, opacity: 1.0 });
            }

            let label_y = MEDIA_BUTTON_Y - 9.0;
            painting.texts.push(canvas.text(&timing.elapsed, canvas.bounds(MARGIN, label_y, 72.0, 18.0), 11.0, false, 0.6, Align::Leading));
            painting.texts.push(canvas.text(&timing.remaining, canvas.bounds(canvas.width - MARGIN - 72.0, label_y, 72.0, 18.0), 11.0, false, 0.6, Align::Trailing));
        }

        for button in MediaButton::TRANSPORT {
            let (glyph, size) = match button {
                MediaButton::Previous => (ICON_PREVIOUS, 19.0),
                MediaButton::PlayPause if media.playing => (ICON_PAUSE, 25.0),
                MediaButton::PlayPause => (ICON_PLAY, 25.0),
                _ => (ICON_NEXT, 19.0),
            };
            let half = 21.0;
            let bounds = canvas.bounds(button.center_x() - half, MEDIA_BUTTON_Y - half, half * 2.0, half * 2.0);
            painting.texts.push(canvas.icon(glyph, bounds, size));
        }
        if let Some(on) = media.shuffle {
            canvas.mode_button(&mut painting, MediaButton::Shuffle, ICON_SHUFFLE, on);
        }
        if let Some(repeat) = media.repeat {
            let glyph = if repeat == RepeatMode::One { ICON_REPEAT_ONE } else { ICON_REPEAT_ALL };
            canvas.mode_button(&mut painting, MediaButton::Repeat, glyph, repeat != RepeatMode::Off);
        }
        painting
    }
}

/// What the small bubble beside the pill shows.
pub fn bubble_painting(bubble: &Bubble, size: f32, scale: f32) -> Painting<'_> {
    let canvas = Canvas { scale, width: size / scale, height: size / scale, corner: size / scale / 2.0 };
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
            painting.texts.push(canvas.icon(glyph(*icon), bounds, 16.0));
        }
        Bubble::Count { value, icon, ring } => {
            if let Some(percent) = ring {
                painting.rings.push(RingRun {
                    center_x: middle * scale,
                    center_y: middle * scale,
                    radius: (middle - 3.0) * scale,
                    width: 2.0 * scale,
                    fraction: *percent as f32 / 100.0,
                });
            }
            let (text_top, text_height) = match icon {
                Some(icon) => {
                    painting.texts.push(canvas.icon(glyph(*icon), canvas.bounds(0.0, 5.0, canvas.width, 12.0), 10.0));
                    (16.0, 15.0)
                }
                None => (0.0, canvas.height),
            };
            let bounds = canvas.bounds(0.0, text_top, canvas.width, text_height);
            painting.texts.push(canvas.text(value, bounds, 12.0, true, 1.0, Align::Center));
        }
    }
    painting
}

/// Converts layout written in DIPs to the pixels the surface is drawn in.
struct Canvas {
    scale: f32,
    width: f32,
    height: f32,
    corner: f32,
}

impl Canvas {
    fn bounds(&self, x: f32, y: f32, width: f32, height: f32) -> Bounds {
        let scale = self.scale;
        Bounds { x: x * scale, y: y * scale, width: width * scale, height: height * scale }
    }

    fn text<'a>(&self, text: &'a str, bounds: Bounds, size: f32, bold: bool, opacity: f32, align: Align) -> TextRun<'a> {
        TextRun { text, bounds, size: size * self.scale, bold, opacity, align, font: Font::Text }
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
        }
    }

    /// A switch beside the transport buttons, with a dot under it while on.
    fn mode_button(&self, painting: &mut Painting<'_>, button: MediaButton, glyph: &'static str, on: bool) {
        const DOT: f32 = 4.0;
        let (center, half) = (button.center_x(), 21.0);
        let bounds = self.bounds(center - half, MEDIA_BUTTON_Y - half, half * 2.0, half * 2.0);
        let opacity = if on { 1.0 } else { 0.5 };
        painting.texts.push(TextRun { opacity, ..self.icon(glyph, bounds, 16.0) });
        if on {
            let bounds = self.bounds(center - DOT / 2.0, MEDIA_BUTTON_Y + 13.0, DOT, DOT);
            painting.bars.push(BarRun { bounds, opacity: 1.0 });
        }
    }

    /// The way to the home view, which every expanded view has in the same spot.
    fn home_shortcut(&self, painting: &mut Painting<'_>) {
        let half = HOME_SHORTCUT_SIZE / 2.0;
        let bounds = self.bounds(HOME_SHORTCUT_X - half, HOME_SHORTCUT_Y - half, HOME_SHORTCUT_SIZE, HOME_SHORTCUT_SIZE);
        painting.texts.push(self.icon(ICON_STOPWATCH, bounds, 16.0));
    }

    fn artwork<'a>(&self, painting: &mut Painting<'a>, media: &'a MediaScene, x: f32, y: f32, size: f32, radius: f32) {
        match &media.artwork {
            Some(encoded) => painting.images.push(ImageRun {
                encoded,
                x: x * self.scale,
                y: y * self.scale,
                size: size * self.scale,
                radius: radius * self.scale,
            }),
            None => painting.texts.push(self.icon(ICON_NOTE, self.bounds(x, y, size, size), size * 0.62)),
        }
    }
}
