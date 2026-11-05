use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use island_core::clock_timer::{self, ClockTimer};
use island_core::config;
use island_core::convert::Target;
use island_core::drop::{DropView, ShelfHit, Zone, on_transfer_stop, shelf_hit};
use island_core::files::{DroppedFile, file_name, summary};
use island_core::focus::{
    CHANGE_WAIT_MS, FocusReport, FocusStatus, FocusTracker, START_WAIT_MS,
};
use island_core::ics::{self, Event};
use island_core::media::RepeatMode;
use island_core::qr::QrMatrix;
use island_core::transfer::{Stage, Transfer, WAIT_MS};
use island_core::layout::{
    TIMER_PRESETS, on_event_hide, on_home_shortcut, on_progress_bar, on_settings_shortcut,
    preset_at, progress_fraction,
};
use island_core::settings_view::{
    Action, NOTICES, Page, SettingsState, is_calendar_address, stepped_preset, toggled_notice,
};
use island_core::{
    Activity, CalendarState, FocusView, Frame, FullscreenWatch, Icon, Indicator, Island, IslandStyle,
    Kind, Material, MediaButton, MediaSnapshot, Millis, PanelView, Pill, Presentation, ProgressStyle,
    StartMode, Suppression, SystemTracker, TimerButton, Upcoming,
};
use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    VIRTUAL_KEY, VK_ESCAPE, VK_SHIFT,
};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::Shell::ABN_FULLSCREENAPP;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Result, w};

use crate::platform::settings_window::{
    self, MSG_OPEN_SETTINGS, MSG_SETTINGS_CLOSED, SettingsWindow,
};
use crate::platform::tray::{self, Command, MSG_TRAY, Tray};
use crate::platform::window::{self, Placement};
use crate::platform::{autostart, drop_target, hotkey, settings, shell};
use crate::render::{self, ClockText, Renderer, WaveMotion};
use crate::sources::audio::{AudioWatch, DEVICE_MICROPHONE, MSG_AUDIO_DEVICE};
use crate::sources::bluetooth::BluetoothWatch;
use crate::sources::calendar::{CalendarSource, CalendarUpdate, MSG_CALENDAR, unix_now};
use crate::sources::clock_timers::{ClockTimerWatch, MSG_CLOCK_TIMERS};
use crate::sources::drives::Drives;
use crate::sources::focus::{self, FocusWatch, MSG_FOCUS, MSG_FOCUS_REFUSED};
use crate::sources::fullscreen::{self, MSG_APPBAR, MSG_FRONT_CHANGED, MSG_WINDOW_DRAG, Presence};
use crate::sources::imaging::{Imaging, MSG_IMAGING, Made};
use crate::sources::media::{MSG_MEDIA, MediaSource};
use crate::sources::power::{self, PowerEvent, PowerWatch};
use crate::sources::privacy::{CameraWatch, MSG_CAMERA};
use crate::sources::sharing::{self, MSG_SHARING, SharedFile, Sharing, SharingEvent};
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
const TIMER_FULLSCREEN: usize = 12;
const TIMER_DRAG_LEAVE: usize = 13;

/// When the pill looks settled. The compositor's own completion event only
/// fires about 1.2 s after an animation starts, long after it visibly stopped.
const SETTLE_MS: u32 = 520;
/// How long the pill takes to shrink away before the window hides.
const HIDE_MS: u32 = 380;
/// How long the pointer must rest on the top edge, so that passing over it to
/// reach a browser tab does not open anything.
const EDGE_DWELL_MS: u32 = 120;
const DEMO_STEP_MS: u32 = 3_500;
const DEMO_STEPS: u32 = 11;
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
/// A window opened from the bare desktop counts as full screen for a moment, as
/// Windows takes back what it said about the desktop only after it appears.
const FULLSCREEN_SETTLE_MS: u32 = 200;

const NOTIFICATION_MS: Millis = 3_000;
/// A converted picture can be shown in its folder, which takes a moment to decide.
const CONVERTED_MS: Millis = 6_000;
/// Moving from the top edge onto the panel leaves one window for the other,
/// which must not close the panel in between.
const DRAG_LEAVE_MS: u32 = 150;
/// Pictures of files on the shelf, in DIPs, a little over what is drawn.
const THUMBNAIL_DIPS: f32 = 72.0;

static DROP_HANDLERS: drop_target::Handlers = drop_target::Handlers {
    enter: |files, point| with_app(|app| app.on_drag_enter(files, point)).unwrap_or(false),
    over: |point| with_app(|app| app.on_drag_over(point)).unwrap_or(false),
    leave: || {
        with_app(App::on_drag_leave);
    },
    drop: |point| with_app(|app| app.on_drop(point)).unwrap_or(false),
};

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
    Transfer,
    Drop,
    Shelf,
    Convert,
    Other,
}

impl OpenView {
    /// Views of an activity have a way to the home view in their corner.
    fn has_home_shortcut(self) -> bool {
        matches!(self, OpenView::Media | OpenView::Timer | OpenView::Focus | OpenView::Event | OpenView::Other)
    }
}

struct App {
    island_window: HWND,
    edge_window: HWND,
    placement: Placement,
    island: Island,
    renderer: Renderer,
    tray: Tray,
    presence: Presence,
    fullscreen: FullscreenWatch,
    /// Whether the island is out of the way for a full screen app right now.
    out_for_fullscreen: bool,
    /// Only there while open, so that it costs nothing the rest of the time.
    settings: Option<SettingsWindow>,
    /// What the user chose, which Energy Saver can overrule for a while.
    material: Material,
    sound_wave: bool,
    battery_saver: bool,
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
    /// Whether that click shows the file in its folder instead.
    alert_reveals: bool,
    /// Pictures just converted, which "show" selects in their folder.
    converted: Vec<String>,
    /// What is being dragged over the island, read when it arrived.
    dragged: Vec<DroppedFile>,
    drop_view: Option<DropView>,
    /// The island itself is dragging files out, which it must not take back.
    dragging_out: bool,
    /// A file on the shelf under a pressed button, with where the press began.
    pressed_file: Option<(usize, i32, i32)>,
    /// Pictures waiting for a choice of format.
    to_convert: Vec<String>,
    sharing: Option<Sharing>,
    imaging: Imaging,
    progress_style: ProgressStyle,
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
        // Opening the app again is how most people will look for its settings.
        // Tests start it with arguments, and expect nothing but a quick exit.
        if std::env::args().len() == 1 {
            ask_running_copy_for_settings();
        }
        return Ok(());
    }
    let first_run = settings::is_first_run();

    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)? };
    TASKBAR_CREATED.store(unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }, Ordering::Relaxed);

    let _dispatcher = render::create_dispatcher_queue()?;
    let island_window = window::create_island(Some(island_proc))?;
    // Tests read the screen, and need to see what they are checking.
    if !std::env::args().any(|a| a == "--capturable") {
        window::exclude_from_capture(island_window);
    }
    let edge_window = window::create_edge(Some(edge_proc))?;
    for target in [island_window, edge_window] {
        optional("drop", drop_target::register(target, &DROP_HANDLERS));
    }
    let placement = window::place(island_window, edge_window, stored_style())?;
    let sound_wave = match argument_after("--wave").as_deref() {
        Some("still") => false,
        _ => stored_sound_wave(),
    };
    let wave_motion = if sound_wave { WaveMotion::Always } else { WaveMotion::Still };
    let renderer = Renderer::new(island_window, placement.scale, placement.width, placement.style, wave_motion)?;

    if !hotkey::register(island_window) {
        eprintln!("no global shortcut available, only the top edge will open the island");
    }
    let tray = Tray::add(island_window)?;
    let mut presence = Presence::start(island_window)?;

    let mut island = Island::new();
    if std::env::args().any(|a| a == "--local-timers") {
        island.offer_only_timers();
    }
    // Before the calendar thread, which reads the same file.
    apply_settings(&mut island);
    let reported = fullscreen::fullscreen_now();
    let mut fullscreen_watch = FullscreenWatch::default();
    fullscreen_watch.report(reported, fullscreen::desktop_in_front());
    presence.follow_front(reported);
    island.set_suppressed(Suppression::Fullscreen, fullscreen_watch.active());
    if !fullscreen_watch.active() {
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
        presence,
        fullscreen: fullscreen_watch,
        out_for_fullscreen: fullscreen_watch.active(),
        settings: None,
        material: stored_material(),
        sound_wave,
        battery_saver: false,
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
        alert_reveals: false,
        converted: Vec::new(),
        dragged: Vec::new(),
        drop_view: None,
        dragging_out: false,
        pressed_file: None,
        to_convert: Vec::new(),
        sharing: None,
        imaging: Imaging::new(island_window),
        progress_style: stored_progress_style(),
        offers_shuffle: false,
        offers_repeat: false,
        scrub: None,
        demo_step: None,
        indicator_demo_step: None,
    };
    app.apply_material();
    if std::env::args().any(|a| a == "--demo") {
        app.toggle_demo();
    }
    // Tests send a file as if it were dropped, to this computer only.
    if let Some(path) = argument_after("--send-file") {
        let size = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
        app.send_to_phone(vec![DroppedFile { path, size, is_folder: false }]);
    }
    // Tests convert a picture as if it were dropped and a format chosen.
    if let Some(path) = argument_after("--convert-file") {
        let target = match argument_after("--convert-to").as_deref() {
            Some("png") => Target::Png,
            Some("smaller") => Target::Smaller,
            _ => Target::Jpg,
        };
        app.to_convert = vec![path];
        app.start_convert(target);
    }
    // One still view of the drop panel and what follows it, for screenshots.
    if let Some(step) = argument_after("--demo-drop").and_then(|step| step.parse().ok()) {
        app.show_drop_demo(step);
    }
    if std::env::args().any(|a| a == "--demo-indicators") {
        app.indicator_demo_step = Some(0);
        app.advance_indicator_demo();
        app.set_timer(TIMER_INDICATOR_DEMO, INDICATOR_DEMO_STEP_MS);
    }
    APP.with(|cell| *cell.borrow_mut() = Some(app));

    // The island hides when idle, so nothing else would show that the app started.
    let welcome = first_run && std::env::args().len() == 1;
    if welcome || std::env::args().any(|a| a == "--settings-offscreen") {
        with_app(App::open_settings);
    }

    // Windows answers this with the current state, which is what tells a real
    // change from then on. The app must be there to hear it, or the first time
    // the charger is plugged in would pass for that report and show nothing.
    let power = optional("power", PowerWatch::start(island_window));
    with_app(|app| app._power = power);

    unsafe {
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            // Tab, Enter and Esc in the address box act on its window, as in a dialog.
            if settings_window::is_dialog_key(&message) {
                let _ = PostMessageW(GetParent(message.hwnd).ok(), WM_KEYDOWN, message.wParam, message.lParam);
                continue;
            }
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
        // The code timed out, or the view with it was closed.
        if self.sharing.is_some() && self.island.transfer_stage().is_none() {
            self.sharing = None;
        }
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
            _ if matches!(frame.panel, Some(PanelView::Drop(_))) => OpenView::Drop,
            _ if matches!(frame.panel, Some(PanelView::Shelf(_))) => OpenView::Shelf,
            _ if matches!(frame.panel, Some(PanelView::Convert(_))) => OpenView::Convert,
            Some(activity) if activity.kind == Kind::Transfer => OpenView::Transfer,
            Some(activity) if activity.media.is_some() => OpenView::Media,
            Some(activity) if activity.kind == Kind::Focus => OpenView::Focus,
            Some(activity) if activity.kind == Kind::Upcoming => OpenView::Event,
            Some(activity) if activity.timer.is_some_and(|timer| timer.has_controls) => OpenView::Timer,
            _ => OpenView::Other,
        };
        self.seekable_ms = media.filter(|media| media.can_seek && media.duration_ms > 0).map(|media| media.duration_ms);
        self.alert_file = frame.activity.as_ref().and_then(|activity| activity.file.clone());
        self.alert_reveals = frame.activity.as_ref().is_some_and(|activity| activity.reveal);
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
            TIMER_DRAG_LEAVE => {
                self.cancel_timer(TIMER_DRAG_LEAVE);
                self.end_drag();
            }
            TIMER_FULLSCREEN => {
                self.cancel_timer(TIMER_FULLSCREEN);
                self.fullscreen.front_changed(fullscreen::desktop_in_front());
                self.apply_fullscreen();
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
            if self.alert_reveals {
                shell::reveal(std::mem::take(&mut self.converted));
            } else {
                screenshots::open(file);
            }
        }
        if self.shown == Presentation::Expanded {
            let (pill_x, pill_y) = self.to_expanded_pill(x, y);
            if self.open_view.has_home_shortcut() && on_home_shortcut(pill_x, pill_y) {
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
                    if on_settings_shortcut(pill_x, pill_y) {
                        self.open_settings();
                        return;
                    }
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
                OpenView::Transfer => {
                    let sending = matches!(self.island.transfer_stage(), Some(Stage::Waiting { .. } | Stage::Sending { .. }));
                    if sending && on_transfer_stop(pill_x, pill_y) {
                        self.island.cancel_transfer();
                        self.sharing = None;
                        self.refresh();
                        return;
                    }
                }
                OpenView::Drop => return,
                OpenView::Shelf => {
                    let shelf = self.island.shelf();
                    match shelf_hit(pill_x, pill_y, shelf.first(), shelf.len()) {
                        Some(ShelfHit::File(index)) => {
                            self.pressed_file = Some((index, x, y));
                            unsafe { SetCapture(self.island_window) };
                            return;
                        }
                        Some(ShelfHit::Remove(index)) => self.island.remove_from_shelf(index),
                        Some(ShelfHit::Clear) => self.island.clear_shelf(),
                        Some(ShelfHit::Back) => self.island.scroll_shelf(-1),
                        Some(ShelfHit::Forward) => self.island.scroll_shelf(1),
                        None => {
                            self.island.click(now);
                            self.refresh();
                            return;
                        }
                    }
                    self.refresh();
                    return;
                }
                OpenView::Convert => {
                    if let Some(target) = Target::at(pill_x, pill_y) {
                        self.start_convert(target);
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
        self.progress_style = stored_progress_style();
        self.notices_off = stored_notices_off();
        self.calendar.refresh();
        self.sound_wave = stored_sound_wave();
        self.apply_wave();
        self.material = stored_material();
        self.apply_material();
        self.set_style(stored_style());
        self.refresh();
    }

    /// Puts the chosen material on, unless Windows has a reason against it now.
    fn apply_material(&mut self) {
        let material = self.material.in_effect(tray::transparency_is_on(), self.battery_saver);
        if let Err(error) = self.renderer.set_material(material) {
            eprintln!("material failed: {error}");
        }
    }

    fn open_settings(&mut self) {
        if let Some(window) = &self.settings {
            window.bring_to_front();
            return;
        }
        let address = settings::read()
            .and_then(|text| config::read_value(&text, "calendar_ics_url"))
            .unwrap_or_default();
        let page = argument_after("--settings-page")
            .and_then(|page| page.parse::<usize>().ok())
            .and_then(|page| Page::ALL.get(page).copied())
            .unwrap_or_default();
        let hidden_away = std::env::args().any(|a| a == "--settings-offscreen");
        let state = self.settings_state(page);
        match SettingsWindow::open(self.island_window, Some(settings_proc), state, &address, hidden_away) {
            Ok(window) => self.settings = Some(window),
            Err(error) => eprintln!("settings window failed: {error}"),
        }
    }

    fn settings_state(&self, page: Page) -> SettingsState {
        SettingsState {
            page,
            material: self.material,
            style: self.placement.style,
            progress: self.progress_style,
            sound_wave: self.sound_wave,
            notices_off: self.notices_off.clone(),
            presets: self.island.presets(),
            mode: self.island.start_mode(),
            autostart: autostart::is_on(),
            demo_running: self.demo_step.is_some(),
            calendar_note: None,
        }
    }

    fn paint_settings(&mut self) -> bool {
        match &mut self.settings {
            Some(window) => {
                window.paint(self.renderer.painter());
                true
            }
            None => false,
        }
    }

    fn on_settings_click(&mut self, x: i32, y: i32) {
        let Some(window) = &mut self.settings else { return };
        if let Some(action) = window.action_at(x, y) {
            window.focus_on(action);
            self.on_settings_action(action);
        }
    }

    fn on_settings_key(&mut self, key: VIRTUAL_KEY, shift: bool) {
        if let Some(action) = self.settings.as_mut().and_then(|window| window.on_key(key, shift)) {
            self.on_settings_action(action);
        }
    }

    /// Every change takes effect at once, and the window then shows what is true.
    fn on_settings_action(&mut self, action: Action) {
        let Some(mut page) = self.settings.as_ref().map(|window| window.state().page) else { return };
        let mut calendar_note = None;
        match action {
            Action::Open(chosen) => page = chosen,
            Action::Material(material) => self.choose_material(material),
            Action::Style(style) => self.choose_style(style),
            Action::Progress(style) => {
                self.progress_style = style;
                settings::remember("progress_style", style.setting());
                self.island.set_progress_style(style);
                self.refresh();
            }
            Action::SoundWave => {
                self.sound_wave = !self.sound_wave;
                settings::remember("sound_wave", if self.sound_wave { "on" } else { "off" });
                self.apply_wave();
            }
            Action::Notice(index) => self.toggle_notice(NOTICES[index].0),
            Action::Preset { index, longer } => {
                let mut presets = self.island.presets();
                presets[index] = stepped_preset(presets[index], longer);
                settings::remember("timer_presets", &presets.map(|minutes| minutes.to_string()).join(", "));
                self.island.set_presets(presets);
                self.refresh();
            }
            Action::Mode(mode) => {
                if self.island.set_start_mode(mode) {
                    settings::remember("start_mode", mode.setting());
                    self.refresh();
                }
            }
            Action::SaveCalendar => calendar_note = self.save_calendar(),
            Action::Autostart => autostart::set(!autostart::is_on()),
            Action::Demo => self.toggle_demo(),
            Action::Reload => self.reload(),
            Action::EditFile => settings::open_in_editor(),
            Action::Quit => {
                unsafe {
                    let _ = DestroyWindow(self.island_window);
                }
                return;
            }
        }
        let state = SettingsState { calendar_note, ..self.settings_state(page) };
        if let Some(window) = &mut self.settings {
            window.update(state);
        }
    }

    fn choose_material(&mut self, material: Material) {
        if material == self.material {
            return;
        }
        self.material = material;
        settings::remember("material", material.setting());
        self.apply_material();
        // Otherwise a change made with nothing on screen would not show.
        self.island.peek(self.now());
        self.refresh();
    }

    fn choose_style(&mut self, style: IslandStyle) {
        if style == self.placement.style {
            return;
        }
        settings::remember("island_style", style.setting());
        self.set_style(style);
        // Otherwise a change made with nothing on screen would not show.
        self.island.peek(self.now());
        self.refresh();
    }

    /// The moving wave is the costliest thing on screen, so Energy Saver stills
    /// it as well, whatever the user chose.
    fn apply_wave(&mut self) {
        let moving = self.sound_wave && !self.battery_saver;
        let motion = if moving { WaveMotion::Always } else { WaveMotion::Still };
        if let Err(error) = self.renderer.set_wave_motion(motion) {
            eprintln!("wave failed: {error}");
        }
    }

    fn toggle_notice(&mut self, name: &str) {
        self.notices_off = toggled_notice(&self.notices_off, name);
        settings::remember("notices_off", &self.notices_off.join(", "));
        apply_settings(&mut self.island);
        self.on_camera();
    }

    fn save_calendar(&mut self) -> Option<&'static str> {
        let address = self.settings.as_ref()?.address();
        if !is_calendar_address(&address) {
            return Some("Bu bir takvim adresi değil, https:// ile başlamalı");
        }
        settings::remember("calendar_ics_url", &address);
        self.calendar.refresh();
        Some(if address.is_empty() { "Takvim adresi kaldırıldı" } else { "Kaydedildi, takvim yenileniyor" })
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
        if let Some((index, ..)) = self.pressed_file.take() {
            let _ = unsafe { ReleaseCapture() };
            if let Some(item) = self.island.shelf().items().get(index) {
                shell::open(&item.path);
            }
            return;
        }
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
        self.pressed_file = None;
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

    /// Returns files to drag out of the island, once a press on the shelf
    /// has moved far enough to be a drag.
    fn on_pointer_moved(&mut self, x: i32, y: i32) -> Option<Vec<String>> {
        if let Some((index, start_x, start_y)) = self.pressed_file {
            let (reach_x, reach_y) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
            if (x - start_x).abs() > reach_x || (y - start_y).abs() > reach_y {
                self.pressed_file = None;
                let _ = unsafe { ReleaseCapture() };
                let path = self.island.shelf().items().get(index)?.path.clone();
                self.dragging_out = true;
                return Some(vec![path]);
            }
            return None;
        }
        if self.scrub.is_some() {
            let (pill_x, _) = self.to_expanded_pill(x, 0);
            self.scrub = Some(progress_fraction(pill_x));
            self.refresh();
            return None;
        }
        self.track_pointer();
        None
    }

    /// Files dragged out may have been moved, which the shelf then forgets.
    fn after_drag_out(&mut self) {
        self.dragging_out = false;
        self.island.forget_missing(|path| std::path::Path::new(path).exists());
        self.settle_hover();
        self.refresh();
    }

    fn on_drag_enter(&mut self, files: Vec<DroppedFile>, point: POINT) -> bool {
        if self.dragging_out || self.island.is_suppressed() {
            return false;
        }
        self.cancel_timer(TIMER_DRAG_LEAVE);
        if self.drop_view.is_none() || self.dragged != files {
            self.drop_view = Some(DropView::for_files(&files));
            self.dragged = files;
            if let Some(view) = &self.drop_view {
                self.island.drag_over(view.clone());
            }
            self.refresh();
        }
        self.on_drag_over(point)
    }

    /// Lights the zone under the pointer, and says whether it takes the files.
    fn on_drag_over(&mut self, point: POINT) -> bool {
        let zone = self.zone_at(point);
        let Some(view) = &mut self.drop_view else { return false };
        if view.hovered != zone {
            view.hovered = zone;
            self.island.drag_over(view.clone());
            self.refresh();
        }
        zone.is_some()
    }

    fn zone_at(&self, point: POINT) -> Option<Zone> {
        let view = self.drop_view.as_ref()?;
        let mut client = point;
        let _ = unsafe { ScreenToClient(self.island_window, &mut client) };
        let (x, y) = self.to_expanded_pill(client.x, client.y);
        view.accepting(x, y)
    }

    fn on_drag_leave(&mut self) {
        if self.drop_view.is_some() {
            self.set_timer(TIMER_DRAG_LEAVE, DRAG_LEAVE_MS);
        }
    }

    fn end_drag(&mut self) {
        self.drop_view = None;
        self.dragged.clear();
        self.island.drag_ended();
        self.refresh();
    }

    fn on_drop(&mut self, point: POINT) -> bool {
        self.cancel_timer(TIMER_DRAG_LEAVE);
        let zone = self.zone_at(point);
        let files = std::mem::take(&mut self.dragged);
        self.drop_view = None;
        self.island.drag_ended();
        let now = self.now();
        match zone {
            Some(Zone::Shelf) => {
                let paths: Vec<String> = files.iter().map(|file| file.path.clone()).collect();
                let added = self.island.drop_on_shelf(&paths, now);
                let size = (THUMBNAIL_DIPS * self.placement.scale).ceil() as i32;
                self.imaging.thumbnails(added, size);
            }
            Some(Zone::Phone) => self.send_to_phone(files),
            Some(Zone::Convert) => {
                let pictures: Vec<DroppedFile> = files.into_iter().filter(DroppedFile::is_image).collect();
                self.to_convert = pictures.iter().map(|file| file.path.clone()).collect();
                self.island.open_convert(summary(&pictures), now);
            }
            None => {}
        }
        self.refresh();
        zone.is_some()
    }

    fn send_to_phone(&mut self, files: Vec<DroppedFile>) {
        let files: Vec<DroppedFile> = files.into_iter().filter(|file| !file.is_folder).collect();
        if files.is_empty() {
            return;
        }
        // Tests reach the server from this computer, which needs no firewall rule.
        let loopback = std::env::args().any(|a| a == "--share-loopback");
        let network = if loopback { Some(sharing::loopback()) } else { sharing::local_network() };
        let Some(network) = network else {
            self.notify("Telefona gönderilemedi", "Bilgisayar bir ağa bağlı değil", Icon::Warning);
            return;
        };
        // One send at a time: a new one takes over from the last.
        self.sharing = None;
        let shared = files
            .iter()
            .map(|file| SharedFile { path: file.path.clone(), name: file.name().to_owned(), size: file.size })
            .collect();
        let started = Sharing::start(self.island_window, shared, network.address);
        if loopback && let Ok(sharing) = &started {
            eprintln!("sharing at {}", sharing.url);
        }
        let Some((sharing, code)) = started.ok().and_then(|sharing| {
            let code = QrMatrix::encode(&sharing.url)?;
            Some((sharing, code))
        }) else {
            self.notify("Telefona gönderilemedi", "Bağlantı açılamadı", Icon::Warning);
            return;
        };
        let name = match files.as_slice() {
            [only] => only.name().to_owned(),
            several => format!("{} dosya", several.len()),
        };
        self.island.start_transfer(Transfer {
            name,
            summary: summary(&files),
            total: files.iter().map(|file| file.size).sum(),
            code: Arc::new(code),
            network: sharing::describe(&network),
            warning: network.public.then(|| "Ortak ağ: telefon ulaşamayabilir".to_owned()),
            stage: Stage::Waiting { until: self.now() + WAIT_MS },
        });
        self.sharing = Some(sharing);
    }

    fn on_sharing(&mut self) {
        let Some(events) = self.sharing.as_ref().map(Sharing::events) else { return };
        let now = self.now();
        for event in events {
            match event {
                SharingEvent::Progress(sent) => self.island.transfer_progress(sent),
                SharingEvent::BrokeOff => self.island.transfer_broke_off(now),
                SharingEvent::Done => {
                    self.island.transfer_done(now);
                    self.sharing = None;
                    if let Err(error) = self.renderer.bounce() {
                        eprintln!("bounce failed: {error}");
                    }
                }
                SharingEvent::Expired => {
                    self.island.cancel_transfer();
                    self.sharing = None;
                }
            }
        }
        self.refresh();
    }

    fn start_convert(&mut self, target: Target) {
        if self.to_convert.is_empty() {
            return;
        }
        self.island.set_converting(true);
        self.imaging.convert(std::mem::take(&mut self.to_convert), target);
        self.refresh();
    }

    fn on_imaging(&mut self) {
        for made in self.imaging.results() {
            match made {
                Made::Thumbnail { path, picture } => self.island.set_thumbnail(&path, picture),
                Made::Converted(done) => {
                    self.island.set_converting(false);
                    self.island.close_views();
                    let now = self.now();
                    match done.outputs.first() {
                        Some(first) => {
                            let title = match done.outputs.len() {
                                1 => "Dönüştürüldü".to_owned(),
                                count => format!("{count} görsel dönüştürüldü"),
                            };
                            let detail = match &done.failure {
                                Some(reason) => format!("{} · {reason}", file_name(first)),
                                None => file_name(first).to_owned(),
                            };
                            self.island.upsert(Activity {
                                title,
                                detail,
                                icon: Some(Icon::Convert),
                                preview: done.preview.clone(),
                                file: Some(first.clone()),
                                reveal: true,
                                expires_at: Some(now + CONVERTED_MS),
                                ..Activity::new(Kind::Notification)
                            });
                            self.converted = done.outputs;
                        }
                        None => {
                            let reason = done.failure.unwrap_or_else(|| "Dönüştürülemedi".to_owned());
                            self.notify("Dönüştürülemedi", &reason, Icon::Warning);
                        }
                    }
                }
            }
        }
        self.refresh();
    }

    fn notify(&mut self, title: &str, detail: &str, icon: Icon) {
        let now = self.now();
        self.island.upsert(Activity {
            title: title.to_owned(),
            detail: detail.to_owned(),
            icon: Some(icon),
            expires_at: Some(now + NOTIFICATION_MS),
            ..Activity::new(Kind::Notification)
        });
        self.refresh();
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
        let now = self.now();
        let indicator = match event {
            PowerEvent::Source { on_battery } => self.tracker.power_source_changed(on_battery),
            PowerEvent::Battery(percent) => self.tracker.battery_changed(percent),
            PowerEvent::Brightness(percent) => self.tracker.brightness_changed(percent, now),
            PowerEvent::BatterySaver(on) => {
                self.battery_saver = on;
                self.apply_material();
                self.apply_wave();
                self.tracker.battery_saver_changed(on, now)
            }
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

    fn on_fullscreen(&mut self, reported: bool) {
        self.fullscreen.report(reported, fullscreen::desktop_in_front());
        self.presence.follow_front(reported);
        self.cancel_timer(TIMER_FULLSCREEN);
        self.apply_fullscreen();
    }

    fn on_front_changed(&mut self) {
        self.fullscreen.front_changed(fullscreen::desktop_in_front());
        if self.fullscreen.active() {
            self.set_timer(TIMER_FULLSCREEN, FULLSCREEN_SETTLE_MS);
        } else {
            self.cancel_timer(TIMER_FULLSCREEN);
            self.apply_fullscreen();
        }
    }

    fn apply_fullscreen(&mut self) {
        let active = self.fullscreen.active();
        if active == self.out_for_fullscreen {
            return;
        }
        self.out_for_fullscreen = active;
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

    /// Shows one view of the drop panel, the shelf, a conversion or a send,
    /// with made-up files. Nothing is sent: the code leads nowhere.
    fn show_drop_demo(&mut self, step: u32) {
        let now = self.now();
        let file = |path: &str, size| DroppedFile { path: path.into(), size, is_folder: false };
        let photo = file(r"C:\Demo\Kapadokya.jpg", 2_516_582);
        let report = file(r"C:\Demo\Rapor.pdf", 1_048_576);
        // The pointer is said to rest on the island, which keeps every view open.
        self.island.set_hovered(true, now);
        match step {
            0..=2 => {
                let files = if step == 2 { vec![report] } else { vec![photo] };
                let mut view = DropView::for_files(&files);
                view.hovered = match step {
                    1 => Some(Zone::Phone),
                    2 => Some(Zone::Shelf),
                    _ => None,
                };
                self.island.drag_over(view);
            }
            3 | 4 | 13 => {
                let mut paths = demo_pictures(if step == 4 { 6 } else { 2 });
                paths.push(report.path.clone());
                let added = self.island.drop_on_shelf(&paths, now);
                self.imaging.thumbnails(added, (THUMBNAIL_DIPS * self.placement.scale).ceil() as i32);
                if step == 13 {
                    self.island.close_views();
                }
            }
            5 => self.island.open_convert(summary(std::slice::from_ref(&photo)), now),
            6..=11 => {
                let code = QrMatrix::encode("http://192.168.1.20:52731/q7m2k4vx9pd3hz6tbw5n");
                let Some(code) = code else { return };
                self.island.start_transfer(Transfer {
                    name: photo.name().to_owned(),
                    summary: summary(std::slice::from_ref(&photo)),
                    total: photo.size,
                    code: Arc::new(code),
                    network: "Wi-Fi · Ev".into(),
                    warning: (step == 6).then(|| "Ortak ağ: telefon ulaşamayabilir".to_owned()),
                    stage: Stage::Waiting { until: now + WAIT_MS },
                });
                let (style, permille) = match step {
                    7 => (ProgressStyle::Ring, 400),
                    8 => (ProgressStyle::Edge, 600),
                    9 => (ProgressStyle::Fill, 750),
                    11 => (self.progress_style, 640),
                    _ => (self.progress_style, 0),
                };
                self.island.set_progress_style(style);
                if permille > 0 {
                    self.island.transfer_progress(photo.size * permille / 1000);
                }
                if step == 10 {
                    // Done stays for long enough to be photographed.
                    self.island.transfer_done(now + 60_000);
                }
                if step == 11 {
                    self.island.close_views();
                }
            }
            12 => {
                self.island.upsert(Activity {
                    title: "Dönüştürüldü".into(),
                    detail: "Kapadokya.png".into(),
                    icon: Some(Icon::Convert),
                    file: Some(r"C:\Demo\Kapadokya.png".into()),
                    reveal: true,
                    expires_at: Some(now + 60_000),
                    ..Activity::new(Kind::Notification)
                });
            }
            _ => {}
        }
        self.refresh();
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
            7 => {
                self.island.set_camera(None);
                self.island.remove(Kind::Media);
                self.island.click(now);
            }
            8 => self.island.toggle_timer(now),
            9 => {
                self.island.start_timer(90 * 60_000, now);
                self.island.click(now);
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

/// Pictures Windows ships with, which stand in for the user's own files.
fn demo_pictures(count: usize) -> Vec<String> {
    let folders = [r"C:\Windows\Web\Wallpaper\Windows", r"C:\Windows\Web\Screen", r"C:\Windows\Web\4K\Wallpaper\Windows"];
    let mut pictures: Vec<String> = folders
        .iter()
        .filter_map(|folder| std::fs::read_dir(folder).ok())
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path().to_string_lossy().into_owned()))
        .filter(|path| island_core::files::is_image(path))
        .collect();
    pictures.truncate(count);
    pictures
}

fn demo_track(title: &str, playing: bool, now: Millis) -> MediaSnapshot {
    MediaSnapshot {
        new_position: true,
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

fn stored_material() -> Material {
    settings::read().and_then(|text| config::read_material(&text)).unwrap_or_default()
}

fn stored_sound_wave() -> bool {
    settings::read().is_none_or(|text| config::read_sound_wave(&text))
}

fn ask_running_copy_for_settings() {
    let Some(running) = window::find_running() else { return };
    unsafe {
        let mut process = 0;
        GetWindowThreadProcessId(running, Some(&mut process));
        // Only the app in front may bring another to the front.
        let _ = AllowSetForegroundWindow(process);
        let _ = PostMessageW(Some(running), MSG_OPEN_SETTINGS, WPARAM(0), LPARAM(0));
    }
}

fn stored_progress_style() -> ProgressStyle {
    settings::read().map(|text| config::read_progress_style(&text)).unwrap_or_default()
}

fn apply_settings(island: &mut Island) {
    let text = settings::read().unwrap_or_default();
    island.set_progress_style(config::read_progress_style(&text));
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
            let (x, y) = pointer_position(lparam);
            if let Some(Some(paths)) = with_app(|app| app.on_pointer_moved(x, y)) {
                // The drag runs its own loop, during which the app must be free
                // to answer the messages that arrive.
                if let Err(error) = shell::drag_out(hwnd, &paths) {
                    eprintln!("drag out failed: {error}");
                }
                with_app(App::after_drag_out);
            }
        }
        WM_MOUSELEAVE => {
            with_app(App::on_pointer_left);
        }
        MSG_SHARING => {
            with_app(App::on_sharing);
        }
        MSG_IMAGING => {
            with_app(App::on_imaging);
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
        MSG_FRONT_CHANGED => {
            with_app(App::on_front_changed);
        }
        MSG_OPEN_SETTINGS => {
            with_app(App::open_settings);
        }
        MSG_SETTINGS_CLOSED => {
            with_app(|app| app.settings = None);
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
                with_app(|app| {
                    app.tray.follow_theme();
                    app.apply_material();
                });
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

unsafe extern "system" fn settings_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Painted in full each time, which an erased background would only flash.
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT if with_app(App::paint_settings) == Some(true) => return LRESULT(0),
        WM_MOUSEMOVE => {
            let (x, y) = pointer_position(lparam);
            with_app(|app| app.settings.as_mut().map(|window| window.on_pointer(x, y)));
        }
        WM_MOUSELEAVE => {
            with_app(|app| app.settings.as_mut().map(SettingsWindow::on_pointer_left));
        }
        WM_LBUTTONDOWN => {
            with_app(|app| app.settings.as_ref().map(SettingsWindow::take_focus));
        }
        WM_LBUTTONUP => {
            let (x, y) = pointer_position(lparam);
            with_app(|app| app.on_settings_click(x, y));
        }
        WM_CTLCOLOREDIT => {
            let dc = HDC(wparam.0 as _);
            if let Some(Some(brush)) = with_app(|app| app.settings.as_ref().map(|window| window.color_field(dc))) {
                return brush;
            }
        }
        WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        WM_KEYDOWN => {
            let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
            with_app(|app| app.on_settings_key(VIRTUAL_KEY(wparam.0 as u16), shift));
            return LRESULT(0);
        }
        WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT && wparam.0 == hwnd.0 as usize => {
            let on_control = with_app(|app| app.settings.as_ref().is_some_and(SettingsWindow::pointer_on_control));
            if on_control == Some(true) {
                unsafe { SetCursor(LoadCursorW(None, IDC_HAND).ok()) };
                return LRESULT(1);
            }
        }
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xFFFF) as u32;
            let suggested = unsafe { *(lparam.0 as *const RECT) };
            with_app(|app| app.settings.as_mut().map(|window| window.on_dpi_changed(dpi, &suggested)));
        }
        WM_DESTROY => settings_window::notify_closed(),
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn pointer_position(lparam: LPARAM) -> (i32, i32) {
    (lparam.0 as i16 as i32, (lparam.0 >> 16) as i16 as i32)
}

fn open_tray_menu(hwnd: HWND) {
    match tray::show_menu(hwnd) {
        Some(Command::OpenSettings) => {
            with_app(App::open_settings);
        }
        Some(Command::ReleaseNotes) => tray::open_release_notes(),
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
