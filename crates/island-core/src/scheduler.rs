use crate::activity::{Activity, Bubble, Icon, Kind, Millis, TimerView};
use crate::drop::DropView;
use crate::layout::{ProgressStyle, StartMode, TIMER_PRESETS, TaskLayout};
use crate::media::{MediaSnapshot, PAUSED_LINGER_MS, TRACK_NOTICE_MS};
use crate::schedule::{EVENT_LEAD_MS, Upcoming};
use crate::shelf::{Shelf, ShelfView};
use crate::timer::Timer;
use crate::qr::QrMatrix;
use crate::quick_controls::QuickControls;
use crate::transfer::{DONE_MS, Direct, Stage, Transfer, WAIT_MS};

/// How long the clock stays up after a peek with nothing else to show.
pub const PEEK_MS: Millis = 3_000;
/// Grace period after the pointer leaves, so brushing past does not collapse.
pub const COLLAPSE_DELAY_MS: Millis = 500;
/// How long a keyboard-opened island stays expanded without the pointer on it.
pub const KEYBOARD_EXPAND_MS: Millis = 4_000;
/// A finished countdown waits for a click for this long, as whoever started it
/// may have left the desk meanwhile.
const TIMER_DONE_MS: Millis = 3 * 60_000;
/// The Clock app rings until it is answered, so a short word is enough here.
const CLOCK_TIMER_DONE_MS: Millis = 8_000;
/// Windows and the island count apart. A countdown reported gone this close to
/// its end ran to its end, even if the report arrives first.
const END_GRACE_MS: Millis = 3_000;
/// How many coming events the home view lists.
const HOME_EVENTS: usize = 2;
/// How long a view opened by a drop stays without the pointer on it. The
/// pointer is on the island when the files land, but Windows only says so once
/// it moves again.
const DROP_VIEW_MS: Millis = 6_000;

fn is_layout_task(kind: Kind) -> bool {
    matches!(kind, Kind::Transfer | Kind::Focus | Kind::TimerRunning | Kind::ClockTimer)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presentation {
    Hidden,
    Compact,
    Level,
    Alert,
    Expanded,
}

/// Reasons the island must stay out of the way. Any active one hides it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Suppression {
    Fullscreen = 1 << 0,
    WindowDrag = 1 << 1,
    DisplayOff = 1 << 2,
    SessionLocked = 1 << 3,
}

/// How the calendar is doing, which explains an empty home view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CalendarState {
    #[default]
    Ready,
    /// No address in the settings yet.
    NotSet,
    /// The last download failed, so what is shown may be out of date.
    Unreachable,
}

/// The expanded view with nothing else open: what is next, and timer presets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeView {
    pub coming: Vec<Upcoming>,
    pub calendar: CalendarState,
    pub mode: StartMode,
    /// Whether the user can choose, which needs the Windows Clock app.
    pub modes_offered: bool,
    pub presets: [u64; TIMER_PRESETS.len()],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickCalendarView {
    pub state: CalendarState,
    pub coming: Vec<Upcoming>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickControlsView {
    pub page: crate::quick_controls::Page,
    pub focused: Option<crate::quick_controls::Target>,
    pub calendar: QuickCalendarView,
    pub presets: [u64; TIMER_PRESETS.len()],
    pub mode: StartMode,
    pub modes_offered: bool,
    pub volume: Option<(u8, bool)>,
    pub brightness: Option<u8>,
    pub wifi: crate::quick_controls::RadioState,
    pub bluetooth: crate::quick_controls::RadioState,
}

/// A Windows focus session on the island's clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusView {
    Off,
    /// On, but nothing says for how long.
    Open,
    Running { ends_at: Millis, total: Millis, on_break: bool },
    Paused { remaining: Millis, total: Millis, on_break: bool },
}

/// The choice of format for pictures dropped to be converted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConvertView {
    pub header: String,
    /// A conversion is running, so the choices wait.
    pub busy: bool,
}

/// A view that a drop opened, which stays until it is closed.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Panel {
    Shelf,
    Convert(ConvertView),
}

/// What the expanded pill shows instead of an activity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelView {
    /// Files are being dragged over the island.
    Drop(DropView),
    Shelf(ShelfView),
    Convert(ConvertView),
    QuickControls(QuickControlsView),
}

/// What should be on screen right now.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub presentation: Presentation,
    /// `None` while hidden, in the home view, or for the clock shown by a peek.
    pub activity: Option<Activity>,
    /// The next most important activity, shown as a bubble beside the pill.
    pub secondary: Option<Activity>,
    /// What shares the pill with `activity` when a task sits side by side
    /// with what lasts: the task beside music, or music under the open code.
    pub companion: Option<Activity>,
    pub home: Option<HomeView>,
    pub panel: Option<PanelView>,
    /// Something finished and deserves a sound. True for one frame only.
    pub chime: bool,
    /// When `Island::frame` must be called again even if nothing else happens.
    pub wake_at: Option<Millis>,
}

/// The island's state machine. Every method takes the current time as input
/// and nothing here keeps a timer: the caller arms one for `Frame::wake_at`.
#[derive(Debug, Default)]
pub struct Island {
    activities: Vec<Activity>,
    suppressed: u8,
    expanded: bool,
    hovered: bool,
    collapse_at: Option<Millis>,
    peek_until: Option<Millis>,
    timer: Option<Timer>,
    focus: Option<Timer>,
    focus_open: bool,
    focus_break: bool,
    /// A timer running in the Windows Clock app, with its name there.
    clock_timer: Option<(Timer, String)>,
    /// When a paused one stops showing by itself.
    clock_timer_dormant_at: Option<Millis>,
    /// The app using the camera, for as long as it does.
    camera: Option<String>,
    schedule: Vec<Upcoming>,
    /// Events the user hid, by start and title, which stay quiet from then on.
    hidden_events: Vec<(i64, String)>,
    calendar: CalendarState,
    /// What the user opened from a bubble, shown ahead of the usual order.
    focused: Option<Kind>,
    home: bool,
    quick_controls: Option<QuickControls>,
    quick_volume: Option<(u8, bool)>,
    quick_brightness: Option<u8>,
    quick_wifi: crate::quick_controls::RadioState,
    quick_bluetooth: crate::quick_controls::RadioState,
    chime: bool,
    start_mode: StartMode,
    modes_offered: bool,
    presets: [u64; TIMER_PRESETS.len()],
    /// Whether a new track is announced, which the user can turn off.
    announces_tracks: bool,
    /// Files being dragged over the island right now.
    dragging: Option<DropView>,
    panel: Option<Panel>,
    shelf: Shelf,
    transfer: Option<Transfer>,
    progress_style: ProgressStyle,
    task_layout: TaskLayout,
    /// What the last frame put in the bubble and beside the activity in the
    /// pill, which a click there opens.
    bubble_kind: Option<Kind>,
    companion_kind: Option<Kind>,
}

/// Volume or brightness as a ring in the bubble, while a view stays open.
fn level_bubble(level: &Activity) -> Activity {
    let percent = level.level.unwrap_or(0);
    Activity {
        badge: Some(Bubble::Count { value: percent.to_string(), icon: level.icon, ring: Some(percent) }),
        ..level.clone()
    }
}

/// Whether a countdown is at its very end, on either side of it.
fn ending_now(timer: &Timer, now: Millis) -> bool {
    timer.ends_at().is_some_and(|ends_at| ends_at.abs_diff(now) <= END_GRACE_MS)
}

impl Island {
    pub fn new() -> Self {
        Self { modes_offered: true, presets: TIMER_PRESETS, announces_tracks: true, ..Self::default() }
    }

    pub fn set_track_notices(&mut self, on: bool) {
        self.announces_tracks = on;
    }

    /// Whether a view is open, which a small confirmation would only disturb.
    pub fn is_open(&self) -> bool {
        self.expanded || self.home || self.panel.is_some() || self.dragging.is_some() || self.quick_controls.is_some()
    }

    /// Opens Quick Controls in the same island state machine as the other views.
    pub fn open_quick_controls(&mut self, now: Millis, inactivity: Option<Millis>) {
        self.close_expanded();
        self.quick_controls = Some(QuickControls::new(now, inactivity));
    }

    pub fn quick_controls(&self) -> Option<&QuickControls> { self.quick_controls.as_ref() }
    pub fn quick_controls_mut(&mut self) -> Option<&mut QuickControls> { self.quick_controls.as_mut() }
    pub fn set_quick_volume(&mut self, volume: Option<(u8, bool)>) { self.quick_volume = volume; }
    pub fn set_quick_brightness(&mut self, brightness: Option<u8>) { self.quick_brightness = brightness; }
    pub fn quick_brightness(&self) -> Option<u8> { self.quick_brightness }
    pub fn quick_volume(&self) -> Option<(u8, bool)> { self.quick_volume }
    pub fn set_quick_radios(&mut self, wifi: crate::quick_controls::RadioState, bluetooth: crate::quick_controls::RadioState) {
        self.quick_wifi = wifi;
        self.quick_bluetooth = bluetooth;
    }

    /// The existing calendar schedule, shared with the home view.
    pub fn quick_calendar(&self) -> QuickCalendarView {
        QuickCalendarView { state: self.calendar, coming: self.coming_events() }
    }

    fn coming_events(&self) -> Vec<Upcoming> {
        self.schedule.iter().filter(|event| !self.is_hidden(event)).take(HOME_EVENTS).cloned().collect()
    }

    pub fn set_task_layout(&mut self, layout: TaskLayout) {
        self.task_layout = layout;
    }

    pub fn set_progress_style(&mut self, style: ProgressStyle) {
        self.progress_style = style;
    }

    /// Files are being dragged over the island, which opens the drop panel.
    pub fn drag_over(&mut self, view: DropView) {
        self.dragging = Some(view);
        self.collapse_at = None;
    }

    /// The files went elsewhere, or the drag was called off.
    pub fn drag_ended(&mut self) {
        self.dragging = None;
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging.is_some()
    }

    /// Puts files on the shelf and shows it. Returns the ones that are new.
    pub fn drop_on_shelf(&mut self, paths: &[String], now: Millis) -> Vec<String> {
        let added = self.shelf.add(paths);
        self.open_panel(Panel::Shelf, now);
        added
    }

    pub fn open_shelf(&mut self, now: Millis) {
        if !self.shelf.is_empty() {
            self.open_panel(Panel::Shelf, now);
            self.collapse_at = None;
        }
    }

    pub fn shelf(&self) -> &Shelf {
        &self.shelf
    }

    pub fn set_thumbnail(&mut self, path: &str, thumbnail: std::sync::Arc<[u8]>) {
        self.shelf.set_thumbnail(path, thumbnail);
    }

    pub fn remove_from_shelf(&mut self, index: usize) {
        self.shelf.remove(index);
        self.close_empty_shelf();
    }

    pub fn clear_shelf(&mut self) {
        self.shelf.clear();
        self.close_empty_shelf();
    }

    pub fn scroll_shelf(&mut self, pages: i32) {
        self.shelf.scroll(pages);
    }

    /// Forgets files on the shelf that were moved or deleted.
    pub fn forget_missing(&mut self, exists: impl Fn(&str) -> bool) {
        self.shelf.retain(exists);
        self.close_empty_shelf();
    }

    fn close_empty_shelf(&mut self) {
        if self.shelf.is_empty() && self.panel == Some(Panel::Shelf) {
            self.close_expanded();
        }
    }

    /// Offers formats for pictures that were dropped to be converted.
    pub fn open_convert(&mut self, header: String, now: Millis) {
        self.open_panel(Panel::Convert(ConvertView { header, busy: false }), now);
    }

    pub fn set_converting(&mut self, busy: bool) {
        if let Some(Panel::Convert(view)) = &mut self.panel {
            view.busy = busy;
        }
    }

    fn open_panel(&mut self, panel: Panel, now: Millis) {
        self.quick_controls = None;
        self.dragging = None;
        self.home = false;
        self.expanded = false;
        self.focused = None;
        self.panel = Some(panel);
        self.collapse_at = (!self.hovered).then_some(now + DROP_VIEW_MS);
    }

    /// Shows the code for the phone, and keeps it open until it is answered.
    pub fn start_transfer(&mut self, transfer: Transfer) {
        self.quick_controls = None;
        self.dragging = None;
        self.panel = None;
        self.home = false;
        self.transfer = Some(transfer);
        self.expanded = true;
        self.focused = Some(Kind::Transfer);
        self.collapse_at = None;
    }

    /// How much has gone so far, in bytes.
    pub fn transfer_progress(&mut self, sent: u64) {
        if let Some(transfer) = &mut self.transfer
            && !matches!(transfer.stage, Stage::Done { .. })
        {
            transfer.stage = Stage::Sending { sent };
        }
    }

    /// The island's own network is on its way.
    pub fn transfer_direct_preparing(&mut self) {
        if let Some(transfer) = &mut self.transfer {
            transfer.direct = Some(Direct::Preparing);
            transfer.warning = None;
        }
    }

    /// The network is up: a code to join it, and the download code that
    /// works on it once the phone is there.
    pub fn transfer_direct_ready(&mut self, join: QrMatrix, network: String, download: QrMatrix) {
        if let Some(transfer) = &mut self.transfer {
            transfer.direct = Some(Direct::Ready { code: std::sync::Arc::new(join), network: network.clone() });
            transfer.code = std::sync::Arc::new(download);
            transfer.network = network;
        }
    }

    pub fn transfer_direct_joined(&mut self) {
        if let Some(transfer) = &mut self.transfer
            && let Some(Direct::Ready { network, .. }) = &transfer.direct
        {
            transfer.direct = Some(Direct::Joined { network: network.clone() });
        }
    }

    /// The network could not be made, so the usual code stands again.
    pub fn transfer_direct_failed(&mut self) {
        if let Some(transfer) = &mut self.transfer {
            transfer.direct = None;
        }
    }

    /// Wi-Fi went off with nothing else to connect the computer, or came back.
    pub fn transfer_offline(&mut self, offline: bool) {
        if let Some(transfer) = &mut self.transfer {
            transfer.offline = offline;
        }
    }

    /// A phone opened the code's page, so it found its way.
    pub fn transfer_visited(&mut self) {
        if let Some(transfer) = &mut self.transfer {
            transfer.nudge_at = None;
        }
    }

    /// What may stand in the phone's way, if anything.
    pub fn transfer_warning(&mut self, warning: Option<String>) {
        if let Some(transfer) = &mut self.transfer {
            transfer.warning = warning;
        }
    }

    /// Windows' Mobile Hotspot is on: the files are served there instead, for
    /// a phone that is already on it.
    pub fn transfer_hotspot(&mut self, download: QrMatrix, network: String) {
        if let Some(transfer) = &mut self.transfer {
            transfer.direct = Some(Direct::Hotspot { network: network.clone() });
            transfer.code = std::sync::Arc::new(download);
            transfer.network = network;
            transfer.warning = None;
        }
    }

    pub fn transfer_is_direct(&self) -> bool {
        self.transfer.as_ref().is_some_and(|transfer| transfer.direct.is_some())
    }

    /// The phone stopped before it had everything. The code waits again.
    pub fn transfer_broke_off(&mut self, now: Millis) {
        if let Some(transfer) = &mut self.transfer
            && matches!(transfer.stage, Stage::Sending { .. })
        {
            transfer.stage = Stage::Waiting { until: now + WAIT_MS };
        }
    }

    pub fn transfer_done(&mut self, now: Millis) {
        if let Some(transfer) = &mut self.transfer {
            transfer.stage = Stage::Done { at: now };
        }
    }

    pub fn cancel_transfer(&mut self) {
        if self.transfer.take().is_some() {
            self.remove(Kind::Transfer);
            if self.focused == Some(Kind::Transfer) {
                self.close_expanded();
            }
        }
    }

    pub fn transfer_stage(&self) -> Option<Stage> {
        self.transfer.as_ref().map(|transfer| transfer.stage)
    }

    /// The code stays open while it waits to be scanned or the files go, even
    /// without the pointer on it: the user is busy with the phone.
    fn held_open(&self) -> bool {
        self.expanded
            && self.focused == Some(Kind::Transfer)
            && self.transfer.as_ref().is_some_and(|transfer| !matches!(transfer.stage, Stage::Done { .. }))
    }

    pub fn set_presets(&mut self, presets: [u64; TIMER_PRESETS.len()]) {
        self.presets = presets;
    }

    pub fn presets(&self) -> [u64; TIMER_PRESETS.len()] {
        self.presets
    }

    /// Returns whether that changed anything.
    pub fn set_start_mode(&mut self, mode: StartMode) -> bool {
        let changes = self.modes_offered && self.start_mode != mode;
        if changes {
            self.start_mode = mode;
        }
        changes
    }

    pub fn start_mode(&self) -> StartMode {
        self.start_mode
    }

    /// Without the Clock app there is only the island's own timer to offer.
    pub fn offer_only_timers(&mut self) {
        self.start_mode = StartMode::Timer;
        self.modes_offered = false;
    }

    pub fn set_calendar_state(&mut self, state: CalendarState) {
        self.calendar = state;
    }

    pub fn upsert(&mut self, activity: Activity) {
        self.remove(activity.kind);
        self.activities.push(activity);
    }

    pub fn remove(&mut self, kind: Kind) {
        self.activities.retain(|a| a.kind != kind);
    }

    /// Replaces the media activity. A session that stays paused keeps its
    /// original deadline, or small updates would keep it on screen forever.
    pub fn upsert_media(&mut self, mut snapshot: MediaSnapshot, now: Millis) {
        self.announce_track(&snapshot, now);
        let previous = self.activities.iter().find(|a| a.kind == Kind::Media);
        if let Some(earlier) = previous.and_then(|a| a.media.as_ref()) {
            snapshot.continue_from(earlier, now);
        }
        let paused_deadline = previous.and_then(|a| a.dormant_at);
        let mut activity = snapshot.into_activity(now);
        activity.dormant_at = activity.dormant_at.and(paused_deadline.or(activity.dormant_at));
        self.upsert(activity);
    }

    /// Announces a new track for a moment. Players often report the text first
    /// and the image a little later, which then joins the notice already up.
    fn announce_track(&mut self, snapshot: &MediaSnapshot, now: Millis) {
        let before = self.activities.iter().find(|a| a.kind == Kind::Media).and_then(|a| a.media.as_ref());
        let new_track = before.is_some_and(|before| !before.is_same_track(snapshot));
        let up = self.activities.iter().find(|a| a.kind == Kind::NowPlaying && a.title == snapshot.title);
        let expires_at = up.and_then(|notice| notice.expires_at).unwrap_or(now + TRACK_NOTICE_MS);
        // Only songs: the next video in a browser is no news.
        let news = (new_track || up.is_some()) && snapshot.playing && snapshot.is_music;
        if news && self.announces_tracks && !self.is_open() {
            self.upsert(snapshot.track_notice(expires_at));
        }
    }

    /// Moves the shown position at once. Players take a moment to report a
    /// jump, and the bar would otherwise snap back until they do.
    pub fn seek_media(&mut self, position_ms: u64, now: Millis) {
        let activity = self.activities.iter_mut().find(|a| a.kind == Kind::Media);
        if let Some(media) = activity.and_then(|a| a.media.as_mut()) {
            media.position_ms = position_ms.min(media.duration_ms);
            media.sampled_at = now;
        }
    }

    pub fn start_timer(&mut self, length: Millis, now: Millis) {
        self.timer = Some(Timer::start(length, now));
        self.close_expanded();
    }

    pub fn toggle_timer(&mut self, now: Millis) {
        if let Some(timer) = &mut self.timer {
            timer.toggle(now);
        }
    }

    pub fn cancel_timer(&mut self) {
        self.timer = None;
        self.remove(Kind::TimerRunning);
        self.close_expanded();
    }

    pub fn set_focus(&mut self, view: FocusView, now: Millis) {
        if view == FocusView::Off && self.focus.is_some_and(|focus| ending_now(&focus, now)) {
            return;
        }
        self.focus_open = view == FocusView::Open;
        (self.focus, self.focus_break) = match view {
            FocusView::Off | FocusView::Open => (None, false),
            FocusView::Running { ends_at, total, on_break } => (Some(Timer::ending_at(ends_at, total)), on_break),
            FocusView::Paused { remaining, total, on_break } => (Some(Timer::paused(remaining, total)), on_break),
        };
    }

    pub fn set_camera(&mut self, app: Option<String>) {
        self.camera = app;
    }

    /// What the Clock app is counting down, with the name it has there.
    pub fn set_clock_timer(&mut self, timer: Option<(Timer, String)>, now: Millis) {
        let running = self.clock_timer.as_ref().is_some_and(|(shown, _)| shown.ends_at().is_some());
        let at_its_end = self.clock_timer.as_ref().is_some_and(|(shown, _)| ending_now(shown, now));
        if timer.is_none() && at_its_end {
            return;
        }
        // A paused timer can sit in the Clock for days, which is no reason to
        // keep the pill open. It shows briefly if it was paused just now.
        let paused = timer.as_ref().is_some_and(|(timer, _)| timer.ends_at().is_none());
        self.clock_timer_dormant_at = match (paused, running) {
            (false, _) => None,
            (true, true) => Some(now + PAUSED_LINGER_MS),
            (true, false) => self.clock_timer_dormant_at.or(Some(now)),
        };
        self.clock_timer = timer;
    }

    /// Replaces the coming events. Anything already started is dropped quietly.
    pub fn set_schedule(&mut self, mut events: Vec<Upcoming>, now: Millis) {
        events.retain(|event| event.start > now);
        events.sort_by_key(|event| event.start);
        // An event that moved counts as new, and the user should hear about it.
        self.hidden_events.retain(|(start, title)| {
            events.iter().any(|event| event.wall_start == *start && event.title == *title)
        });
        self.schedule = events;
    }

    fn is_hidden(&self, event: &Upcoming) -> bool {
        self.hidden_events.iter().any(|(start, title)| *start == event.wall_start && *title == event.title)
    }

    /// The next event that may show by itself.
    fn next_event(&self) -> Option<&Upcoming> {
        self.schedule.iter().find(|event| !self.is_hidden(event))
    }

    /// Hides the event on display, which stops it from taking the pill, and
    /// from announcing itself when it starts.
    pub fn hide_event(&mut self) {
        if let Some(event) = self.next_event() {
            let hidden = (event.wall_start, event.title.clone());
            self.hidden_events.push(hidden);
        }
        self.remove(Kind::Upcoming);
        self.close_expanded();
    }

    /// Opens the home view from another expanded view.
    pub fn open_home(&mut self) {
        self.quick_controls = None;
        self.home = true;
        self.collapse_at = None;
    }

    /// A click on the bubble opens what it stands for.
    pub fn click_secondary(&mut self, now: Millis) {
        match self.bubble_kind {
            Some(Kind::Shelf) | None => self.open_shelf(now),
            // Volume set aside while a view stays open needs no view of its own.
            Some(kind) if kind.is_transient() => {}
            Some(kind) => self.open_kind(kind),
        }
    }

    /// A click on what shares the pill opens it.
    pub fn click_companion(&mut self) {
        if let Some(kind) = self.companion_kind {
            self.open_kind(kind);
        }
    }

    fn open_kind(&mut self, kind: Kind) {
        self.quick_controls = None;
        self.home = false;
        self.panel = None;
        self.focused = Some(kind);
        self.expanded = true;
        self.collapse_at = None;
    }

    /// Closes whatever is open, once a choice made in it has been carried out.
    pub fn close_views(&mut self) {
        self.close_expanded();
    }

    fn close_expanded(&mut self) {
        self.quick_controls = None;
        self.expanded = false;
        self.home = false;
        self.panel = None;
        self.focused = None;
        self.collapse_at = None;
    }

    pub fn set_suppressed(&mut self, reason: Suppression, active: bool) {
        if active {
            self.suppressed |= reason as u8;
        } else {
            self.suppressed &= !(reason as u8);
        }
    }

    pub fn is_suppressed(&self) -> bool {
        self.suppressed != 0
    }

    /// A click on the pill: dismiss an interruption, otherwise toggle.
    pub fn click(&mut self, now: Millis) {
        self.update_derived(now);
        self.expire(now);
        match self.top(now).map(|a| a.kind) {
            Some(kind) if kind.is_transient() => self.remove(kind),
            _ if self.home || self.panel.is_some() || self.quick_controls.is_some() => self.close_expanded(),
            Some(_) if self.expanded => self.close_expanded(),
            Some(_) => {
                self.expanded = true;
                self.collapse_at = None;
            }
            None => {
                self.home = true;
                self.collapse_at = None;
            }
        }
    }

    /// Pointer entered or left the pill.
    pub fn set_hovered(&mut self, hovered: bool, now: Millis) {
        self.hovered = hovered;
        if let Some(controls) = &mut self.quick_controls {
            controls.touch(now);
        }
        if hovered {
            self.collapse_at = None;
            self.peek_until = None;
        } else {
            self.update_derived(now);
            if (self.expanded || self.home || self.panel.is_some()) && !self.held_open() {
                self.collapse_at = Some(now + COLLAPSE_DELAY_MS);
            }
            if self.awake(now).is_none() {
                self.peek_until = Some(now + PEEK_MS);
            }
        }
    }

    /// Keyboard shortcut: show the clock, or open what is already showing.
    pub fn summon(&mut self, now: Millis) {
        self.update_derived(now);
        self.expire(now);
        match self.top(now).map(|a| a.kind) {
            // This one waits to be answered, which the keyboard can do too.
            Some(Kind::TimerFinished) => self.remove(Kind::TimerFinished),
            Some(kind) if kind.is_transient() => {}
            _ if self.home || self.panel.is_some() || self.quick_controls.is_some() => self.close_expanded(),
            Some(_) => {
                self.expanded = !self.expanded;
                self.collapse_at = self.expanded.then_some(now + KEYBOARD_EXPAND_MS);
                self.peek_until = self.peek_until.map(|_| now + KEYBOARD_EXPAND_MS);
                if !self.expanded {
                    self.focused = None;
                }
            }
            // The clock is up, whether from a peek or from the pointer resting
            // where it appeared, which ends the peek but keeps it in view.
            None if self.peek_until.is_some() || self.hovered => {
                self.home = true;
                self.collapse_at = Some(now + KEYBOARD_EXPAND_MS);
                self.peek_until = Some(now + KEYBOARD_EXPAND_MS);
            }
            None => self.peek(now),
        }
    }

    /// Pointer reached the top edge: reveal the clock, or a dormant activity,
    /// if nothing is showing by itself.
    pub fn peek(&mut self, now: Millis) {
        self.update_derived(now);
        if self.awake(now).is_none() {
            self.peek_until = Some(now + PEEK_MS);
        }
    }

    /// Applies everything due by `now` and returns what to show.
    pub fn frame(&mut self, now: Millis) -> Frame {
        self.update_derived(now);
        self.expire(now);
        if self.quick_controls.as_ref().and_then(QuickControls::deadline).is_some_and(|deadline| deadline <= now) {
            self.close_expanded();
        }
        if self.collapse_at.is_some_and(|at| at <= now) {
            self.close_expanded();
        }
        if self.peek_until.is_some_and(|at| at <= now) {
            self.peek_until = None;
        }

        let top = self.top(now).cloned();
        // An interruption only covers what is open. Closing it would throw away
        // the view the user is in, just because the volume changed.
        if top.is_none() {
            self.expanded = false;
            self.focused = None;
        }
        let lasting: Vec<Activity> = self.long_lived(now).into_iter().cloned().collect();

        // Volume and brightness never cover a code waiting to be scanned or a
        // view a drop opened: they go in the bubble while it stays open.
        let task_open = self.dragging.is_some()
            || self.panel.is_some()
            || self.quick_controls.is_some()
            || (self.expanded && lasting.first().is_some_and(|a| a.kind == Kind::Transfer));
        let (top, level) = match top {
            Some(level) if level.level.is_some() && task_open => (lasting.first().cloned(), Some(level)),
            top => (top, None),
        };
        let interrupted = top.as_ref().is_some_and(|a| a.kind.is_transient());

        let presentation = match &top {
            _ if self.is_suppressed() => Presentation::Hidden,
            // Nothing may cover the zones while something is carried to them.
            _ if self.dragging.is_some() => Presentation::Expanded,
            Some(activity) if activity.level.is_some() => Presentation::Level,
            Some(activity) if activity.brief => Presentation::Compact,
            Some(activity) if activity.kind.is_transient() => Presentation::Alert,
            _ if self.panel.is_some() || self.quick_controls.is_some() => Presentation::Expanded,
            _ if self.home => Presentation::Expanded,
            Some(_) if self.expanded => Presentation::Expanded,
            Some(_) => Presentation::Compact,
            None if self.peek_until.is_some() || self.hovered => Presentation::Compact,
            None => Presentation::Hidden,
        };

        let expanded = presentation == Presentation::Expanded;
        let panel = match (&self.dragging, &self.panel) {
            (Some(view), _) if expanded => Some(PanelView::Drop(view.clone())),
            (None, _) if expanded && !interrupted && self.quick_controls.is_some() => self.quick_controls.as_ref().map(|controls| PanelView::QuickControls(QuickControlsView {
                page: controls.page,
                focused: controls.focused,
                calendar: self.quick_calendar(),
                presets: self.presets,
                mode: self.start_mode,
                modes_offered: self.modes_offered,
                volume: self.quick_volume,
                brightness: self.quick_brightness,
                wifi: self.quick_wifi,
                bluetooth: self.quick_bluetooth,
            })),
            (None, Some(Panel::Shelf)) if expanded && !interrupted => Some(PanelView::Shelf(self.shelf.view())),
            (None, Some(Panel::Convert(view))) if expanded && !interrupted => Some(PanelView::Convert(view.clone())),
            _ => None,
        };
        let in_home = self.home && panel.is_none() && !interrupted && expanded;
        let transfer_shown = top.as_ref().is_some_and(|a| a.kind == Kind::Transfer) && panel.is_none() && !in_home;
        let transfer_open = expanded && transfer_shown;

        // Side by side, the task rides in the music pill, and music rides
        // under the open code. Anything else that lasts keeps to the bubble.
        let shared = self.task_layout == TaskLayout::Shared;
        let music_in_pill = top.as_ref().is_some_and(|a| a.media.is_some() && !a.kind.is_transient());
        let companion = match presentation {
            Presentation::Compact if shared && music_in_pill => lasting.iter().find(|a| is_layout_task(a.kind)).cloned(),
            Presentation::Expanded if shared && transfer_open => {
                lasting.iter().find(|a| a.kind != Kind::Transfer && a.media.is_some()).cloned()
            }
            _ => None,
        };

        // An interruption covers the pill only. Keeping the bubble means it does
        // not vanish and return on every volume step. The open code keeps what
        // lasts in view beside it, unless music already shows under it.
        let shows_bubble = level.is_some()
            || (transfer_open && companion.is_none())
            || matches!(presentation, Presentation::Compact | Presentation::Level | Presentation::Alert);
        let secondary = match &level {
            _ if !shows_bubble => None,
            Some(level) => Some(level_bubble(level)),
            None => lasting
                .iter()
                .skip(1)
                .find(|a| companion.as_ref().is_none_or(|c| c.kind != a.kind))
                .cloned()
                .or_else(|| self.shelf_bubble()),
        };
        self.bubble_kind = secondary.as_ref().map(|a| a.kind);
        self.companion_kind = companion.as_ref().map(|a| a.kind);

        let timer_in_pill = top
            .as_ref()
            .is_some_and(|a| matches!(a.kind, Kind::TimerRunning | Kind::ClockTimer | Kind::Focus));
        // The time left only shows while the task has the pill or is open.
        let transfer_counts = transfer_shown && matches!(presentation, Presentation::Compact | Presentation::Expanded);
        let wake_at = self.next_deadline(now, timer_in_pill, transfer_counts);
        Frame {
            presentation,
            activity: if presentation == Presentation::Hidden || in_home || panel.is_some() { None } else { top },
            secondary,
            companion,
            home: in_home.then(|| HomeView {
                coming: self.coming_events(),
                calendar: self.calendar,
                mode: self.start_mode,
                modes_offered: self.modes_offered,
                presets: self.presets,
            }),
            panel,
            chime: std::mem::take(&mut self.chime),
            wake_at,
        }
    }

    /// Files on the shelf show as a bubble when nothing else wants it.
    fn shelf_bubble(&self) -> Option<Activity> {
        (!self.shelf.is_empty()).then(|| Activity {
            badge: Some(Bubble::Count { value: self.shelf.len().to_string(), icon: Some(Icon::Shelf), ring: None }),
            ..Activity::new(Kind::Shelf)
        })
    }

    /// Keeps the activities that depend on time up to date, and raises what
    /// falls due: a finished timer, or an event that is starting.
    fn update_derived(&mut self, now: Millis) {
        if let Some(transfer) = &self.transfer {
            let (over, finished) = match transfer.stage {
                Stage::Waiting { until } => (until <= now, false),
                Stage::Done { at } => (at + DONE_MS <= now, true),
                Stage::Sending { .. } => (false, false),
            };
            if over {
                self.transfer = None;
                if finished || self.focused == Some(Kind::Transfer) {
                    self.close_expanded();
                }
            }
        }
        match &self.transfer {
            Some(transfer) => self.upsert(transfer.activity(now, self.progress_style)),
            None => self.remove(Kind::Transfer),
        }

        if self.timer.is_some_and(|timer| timer.is_finished(now)) {
            self.timer = None;
            self.chime = true;
            self.upsert(Activity {
                title: "Süre doldu".into(),
                detail: "Zamanlayıcı bitti".into(),
                expires_at: Some(now + TIMER_DONE_MS),
                icon: Some(Icon::Timer),
                ..Activity::new(Kind::TimerFinished)
            });
        }
        match self.timer {
            Some(timer) => self.upsert(timer.activity(now)),
            None => self.remove(Kind::TimerRunning),
        }

        if self.clock_timer.as_ref().is_some_and(|(timer, _)| timer.is_finished(now)) {
            let name = self.clock_timer.take().map(|(_, name)| name).unwrap_or_default();
            // No sound here, as the Clock app rings by itself.
            self.upsert(Activity {
                title: "Süre doldu".into(),
                detail: format!("Saat uygulaması · {name}"),
                expires_at: Some(now + CLOCK_TIMER_DONE_MS),
                icon: Some(Icon::Timer),
                ..Activity::new(Kind::TimerFinished)
            });
        }
        match &self.clock_timer {
            Some((timer, name)) => {
                let activity = timer.clock_activity(now, name);
                self.upsert(Activity { dormant_at: self.clock_timer_dormant_at, ..activity });
            }
            None => self.remove(Kind::ClockTimer),
        }

        match self.camera.clone() {
            Some(app) => self.upsert(Activity {
                leading: "Kamera".into(),
                trailing: app.clone(),
                title: "Kamera açık".into(),
                detail: app,
                icon: Some(Icon::Camera),
                badge: Some(Bubble::Symbol(Icon::Camera)),
                ..Activity::new(Kind::Camera)
            }),
            None => self.remove(Kind::Camera),
        }

        if self.focus.is_some_and(|focus| focus.is_finished(now)) {
            self.focus = None;
            self.chime = true;
            self.upsert(Activity {
                title: "Odak bitti".into(),
                detail: "Odak oturumu tamamlandı".into(),
                expires_at: Some(now + TIMER_DONE_MS),
                icon: Some(Icon::Focus),
                ..Activity::new(Kind::TimerFinished)
            });
        }
        match self.focus {
            Some(focus) => self.upsert(focus.focus_activity(now, self.focus_break)),
            None if self.focus_open => self.upsert(Activity {
                leading: "Odak".into(),
                trailing: "Açık".into(),
                title: "Odak".into(),
                detail: "Açık".into(),
                icon: Some(Icon::Focus),
                timer: Some(TimerView { running: true, has_controls: true, left: None }),
                ..Activity::new(Kind::Focus)
            }),
            None => self.remove(Kind::Focus),
        }

        let due = self.schedule.iter().take_while(|event| event.start <= now).count();
        let started: Vec<Upcoming> = self.schedule.drain(..due).collect();
        let mut starting = started.iter().filter(|event| event.just_started(now) && !self.is_hidden(event));
        if let Some(first) = starting.next() {
            let alert = first.starting_alert(now, starting.count());
            self.chime = true;
            self.upsert(alert);
        }
        match self.next_event().filter(|event| event.is_near(now)).map(|event| event.activity(now)) {
            Some(activity) => self.upsert(activity),
            None => self.remove(Kind::Upcoming),
        }
    }

    /// Activities in the order they claim the island, the dormant ones too if
    /// asked. A short task leaves the pill to what lasts and takes the place
    /// right after it, unless it is set to come to the front or was opened.
    fn ranked(&self, now: Millis, dormant: bool) -> Vec<&Activity> {
        let mut ranked: Vec<&Activity> = self
            .activities
            .iter()
            .filter(|a| dormant || a.dormant_at.is_none_or(|at| at > now))
            .collect();
        ranked.sort_by_key(|a| a.order(now, self.focused));
        if self.task_layout != TaskLayout::Front && self.focused != Some(Kind::Transfer) {
            let lasting: Vec<usize> = (0..ranked.len()).filter(|&i| !ranked[i].kind.is_transient()).collect();
            if let [first, second, ..] = lasting[..]
                && ranked[first].kind == Kind::Transfer
            {
                let task = ranked.remove(first);
                ranked.insert(second, task);
            }
        }
        if self.task_layout == TaskLayout::Front
            && let Some(music) = ranked.iter().position(|a| a.kind == Kind::Media)
            && let Some(task) = ranked.iter().position(|a| is_layout_task(a.kind) && a.kind != Kind::Transfer)
            && task > music
        {
            let activity = ranked.remove(task);
            ranked.insert(music, activity);
        }
        ranked
    }

    /// Everything that lasts, most important first.
    fn long_lived(&self, now: Millis) -> Vec<&Activity> {
        let peeking = self.peek_until.is_some() || self.hovered;
        self.ranked(now, peeking).into_iter().filter(|a| !a.kind.is_transient()).collect()
    }

    /// The highest priority activity that shows without being asked for.
    fn awake(&self, now: Millis) -> Option<&Activity> {
        self.ranked(now, false).first().copied()
    }

    /// What is on display, which includes dormant activities during a peek.
    fn top(&self, now: Millis) -> Option<&Activity> {
        let peeking = self.peek_until.is_some() || self.hovered;
        self.awake(now).or_else(|| if peeking { self.ranked(now, true).first().copied() } else { None })
    }

    fn expire(&mut self, now: Millis) {
        self.activities.retain(|a| a.expires_at.is_none_or(|at| at > now));
    }

    fn next_deadline(&self, now: Millis, timer_in_pill: bool, transfer_counts: bool) -> Option<Millis> {
        let expiries = self.activities.iter().filter_map(|a| a.expires_at);
        let dormancies = self.activities.iter().filter_map(|a| a.dormant_at).filter(|&at| at > now);
        let urgencies = self.activities.iter().filter_map(|a| a.urgent_at).filter(|&at| at > now);

        let clock_timer = self.clock_timer.as_ref().map(|(timer, _)| timer);
        let ticking = self.timer.iter().chain(&self.focus).chain(clock_timer).flat_map(|timer| {
            let shows_seconds = timer_in_pill || self.expanded;
            [timer.ends_at(), timer.next_tick(now, shows_seconds)]
        });

        let next_event = self.next_event();
        let event_moments = next_event.iter().flat_map(|event| {
            let until = event.start.saturating_sub(now);
            let minute = if until.is_multiple_of(60_000) { 60_000 } else { until % 60_000 };
            let lead_starts = event.start.saturating_sub(EVENT_LEAD_MS);
            [
                Some(event.start),
                (lead_starts > now).then_some(lead_starts),
                event.is_near(now).then_some(now + minute),
            ]
        });

        expiries
            .chain(dormancies)
            .chain(urgencies)
            .chain(ticking.flatten())
            .chain(event_moments.flatten())
            .chain(self.schedule.first().map(|event| event.start))
            .chain(self.transfer.as_ref().and_then(|transfer| transfer.next_moment(now, transfer_counts)))
            .chain(self.collapse_at)
            .chain(self.quick_controls.as_ref().and_then(QuickControls::deadline))
            .chain(self.peek_until)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_controls_use_the_island_deadline_and_close_without_removing_media() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.open_quick_controls(100, Some(1_000));
        assert_eq!(island.frame(100).wake_at, Some(1_100));
        island.quick_controls_mut().unwrap().set_interacting(true, 500);
        assert!(island.quick_controls().unwrap().deadline().is_none());
        island.frame(2_000);
        assert!(island.quick_controls().is_some());
        island.quick_controls_mut().unwrap().set_interacting(false, 2_000);
        island.frame(3_000);
        assert!(island.quick_controls().is_none());
        assert_eq!(shown(&island.frame(3_000)), Some(Kind::Media));
    }

    #[test]
    fn opening_an_existing_view_dismisses_quick_controls() {
        let mut island = Island::new();
        island.open_quick_controls(0, None);
        assert!(island.is_open());
        let frame = island.frame(0);
        assert_eq!(frame.presentation, Presentation::Expanded);
        assert!(matches!(frame.panel, Some(PanelView::QuickControls(_))));
        island.open_home();
        assert!(island.quick_controls().is_none());
        assert!(island.frame(1).home.is_some());
    }

    #[test]
    fn quick_calendar_uses_the_existing_schedule_and_connection_state() {
        let mut island = Island::new();
        island.set_calendar_state(CalendarState::NotSet);
        assert_eq!(island.quick_calendar().state, CalendarState::NotSet);
        assert!(island.quick_calendar().coming.is_empty());
        island.set_calendar_state(CalendarState::Ready);
        island.set_schedule(vec![event(30 * MINUTE), event(20 * MINUTE)], 0);
        let calendar = island.quick_calendar();
        assert_eq!(calendar.state, CalendarState::Ready);
        assert_eq!(calendar.coming.len(), 2);
        assert_eq!(calendar.coming[0].start, 20 * MINUTE);
        island.open_home();
        assert_eq!(island.frame(1).home.unwrap().coming, calendar.coming);
    }
    use crate::drop::DropView;

    fn activity(kind: Kind) -> Activity {
        Activity { title: format!("{kind:?}"), ..Activity::new(kind) }
    }

    fn event(start: Millis) -> Upcoming {
        Upcoming {
            start,
            wall_start: start as i64,
            title: "Ders".into(),
            location: None,
            time: "19:30".into(),
            when: "Bugün 19:30".into(),
        }
    }

    fn second(frame: &Frame) -> Option<Kind> {
        frame.secondary.as_ref().map(|a| a.kind)
    }

    const MINUTE: Millis = 60_000;

    #[test]
    fn two_lasting_things_share_the_island() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.start_timer(25 * MINUTE, 0);
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::Media));
        assert_eq!(second(&frame), Some(Kind::TimerRunning));
    }

    #[test]
    fn one_thing_gets_no_bubble() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        assert_eq!(second(&island.frame(0)), None);
    }

    #[test]
    fn a_third_thing_waits_its_turn() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.start_timer(25 * MINUTE, 0);
        island.set_schedule(vec![event(20 * MINUTE)], 0);
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::Media));
        assert_eq!(second(&frame), Some(Kind::TimerRunning));
    }

    #[test]
    fn a_timer_takes_the_pill_in_its_last_minute() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.start_timer(5 * MINUTE, 0);
        let frame = island.frame(4 * MINUTE - 1);
        assert_eq!(shown(&frame), Some(Kind::Media));
        assert_eq!(frame.wake_at, Some(4 * MINUTE));

        let frame = island.frame(4 * MINUTE);
        assert_eq!(shown(&frame), Some(Kind::TimerRunning));
        assert_eq!(second(&frame), Some(Kind::Media));
    }

    #[test]
    fn an_interruption_leaves_the_bubble_where_it_is() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.start_timer(25 * MINUTE, 0);
        island.upsert(Activity { level: Some(40), ..expiring(Kind::Alert, 1_000) });
        let frame = island.frame(0);
        assert_eq!(frame.presentation, Presentation::Level);
        assert_eq!(second(&frame), Some(Kind::TimerRunning));
    }

    #[test]
    fn clicking_the_bubble_opens_that_activity_and_collapsing_restores_the_order() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.start_timer(25 * MINUTE, 0);
        // A click lands on what the last frame drew.
        island.frame(0);
        island.click_secondary(0);
        let frame = island.frame(0);
        assert_eq!(frame.presentation, Presentation::Expanded);
        assert_eq!(shown(&frame), Some(Kind::TimerRunning));
        assert_eq!(second(&frame), None);

        island.click(0);
        let frame = island.frame(0);
        assert_eq!(frame.presentation, Presentation::Compact);
        assert_eq!(shown(&frame), Some(Kind::Media));
    }

    #[test]
    fn a_finished_timer_interrupts_once_with_a_sound() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.start_timer(MINUTE, 0);
        assert!(!island.frame(0).chime);
        let frame = island.frame(MINUTE);
        assert_eq!(shown(&frame), Some(Kind::TimerFinished));
        assert_eq!(frame.presentation, Presentation::Alert);
        assert!(frame.chime);
        assert!(!island.frame(MINUTE + 1).chime);
        assert_eq!(shown(&island.frame(MINUTE + TIMER_DONE_MS)), Some(Kind::Media));
    }

    #[test]
    fn an_external_notification_interrupts_music_and_timer_then_they_return() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.start_timer(25 * MINUTE, 0);
        let notice = crate::notifications::Notice {
            id: 1, app_id: "chat".into(), app: "Chat".into(), title: "Hello".into(), body: "Message".into(), logo: None,
        };
        island.upsert(notice.activity(0));
        let alert = island.frame(0);
        assert_eq!(alert.presentation, Presentation::Alert);
        assert_eq!(shown(&alert), Some(Kind::ExternalNotification));
        let after = island.frame(6_000);
        assert_eq!(shown(&after), Some(Kind::Media));
        assert_eq!(second(&after), Some(Kind::TimerRunning));
    }

    #[test]
    fn a_finished_timer_waits_for_a_click_but_not_forever() {
        let mut island = Island::new();
        island.start_timer(MINUTE, 0);
        island.frame(MINUTE);
        let frame = island.frame(MINUTE + 2 * MINUTE);
        assert_eq!(shown(&frame), Some(Kind::TimerFinished));
        assert_eq!(island.frame(MINUTE + TIMER_DONE_MS).presentation, Presentation::Hidden);

        let mut island = Island::new();
        island.start_timer(MINUTE, 0);
        island.frame(MINUTE);
        island.click(MINUTE + 1_000);
        assert_eq!(island.frame(MINUTE + 1_000).presentation, Presentation::Hidden);
    }

    #[test]
    fn the_shortcut_answers_a_finished_timer_too() {
        let mut island = Island::new();
        island.start_timer(MINUTE, 0);
        island.frame(MINUTE);
        island.summon(MINUTE + 1_000);
        assert_eq!(island.frame(MINUTE + 1_000).presentation, Presentation::Hidden);
    }

    #[test]
    fn a_finished_focus_session_waits_for_a_click_too() {
        let mut island = Island::new();
        island.set_focus(focus(MINUTE, MINUTE), 0);
        island.frame(MINUTE);
        assert_eq!(shown(&island.frame(MINUTE + 2 * MINUTE)), Some(Kind::TimerFinished));
    }

    #[test]
    fn a_timer_wakes_every_second_in_the_pill_and_every_minute_as_a_bubble() {
        let mut island = Island::new();
        island.start_timer(10 * MINUTE, 0);
        assert_eq!(island.frame(0).wake_at, Some(1_000));

        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.start_timer(10 * MINUTE, 0);
        assert_eq!(island.frame(0).wake_at, Some(MINUTE));
    }

    #[test]
    fn a_paused_timer_stays_but_stops_waking() {
        let mut island = Island::new();
        island.start_timer(10 * MINUTE, 0);
        island.toggle_timer(1_000);
        let frame = island.frame(1_000);
        assert_eq!(shown(&frame), Some(Kind::TimerRunning));
        assert_eq!(frame.wake_at, None);
    }

    #[test]
    fn cancelling_removes_the_timer_at_once() {
        let mut island = Island::new();
        island.start_timer(10 * MINUTE, 0);
        island.cancel_timer();
        assert_eq!(island.frame(0).presentation, Presentation::Hidden);
    }

    #[test]
    fn an_event_stays_away_until_its_lead_time() {
        let mut island = Island::new();
        island.set_schedule(vec![event(90 * MINUTE)], 0);
        let frame = island.frame(0);
        assert_eq!(frame.presentation, Presentation::Hidden);
        assert_eq!(frame.wake_at, Some(60 * MINUTE));
        assert_eq!(shown(&island.frame(60 * MINUTE)), Some(Kind::Upcoming));
    }

    fn focus(ends_at: Millis, total: Millis) -> FocusView {
        FocusView::Running { ends_at, total, on_break: false }
    }

    #[test]
    fn pausing_keeps_the_open_timer_open() {
        let mut island = Island::new();
        island.start_timer(5 * MINUTE, 0);
        island.frame(0);
        island.click(1_000);
        assert_eq!(island.frame(1_000).presentation, Presentation::Expanded);
        island.toggle_timer(2_000);
        let frame = island.frame(2_000);
        assert_eq!(frame.presentation, Presentation::Expanded);
        assert!(!frame.activity.unwrap().timer.unwrap().running);
        assert_eq!(island.frame(MINUTE).presentation, Presentation::Expanded);
    }

    #[test]
    fn a_focus_session_counts_down_with_controls() {
        let mut island = Island::new();
        island.set_focus(focus(15 * MINUTE, 15 * MINUTE), 0);
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::Focus));
        let activity = frame.activity.unwrap();
        assert_eq!(activity.leading, "Odak");
        assert_eq!(activity.trailing, "15:00");
        assert!(activity.timer.unwrap().has_controls);
    }

    #[test]
    fn a_paused_focus_session_stays_and_stops_waking() {
        let mut island = Island::new();
        island.set_focus(FocusView::Paused { remaining: 2 * MINUTE, total: 5 * MINUTE, on_break: false }, 0);
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::Focus));
        assert_eq!(frame.wake_at, None);
        let activity = frame.activity.unwrap();
        assert_eq!(activity.detail, "Duraklatıldı");
        assert!(!activity.timer.unwrap().running);
    }

    #[test]
    fn a_break_reads_as_a_break() {
        let mut island = Island::new();
        island.set_focus(FocusView::Running { ends_at: MINUTE, total: MINUTE, on_break: true }, 0);
        assert_eq!(island.frame(0).activity.unwrap().leading, "Mola");
    }

    #[test]
    fn a_session_of_unknown_length_still_shows() {
        let mut island = Island::new();
        island.set_focus(FocusView::Open, 0);
        let frame = island.frame(0);
        assert_eq!(frame.wake_at, None);
        let activity = frame.activity.unwrap();
        assert_eq!(activity.trailing, "Açık");
        assert_eq!(activity.timer.unwrap().left, None);
    }

    #[test]
    fn a_focus_session_sits_beside_music_then_takes_the_pill() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.set_focus(focus(15 * MINUTE, 15 * MINUTE), 0);
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::Media));
        assert_eq!(second(&frame), Some(Kind::Focus));
        assert_eq!(shown(&island.frame(14 * MINUTE)), Some(Kind::Focus));
    }

    #[test]
    fn a_finished_focus_session_says_so() {
        let mut island = Island::new();
        island.set_focus(focus(MINUTE, MINUTE), 0);
        let frame = island.frame(MINUTE);
        assert_eq!(frame.activity.unwrap().title, "Odak bitti");
        assert!(frame.chime);
    }

    #[test]
    fn ending_a_focus_session_early_leaves_quietly() {
        let mut island = Island::new();
        island.set_focus(focus(15 * MINUTE, 15 * MINUTE), 0);
        island.frame(0);
        island.set_focus(FocusView::Off, 1_000);
        let frame = island.frame(1_000);
        assert_eq!(frame.presentation, Presentation::Hidden);
        assert!(!frame.chime);
    }

    #[test]
    fn windows_reporting_the_end_first_still_counts_as_finishing() {
        let mut island = Island::new();
        island.set_focus(focus(MINUTE, MINUTE), 0);
        island.frame(0);
        island.set_focus(FocusView::Off, MINUTE - 400);
        assert_eq!(shown(&island.frame(MINUTE - 400)), Some(Kind::Focus));
        let frame = island.frame(MINUTE);
        assert_eq!(frame.activity.unwrap().title, "Odak bitti");
        assert!(frame.chime);
    }

    #[test]
    fn a_session_found_over_after_a_long_sleep_leaves_quietly() {
        let mut island = Island::new();
        island.set_focus(focus(MINUTE, MINUTE), 0);
        island.frame(0);
        island.set_focus(FocusView::Off, 60 * MINUTE);
        let frame = island.frame(60 * MINUTE);
        assert_eq!(frame.presentation, Presentation::Hidden);
        assert!(!frame.chime);
    }

    fn clock_timer(ends_at: Millis) -> Option<(Timer, String)> {
        Some((Timer::ending_at(ends_at, 5 * MINUTE), "5 dk".into()))
    }

    #[test]
    fn a_clock_timer_shows_without_buttons() {
        let mut island = Island::new();
        island.set_clock_timer(clock_timer(5 * MINUTE), 0);
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::ClockTimer));
        assert_eq!(frame.wake_at, Some(1_000));
        assert!(!frame.activity.unwrap().timer.unwrap().has_controls);
    }

    #[test]
    fn a_clock_timer_shares_the_island_with_the_islands_own() {
        let mut island = Island::new();
        island.start_timer(25 * MINUTE, 0);
        island.set_clock_timer(clock_timer(20 * MINUTE), 0);
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::TimerRunning));
        assert_eq!(second(&frame), Some(Kind::ClockTimer));
    }

    #[test]
    fn a_clock_timer_says_when_it_ends_but_leaves_the_sound_to_the_clock() {
        let mut island = Island::new();
        island.set_clock_timer(clock_timer(MINUTE), 0);
        island.frame(0);
        let frame = island.frame(MINUTE);
        assert!(!frame.chime);
        assert_eq!(frame.activity.unwrap().detail, "Saat uygulaması · 5 dk");
        assert_eq!(island.frame(MINUTE + CLOCK_TIMER_DONE_MS).presentation, Presentation::Hidden);
    }

    #[test]
    fn a_clock_timer_stopped_early_leaves_quietly() {
        let mut island = Island::new();
        island.set_clock_timer(clock_timer(5 * MINUTE), 0);
        island.frame(0);
        island.set_clock_timer(None, 1_000);
        assert_eq!(island.frame(1_000).presentation, Presentation::Hidden);
    }

    fn paused_clock_timer() -> Option<(Timer, String)> {
        Some((Timer::paused(MINUTE, 5 * MINUTE), "5 dk".into()))
    }

    #[test]
    fn a_clock_timer_paused_just_now_shows_for_a_moment() {
        let mut island = Island::new();
        island.set_clock_timer(clock_timer(5 * MINUTE), 0);
        island.frame(0);
        island.set_clock_timer(paused_clock_timer(), 1_000);
        let frame = island.frame(1_000);
        assert_eq!(shown(&frame), Some(Kind::ClockTimer));
        assert_eq!(frame.wake_at, Some(1_000 + PAUSED_LINGER_MS));
        assert_eq!(island.frame(1_000 + PAUSED_LINGER_MS).presentation, Presentation::Hidden);
    }

    #[test]
    fn a_clock_timer_found_paused_stays_out_of_the_way() {
        let mut island = Island::new();
        island.set_clock_timer(paused_clock_timer(), 0);
        assert_eq!(island.frame(0).presentation, Presentation::Hidden);
        island.peek(10);
        assert_eq!(shown(&island.frame(10)), Some(Kind::ClockTimer));
    }

    #[test]
    fn staying_paused_does_not_extend_how_long_a_clock_timer_shows() {
        let mut island = Island::new();
        island.set_clock_timer(clock_timer(5 * MINUTE), 0);
        island.frame(0);
        island.set_clock_timer(paused_clock_timer(), 1_000);
        island.set_clock_timer(paused_clock_timer(), 20_000);
        assert_eq!(island.frame(20_000).wake_at, Some(1_000 + PAUSED_LINGER_MS));
    }

    #[test]
    fn a_clock_timer_gone_right_at_its_end_still_announces_it() {
        let mut island = Island::new();
        island.set_clock_timer(clock_timer(MINUTE), 0);
        island.frame(0);
        island.set_clock_timer(None, MINUTE - 200);
        assert_eq!(shown(&island.frame(MINUTE)), Some(Kind::TimerFinished));
    }

    #[test]
    fn the_home_view_lists_the_next_two_events() {
        let mut island = Island::new();
        island.set_schedule(vec![event(120 * MINUTE), event(180 * MINUTE), event(240 * MINUTE)], 0);
        island.open_home();
        let home = island.frame(0).home.unwrap();
        assert_eq!(home.coming.len(), 2);
        assert_eq!(home.coming[0].start, 120 * MINUTE);
    }

    #[test]
    fn the_home_view_says_how_the_calendar_is_doing() {
        let mut island = Island::new();
        island.set_calendar_state(CalendarState::NotSet);
        island.open_home();
        assert_eq!(island.frame(0).home.unwrap().calendar, CalendarState::NotSet);
    }

    #[test]
    fn the_home_view_offers_what_can_be_started() {
        let mut island = Island::new();
        island.set_presets([1, 2, 3, 4]);
        island.set_start_mode(StartMode::Timer);
        island.open_home();
        let home = island.frame(0).home.unwrap();
        assert_eq!(home.presets, [1, 2, 3, 4]);
        assert_eq!(home.mode, StartMode::Timer);
        assert!(home.modes_offered);
    }

    #[test]
    fn without_the_clock_app_only_timers_are_offered() {
        let mut island = Island::new();
        island.offer_only_timers();
        assert!(!island.set_start_mode(StartMode::Focus));
        island.open_home();
        let home = island.frame(0).home.unwrap();
        assert_eq!(home.mode, StartMode::Timer);
        assert!(!home.modes_offered);
    }

    #[test]
    fn a_coming_event_is_a_bubble_then_takes_the_pill() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.set_schedule(vec![event(30 * MINUTE)], 0);
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::Media));
        assert_eq!(second(&frame), Some(Kind::Upcoming));

        let frame = island.frame(25 * MINUTE);
        assert_eq!(shown(&frame), Some(Kind::Upcoming));
        assert_eq!(second(&frame), Some(Kind::Media));
    }

    #[test]
    fn a_coming_event_updates_once_a_minute() {
        let mut island = Island::new();
        island.set_schedule(vec![event(20 * MINUTE)], 0);
        assert_eq!(island.frame(0).wake_at, Some(MINUTE));
        assert_eq!(island.frame(30_000).wake_at, Some(MINUTE));
    }

    #[test]
    fn a_starting_event_is_announced_once_then_leaves() {
        let mut island = Island::new();
        island.set_schedule(vec![event(10 * MINUTE)], 0);
        let frame = island.frame(10 * MINUTE);
        assert_eq!(shown(&frame), Some(Kind::Notification));
        assert!(frame.chime);
        assert!(!island.frame(10 * MINUTE + 1).chime);
        let after = island.frame(10 * MINUTE + 60_000);
        assert_eq!(after.presentation, Presentation::Hidden);
    }

    #[test]
    fn the_camera_shows_for_as_long_as_it_is_on() {
        let mut island = Island::new();
        island.set_camera(Some("Zoom".into()));
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::Camera));
        assert_eq!(frame.activity.unwrap().trailing, "Zoom");
        assert_eq!(frame.wake_at, None);
        island.set_camera(None);
        assert_eq!(island.frame(1).presentation, Presentation::Hidden);
    }

    #[test]
    fn the_camera_sits_beside_music() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.set_camera(Some("Zoom".into()));
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::Media));
        assert_eq!(second(&frame), Some(Kind::Camera));
    }

    #[test]
    fn a_hidden_event_leaves_the_island_and_gives_back_the_pill() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.set_schedule(vec![event(3 * MINUTE)], 0);
        assert_eq!(shown(&island.frame(0)), Some(Kind::Upcoming));
        island.click(0);
        island.hide_event();
        let frame = island.frame(0);
        assert_eq!(frame.presentation, Presentation::Compact);
        assert_eq!(shown(&frame), Some(Kind::Media));
        assert_eq!(second(&frame), None);
    }

    #[test]
    fn a_hidden_event_says_nothing_when_it_starts() {
        let mut island = Island::new();
        island.set_schedule(vec![event(3 * MINUTE)], 0);
        island.hide_event();
        let frame = island.frame(3 * MINUTE);
        assert_eq!(frame.presentation, Presentation::Hidden);
        assert!(!frame.chime);
    }

    #[test]
    fn a_hidden_event_stays_hidden_after_the_calendar_reloads() {
        let mut island = Island::new();
        island.set_schedule(vec![event(3 * MINUTE)], 0);
        island.hide_event();
        island.set_schedule(vec![event(3 * MINUTE)], MINUTE);
        assert_eq!(island.frame(MINUTE).presentation, Presentation::Hidden);
    }

    #[test]
    fn an_event_that_moved_shows_again() {
        let mut island = Island::new();
        island.set_schedule(vec![event(3 * MINUTE)], 0);
        island.hide_event();
        island.set_schedule(vec![event(4 * MINUTE)], MINUTE);
        assert_eq!(shown(&island.frame(MINUTE)), Some(Kind::Upcoming));
    }

    #[test]
    fn the_next_event_still_shows_after_a_hidden_one() {
        let mut island = Island::new();
        island.set_schedule(vec![event(3 * MINUTE), event(10 * MINUTE)], 0);
        island.hide_event();
        let frame = island.frame(0);
        assert_eq!(frame.activity.unwrap().trailing, "10 dk");
        assert_eq!(frame.wake_at, Some(MINUTE));
    }

    #[test]
    fn a_hidden_event_is_left_out_of_the_home_view() {
        let mut island = Island::new();
        island.set_schedule(vec![event(120 * MINUTE), event(180 * MINUTE)], 0);
        island.upsert(activity(Kind::Upcoming));
        island.hide_event();
        island.open_home();
        let home = island.frame(0).home.unwrap();
        assert_eq!(home.coming.len(), 1);
        assert_eq!(home.coming[0].start, 180 * MINUTE);
    }

    #[test]
    fn events_starting_together_are_announced_together() {
        let mut island = Island::new();
        let other = Upcoming { title: "Toplantı".into(), ..event(3 * MINUTE) };
        island.set_schedule(vec![event(3 * MINUTE), other], 0);
        let frame = island.frame(3 * MINUTE);
        assert!(frame.chime);
        let alert = frame.activity.unwrap();
        assert_eq!(alert.title, "Ders");
        assert_eq!(alert.detail, "Başlıyor · +1 etkinlik");
    }

    #[test]
    fn a_hidden_event_does_not_count_among_those_starting() {
        let mut island = Island::new();
        let other = Upcoming { title: "Toplantı".into(), ..event(3 * MINUTE) };
        island.set_schedule(vec![event(3 * MINUTE), other], 0);
        island.hide_event();
        let alert = island.frame(3 * MINUTE).activity.unwrap();
        assert_eq!(alert.title, "Toplantı");
        assert_eq!(alert.detail, "Başlıyor");
    }

    #[test]
    fn events_that_already_started_are_dropped_quietly() {
        let mut island = Island::new();
        island.set_schedule(vec![event(5), event(20 * MINUTE)], 10);
        let frame = island.frame(10);
        assert_eq!(shown(&frame), Some(Kind::Upcoming));
        assert!(!frame.chime);
    }

    #[test]
    fn clicking_the_clock_opens_the_home_view() {
        let mut island = Island::new();
        island.set_schedule(vec![event(120 * MINUTE)], 0);
        island.peek(0);
        island.set_hovered(true, 100);
        island.click(200);
        let frame = island.frame(200);
        assert_eq!(frame.presentation, Presentation::Expanded);
        assert_eq!(frame.activity, None);
        assert_eq!(frame.home.unwrap().coming[0].title, "Ders");
    }

    #[test]
    fn the_home_view_closes_with_the_pointer() {
        let mut island = Island::new();
        island.peek(0);
        island.set_hovered(true, 100);
        island.click(200);
        island.set_hovered(false, 1_000);
        assert!(island.frame(1_000).home.is_some());
        let frame = island.frame(1_000 + COLLAPSE_DELAY_MS);
        assert_eq!(frame.home, None);
        assert_eq!(frame.presentation, Presentation::Compact);
    }

    #[test]
    fn a_preset_starts_the_timer_and_closes_the_home_view() {
        let mut island = Island::new();
        island.peek(0);
        island.set_hovered(true, 100);
        island.click(200);
        island.start_timer(5 * MINUTE, 300);
        let frame = island.frame(300);
        assert_eq!(frame.home, None);
        assert_eq!(frame.presentation, Presentation::Compact);
        assert_eq!(shown(&frame), Some(Kind::TimerRunning));
    }

    #[test]
    fn the_home_view_opens_over_playing_music() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.click(0);
        island.open_home();
        let frame = island.frame(0);
        assert!(frame.home.is_some());
        assert_eq!(frame.activity, None);
        island.click(10);
        let frame = island.frame(10);
        assert_eq!(frame.presentation, Presentation::Compact);
        assert_eq!(shown(&frame), Some(Kind::Media));
    }

    #[test]
    fn the_shortcut_pressed_twice_opens_the_home_view() {
        let mut island = Island::new();
        island.summon(0);
        assert_eq!(island.frame(0).home, None);
        island.summon(500);
        assert!(island.frame(500).home.is_some());
        let frame = island.frame(500 + KEYBOARD_EXPAND_MS);
        assert_eq!(frame.home, None);
        assert_eq!(frame.presentation, Presentation::Hidden);
    }

    #[test]
    fn the_shortcut_twice_opens_the_home_view_under_a_resting_pointer() {
        let mut island = Island::new();
        island.summon(0);
        // The clock came up right under the pointer.
        island.set_hovered(true, 100);
        assert_eq!(island.frame(100).presentation, Presentation::Compact);
        island.summon(500);
        assert!(island.frame(500).home.is_some());
    }

    #[test]
    fn an_interruption_does_not_lose_the_home_view() {
        let mut island = Island::new();
        island.peek(0);
        island.set_hovered(true, 100);
        island.click(200);
        island.upsert(expiring(Kind::Alert, 1_000));
        assert_eq!(island.frame(500).presentation, Presentation::Alert);
        assert!(island.frame(1_000).home.is_some());
    }

    fn expiring(kind: Kind, at: Millis) -> Activity {
        Activity { expires_at: Some(at), ..activity(kind) }
    }

    fn shown(frame: &Frame) -> Option<Kind> {
        frame.activity.as_ref().map(|a| a.kind)
    }

    #[test]
    fn stays_hidden_and_never_wakes_with_nothing_to_show() {
        let frame = Island::new().frame(0);
        assert_eq!(frame.presentation, Presentation::Hidden);
        assert_eq!(frame.wake_at, None);
    }

    #[test]
    fn activities_show_in_priority_order() {
        let mut island = Island::new();
        island.set_schedule(vec![event(20 * MINUTE)], 0);
        assert_eq!(shown(&island.frame(0)), Some(Kind::Upcoming));
        island.start_timer(25 * MINUTE, 0);
        assert_eq!(shown(&island.frame(0)), Some(Kind::TimerRunning));
        island.upsert(activity(Kind::Media));
        assert_eq!(shown(&island.frame(0)), Some(Kind::Media));
        island.upsert(activity(Kind::TimerFinished));
        assert_eq!(shown(&island.frame(0)), Some(Kind::TimerFinished));
        island.upsert(activity(Kind::Notification));
        assert_eq!(shown(&island.frame(0)), Some(Kind::Notification));
        island.upsert(activity(Kind::Alert));
        assert_eq!(shown(&island.frame(0)), Some(Kind::Alert));
    }

    #[test]
    fn upsert_replaces_the_same_kind() {
        let mut island = Island::new();
        island.upsert(activity(Kind::Media));
        island.upsert(Activity { title: "new".into(), ..activity(Kind::Media) });
        assert_eq!(island.frame(0).activity.unwrap().title, "new");
        island.remove(Kind::Media);
        assert_eq!(island.frame(0).presentation, Presentation::Hidden);
    }

    #[test]
    fn transient_interrupts_then_returns_to_the_previous_view() {
        let mut island = Island::new();
        island.upsert(activity(Kind::Media));
        island.click(0);
        assert_eq!(island.frame(0).presentation, Presentation::Expanded);

        island.upsert(expiring(Kind::Alert, 2_000));
        let frame = island.frame(100);
        assert_eq!(frame.presentation, Presentation::Alert);
        assert_eq!(frame.wake_at, Some(2_000));

        let frame = island.frame(2_000);
        assert_eq!(shown(&frame), Some(Kind::Media));
        assert_eq!(frame.presentation, Presentation::Expanded);
        assert_eq!(frame.wake_at, None);
    }

    #[test]
    fn a_transient_arriving_in_the_compact_view_returns_to_it() {
        let mut island = Island::new();
        island.upsert(activity(Kind::Media));
        island.upsert(expiring(Kind::Alert, 2_000));
        assert_eq!(island.frame(100).presentation, Presentation::Alert);
        assert_eq!(island.frame(2_000).presentation, Presentation::Compact);
    }

    #[test]
    fn a_level_uses_the_slim_pill_then_returns() {
        let mut island = Island::new();
        island.upsert(activity(Kind::Media));
        island.upsert(Activity { level: Some(40), ..expiring(Kind::Alert, 1_000) });
        assert_eq!(island.frame(0).presentation, Presentation::Level);
        assert_eq!(island.frame(1_000).presentation, Presentation::Compact);
    }

    #[test]
    fn each_step_of_a_level_extends_how_long_it_stays() {
        let mut island = Island::new();
        island.upsert(Activity { level: Some(40), ..expiring(Kind::Alert, 1_000) });
        island.frame(0);
        island.upsert(Activity { level: Some(42), ..expiring(Kind::Alert, 1_500) });
        let frame = island.frame(900);
        assert_eq!(frame.activity.unwrap().level, Some(42));
        assert_eq!(frame.wake_at, Some(1_500));
    }

    #[test]
    fn clicking_an_alert_dismisses_it() {
        let mut island = Island::new();
        island.upsert(activity(Kind::Media));
        island.upsert(expiring(Kind::Notification, 5_000));
        island.click(10);
        assert_eq!(shown(&island.frame(10)), Some(Kind::Media));
    }

    #[test]
    fn click_toggles_compact_and_expanded() {
        let mut island = Island::new();
        island.upsert(activity(Kind::Media));
        island.click(0);
        assert_eq!(island.frame(0).presentation, Presentation::Expanded);
        island.click(0);
        assert_eq!(island.frame(0).presentation, Presentation::Compact);
    }

    #[test]
    fn leaving_collapses_after_the_delay_unless_the_pointer_returns() {
        let mut island = Island::new();
        island.upsert(activity(Kind::Media));
        island.click(0);
        island.set_hovered(true, 0);

        island.set_hovered(false, 1_000);
        let frame = island.frame(1_000);
        assert_eq!(frame.presentation, Presentation::Expanded);
        assert_eq!(frame.wake_at, Some(1_000 + COLLAPSE_DELAY_MS));

        island.set_hovered(true, 1_100);
        let frame = island.frame(1_600);
        assert_eq!(frame.presentation, Presentation::Expanded);
        assert_eq!(frame.wake_at, None);

        island.set_hovered(false, 2_000);
        assert_eq!(island.frame(2_000 + COLLAPSE_DELAY_MS).presentation, Presentation::Compact);
    }

    #[test]
    fn peek_shows_the_clock_then_hides() {
        let mut island = Island::new();
        island.peek(1_000);
        let frame = island.frame(1_000);
        assert_eq!(frame.presentation, Presentation::Compact);
        assert_eq!(frame.activity, None);
        assert_eq!(frame.wake_at, Some(1_000 + PEEK_MS));
        assert_eq!(island.frame(1_000 + PEEK_MS).presentation, Presentation::Hidden);
    }

    #[test]
    fn hovering_the_clock_keeps_it_until_the_pointer_leaves() {
        let mut island = Island::new();
        island.peek(0);
        island.set_hovered(true, 100);
        let frame = island.frame(10_000);
        assert_eq!(frame.presentation, Presentation::Compact);
        assert_eq!(frame.wake_at, None);

        island.set_hovered(false, 10_000);
        assert_eq!(island.frame(10_000).presentation, Presentation::Compact);
        assert_eq!(island.frame(10_000 + PEEK_MS).presentation, Presentation::Hidden);
    }

    #[test]
    fn summon_opens_the_activity_and_closes_itself() {
        let mut island = Island::new();
        island.upsert(activity(Kind::Media));
        island.summon(0);
        assert_eq!(island.frame(0).presentation, Presentation::Expanded);
        assert_eq!(island.frame(KEYBOARD_EXPAND_MS).presentation, Presentation::Compact);
    }

    #[test]
    fn suppression_hides_everything_and_restores_it() {
        let mut island = Island::new();
        island.upsert(activity(Kind::Media));
        island.set_suppressed(Suppression::Fullscreen, true);
        island.set_suppressed(Suppression::WindowDrag, true);
        let frame = island.frame(0);
        assert_eq!(frame.presentation, Presentation::Hidden);
        assert_eq!(frame.activity, None);

        island.set_suppressed(Suppression::WindowDrag, false);
        assert_eq!(island.frame(0).presentation, Presentation::Hidden);

        island.set_suppressed(Suppression::Fullscreen, false);
        assert_eq!(island.frame(0).presentation, Presentation::Compact);
    }

    #[test]
    fn transients_still_expire_while_suppressed() {
        let mut island = Island::new();
        island.upsert(expiring(Kind::Alert, 1_000));
        island.set_suppressed(Suppression::Fullscreen, true);
        assert_eq!(island.frame(0).wake_at, Some(1_000));
        island.frame(1_000);
        island.set_suppressed(Suppression::Fullscreen, false);
        assert_eq!(island.frame(1_000).presentation, Presentation::Hidden);
    }

    fn dormant(at: Millis) -> Activity {
        Activity { dormant_at: Some(at), ..activity(Kind::Media) }
    }

    #[test]
    fn a_dormant_activity_leaves_the_screen_on_time() {
        let mut island = Island::new();
        island.upsert(dormant(1_000));
        let frame = island.frame(0);
        assert_eq!(frame.presentation, Presentation::Compact);
        assert_eq!(frame.wake_at, Some(1_000));

        let frame = island.frame(1_000);
        assert_eq!(frame.presentation, Presentation::Hidden);
        assert_eq!(frame.wake_at, None);
    }

    #[test]
    fn a_peek_brings_a_dormant_activity_back() {
        let mut island = Island::new();
        island.upsert(dormant(1_000));
        island.peek(2_000);
        let frame = island.frame(2_000);
        assert_eq!(shown(&frame), Some(Kind::Media));
        assert_eq!(island.frame(2_000 + PEEK_MS).presentation, Presentation::Hidden);
    }

    #[test]
    fn a_dormant_activity_can_be_opened_while_peeking() {
        let mut island = Island::new();
        island.upsert(dormant(1_000));
        island.peek(2_000);
        island.set_hovered(true, 2_100);
        island.click(2_200);
        assert_eq!(island.frame(2_200).presentation, Presentation::Expanded);

        island.set_hovered(false, 3_000);
        assert_eq!(island.frame(3_000).presentation, Presentation::Expanded);
        assert_eq!(island.frame(3_000 + PEEK_MS).presentation, Presentation::Hidden);
    }

    #[test]
    fn a_second_summon_opens_a_dormant_activity() {
        let mut island = Island::new();
        island.upsert(dormant(1_000));
        island.summon(2_000);
        assert_eq!(island.frame(2_000).presentation, Presentation::Compact);
        island.summon(2_500);
        assert_eq!(island.frame(2_500).presentation, Presentation::Expanded);
        assert_eq!(island.frame(2_500 + KEYBOARD_EXPAND_MS).presentation, Presentation::Hidden);
    }

    #[test]
    fn a_dormant_activity_does_not_outrank_the_clock_peek_rules() {
        let mut island = Island::new();
        island.upsert(dormant(0));
        island.set_schedule(vec![event(20 * MINUTE)], 0);
        assert_eq!(shown(&island.frame(10)), Some(Kind::Upcoming));
    }

    fn snapshot(playing: bool) -> MediaSnapshot {
        MediaSnapshot {
            title: "Song".into(),
            artist: "Artist".into(),
            playing,
            position_ms: 0,
            duration_ms: 1_000,
            rate: 1.0,
            sampled_at: 0,
            can_seek: true,
            artwork: None,
            new_position: true,
            shuffle: None,
            repeat: None,
            is_music: true,
            can_previous: true,
            can_next: true,
        }
    }

    fn track(title: &str, playing: bool) -> MediaSnapshot {
        MediaSnapshot { title: title.into(), ..snapshot(playing) }
    }

    #[test]
    fn pressing_shuffle_does_not_move_the_time_on_screen() {
        let mut island = Island::new();
        let long = MediaSnapshot { duration_ms: 200_000, sampled_at: 0, ..track("Bir", true) };
        island.upsert_media(MediaSnapshot { position_ms: 30_000, ..long.clone() }, 0);
        for at in [1_000, 2_000, 3_000] {
            let stale = MediaSnapshot { position_ms: 1_000, sampled_at: at, new_position: false, shuffle: Some(true), ..long.clone() };
            island.upsert_media(stale, at);
            let media = island.frame(at).activity.unwrap().media.unwrap();
            assert_eq!(media.position_at(at), 30_000 + at);
            assert_eq!(media.shuffle, Some(true));
        }
    }

    #[test]
    fn a_new_track_is_announced_for_a_moment() {
        let mut island = Island::new();
        island.upsert_media(track("Bir", true), 0);
        island.upsert_media(track("İki", true), 1_000);
        let frame = island.frame(1_000);
        assert_eq!(frame.presentation, Presentation::Alert);
        assert_eq!(frame.activity.as_ref().unwrap().kind, Kind::NowPlaying);
        assert_eq!(frame.activity.unwrap().title, "İki");
        let after = island.frame(1_000 + TRACK_NOTICE_MS);
        assert_eq!(after.presentation, Presentation::Compact);
        assert_eq!(shown(&after), Some(Kind::Media));
    }

    #[test]
    fn the_first_track_needs_no_announcing() {
        let mut island = Island::new();
        island.upsert_media(track("Bir", true), 0);
        assert_eq!(island.frame(0).presentation, Presentation::Compact);
    }

    #[test]
    fn the_same_track_going_on_is_not_news() {
        let mut island = Island::new();
        island.upsert_media(track("Bir", false), 0);
        island.upsert_media(track("Bir", true), 500);
        assert_eq!(island.frame(500).presentation, Presentation::Compact);
    }

    #[test]
    fn a_track_changing_while_paused_stays_quiet() {
        let mut island = Island::new();
        island.upsert_media(track("Bir", true), 0);
        island.upsert_media(track("İki", false), 500);
        assert_eq!(island.frame(500).presentation, Presentation::Compact);
    }

    #[test]
    fn the_open_view_is_not_covered_for_a_new_track() {
        let mut island = Island::new();
        island.upsert_media(track("Bir", true), 0);
        island.click(0);
        island.upsert_media(track("İki", true), 500);
        assert_eq!(island.frame(500).presentation, Presentation::Expanded);
    }

    #[test]
    fn a_new_video_is_not_announced() {
        let mut island = Island::new();
        let video = |title: &str| MediaSnapshot { is_music: false, ..track(title, true) };
        island.upsert_media(video("Bir"), 0);
        island.upsert_media(video("İki"), 1_000);
        assert_eq!(island.frame(1_000).activity.map(|a| a.kind), Some(Kind::Media));
    }

    #[test]
    fn track_notices_can_be_turned_off() {
        let mut island = Island::new();
        island.set_track_notices(false);
        island.upsert_media(track("Bir", true), 0);
        island.upsert_media(track("İki", true), 500);
        assert_eq!(island.frame(500).presentation, Presentation::Compact);
    }

    #[test]
    fn artwork_arriving_late_joins_the_notice_without_extending_it() {
        let mut island = Island::new();
        island.upsert_media(track("Bir", true), 0);
        island.upsert_media(track("İki", true), 1_000);
        let artwork = Some(std::sync::Arc::from(&b"png"[..]));
        island.upsert_media(MediaSnapshot { artwork, ..track("İki", true) }, 1_500);
        let frame = island.frame(1_500);
        let notice = frame.activity.unwrap();
        assert!(notice.preview.is_some());
        assert_eq!(notice.expires_at, Some(1_000 + TRACK_NOTICE_MS));
    }

    #[test]
    fn a_brief_notice_uses_the_compact_pill_and_leaves() {
        let mut island = Island::new();
        island.upsert_media(track("Bir", true), 0);
        island.upsert(Activity { brief: true, ..expiring(Kind::Alert, 1_000) });
        let frame = island.frame(0);
        assert_eq!(frame.presentation, Presentation::Compact);
        assert_eq!(shown(&frame), Some(Kind::Alert));
        assert_eq!(shown(&island.frame(1_000)), Some(Kind::Media));
    }

    #[test]
    fn a_jump_shows_immediately_and_playback_continues_from_it() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.seek_media(600, 100);
        let media = island.frame(100).activity.unwrap().media.unwrap();
        assert_eq!(media.position_at(100), 600);
        assert_eq!(media.position_at(300), 800);
    }

    #[test]
    fn a_jump_cannot_land_past_the_end() {
        let mut island = Island::new();
        island.upsert_media(snapshot(false), 0);
        island.seek_media(99_000, 0);
        let media = island.frame(0).activity.unwrap().media.unwrap();
        assert_eq!(media.position_at(0), 1_000);
    }

    #[test]
    fn a_jump_without_media_does_nothing() {
        let mut island = Island::new();
        island.seek_media(600, 0);
        assert_eq!(island.frame(0).presentation, Presentation::Hidden);
    }

    #[test]
    fn staying_paused_does_not_extend_the_linger() {
        let mut island = Island::new();
        island.upsert_media(snapshot(false), 0);
        island.upsert_media(snapshot(false), 20_000);
        let deadline = island.frame(20_000).wake_at;
        assert_eq!(deadline, Some(crate::media::PAUSED_LINGER_MS));
    }

    #[test]
    fn resuming_clears_the_linger_and_a_new_pause_starts_over() {
        let mut island = Island::new();
        island.upsert_media(snapshot(false), 0);
        island.upsert_media(snapshot(true), 10_000);
        assert_eq!(island.frame(10_000).wake_at, None);

        island.upsert_media(snapshot(false), 50_000);
        let deadline = island.frame(50_000).wake_at;
        assert_eq!(deadline, Some(50_000 + crate::media::PAUSED_LINGER_MS));
    }

    #[test]
    fn expansion_does_not_survive_the_activity_ending() {
        let mut island = Island::new();
        island.upsert(activity(Kind::Media));
        island.click(0);
        island.remove(Kind::Media);
        island.frame(0);
        island.upsert(activity(Kind::Media));
        assert_eq!(island.frame(0).presentation, Presentation::Compact);
    }

    fn drop_view() -> DropView {
        DropView { header: "a.jpg · 1 KB".into(), hovered: None, phone_refusal: None, convert_refusal: None }
    }

    fn transfer() -> Transfer {
        Transfer {
            name: "a.jpg".into(),
            summary: "a.jpg · 1 KB".into(),
            total: 1000,
            code: std::sync::Arc::new(crate::qr::QrMatrix::encode("http://10.0.0.2:5000/x").expect("fits")),
            network: "Wi-Fi · Ev".into(),
            warning: None,
            stage: Stage::Waiting { until: WAIT_MS },
            direct: None,
            offline: false,
            nudge_at: Some(crate::transfer::NUDGE_MS),
        }
    }

    #[test]
    fn a_phone_that_found_the_page_stops_the_suggestion() {
        let mut island = Island::new();
        island.start_transfer(transfer());
        let nudged = |island: &mut Island| island.frame(crate::transfer::NUDGE_MS).activity.and_then(|a| a.transfer).is_some_and(|t| t.nudge);
        assert!(nudged(&mut island));
        island.transfer_visited();
        assert!(!nudged(&mut island));
    }

    #[test]
    fn the_hotspot_serves_the_download_code_without_a_join_code() {
        let mut island = Island::new();
        island.start_transfer(transfer());
        island.transfer_warning(Some("Güvenlik duvarı bu ağda engelliyor".into()));
        let download = crate::qr::QrMatrix::encode("http://192.168.137.1:5000/t").expect("fits");
        island.transfer_hotspot(download, "Mobil etkin nokta".into());
        let view = island.frame(0).activity.and_then(|a| a.transfer).expect("view");
        assert_eq!(view.direct, Some(Direct::Hotspot { network: "Mobil etkin nokta".into() }));
        assert_eq!(view.network, "Mobil etkin nokta");
        assert_eq!(view.warning, None);
        assert!(island.transfer_is_direct());
    }

    #[test]
    fn without_wifi_the_send_waits_for_it() {
        let mut island = Island::new();
        island.start_transfer(transfer());
        island.transfer_offline(true);
        let view = island.frame(0).activity.and_then(|a| a.transfer).expect("view");
        assert!(view.offline);
        island.transfer_offline(false);
        assert!(!island.frame(0).activity.and_then(|a| a.transfer).expect("view").offline);
    }

    #[test]
    fn the_own_network_shows_its_code_then_the_download_code() {
        let code = |text: &str| crate::qr::QrMatrix::encode(text).expect("fits");
        let mut island = Island::new();
        island.start_transfer(transfer());
        island.transfer_direct_preparing();
        let view = |island: &mut Island| island.frame(0).activity.and_then(|a| a.transfer).and_then(|t| t.direct);
        assert_eq!(view(&mut island), Some(Direct::Preparing));
        island.transfer_direct_ready(code("WIFI:T:WPA;S:Islora-K4FX;P:x;;"), "Islora-K4FX".into(), code("http://192.168.137.1:5000/t"));
        assert!(matches!(view(&mut island), Some(Direct::Ready { .. })));
        island.transfer_direct_joined();
        assert_eq!(view(&mut island), Some(Direct::Joined { network: "Islora-K4FX".into() }));
        let shown = island.frame(0).activity.and_then(|a| a.transfer).expect("view");
        assert_eq!(shown.network, "Islora-K4FX");
    }

    #[test]
    fn a_network_that_fails_brings_back_the_usual_code() {
        let mut island = Island::new();
        island.start_transfer(transfer());
        island.transfer_direct_preparing();
        island.transfer_direct_failed();
        assert!(!island.transfer_is_direct());
    }

    fn shelf_files() -> Vec<String> {
        vec![r"C:\a\bir.txt".to_owned(), r"C:\a\iki.png".to_owned()]
    }

    #[test]
    fn dragging_files_opens_the_drop_panel_over_anything() {
        let mut island = Island::new();
        island.upsert(Activity { level: Some(40), ..activity(Kind::Alert) });
        island.drag_over(drop_view());
        let frame = island.frame(0);
        assert_eq!(frame.presentation, Presentation::Expanded);
        assert!(matches!(frame.panel, Some(PanelView::Drop(_))));
        assert!(frame.activity.is_none());
        island.drag_ended();
        assert_eq!(island.frame(0).presentation, Presentation::Level);
    }

    #[test]
    fn the_shelf_shows_what_landed_then_steps_aside() {
        let mut island = Island::new();
        island.drag_over(drop_view());
        assert_eq!(island.drop_on_shelf(&shelf_files(), 0).len(), 2);
        let frame = island.frame(0);
        match frame.panel {
            Some(PanelView::Shelf(view)) => assert_eq!(view.total, 2),
            other => panic!("{other:?}"),
        }
        assert_eq!(frame.wake_at, Some(DROP_VIEW_MS));
        assert_eq!(island.frame(DROP_VIEW_MS).presentation, Presentation::Hidden);
    }

    #[test]
    fn the_shelf_is_a_bubble_beside_whatever_shows() {
        let mut island = Island::new();
        island.drop_on_shelf(&shelf_files(), 0);
        island.close_views();
        island.peek(10);
        let frame = island.frame(10);
        assert_eq!(frame.presentation, Presentation::Compact);
        assert_eq!(second(&frame), Some(Kind::Shelf));
        // A second lasting activity has the bubble first.
        island.upsert(activity(Kind::Media));
        island.set_camera(Some("Teams".into()));
        assert_eq!(second(&island.frame(20)), Some(Kind::Camera));
    }

    #[test]
    fn clicking_the_shelf_bubble_opens_the_shelf() {
        let mut island = Island::new();
        island.drop_on_shelf(&shelf_files(), 0);
        island.close_views();
        island.upsert(activity(Kind::Media));
        assert_eq!(second(&island.frame(10)), Some(Kind::Shelf));
        island.click_secondary(10);
        assert!(matches!(island.frame(10).panel, Some(PanelView::Shelf(_))));
        island.click(20);
        assert!(island.frame(20).panel.is_none());
    }

    #[test]
    fn emptying_the_shelf_closes_it() {
        let mut island = Island::new();
        island.drop_on_shelf(&shelf_files(), 0);
        island.remove_from_shelf(0);
        assert!(island.frame(0).panel.is_some());
        island.clear_shelf();
        let frame = island.frame(0);
        assert!(frame.panel.is_none());
        assert!(frame.secondary.is_none());
    }

    #[test]
    fn the_convert_choice_waits_while_it_works() {
        let mut island = Island::new();
        island.open_convert("a.png · 1 KB".into(), 0);
        island.set_converting(true);
        match island.frame(0).panel {
            Some(PanelView::Convert(view)) => assert!(view.busy),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_code_stays_open_while_the_user_is_busy_with_the_phone() {
        let mut island = Island::new();
        island.start_transfer(transfer());
        island.set_hovered(true, 0);
        island.set_hovered(false, 100);
        let frame = island.frame(10_000);
        assert_eq!(frame.presentation, Presentation::Expanded);
        assert_eq!(frame.activity.map(|a| a.kind), Some(Kind::Transfer));
        // The countdown on screen moves once a second.
        assert_eq!(frame.wake_at, Some(11_000));
    }

    #[test]
    fn a_finished_transfer_says_so_and_closes() {
        let mut island = Island::new();
        island.upsert(activity(Kind::Media));
        island.start_transfer(transfer());
        island.transfer_progress(500);
        let sending = island.frame(1_000);
        assert_eq!(sending.activity.as_ref().map(|a| a.trailing.as_str()), Some("%50"));
        assert_eq!(sending.wake_at, None);
        island.transfer_done(2_000);
        let done = island.frame(2_000);
        assert_eq!(done.activity.map(|a| a.leading), Some("Gönderildi".to_owned()));
        assert_eq!(done.wake_at, Some(2_000 + DONE_MS));
        let after = island.frame(2_000 + DONE_MS);
        assert_eq!(after.presentation, Presentation::Compact);
        assert_eq!(after.activity.map(|a| a.kind), Some(Kind::Media));
        assert_eq!(island.transfer_stage(), None);
    }

    #[test]
    fn an_unanswered_code_gives_up() {
        let mut island = Island::new();
        island.start_transfer(transfer());
        assert_eq!(island.frame(WAIT_MS).presentation, Presentation::Hidden);
        assert_eq!(island.transfer_stage(), None);
    }

    #[test]
    fn a_download_that_broke_off_waits_again() {
        let mut island = Island::new();
        island.start_transfer(transfer());
        island.transfer_progress(100);
        island.transfer_broke_off(60_000);
        assert_eq!(island.transfer_stage(), Some(Stage::Waiting { until: 60_000 + WAIT_MS }));
    }

    #[test]
    fn stopping_the_transfer_closes_its_view() {
        let mut island = Island::new();
        island.start_transfer(transfer());
        island.cancel_transfer();
        assert_eq!(island.frame(0).presentation, Presentation::Hidden);
    }

    /// Music playing and a code that was shown and then closed.
    fn music_and_a_closed_code(layout: TaskLayout) -> Island {
        let mut island = Island::new();
        island.set_task_layout(layout);
        island.upsert_media(snapshot(true), 0);
        island.start_transfer(transfer());
        island.click(0);
        island
    }

    fn bubble(frame: &Frame) -> Option<Bubble> {
        frame.secondary.as_ref().map(|a| a.bubble())
    }

    #[test]
    fn by_default_music_keeps_the_pill_and_the_send_waits_in_the_bubble() {
        let mut island = music_and_a_closed_code(TaskLayout::Bubble);
        let frame = island.frame(0);
        assert_eq!(frame.presentation, Presentation::Compact);
        assert_eq!(shown(&frame), Some(Kind::Media));
        assert_eq!(bubble(&frame), Some(Bubble::Symbol(Icon::Phone)));
        assert!(frame.companion.is_none());
        // The time left is not on screen, so nothing wakes every second.
        assert_eq!(frame.wake_at, Some(WAIT_MS));
        // The bubble opens the code again.
        island.click_secondary(0);
        let open = island.frame(0);
        assert_eq!(open.presentation, Presentation::Expanded);
        assert_eq!(shown(&open), Some(Kind::Transfer));
    }

    #[test]
    fn the_send_has_the_bubble_before_other_lasting_things() {
        let mut island = music_and_a_closed_code(TaskLayout::Bubble);
        island.start_timer(25 * MINUTE, 0);
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::Media));
        assert_eq!(second(&frame), Some(Kind::Transfer));
    }

    #[test]
    fn alone_the_send_has_the_pill_and_counts_down_there() {
        let mut island = Island::new();
        island.start_transfer(transfer());
        island.click(0);
        let frame = island.frame(1_500);
        assert_eq!(frame.presentation, Presentation::Compact);
        assert_eq!(shown(&frame), Some(Kind::Transfer));
        assert_eq!(frame.activity.as_ref().map(|a| a.trailing.as_str()), Some("4:59"));
        assert_eq!(frame.wake_at, Some(2_000));
    }

    #[test]
    fn in_front_the_send_takes_the_pill_and_music_the_bubble() {
        let mut island = music_and_a_closed_code(TaskLayout::Front);
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::Transfer));
        assert_eq!(second(&frame), Some(Kind::Media));
        assert_eq!(frame.wake_at, Some(1_000));
    }

    #[test]
    fn side_by_side_the_send_rides_in_the_music_pill() {
        let mut island = music_and_a_closed_code(TaskLayout::Shared);
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::Media));
        assert_eq!(frame.companion.as_ref().map(|a| a.kind), Some(Kind::Transfer));
        assert_eq!(second(&frame), None);
        // The bubble stays free for something else that lasts.
        island.start_timer(25 * MINUTE, 0);
        assert_eq!(second(&island.frame(0)), Some(Kind::TimerRunning));
        // A click on the send's side of the pill opens the code again, with
        // the music under it.
        island.click_companion();
        let open = island.frame(0);
        assert_eq!(open.presentation, Presentation::Expanded);
        assert_eq!(shown(&open), Some(Kind::Transfer));
        assert_eq!(open.companion.as_ref().map(|a| a.kind), Some(Kind::Media));
        assert_eq!(second(&open), None);
    }

    #[test]
    fn changing_layout_updates_a_running_closed_send() {
        let mut island = music_and_a_closed_code(TaskLayout::Bubble);
        assert_eq!(shown(&island.frame(0)), Some(Kind::Media));
        assert_eq!(second(&island.frame(0)), Some(Kind::Transfer));

        island.set_task_layout(TaskLayout::Shared);
        let shared = island.frame(0);
        assert_eq!(shown(&shared), Some(Kind::Media));
        assert_eq!(shared.companion.as_ref().map(|a| a.kind), Some(Kind::Transfer));
        assert_eq!(second(&shared), None);

        island.set_task_layout(TaskLayout::Front);
        let front = island.frame(0);
        assert_eq!(shown(&front), Some(Kind::Transfer));
        assert_eq!(second(&front), Some(Kind::Media));
    }

    #[test]
    fn changing_layout_updates_music_and_focus() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.set_focus(focus(25 * MINUTE, 25 * MINUTE), 0);

        let bubble = island.frame(0);
        assert_eq!(shown(&bubble), Some(Kind::Media));
        assert_eq!(second(&bubble), Some(Kind::Focus));

        island.set_task_layout(TaskLayout::Shared);
        let shared = island.frame(0);
        assert_eq!(shown(&shared), Some(Kind::Media));
        assert_eq!(shared.companion.as_ref().map(|a| a.kind), Some(Kind::Focus));
        assert_eq!(second(&shared), None);

        island.set_task_layout(TaskLayout::Front);
        let front = island.frame(0);
        assert_eq!(shown(&front), Some(Kind::Focus));
        assert_eq!(second(&front), Some(Kind::Media));
    }

    #[test]
    fn side_by_side_without_music_the_send_keeps_to_the_bubble() {
        let mut island = Island::new();
        island.set_task_layout(TaskLayout::Shared);
        island.start_timer(25 * MINUTE, 0);
        island.start_transfer(transfer());
        // The open code has no music to show under it, so the timer stays beside.
        assert_eq!(second(&island.frame(0)), Some(Kind::TimerRunning));
        island.click(0);
        let frame = island.frame(0);
        assert_eq!(shown(&frame), Some(Kind::TimerRunning));
        assert!(frame.companion.is_none());
        assert_eq!(second(&frame), Some(Kind::Transfer));
    }

    #[test]
    fn the_open_code_keeps_music_in_view() {
        for layout in [TaskLayout::Bubble, TaskLayout::Front] {
            let mut island = Island::new();
            island.set_task_layout(layout);
            island.upsert_media(snapshot(true), 0);
            island.start_transfer(transfer());
            let frame = island.frame(0);
            assert_eq!(frame.presentation, Presentation::Expanded, "{layout:?}");
            assert_eq!(shown(&frame), Some(Kind::Transfer), "{layout:?}");
            assert_eq!(second(&frame), Some(Kind::Media), "{layout:?}");
        }
    }

    #[test]
    fn volume_goes_in_the_bubble_while_the_code_is_open() {
        let mut island = Island::new();
        island.upsert_media(snapshot(true), 0);
        island.start_transfer(transfer());
        island.upsert(Activity { level: Some(40), icon: Some(Icon::Volume), expires_at: Some(1_500), ..activity(Kind::Alert) });
        let frame = island.frame(0);
        assert_eq!(frame.presentation, Presentation::Expanded);
        assert_eq!(shown(&frame), Some(Kind::Transfer));
        assert_eq!(bubble(&frame), Some(Bubble::Count { value: "40".into(), icon: Some(Icon::Volume), ring: Some(40) }));
        // A click on it opens nothing.
        island.click_secondary(0);
        assert_eq!(shown(&island.frame(0)), Some(Kind::Transfer));
        // Once it is gone, music is back in the bubble.
        assert_eq!(second(&island.frame(1_500)), Some(Kind::Media));
    }

    #[test]
    fn volume_goes_in_the_bubble_while_the_shelf_is_open() {
        let mut island = Island::new();
        island.drop_on_shelf(&shelf_files(), 0);
        island.upsert(Activity { level: Some(70), icon: Some(Icon::Brightness), ..activity(Kind::Alert) });
        let frame = island.frame(0);
        assert_eq!(frame.presentation, Presentation::Expanded);
        assert!(matches!(frame.panel, Some(PanelView::Shelf(_))));
        assert_eq!(bubble(&frame), Some(Bubble::Count { value: "70".into(), icon: Some(Icon::Brightness), ring: Some(70) }));
    }

    #[test]
    fn volume_still_covers_a_closed_pill() {
        let mut island = music_and_a_closed_code(TaskLayout::Bubble);
        island.upsert(Activity { level: Some(40), icon: Some(Icon::Volume), ..activity(Kind::Alert) });
        assert_eq!(island.frame(0).presentation, Presentation::Level);
    }

    #[test]
    fn a_finished_send_ticks_in_the_bubble_then_leaves() {
        let mut island = music_and_a_closed_code(TaskLayout::Bubble);
        island.transfer_progress(500);
        assert_eq!(bubble(&island.frame(0)), Some(Bubble::Count { value: "50".into(), icon: None, ring: Some(50) }));
        island.transfer_done(1_000);
        let done = island.frame(1_000);
        assert_eq!(shown(&done), Some(Kind::Media));
        assert_eq!(bubble(&done), Some(Bubble::Symbol(Icon::Check)));
        assert_eq!(second(&island.frame(1_000 + DONE_MS)), None);
    }
}
