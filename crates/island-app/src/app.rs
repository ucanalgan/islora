use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use island_core::clock_timer::{self, ClockTimer};
use island_core::config;
use island_core::focus::{
    CHANGE_WAIT_MS, FocusReport, FocusStatus, FocusTracker, START_WAIT_MS,
};
use island_core::ics::{self, Event};
use island_core::media::RepeatMode;
use island_core::layout::{
    TIMER_PRESETS, on_event_hide, on_home_shortcut, on_progress_bar, preset_at, progress_fraction,
};
use island_core::{
    Activity, CalendarState, FocusView, Frame, Indicator, Island, IslandStyle, Kind, MediaButton,
    MediaSnapshot, Millis, Pill, Presentation, StartMode, Suppression, SystemTracker, TimerButton,
    Upcoming,
};
use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::Shell::ABN_FULLSCREENAPP;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Result, w};

use crate::platform::tray::{self, Command, MSG_TRAY, MenuState, Tray};
use crate::platform::window::{self, Placement};
use crate::platform::{autostart, hotkey, settings};
use crate::render::{self, ClockText, Renderer, WaveMotion};
use crate::sources::audio::{AudioWatch, DEVICE_MICROPHONE, MSG_AUDIO_DEVICE};
use crate::sources::bluetooth::BluetoothWatch;
use crate::sources::calendar::{CalendarSource, CalendarUpdate, MSG_CALENDAR, unix_now};
use crate::sources::clock_timers::{ClockTimerWatch, MSG_CLOCK_TIMERS};
use crate::sources::drives::Drives;
use crate::sources::focus::{self, FocusWatch, MSG_FOCUS, MSG_FOCUS_REFUSED};
use crate::sources::fullscreen::{self, MSG_APPBAR, MSG_WINDOW_DRAG, Presence};
use crate::sources::media::{MSG_MEDIA, MediaSource};
use crate::sources::power::{self, PowerEvent, PowerWatch};
use crate::sources::privacy::{CameraWatch, MSG_CAMERA};
use crate::sources::keys::KeyEvent;
use crate::sources::{IndicatorSink, MSG_INDICATOR, clipboard, keys, network, screenshots};

// Lives in `Win32::UI::Controls`, a large feature to enable for one constant.
const WM_MOUSELEAVE: u32 = 0x02A3;

const TIMER_WAKE: usize = 1;
const TIMER_SETTLE: usize = 2;
const TIMER_HIDE: usize = 3;
const TIMER_EDGE: usize = 4;
const TIMER_DEMO: usize = 5;
const TIMER_MEDIA: usize = 6;
const TIMER_INDICATOR_DEMO: usize = 8;
const TIMER_FOCUS_CHECK: usize = 9;
const TIMER_CLOCK: usize = 10;
const TIMER_LANGUAGE: usize = 11;

/// When the pill looks settled. The compositor's own completion event only
/// fires about 1.2 s after an animation starts, long after it visibly stopped.
const SETTLE_MS: u32 = 520;
/// How long the pill takes to shrink away before the window hides.
const HIDE_MS: u32 = 380;
/// How long the pointer must rest on the top edge, so that passing over it to
/// reach a browser tab does not open anything.
const EDGE_DWELL_MS: u32 = 120;
const DEMO_STEP_MS: u32 = 3_500;
const DEMO_STEPS: u32 = 8;
const INDICATOR_DEMO_STEP_MS: u32 = 1_500;
/// Running time only needs repainting while it is on screen, once a second.
const MEDIA_TICK_MS: u32 = 1_000;
const USER_TIMER_MINIMUM_MS: Millis = 10;
/// A waking laptop reconnects by itself, which needs no announcing.
const NETWORK_QUIET_AFTER_WAKE_MS: Millis = 20_000;
/// Apps often write the clipboard several times for one copy.
const CLIPBOARD_QUIET_MS: Millis = 800;
/// A screenshot lands on the clipboard too, around when its file appears.
const CLIPBOARD_QUIET_AFTER_SCREENSHOT_MS: Millis = 3_000;
/// Windows switches language a moment after the shortcut's last key comes up.
const LANGUAGE_SETTLE_MS: u32 = 300;

const NOTIFICATION_MS: Millis = 3_000;

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app unless it is already borrowed. Win32 delivers some
/// messages synchronously from inside calls the app itself makes.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(f))
}

/// What the expanded pill shows, which decides what a click on it means.
#[derive(Clone, Copy, PartialEq)]
enum OpenView {
    Media,
    Timer,
    Focus,
    Event,
    Home,
    Other,
}

struct App {
    island_window: HWND,
    edge_window: HWND,
    placement: Placement,
    island: Island,
    renderer: Renderer,
    tray: Tray,
    _presence: Presence,
    media: MediaSource,
    calendar: CalendarSource,
    events: Vec<Event>,
    focus_watch: FocusWatch,
    focus_state: FocusTracker,
    /// Length of the session on screen, kept for while it is paused.
    focus_total: Millis,
    clock_timer_watch: ClockTimerWatch,
    clock_timers: Vec<ClockTimer>,
    camera_watch: CameraWatch,
    drives: Drives,
    /// Names of the notices the user turned off.
    notices_off: Vec<String>,
    /// Until when news about the network is left unsaid.
    network_quiet_until: Millis,
    /// Until when a change of the clipboard is left unsaid.
    clipboard_quiet_until: Millis,
    /// One preset step in milliseconds. Tests shorten it from minutes to seconds.
    preset_unit: Millis,
    tracker: SystemTracker,
    indicators: Receiver<Indicator>,
    audio: Option<AudioWatch>,
    _power: Option<PowerWatch>,
    _bluetooth: Option<BluetoothWatch>,
    started: Instant,
    shown: Presentation,
    window_visible: bool,
    pointer_inside: bool,
    open_view: OpenView,
    shown_bubble: bool,
    /// Length of the track on screen, if the player accepts jumps within it.
    seekable_ms: Option<u64>,
    /// The file behind the alert on screen, which a click on it opens.
    alert_file: Option<String>,
    /// Which of the two optional buttons the player on screen offers.
    offers_shuffle: bool,
    offers_repeat: bool,
    /// Where the position bar is being dragged to, from 0 to 1.
    scrub: Option<f32>,
    demo_step: Option<u32>,
    indicator_demo_step: Option<usize>,
}

pub fn run() -> Result<()> {
    let _single_instance = unsafe { CreateMutexW(None, true, w!("Local\\Islora"))? };
    // A look at a new build, beside the copy the user keeps running.
    let second_copy = std::env::args().any(|a| a == "--second-copy");
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS && !second_copy {
        return Ok(());
    }

    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)? };
    TASKBAR_CREATED.store(unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }, Ordering::Relaxed);

    let _dispatcher = render::create_dispatcher_queue()?;
    let island_window = window::create_island(Some(island_proc))?;
    // Tests read the screen, and need to see what they are checking.
    if !std::env::args().any(|a| a == "--capturable") {
        window::exclude_from_capture(island_window);
    }
    let edge_window = window::create_edge(Some(edge_proc))?;
    let placement = window::place(island_window, edge_window, stored_style())?;
    let wave_motion = match argument_after("--wave").as_deref() {
        Some("still") => WaveMotion::Still,
        _ => WaveMotion::Always,
    };
    let renderer = Renderer::new(island_window, placement.scale, placement.width, placement.style, wave_motion)?;

    if !hotkey::register(island_window) {
        eprintln!("no global shortcut available, only the top edge will open the island");
    }
    let tray = Tray::add(island_window)?;
    let presence = Presence::start(island_window)?;

    let mut island = Island::new();
    if std::env::args().any(|a| a == "--local-timers") {
        island.offer_only_timers();
    }
    // Before the calendar thread, which reads the same file.
    apply_settings(&mut island);
    let fullscreen = fullscreen::fullscreen_now();
    island.set_suppressed(Suppression::Fullscreen, fullscreen);
    if !fullscreen {
        window::show(edge_window);
    }

    let (sink, indicators) = IndicatorSink::channel(island_window);
    let mut audio = optional("volume", AudioWatch::start(island_window, sink.clone()));
    let camera_watch = CameraWatch::start(island_window, sink.clone());
    network::start(sink.clone());
    screenshots::start(sink.clone());
    let bluetooth = optional("bluetooth", BluetoothWatch::start(sink));
    optional("lock keys", keys::start(island_window));
    optional("clipboard", clipboard::start(island_window));

    let mut tracker = SystemTracker::new();
    if let Some(name) = audio.as_mut().and_then(AudioWatch::follow_default_device) {
        tracker.audio_output_changed(name);
    }
    tracker.monitors_changed(monitor_count());
    if let Some(code) = keys::language() {
        tracker.keyboard_language_changed(code);
    }

    let started = Instant::now();
    let mut app = App {
        island_window,
        edge_window,
        placement,
        island,
        renderer,
        tray,
        _presence: presence,
        media: MediaSource::start(island_window, started, argument_after("--only-session")),
        calendar: CalendarSource::start(island_window, argument_after("--calendar-file")),
        events: Vec::new(),
        focus_watch: FocusWatch::start(island_window),
        focus_state: FocusTracker::default(),
        focus_total: 1,
        clock_timer_watch: ClockTimerWatch::start(island_window),
        clock_timers: Vec::new(),
        camera_watch,
        drives: Drives::default(),
        notices_off: stored_notices_off(),
        network_quiet_until: 0,
        clipboard_quiet_until: 0,
        preset_unit: if std::env::args().any(|a| a == "--short-timers") { 1_000 } else { 60_000 },
        tracker,
        indicators,
        audio,
        _power: None,
        _bluetooth: bluetooth,
        started,
        shown: Presentation::Hidden,
        window_visible: false,
        pointer_inside: false,
        open_view: OpenView::Other,
        shown_bubble: false,
        seekable_ms: None,
        alert_file: None,
        offers_shuffle: false,
        offers_repeat: false,
        scrub: None,
        demo_step: None,
        indicator_demo_step: None,
    };
    if std::env::args().any(|a| a == "--demo") {
        app.toggle_demo();
    }
    if std::env::args().any(|a| a == "--demo-indicators") {
        app.indicator_demo_step = Some(0);
        app.advance_indicator_demo();
        app.set_timer(TIMER_INDICATOR_DEMO, INDICATOR_DEMO_STEP_MS);
    }
    APP.with(|cell| *cell.borrow_mut() = Some(app));

    // Windows answers this with the current state, which is what tells a real
    // change from then on. The app must be there to hear it, or the first time
    // the charger is plugged in would pass for that report and show nothing.
    let power = optional("power", PowerWatch::start(island_window));
    with_app(|app| app._power = power);

    unsafe {
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    hotkey::unregister(island_window);
    APP.with(|cell| cell.borrow_mut().take());
    Ok(())
}

impl App {
    fn now(&self) -> Millis {
        self.started.elapsed().as_millis() as Millis
    }

    /// Brings the screen in line with the state machine, then sleeps until
    /// its next deadline. This is the only path that changes what is shown.
    fn refresh(&mut self) {
        let now = self.now();
        let frame = self.island.frame(now);
        if let Err(error) = self.present(&frame, now) {
            eprintln!("present failed: {error}");
        }
        self.arm_wake(frame.wake_at, now);
        self.arm_media_tick(&frame);
        self.arm_clock_tick(&frame);
        if frame.chime {
            unsafe {
                let _ = MessageBeep(MB_ICONASTERISK);
            }
        }
    }

    /// The clock and the home view show the time, which stays on screen for as
    /// long as the pointer rests there. Nothing else would update it.
    fn arm_clock_tick(&self, frame: &Frame) {
        if frame.activity.is_none() && frame.presentation != Presentation::Hidden {
            let time = unsafe { GetLocalTime() };
            let into_minute = time.wSecond as u32 * 1_000 + time.wMilliseconds as u32;
            self.set_timer(TIMER_CLOCK, 60_000 - into_minute);
        } else {
            self.cancel_timer(TIMER_CLOCK);
        }
    }

    fn arm_media_tick(&mut self, frame: &Frame) {
        let media = frame.activity.as_ref().and_then(|activity| activity.media.as_ref());
        self.open_view = match &frame.activity {
            _ if frame.home.is_some() => OpenView::Home,
            Some(activity) if activity.media.is_some() => OpenView::Media,
            Some(activity) if activity.kind == Kind::Focus => OpenView::Focus,
            Some(activity) if activity.kind == Kind::Upcoming => OpenView::Event,
            Some(activity) if activity.timer.is_some_and(|timer| timer.has_controls) => OpenView::Timer,
            _ => OpenView::Other,
        };
        self.seekable_ms = media.filter(|media| media.can_seek && media.duration_ms > 0).map(|media| media.duration_ms);
        self.alert_file = frame.activity.as_ref().and_then(|activity| activity.file.clone());
        self.offers_shuffle = media.is_some_and(|media| media.shuffle.is_some());
        self.offers_repeat = media.is_some_and(|media| media.repeat.is_some());
        let expanded = frame.presentation == Presentation::Expanded;
        if expanded && media.is_some_and(|media| media.playing) {
            self.set_timer(TIMER_MEDIA, MEDIA_TICK_MS);
        } else {
            self.cancel_timer(TIMER_MEDIA);
        }
    }

    fn present(&mut self, frame: &Frame, now: Millis) -> Result<()> {
        let presentation = frame.presentation;
        let appearing = presentation != Presentation::Hidden && !self.window_visible;
        if presentation == Presentation::Hidden && !self.window_visible {
            return Ok(());
        }

        if appearing {
            self.renderer.snap_to_hidden()?;
            window::show(self.island_window);
            self.window_visible = true;
        }
        let resized = self.renderer.present(frame, &clock_text(), now, self.scrub)?;
        let with_bubble = frame.secondary.is_some();
        if resized || presentation != self.shown || with_bubble != self.shown_bubble {
            self.shown_bubble = with_bubble;
            window::open_region(self.island_window);
            self.cancel_timer(TIMER_HIDE);
            let (timer, delay) = match presentation {
                Presentation::Hidden => (TIMER_HIDE, HIDE_MS),
                _ => (TIMER_SETTLE, SETTLE_MS),
            };
            self.set_timer(timer, delay);
        }
        self.shown = presentation;
        Ok(())
    }

    fn arm_wake(&self, wake_at: Option<Millis>, now: Millis) {
        match wake_at {
            Some(at) => {
                let delay = at.saturating_sub(now).max(USER_TIMER_MINIMUM_MS);
                self.set_timer(TIMER_WAKE, delay.min(u32::MAX as Millis) as u32);
            }
            None => self.cancel_timer(TIMER_WAKE),
        }
    }

    fn set_timer(&self, id: usize, delay_ms: u32) {
        unsafe { SetTimer(Some(self.island_window), id, delay_ms, None) };
    }

    fn cancel_timer(&self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.island_window), id);
        }
    }

    fn on_timer(&mut self, id: usize) {
        match id {
            TIMER_WAKE => {
                self.cancel_timer(TIMER_WAKE);
                self.refresh();
            }
            TIMER_SETTLE => {
                self.cancel_timer(TIMER_SETTLE);
                if self.shown != Presentation::Hidden {
                    self.settle_region();
                }
            }
            TIMER_HIDE => {
                self.cancel_timer(TIMER_HIDE);
                self.finish_hiding();
            }
            TIMER_EDGE => {
                self.cancel_timer(TIMER_EDGE);
                if self.pointer_on_edge() {
                    self.island.peek(self.now());
                    self.refresh();
                }
            }
            TIMER_DEMO => self.advance_demo(),
            TIMER_MEDIA | TIMER_CLOCK => self.refresh(),
            TIMER_INDICATOR_DEMO => self.advance_indicator_demo(),
            TIMER_FOCUS_CHECK => {
                self.cancel_timer(TIMER_FOCUS_CHECK);
                self.focus_watch.recheck();
            }
            TIMER_LANGUAGE => {
                self.cancel_timer(TIMER_LANGUAGE);
                self.check_language();
            }
            _ => {}
        }
    }

    fn settle_region(&self) {
        let pill = self.renderer.pill_for(self.shown);
        if !self.renderer.has_bubble() {
            window::set_pill_region(self.island_window, pill, &self.placement);
            return;
        }
        let bubble = Pill::BUBBLE.scaled(self.placement.scale);
        let bubble_left = self.renderer.bubble_left(pill);
        let pill_left = bubble_left - island_core::layout::BUBBLE_GAP * self.placement.scale - pill.width;
        window::set_group_region(self.island_window, pill, pill_left, bubble, bubble_left, &self.placement);
    }

    fn finish_hiding(&mut self) {
        window::hide(self.island_window);
        self.window_visible = false;
        self.pointer_inside = false;
        if let Err(error) = self.renderer.release_content() {
            eprintln!("release failed: {error}");
        }
    }

    fn pointer_on_edge(&self) -> bool {
        let mut cursor = POINT::default();
        unsafe { GetCursorPos(&mut cursor) }.is_ok_and(|_| {
            let edge = self.placement.edge;
            cursor.x >= edge.left && cursor.x < edge.right && cursor.y < edge.bottom
        })
    }

    fn on_button_down(&mut self, x: i32, y: i32) {
        let now = self.now();
        let pill = self.renderer.pill_for(self.shown);
        if self.renderer.on_bubble(x, y, pill) {
            self.island.click_secondary(now);
            self.refresh();
            return;
        }
        if let Some(file) = self.alert_file.take() {
            screenshots::open(file);
        }
        if self.shown == Presentation::Expanded {
            let (pill_x, pill_y) = self.to_expanded_pill(x, y);
            if self.open_view != OpenView::Home && on_home_shortcut(pill_x, pill_y) {
                self.island.open_home();
                self.refresh();
                return;
            }
            match self.open_view {
                OpenView::Media => {
                    if self.seekable_ms.is_some() && on_progress_bar(pill_x, pill_y) {
                        self.scrub = Some(progress_fraction(pill_x));
                        // Keeps the drag going when the pointer strays off the pill.
                        unsafe { SetCapture(self.island_window) };
                        self.refresh();
                        return;
                    }
                    let button = MediaButton::at(pill_x, pill_y).filter(|button| match button {
                        MediaButton::Shuffle => self.offers_shuffle,
                        MediaButton::Repeat => self.offers_repeat,
                        _ => true,
                    });
                    if let Some(button) = button {
                        self.media.press(button);
                        return;
                    }
                }
                OpenView::Timer => {
                    match TimerButton::at(pill_x, pill_y) {
                        Some(TimerButton::Toggle) => self.island.toggle_timer(now),
                        Some(TimerButton::Cancel) => self.island.cancel_timer(),
                        None => self.island.click(now),
                    }
                    self.refresh();
                    return;
                }
                OpenView::Focus => {
                    match TimerButton::at(pill_x, pill_y) {
                        Some(TimerButton::Toggle) => self.toggle_focus(),
                        Some(TimerButton::Cancel) => self.end_focus(),
                        None => {
                            self.island.click(now);
                            self.refresh();
                        }
                    }
                    return;
                }
                OpenView::Event => {
                    if on_event_hide(pill_x, pill_y) {
                        self.island.hide_event();
                        self.refresh();
                        return;
                    }
                }
                OpenView::Home => {
                    if let Some(index) = preset_at(pill_x, pill_y) {
                        self.start_preset(index, now);
                        return;
                    }
                    if let Some(mode) = StartMode::at(pill_x, pill_y) {
                        if self.island.set_start_mode(mode) {
                            settings::remember("start_mode", mode.setting());
                            self.refresh();
                        }
                        return;
                    }
                }
                OpenView::Other => {}
            }
        }
        if let Err(error) = self.renderer.press() {
            eprintln!("press failed: {error}");
        }
        self.island.click(now);
        self.refresh();
    }

    fn start_preset(&mut self, index: usize, now: Millis) {
        let minutes = self.island.presets()[index];
        match self.island.start_mode() {
            StartMode::Focus => {
                focus::start_session(self.island_window, minutes);
                // The Clock app can take seconds to start, and this must feel instant.
                self.focus_state.expect_start((minutes * 60_000) as i64, unix_now());
                self.set_timer(TIMER_FOCUS_CHECK, START_WAIT_MS as u32);
                self.island.close_views();
                self.load_focus();
            }
            StartMode::Timer => {
                self.island.start_timer(minutes * self.preset_unit, now);
                self.refresh();
            }
        }
    }

    /// No Clock app to run sessions, so the island counts by itself from now on.
    fn on_focus_refused(&mut self, minutes: Millis) {
        let now = self.now();
        self.cancel_timer(TIMER_FOCUS_CHECK);
        self.focus_state = FocusTracker::default();
        self.island.offer_only_timers();
        self.island.set_focus(FocusView::Off, now);
        self.island.start_timer(minutes * self.preset_unit, now);
        self.refresh();
    }

    /// Shows at once what a command will do. Windows has a moment to agree,
    /// after which it is asked, in case the Clock app ignored the command.
    fn expect_focus(&mut self, status: FocusStatus) {
        self.focus_state.expect_change(status, unix_now());
        self.set_timer(TIMER_FOCUS_CHECK, CHANGE_WAIT_MS as u32);
        self.load_focus();
    }

    fn toggle_focus(&mut self) {
        let wall_now = unix_now();
        match self.focus_state.status() {
            FocusStatus::Paused { remaining_ms, on_break } => {
                focus::resume_session();
                let ends = wall_now + remaining_ms as i64;
                let started = ends - self.focus_total as i64;
                self.expect_focus(FocusStatus::Running { started, ends, on_break });
            }
            FocusStatus::Running { ends, on_break, .. } => {
                focus::pause_session();
                let remaining_ms = (ends - wall_now).max(0) as u64;
                self.expect_focus(FocusStatus::Paused { remaining_ms, on_break });
            }
            _ => focus::pause_session(),
        }
    }

    fn end_focus(&mut self) {
        focus::end_session();
        self.island.close_views();
        self.expect_focus(FocusStatus::Idle);
    }

    fn on_calendar(&mut self) {
        let Some(update) = self.calendar.latest() else { return };
        let state = match update {
            CalendarUpdate::Events(events) => {
                self.events = events;
                CalendarState::Ready
            }
            CalendarUpdate::NoAddress => CalendarState::NotSet,
            CalendarUpdate::Failed => CalendarState::Unreachable,
        };
        self.island.set_calendar_state(state);
        self.load_schedule();
    }

    fn on_clock_timers(&mut self) {
        if let Some(timers) = self.clock_timer_watch.latest() {
            self.clock_timers = timers;
            self.load_clock_timer();
        }
    }

    /// Stored in wall clock time for the same reason as the events below.
    fn load_clock_timer(&mut self) {
        let (now, wall_now) = (self.now(), unix_now());
        let timer = clock_timer::current(&self.clock_timers, wall_now)
            .and_then(|timer| Some((timer.countdown(now, wall_now)?, timer.name.clone())));
        self.island.set_clock_timer(timer, now);
        self.refresh();
    }

    /// Events are stored in wall clock time and mapped to the island's clock
    /// here, which also keeps them right after the laptop has been asleep.
    fn load_schedule(&mut self) {
        let (now, wall_now) = (self.now(), unix_now());
        let upcoming = self
            .events
            .iter()
            .filter(|event| event.start > wall_now)
            .filter_map(|event| {
                let (time, when) = ics::local_labels(event.start, wall_now)?;
                Some(Upcoming {
                    start: now + (event.start - wall_now) as Millis,
                    wall_start: event.start,
                    title: event.title.clone(),
                    location: event.location.clone(),
                    time,
                    when,
                })
            })
            .collect();
        self.island.set_schedule(upcoming, now);
        self.refresh();
    }

    fn on_focus(&mut self) {
        let Some(status) = self.focus_watch.latest() else { return };
        match self.focus_state.report(status, unix_now()) {
            FocusReport::TooEarly => return,
            FocusReport::Taken => self.load_focus(),
            // The Clock app took the link and did nothing with it. The time the
            // user asked for still runs, here on the island.
            FocusReport::NeverStarted { remaining_ms } => {
                self.load_focus();
                self.island.start_timer(remaining_ms, self.now());
                self.refresh();
            }
        }
        self.cancel_timer(TIMER_FOCUS_CHECK);
    }

    fn load_focus(&mut self) {
        let (now, wall_now) = (self.now(), unix_now());
        let view = match self.focus_state.status() {
            FocusStatus::Idle => FocusView::Off,
            FocusStatus::Open => FocusView::Open,
            FocusStatus::Running { ends, .. } if ends <= wall_now => FocusView::Off,
            FocusStatus::Running { started, ends, on_break } => {
                self.focus_total = (ends - started) as Millis;
                FocusView::Running {
                    ends_at: now + (ends - wall_now) as Millis,
                    total: self.focus_total,
                    on_break,
                }
            }
            FocusStatus::Paused { remaining_ms, on_break } => FocusView::Paused {
                remaining: remaining_ms,
                total: self.focus_total.max(remaining_ms),
                on_break,
            },
        };
        self.island.set_focus(view, now);
        self.refresh();
    }

    fn on_time_changed(&mut self) {
        self.load_focus();
        self.load_clock_timer();
        self.load_schedule();
    }

    fn on_resume(&mut self) {
        self.network_quiet_until = self.now() + NETWORK_QUIET_AFTER_WAKE_MS;
        if self.focus_state.status() != FocusStatus::Idle {
            self.focus_watch.recheck();
        }
        self.on_time_changed();
        self.calendar.refresh_if_stale();
    }

    fn is_turned_off(&self, notice: &str) -> bool {
        self.notices_off.iter().any(|name| name == notice)
    }

    fn on_camera(&mut self) {
        let Some(app) = self.camera_watch.latest() else { return };
        self.island.set_camera(app.filter(|_| !self.is_turned_off("camera")));
        self.refresh();
    }

    fn check_language(&mut self) {
        if let Some(indicator) = keys::language().and_then(|code| self.tracker.keyboard_language_changed(code)) {
            self.show_indicator(indicator);
        }
    }

    fn on_clipboard(&mut self) {
        let now = self.now();
        if now < self.clipboard_quiet_until {
            return;
        }
        if let Some(indicator) = clipboard::copied() {
            self.clipboard_quiet_until = now + CLIPBOARD_QUIET_MS;
            self.show_indicator(indicator);
        }
    }

    fn on_device_change(&mut self, wparam: WPARAM, lparam: LPARAM) {
        for indicator in self.drives.changed(wparam, lparam) {
            self.show_indicator(indicator);
        }
    }

    /// A laptop on modern standby only sleeps with its display off, and may
    /// never announce a resume, so the display coming back counts as one.
    fn on_display(&mut self, on: bool) {
        self.set_suppressed(Suppression::DisplayOff, !on);
        if on {
            self.on_resume();
        }
    }

    fn reload(&mut self) {
        apply_settings(&mut self.island);
        self.notices_off = stored_notices_off();
        self.calendar.refresh();
        self.set_style(stored_style());
        self.refresh();
    }

    fn toggle_notch(&mut self) {
        let style = match self.placement.style {
            IslandStyle::Island => IslandStyle::Notch,
            IslandStyle::Notch => IslandStyle::Island,
        };
        settings::remember("island_style", style.setting());
        self.set_style(style);
        // Otherwise a change made with nothing on screen would not show.
        self.island.peek(self.now());
        self.refresh();
    }

    fn set_style(&mut self, style: IslandStyle) {
        if style == self.placement.style {
            return;
        }
        self.placement.style = style;
        if let Err(error) = self.renderer.set_style(style) {
            eprintln!("style failed: {error}");
        }
        self.reappear();
    }

    /// Brings the pill back from nothing, after a change to how it is placed.
    fn reappear(&mut self) {
        self.shown = Presentation::Hidden;
        self.window_visible = false;
        window::hide(self.island_window);
        self.refresh();
    }

    /// Converts client pixels to DIPs relative to the expanded pill.
    fn to_expanded_pill(&self, x: i32, y: i32) -> (f32, f32) {
        let scale = self.placement.scale;
        let pill = self.renderer.pill_for(Presentation::Expanded);
        let left = (self.placement.width - pill.width) / 2.0;
        ((x as f32 - left) / scale, y as f32 / scale - self.placement.style.margin())
    }

    /// Jumps on release only, so a drag sends the player a single command.
    fn on_button_up(&mut self, x: i32) {
        if self.scrub.take().is_none() {
            return;
        }
        let _ = unsafe { ReleaseCapture() };
        if let Some(duration) = self.seekable_ms {
            let (pill_x, _) = self.to_expanded_pill(x, 0);
            let position = (duration as f32 * progress_fraction(pill_x)) as u64;
            self.media.seek(position);
            self.island.seek_media(position, self.now());
        }
        self.settle_hover();
        self.refresh();
    }

    /// The pointer may have left during the drag without the island hearing.
    fn settle_hover(&mut self) {
        let mut cursor = POINT::default();
        let inside = unsafe { GetCursorPos(&mut cursor).is_ok() && WindowFromPoint(cursor) == self.island_window };
        if inside {
            self.pointer_inside = false;
            self.track_pointer();
        } else {
            self.pointer_inside = false;
            self.island.set_hovered(false, self.now());
        }
    }

    fn on_capture_lost(&mut self) {
        if self.scrub.take().is_some() {
            self.settle_hover();
            self.refresh();
        }
    }

    fn on_media(&mut self) {
        let Some(update) = self.media.latest() else { return };
        // The demo shows a made-up track, which real updates would wipe out.
        if self.demo_step.is_some() {
            return;
        }
        match update {
            Some(snapshot) => self.island.upsert_media(snapshot, self.now()),
            None => self.island.remove(Kind::Media),
        }
        self.refresh();
    }

    fn on_pointer_moved(&mut self, x: i32) {
        if self.scrub.is_some() {
            let (pill_x, _) = self.to_expanded_pill(x, 0);
            self.scrub = Some(progress_fraction(pill_x));
            self.refresh();
            return;
        }
        self.track_pointer();
    }

    fn track_pointer(&mut self) {
        if self.pointer_inside {
            return;
        }
        self.pointer_inside = true;
        let mut tracking = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: self.island_window,
            dwHoverTime: 0,
        };
        unsafe {
            let _ = TrackMouseEvent(&mut tracking);
        }
        self.island.set_hovered(true, self.now());
        self.refresh();
    }

    fn on_pointer_left(&mut self) {
        self.pointer_inside = false;
        if self.scrub.is_some() {
            return;
        }
        self.island.set_hovered(false, self.now());
        self.refresh();
    }

    fn on_indicators(&mut self) {
        while let Ok(indicator) = self.indicators.try_recv() {
            let shown = match indicator {
                Indicator::Volume { percent, muted } => self.tracker.volume_changed(percent, muted),
                Indicator::Microphone { muted } => self.tracker.microphone_changed(muted),
                other if other.is_about_network() && self.now() < self.network_quiet_until => None,
                other => Some(other),
            };
            if let Some(indicator) = shown {
                self.show_indicator(indicator);
            }
        }
    }

    fn on_power(&mut self, event: PowerEvent) {
        let indicator = match event {
            PowerEvent::Source { on_battery } => self.tracker.power_source_changed(on_battery),
            PowerEvent::Battery(percent) => self.tracker.battery_changed(percent),
            PowerEvent::Brightness(percent) => self.tracker.brightness_changed(percent),
            PowerEvent::BatterySaver(on) => self.tracker.battery_saver_changed(on),
        };
        if let Some(indicator) = indicator {
            self.show_indicator(indicator);
        }
    }

    fn show_indicator(&mut self, indicator: Indicator) {
        if matches!(indicator, Indicator::Screenshot { .. }) {
            self.clipboard_quiet_until = self.now() + CLIPBOARD_QUIET_AFTER_SCREENSHOT_MS;
        }
        let turned_off = indicator.setting().is_some_and(|name| self.is_turned_off(name));
        if turned_off || (indicator.is_brief() && self.island.is_open()) {
            return;
        }
        if cfg!(debug_assertions) {
            eprintln!("{} ms: {indicator:?}", self.now());
        }
        self.island.upsert(indicator.into_activity(self.now()));
        self.refresh();
    }

    fn on_audio_device_changed(&mut self, device: usize) {
        let Some(audio) = &mut self.audio else { return };
        if device == DEVICE_MICROPHONE {
            audio.follow_default_microphone();
            return;
        }
        let name = audio.follow_default_device();
        if let Some(indicator) = name.and_then(|name| self.tracker.audio_output_changed(name)) {
            self.show_indicator(indicator);
        }
    }

    /// Shows every indicator in turn, since most need real hardware to trigger.
    fn advance_indicator_demo(&mut self) {
        let Some(step) = self.indicator_demo_step else { return };
        let name = String::from("Kulaklık");
        let all = [
            Indicator::Volume { percent: 40, muted: false },
            Indicator::Volume { percent: 40, muted: true },
            Indicator::Brightness { percent: 70 },
            Indicator::CapsLock { on: true },
            Indicator::NumLock { on: false },
            Indicator::PluggedIn { percent: 64 },
            Indicator::Unplugged { percent: 64 },
            Indicator::LowBattery { percent: 20 },
            Indicator::BluetoothConnected { name: name.clone() },
            Indicator::BluetoothBattery { name: name.clone(), percent: 80 },
            Indicator::BluetoothDisconnected { name: name.clone() },
            Indicator::AudioOutput { name: name.clone() },
            Indicator::Microphone { muted: true },
            Indicator::Microphone { muted: false },
            Indicator::MicrophoneInUse { app: "Discord".into() },
            Indicator::Internet { online: false, network: None },
            Indicator::Internet { online: true, network: Some("Ev Wi-Fi".into()) },
            Indicator::Wifi { name: "Okul Wi-Fi".into() },
            Indicator::Vpn { connected: true, name: "Okul VPN".into() },
            Indicator::AirplaneMode { on: true },
            Indicator::Usb { added: true, name: "SANDISK (E:)".into() },
            Indicator::Usb { added: false, name: "SANDISK (E:)".into() },
            Indicator::Display { added: true, count: 2 },
            Indicator::Display { added: false, count: 1 },
            Indicator::Screenshot { path: String::new(), image: None },
            Indicator::Copied { what: "Metin" },
            Indicator::BatterySaver { on: true },
            Indicator::KeyboardLanguage { code: "EN".into() },
        ];
        self.indicator_demo_step = Some((step + 1) % all.len());
        self.show_indicator(all[step].clone());
    }

    fn on_summon(&mut self) {
        self.island.summon(self.now());
        self.refresh();
    }

    fn on_fullscreen(&mut self, active: bool) {
        self.island.set_suppressed(Suppression::Fullscreen, active);
        if active {
            window::hide(self.edge_window);
        } else {
            window::show(self.edge_window);
        }
        self.refresh();
    }

    fn set_suppressed(&mut self, reason: Suppression, active: bool) {
        self.island.set_suppressed(reason, active);
        self.refresh();
    }

    fn on_display_changed(&mut self) {
        match window::place(self.island_window, self.edge_window, self.placement.style) {
            Ok(placement) => {
                self.placement = placement;
                if let Err(error) = self.renderer.set_metrics(placement.scale, placement.width) {
                    eprintln!("metrics failed: {error}");
                }
                self.reappear();
                if let Some(indicator) = self.tracker.monitors_changed(monitor_count()) {
                    self.show_indicator(indicator);
                }
            }
            Err(error) => eprintln!("placement failed: {error}"),
        }
    }

    fn toggle_demo(&mut self) {
        if self.demo_step.take().is_some() {
            self.cancel_timer(TIMER_DEMO);
            self.island.remove(Kind::Media);
            self.refresh();
        } else {
            self.demo_step = Some(0);
            self.advance_demo();
            self.set_timer(TIMER_DEMO, DEMO_STEP_MS);
        }
    }

    /// Walks through every transition with made-up content.
    fn advance_demo(&mut self) {
        let Some(step) = self.demo_step else { return };
        self.demo_step = Some((step + 1) % DEMO_STEPS);
        let now = self.now();
        match step {
            0 => self.island.upsert_media(demo_track("Kısa Şarkı", true, now), now),
            1 => self.island.upsert_media(demo_track("Örnek Şarkı Adı Burada Görünür", true, now), now),
            2 => self.island.upsert_media(demo_track("Kısa Şarkı", false, now), now),
            3 => self.island.click(now),
            4 => {
                self.island.click(now);
                self.island.upsert(Indicator::Volume { percent: 40, muted: false }.into_activity(now));
            }
            5 => self.island.upsert(demo_activity(
                Kind::Notification,
                ("Yeni ileti", ""),
                ("Yeni bildirim", "Pill bildirimi kısa süre gösterip kapanır"),
                Some(now + NOTIFICATION_MS),
            )),
            6 => {
                self.island.start_timer(25 * 60_000, now);
                self.island.set_camera(Some("Zoom".to_owned()));
            }
            _ => {
                self.island.cancel_timer();
                self.island.set_camera(None);
                self.island.remove(Kind::Media);
            }
        }
        self.refresh();
    }
}

fn demo_track(title: &str, playing: bool, now: Millis) -> MediaSnapshot {
    MediaSnapshot {
        shuffle: Some(true),
        repeat: Some(RepeatMode::Off),
        title: title.to_owned(),
        artist: "Sanatçı".to_owned(),
        playing,
        position_ms: 42_000,
        duration_ms: 201_000,
        rate: 1.0,
        sampled_at: now,
        can_seek: false,
        artwork: None,
    }
}

/// An optional feature that fails to start is logged, not fatal: a machine
/// without Bluetooth should still get everything else.
fn optional<T>(name: &str, started: Result<T>) -> Option<T> {
    started.map_err(|error| eprintln!("{name} unavailable: {error}")).ok()
}

fn argument_after(flag: &str) -> Option<String> {
    std::env::args().skip_while(|argument| argument != flag).nth(1)
}

fn monitor_count() -> u32 {
    unsafe { GetSystemMetrics(SM_CMONITORS) as u32 }
}

fn stored_notices_off() -> Vec<String> {
    settings::read().map_or_else(Vec::new, |text| config::read_notices_off(&text))
}

/// Tests ask for the notch directly, which leaves the user's choice alone.
fn stored_style() -> IslandStyle {
    if std::env::args().any(|a| a == "--notch") {
        return IslandStyle::Notch;
    }
    settings::read().and_then(|text| config::read_island_style(&text)).unwrap_or_default()
}

fn apply_settings(island: &mut Island) {
    let text = settings::read().unwrap_or_default();
    island.set_presets(config::read_presets(&text).unwrap_or(TIMER_PRESETS));
    island.set_track_notices(!config::read_notices_off(&text).iter().any(|name| name == "track_change"));
    if let Some(mode) = config::read_start_mode(&text) {
        island.set_start_mode(mode);
    }
}

fn demo_activity(
    kind: Kind,
    (leading, trailing): (&str, &str),
    (title, detail): (&str, &str),
    expires_at: Option<Millis>,
) -> Activity {
    Activity {
        leading: leading.into(),
        trailing: trailing.into(),
        title: title.into(),
        detail: detail.into(),
        expires_at,
        ..Activity::new(kind)
    }
}

fn clock_text() -> ClockText {
    let now = unsafe { GetLocalTime() };
    ClockText {
        time: format!("{:02}:{:02}", now.wHour, now.wMinute),
        date: ics::local_day(unix_now()).unwrap_or_default(),
    }
}

unsafe extern "system" fn island_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONDOWN => {
            let (x, y) = pointer_position(lparam);
            with_app(|app| app.on_button_down(x, y));
        }
        WM_LBUTTONUP => {
            let (x, _) = pointer_position(lparam);
            with_app(|app| app.on_button_up(x));
        }
        WM_CAPTURECHANGED => {
            with_app(App::on_capture_lost);
        }
        WM_MOUSEMOVE => {
            let (x, _) = pointer_position(lparam);
            with_app(|app| app.on_pointer_moved(x));
        }
        WM_MOUSELEAVE => {
            with_app(App::on_pointer_left);
        }
        WM_TIMER => {
            with_app(|app| app.on_timer(wparam.0));
        }
        WM_HOTKEY => {
            with_app(App::on_summon);
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            with_app(App::on_display_changed);
        }
        WM_POWERBROADCAST => {
            if wparam.0 as u32 == PBT_APMRESUMEAUTOMATIC {
                with_app(App::on_resume);
            } else if let Some(on) = fullscreen::display_state(wparam, lparam) {
                with_app(|app| app.on_display(on));
            } else if let Some(event) = power::decode(wparam, lparam) {
                with_app(|app| app.on_power(event));
            }
        }
        WM_WTSSESSION_CHANGE => {
            if let Some(locked) = fullscreen::session_locked(wparam) {
                with_app(|app| app.set_suppressed(Suppression::SessionLocked, locked));
            }
        }
        MSG_APPBAR if wparam.0 as u32 == ABN_FULLSCREENAPP => {
            with_app(|app| app.on_fullscreen(lparam.0 != 0));
        }
        MSG_MEDIA => {
            with_app(App::on_media);
        }
        MSG_INDICATOR => {
            with_app(App::on_indicators);
        }
        MSG_CALENDAR => {
            with_app(App::on_calendar);
        }
        MSG_FOCUS => {
            with_app(App::on_focus);
        }
        MSG_FOCUS_REFUSED => {
            with_app(|app| app.on_focus_refused(wparam.0 as Millis));
        }
        MSG_CLOCK_TIMERS => {
            with_app(App::on_clock_timers);
        }
        MSG_CAMERA => {
            with_app(App::on_camera);
        }
        WM_DEVICECHANGE => {
            with_app(|app| app.on_device_change(wparam, lparam));
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
        WM_TIMECHANGE => {
            with_app(App::on_time_changed);
        }
        MSG_AUDIO_DEVICE => {
            with_app(|app| app.on_audio_device_changed(wparam.0));
        }
        WM_INPUT => {
            match keys::key_event(lparam) {
                Some(KeyEvent::Lock(indicator)) => {
                    with_app(|app| app.show_indicator(indicator));
                }
                Some(KeyEvent::LanguageKey) => {
                    with_app(|app| app.set_timer(TIMER_LANGUAGE, LANGUAGE_SETTLE_MS));
                }
                None => {}
            }
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
        WM_CLIPBOARDUPDATE => {
            with_app(App::on_clipboard);
        }
        MSG_WINDOW_DRAG => {
            with_app(|app| app.set_suppressed(Suppression::WindowDrag, wparam.0 != 0));
        }
        MSG_TRAY if matches!(lparam.0 as u32, WM_RBUTTONUP | WM_LBUTTONUP) => {
            open_tray_menu(hwnd);
        }
        WM_SETTINGCHANGE => {
            if tray::is_theme_change(lparam) {
                with_app(|app| app.tray.follow_theme());
            }
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
        }
        _ if msg == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            with_app(|app| app.tray.restore());
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}

fn pointer_position(lparam: LPARAM) -> (i32, i32) {
    (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32)
}

fn open_tray_menu(hwnd: HWND) {
    let state = MenuState {
        demo_running: with_app(|app| app.demo_step.is_some()).unwrap_or(false),
        autostart: autostart::is_on(),
        notch: with_app(|app| app.placement.style == IslandStyle::Notch).unwrap_or(false),
    };
    let autostart_was_on = state.autostart;
    match tray::show_menu(hwnd, &state) {
        Some(Command::ToggleDemo) => {
            with_app(App::toggle_demo);
        }
        Some(Command::ToggleAutostart) => autostart::set(!autostart_was_on),
        Some(Command::ToggleNotch) => {
            with_app(App::toggle_notch);
        }
        Some(Command::Reload) => {
            with_app(App::reload);
        }
        Some(Command::OpenSettings) => settings::open_in_editor(),
        Some(Command::Quit) => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        None => {}
    }
}

unsafe extern "system" fn edge_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            with_app(|app| app.set_timer(TIMER_EDGE, EDGE_DWELL_MS));
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
