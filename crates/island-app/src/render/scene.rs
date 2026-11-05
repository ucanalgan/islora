//! Turns a frame into what gets painted. A `Scene` is plain data, so comparing
//! two of them tells whether the pixels would change at all.

use std::sync::Arc;

use island_core::layout::{
    COMPACT_MAX_WIDTH, COMPACT_MIN_WIDTH, EVENT_HIDE_HEIGHT, EVENT_HIDE_WIDTH, HOME_SHORTCUT_SIZE,
    HOME_SHORTCUT_X, HOME_SHORTCUT_Y, MEDIA_BUTTON_Y, MODE_TAB_HEIGHT,
    MODE_TAB_Y, MediaButton, PRESET_HEIGHT, PRESET_WIDTH, PRESET_Y, PROGRESS_HEIGHT,
    PROGRESS_WIDTH, PROGRESS_X, PROGRESS_Y, SETTINGS_SHORTCUT_SIZE, SETTINGS_SHORTCUT_X,
    SETTINGS_SHORTCUT_Y, StartMode, TIMER_BUTTON_SIZE, TIMER_PRESETS,
    TIMER_RING_RADIUS, TIMER_RING_WIDTH, TIMER_RING_X, TIMER_RING_Y, TimerButton, preset_center_x,
};
use island_core::convert::Target;
use island_core::drop::{
    CONVERT_NOTE, DropView, HEADER, Rect, SHELF_BACK, SHELF_CLEAR, SHELF_FORWARD, SHELF_TILES,
    TRANSFER_CODE, TRANSFER_STOP, TRANSFER_TEXT_LEFT, Zone, remove_button, thumbnail, tile_name,
};
use island_core::media::{RepeatMode, Sides, format_time};
use island_core::shelf::ShelfView;
use island_core::transfer::{Shown, TransferView};
use island_core::{
    Bubble, CalendarState, ConvertView, Frame, HomeView, Icon, Kind, Millis, PanelView, Pill,
    Presentation, ProgressStyle, TimerView,
};

use super::text::{
    Align, BarRun, Bounds, CodeRun, Font, ImageRun, OutlineRun, Painting, RingRun, ShapeRun, TextRun,
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
/// A paused countdown steps back, so its state reads without any words.
const PAUSED_OPACITY: f32 = 0.55;

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
    /// A view a drop opened, or the panel for files being dragged.
    pub panel: Option<PanelView>,
    pub transfer: Option<TransferView>,
    /// The alert's file is shown in its folder, which a button on it offers.
    pub reveal: bool,
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
                clock: (home.is_none() && frame.panel.is_none()).then(|| clock.time.clone()),
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
                panel: frame.panel.clone(),
                transfer: None,
                reveal: false,
            };
        };
        let media = activity.media.as_ref().map(|media| MediaScene {
            artwork: media.artwork.clone(),
            playing: media.playing,
            shuffle: media.shuffle,
            repeat: media.repeat,
            sides: media.sides(),
            is_music: media.is_music,
            can_previous: media.can_previous,
            can_next: media.can_next,
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
            panel: None,
            transfer: activity.transfer.clone(),
            reveal: activity.reveal && activity.file.is_some(),
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
            if let Some(transfer) = &mut scene.transfer {
                transfer.shown = match transfer.shown {
                    Shown::Waiting { .. } => Shown::Waiting { left: String::new() },
                    Shown::Sending { .. } => Shown::Sending { permille: 0, amount: String::new() },
                    Shown::Done => Shown::Done,
                };
            }
            match &mut scene.panel {
                Some(PanelView::Drop(view)) => view.hovered = None,
                Some(PanelView::Shelf(view)) => view.items.iter_mut().for_each(|item| item.thumbnail = None),
                Some(PanelView::Convert(view)) => view.busy = false,
                None => {}
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
    /// `notch` says the top edge joins the screen's.
    pub fn painting(&self, pill: Pill, scale: f32, corner: f32, notch: bool) -> Painting<'_> {
        let canvas = Canvas { scale, width: pill.width / scale, height: pill.height / scale, corner, notch };
        if let Some(clock) = &self.clock {
            let mut painting = Painting::default();
            let bounds = canvas.bounds(0.0, 0.0, canvas.width, canvas.height);
            painting.texts.push(canvas.text(clock, bounds, COMPACT_CLOCK_SIZE, true, 1.0, Align::Center));
            return painting;
        }
        if let Some(panel) = &self.panel {
            return match panel {
                PanelView::Drop(view) => drop_panel(view, &canvas),
                PanelView::Shelf(view) => shelf_panel(view, &canvas),
                PanelView::Convert(view) => convert_panel(view, &canvas),
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
        let sending = self.transfer.as_ref().and_then(|transfer| match transfer.shown {
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
        const REVEAL_WIDTH: f32 = 64.0;
        let button = if self.reveal { REVEAL_WIDTH + 10.0 } else { 0.0 };
        let inner = canvas.width - left - 28.0 - button;
        painting.texts.push(canvas.text(&self.title, canvas.bounds(left, 12.0, inner, 22.0), 14.0, true, 1.0, Align::Leading));
        painting.texts.push(canvas.text(&self.detail, canvas.bounds(left, 33.0, inner, 20.0), 12.5, false, 0.7, Align::Leading));
        if self.reveal {
            let bounds = canvas.bounds(canvas.width - 22.0 - REVEAL_WIDTH, (canvas.height - 28.0) / 2.0, REVEAL_WIDTH, 28.0);
            painting.bars.push(BarRun { bounds, opacity: 0.16 });
            painting.texts.push(canvas.text("Göster", bounds, 12.5, true, 1.0, Align::Center));
        }
        painting
    }

    fn expanded_transfer<'a>(&'a self, transfer: &'a TransferView, canvas: &Canvas) -> Painting<'a> {
        let mut painting = Painting::default();
        let code = canvas.rect(TRANSFER_CODE);
        let middle_x = TRANSFER_CODE.x + TRANSFER_CODE.width / 2.0;
        let middle_y = TRANSFER_CODE.y + TRANSFER_CODE.height / 2.0;
        let title = match &transfer.shown {
            Shown::Waiting { left } => {
                painting.codes.push(CodeRun { bounds: code, code: &transfer.code });
                let stop = TRANSFER_STOP;
                let bounds = canvas.bounds(stop.x + stop.width + 10.0, stop.y, canvas.width - stop.x - stop.width - 34.0, stop.height);
                painting.texts.push(canvas.text(left, bounds, 12.0, false, 0.6, Align::Trailing));
                "Telefona gönder"
            }
            Shown::Sending { permille, .. } => {
                let fraction = f32::from(*permille) / 1000.0;
                // Labels made while painting are sized by their height.
                let percent = canvas.bounds(TRANSFER_CODE.x, middle_y - 47.0, TRANSFER_CODE.width, 94.0);
                match transfer.style {
                    ProgressStyle::Ring => {
                        painting.rings.push(RingRun {
                            center_x: middle_x * canvas.scale,
                            center_y: middle_y * canvas.scale,
                            radius: 54.0 * canvas.scale,
                            width: 7.0 * canvas.scale,
                            fraction,
                            opacity: 1.0,
                        });
                        painting.owned_labels.push((format!("%{}", permille / 10), canvas.bounds(TRANSFER_CODE.x, middle_y - 36.0, TRANSFER_CODE.width, 72.0)));
                    }
                    ProgressStyle::Edge => {
                        let whole = canvas.bounds(0.0, 0.0, canvas.width, canvas.height);
                        let radius = canvas.corner * canvas.scale;
                        painting.outlines.push(OutlineRun { bounds: whole, radius, width: 7.0 * canvas.scale, color: with_alpha(PHONE_COLOR, 0.22), fraction, dash: None, open_top: canvas.notch });
                        painting.outlines.push(OutlineRun { bounds: whole, radius, width: 2.5 * canvas.scale, color: with_alpha(PHONE_COLOR, 1.0), fraction, dash: None, open_top: canvas.notch });
                        painting.owned_labels.push((format!("%{}", permille / 10), percent));
                    }
                    ProgressStyle::Fill => {
                        let reach = canvas.width * fraction;
                        let filled = canvas.bounds(0.0, 0.0, reach, canvas.height);
                        painting.shapes.push(ShapeRun { bounds: filled, radius: 0.0, fill: with_alpha(PHONE_COLOR, 0.26), border: None });
                        // A brighter front edge, so it reads as moving rather than as a colour.
                        let front = canvas.bounds(reach - 2.0, 0.0, 2.0, canvas.height);
                        painting.shapes.push(ShapeRun { bounds: front, radius: 0.0, fill: with_alpha(PHONE_COLOR, 0.8), border: None });
                        painting.owned_labels.push((format!("%{}", permille / 10), percent));
                    }
                }
                "Gönderiliyor"
            }
            Shown::Done => {
                let circle = canvas.bounds(middle_x - 40.0, middle_y - 40.0, 80.0, 80.0);
                painting.shapes.push(ShapeRun { bounds: circle, radius: 40.0 * canvas.scale, fill: with_alpha(PHONE_COLOR, 1.0), border: None });
                painting.texts.push(canvas.icon(glyph(Icon::Check), circle, 38.0));
                "Gönderildi"
            }
        };

        let left = TRANSFER_TEXT_LEFT;
        let width = canvas.width - left - 26.0;
        painting.texts.push(canvas.text(title, canvas.bounds(left, 22.0, width, 24.0), 16.0, true, 1.0, Align::Leading));
        painting.texts.push(canvas.text(&transfer.summary, canvas.bounds(left, 47.0, width, 20.0), 13.0, false, 0.75, Align::Leading));
        let line = canvas.bounds(left, 70.0, width, 18.0);
        let note = canvas.bounds(left, 89.0, width, 18.0);
        match &transfer.shown {
            Shown::Waiting { .. } => {
                painting.texts.push(canvas.text(&transfer.network, line, 12.0, false, 0.6, Align::Leading));
                match &transfer.warning {
                    Some(warning) => painting.colored.push((canvas.text(warning, note, 12.0, false, 1.0, Align::Leading), WARNING_COLOR)),
                    None => painting.texts.push(canvas.text("Telefonun kamerasıyla okut", note, 12.0, false, 0.6, Align::Leading)),
                }
            }
            Shown::Sending { amount, .. } => {
                painting.texts.push(canvas.text(amount, line, 12.0, false, 0.6, Align::Leading));
            }
            Shown::Done => {
                painting.texts.push(canvas.text(&transfer.network, line, 12.0, false, 0.6, Align::Leading));
            }
        }
        if transfer.shown != Shown::Done {
            let stop = canvas.rect(TRANSFER_STOP);
            painting.bars.push(BarRun { bounds: stop, opacity: 0.14 });
            painting.texts.push(canvas.text("Durdur", stop, 12.5, true, 1.0, Align::Center));
        }
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
        let opacity = if timer.running { 1.0 } else { PAUSED_OPACITY };
        painting.rings.push(RingRun {
            center_x: TIMER_RING_X * canvas.scale,
            center_y: TIMER_RING_Y * canvas.scale,
            radius: TIMER_RING_RADIUS * canvas.scale,
            width: TIMER_RING_WIDTH * canvas.scale,
            fraction: timer.left.map_or(0.0, |left| left as f32 / 1000.0),
            opacity,
        });

        let size = if self.title.len() > LONG_COUNTDOWN { LONG_COUNTDOWN_SIZE } else { COUNTDOWN_SIZE };
        let inside = (TIMER_RING_RADIUS - TIMER_RING_WIDTH / 2.0) * 2.0;
        let countdown = canvas.bounds(TIMER_RING_X - inside / 2.0, TIMER_RING_Y - 28.0, inside, 44.0);
        painting.texts.push(TextRun {
            even_digits: true,
            ..canvas.text(&self.title, countdown, size, true, opacity, Align::Center)
        });
        let label = canvas.bounds(TIMER_RING_X - COUNTDOWN_LABEL_WIDTH / 2.0, TIMER_RING_Y + 18.0, COUNTDOWN_LABEL_WIDTH, 18.0);
        painting.texts.push(canvas.text(&self.detail, label, 11.5, false, 0.6, Align::Center));

        let buttons: &[TimerButton] = if timer.has_controls { &TimerButton::ALL } else { &[] };
        for &button in buttons {
            let glyph = match button {
                TimerButton::Toggle if timer.running => ICON_PAUSE,
                TimerButton::Toggle => ICON_PLAY,
                TimerButton::Cancel => ICON_CANCEL,
            };
            let half = TIMER_BUTTON_SIZE / 2.0;
            let bounds = canvas.bounds(button.center_x() - half, TIMER_RING_Y - half, TIMER_BUTTON_SIZE, TIMER_BUTTON_SIZE);
            painting.bars.push(BarRun { bounds, opacity: 0.14 });
            painting.texts.push(canvas.icon(glyph, bounds, 19.0));
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
                    CalendarState::NotSet => ("Takvim bağlı değil", "Ayarlar'dan takvim adresini ekle"),
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
        let half = SETTINGS_SHORTCUT_SIZE / 2.0;
        let gear = canvas.bounds(SETTINGS_SHORTCUT_X - half, SETTINGS_SHORTCUT_Y - half, SETTINGS_SHORTCUT_SIZE, SETTINGS_SHORTCUT_SIZE);
        painting.texts.push(canvas.icon(ICON_SETTINGS, gear, 15.0));
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
            let (glyph, size, opacity) = match (button, media.sides) {
                (MediaButton::PlayPause, _) if media.playing => (ICON_PAUSE, 25.0, 1.0),
                (MediaButton::PlayPause, _) => (ICON_PLAY, 25.0, 1.0),
                (MediaButton::Previous, Sides::Jumps) => (ICON_BACK_10, 21.0, 1.0),
                (MediaButton::Previous, Sides::Tracks) if media.can_previous => (ICON_PREVIOUS, 19.0, 1.0),
                (MediaButton::Previous, _) => (ICON_PREVIOUS, 19.0, UNAVAILABLE_OPACITY),
                (_, Sides::Jumps) => (ICON_FORWARD_30, 21.0, 1.0),
                (_, Sides::Tracks) if media.can_next => (ICON_NEXT, 19.0, 1.0),
                _ => (ICON_NEXT, 19.0, UNAVAILABLE_OPACITY),
            };
            let half = 21.0;
            let bounds = canvas.bounds(button.center_x() - half, MEDIA_BUTTON_Y - half, half * 2.0, half * 2.0);
            painting.texts.push(TextRun { opacity, ..canvas.icon(glyph, bounds, size) });
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
    let canvas = Canvas { scale, width: size / scale, height: size / scale, corner: size / scale / 2.0, notch: false };
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
                    opacity: 1.0,
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

/// The three places files can go, while they are dragged over the island.
fn drop_panel<'a>(view: &'a DropView, canvas: &Canvas) -> Painting<'a> {
    let mut painting = Painting::default();
    painting.texts.push(canvas.text(&view.header, canvas.rect(HEADER), 13.0, true, 0.9, Align::Center));
    for zone in Zone::ALL {
        let area = zone.bounds();
        let bounds = canvas.rect(area);
        let color = zone_color(zone);
        let refusal = view.refusal(zone);
        let lit = view.hovered == Some(zone) && refusal.is_none();
        let radius = 16.0 * canvas.scale;
        if lit {
            let glow = canvas.bounds(area.x - 3.0, area.y - 3.0, area.width + 6.0, area.height + 6.0);
            painting.shapes.push(ShapeRun { bounds: glow, radius: radius + 3.0 * canvas.scale, fill: with_alpha(color, 0.16), border: None });
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
        painting.shapes.push(ShapeRun { bounds, radius, fill: with_alpha(color, fill), border: None });
        let width = 1.5 * canvas.scale;
        // The shelf only holds on to where files are, which a dashed edge suggests.
        let dash = (zone == Zone::Shelf).then_some(4.0);
        painting.outlines.push(OutlineRun { bounds, radius, width, color: with_alpha(color, edge), fraction: 1.0, dash, open_top: false });

        let opacity = if refusal.is_some() { 0.4 } else { 1.0 };
        let icon = canvas.bounds(area.x, area.y + 16.0, area.width, 34.0);
        painting.texts.push(TextRun { opacity, ..canvas.icon(glyph(zone_icon(zone)), icon, 22.0) });
        let label = canvas.bounds(area.x + 6.0, area.y + 56.0, area.width - 12.0, 20.0);
        painting.texts.push(canvas.text(zone.label(), label, 13.0, true, opacity, Align::Center));
        let hint = refusal.unwrap_or(match zone {
            Zone::Shelf => "Sonra sürükle",
            Zone::Phone => "QR kod ile",
            Zone::Convert => "JPG, PNG, küçült",
        });
        let note = canvas.bounds(area.x + 6.0, area.y + 78.0, area.width - 12.0, 18.0);
        painting.texts.push(canvas.text(hint, note, 11.0, false, 0.6 * opacity, Align::Center));
    }
    painting
}

/// The files kept on the shelf, a few at a time.
fn shelf_panel<'a>(view: &'a ShelfView, canvas: &Canvas) -> Painting<'a> {
    let mut painting = Painting::default();
    let title = canvas.bounds(24.0, SHELF_CLEAR.y, 220.0, SHELF_CLEAR.height);
    painting.texts.push(canvas.text(&view.title, title, 14.0, true, 1.0, Align::Leading));
    let clear = canvas.rect(SHELF_CLEAR);
    painting.bars.push(BarRun { bounds: clear, opacity: 0.14 });
    painting.texts.push(canvas.text("Temizle", clear, 12.0, true, 1.0, Align::Center));

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
                painting.shapes.push(ShapeRun { bounds, radius: 10.0 * canvas.scale, fill: [1.0, 1.0, 1.0, 0.1], border: None });
                let folder = !item.name.contains('.');
                painting.texts.push(canvas.icon(if folder { ICON_FOLDER } else { ICON_DOCUMENT }, bounds, 24.0));
            }
        }
        painting.texts.push(canvas.text(&item.name, canvas.rect(tile_name(slot)), 11.0, false, 0.8, Align::Center));
        let cross = canvas.rect(remove_button(slot));
        painting.shapes.push(ShapeRun { bounds: cross, radius: cross.width / 2.0, fill: [0.24, 0.24, 0.26, 1.0], border: Some(([0.0, 0.0, 0.0, 1.0], canvas.scale)) });
        painting.texts.push(canvas.icon(ICON_CLOSE, cross, 8.0));
    }

    if view.total > SHELF_TILES {
        let back = view.first > 0;
        let forward = view.first + SHELF_TILES < view.total;
        painting.texts.push(TextRun { opacity: if back { 1.0 } else { 0.3 }, ..canvas.icon(ICON_BACK, canvas.rect(SHELF_BACK), 12.0) });
        painting.texts.push(TextRun { opacity: if forward { 1.0 } else { 0.3 }, ..canvas.icon(ICON_FORWARD, canvas.rect(SHELF_FORWARD), 12.0) });
        let place = canvas.bounds(SHELF_BACK.x + SHELF_BACK.width, SHELF_BACK.y, SHELF_FORWARD.x - SHELF_BACK.x - SHELF_BACK.width, SHELF_BACK.height);
        painting.texts.push(canvas.text(&view.page, place, 11.5, false, 0.6, Align::Center));
    }
    painting
}

/// The formats pictures can be saved in.
fn convert_panel<'a>(view: &'a ConvertView, canvas: &Canvas) -> Painting<'a> {
    let mut painting = Painting::default();
    painting.texts.push(canvas.text(&view.header, canvas.rect(HEADER), 13.0, true, 0.9, Align::Center));
    let opacity = if view.busy { 0.4 } else { 1.0 };
    for target in Target::ALL {
        let area = target.bounds();
        let bounds = canvas.rect(area);
        painting.shapes.push(ShapeRun { bounds, radius: 14.0 * canvas.scale, fill: with_alpha(CONVERT_COLOR, 0.16 * opacity), border: None });
        let label = canvas.bounds(area.x, area.y + 16.0, area.width, 26.0);
        painting.texts.push(canvas.text(target.label(), label, 17.0, true, opacity, Align::Center));
        let hint = canvas.bounds(area.x + 4.0, area.y + 46.0, area.width - 8.0, 18.0);
        painting.texts.push(canvas.text(target.hint(), hint, 11.0, false, 0.6 * opacity, Align::Center));
    }
    let note = if view.busy { "Dönüştürülüyor…" } else { "Yeni dosya aslının yanına kaydedilir" };
    painting.texts.push(canvas.text(note, canvas.rect(CONVERT_NOTE), 11.5, false, 0.6, Align::Center));
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
        Bounds { x: x * scale, y: y * scale, width: width * scale, height: height * scale }
    }

    fn text<'a>(&self, text: &'a str, bounds: Bounds, size: f32, bold: bool, opacity: f32, align: Align) -> TextRun<'a> {
        TextRun { text, bounds, size: size * self.scale, bold, opacity, align, font: Font::Text, even_digits: false }
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
            None => {
                let glyph = if media.is_music { ICON_NOTE } else { ICON_VIDEO };
                painting.texts.push(self.icon(glyph, self.bounds(x, y, size, size), size * 0.62));
            }
        }
    }
}
