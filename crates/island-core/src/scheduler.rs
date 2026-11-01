use crate::activity::{Activity, Bubble, Icon, Kind, Millis, TimerView};
use crate::layout::{StartMode, TIMER_PRESETS};
use crate::media::{MediaSnapshot, PAUSED_LINGER_MS, TRACK_NOTICE_MS};
use crate::schedule::{EVENT_LEAD_MS, Upcoming};
use crate::timer::Timer;

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

/// A Windows focus session on the island's clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusView {
    Off,
    /// On, but nothing says for how long.
    Open,
    Running { ends_at: Millis, total: Millis, on_break: bool },
    Paused { remaining: Millis, total: Millis, on_break: bool },
}

/// What should be on screen right now.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub presentation: Presentation,
    /// `None` while hidden, in the home view, or for the clock shown by a peek.
    pub activity: Option<Activity>,
    /// The next most important activity, shown as a bubble beside the pill.
    pub secondary: Option<Activity>,
    pub home: Option<HomeView>,
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
    chime: bool,
    start_mode: StartMode,
    modes_offered: bool,
    presets: [u64; TIMER_PRESETS.len()],
    /// Whether a new track is announced, which the user can turn off.
    announces_tracks: bool,
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
        self.expanded || self.home
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
        if (new_track || up.is_some()) && snapshot.playing && self.announces_tracks && !self.is_open() {
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
        self.home = true;
        self.collapse_at = None;
    }

    /// A click on the bubble opens what it stands for.
    pub fn click_secondary(&mut self, now: Millis) {
        self.update_derived(now);
        if let Some(kind) = self.long_lived(now).get(1).map(|a| a.kind) {
            self.focused = Some(kind);
            self.expanded = true;
            self.collapse_at = None;
        }
    }

    /// Closes whatever is open, once a choice made in it has been carried out.
    pub fn close_views(&mut self) {
        self.close_expanded();
    }

    fn close_expanded(&mut self) {
        self.expanded = false;
        self.home = false;
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
            _ if self.home => self.close_expanded(),
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
        if hovered {
            self.collapse_at = None;
            self.peek_until = None;
        } else {
            self.update_derived(now);
            if self.expanded || self.home {
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
            _ if self.home => self.close_expanded(),
            Some(_) => {
                self.expanded = !self.expanded;
                self.collapse_at = self.expanded.then_some(now + KEYBOARD_EXPAND_MS);
                self.peek_until = self.peek_until.map(|_| now + KEYBOARD_EXPAND_MS);
                if !self.expanded {
                    self.focused = None;
                }
            }
            None if self.peek_until.is_some() => {
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
        let interrupted = top.as_ref().is_some_and(|a| a.kind.is_transient());

        let presentation = match &top {
            _ if self.is_suppressed() => Presentation::Hidden,
            Some(activity) if activity.level.is_some() => Presentation::Level,
            Some(activity) if activity.brief => Presentation::Compact,
            Some(activity) if activity.kind.is_transient() => Presentation::Alert,
            _ if self.home => Presentation::Expanded,
            Some(_) if self.expanded => Presentation::Expanded,
            Some(_) => Presentation::Compact,
            None if self.peek_until.is_some() || self.hovered => Presentation::Compact,
            None => Presentation::Hidden,
        };

        let in_home = self.home && !interrupted && presentation == Presentation::Expanded;
        let shows_bubble = matches!(
            presentation,
            Presentation::Compact | Presentation::Level | Presentation::Alert
        );
        // An interruption covers the pill only. Keeping the bubble means it does
        // not vanish and return on every volume step.
        let secondary = if shows_bubble { self.long_lived(now).get(1).cloned().cloned() } else { None };

        let timer_in_pill = top
            .as_ref()
            .is_some_and(|a| matches!(a.kind, Kind::TimerRunning | Kind::ClockTimer | Kind::Focus));
        Frame {
            presentation,
            activity: if presentation == Presentation::Hidden || in_home { None } else { top },
            secondary,
            home: in_home.then(|| HomeView {
                coming: self
                    .schedule
                    .iter()
                    .filter(|event| !self.is_hidden(event))
                    .take(HOME_EVENTS)
                    .cloned()
                    .collect(),
                calendar: self.calendar,
                mode: self.start_mode,
                modes_offered: self.modes_offered,
                presets: self.presets,
            }),
            chime: std::mem::take(&mut self.chime),
            wake_at: self.next_deadline(now, timer_in_pill),
        }
    }

    /// Keeps the activities that depend on time up to date, and raises what
    /// falls due: a finished timer, or an event that is starting.
    fn update_derived(&mut self, now: Millis) {
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
                detail: "Odak oturumu".into(),
                icon: Some(Icon::Focus),
                timer: Some(TimerView { running: true, has_controls: true }),
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

    /// Everything that lasts, most important first.
    fn long_lived(&self, now: Millis) -> Vec<&Activity> {
        let peeking = self.peek_until.is_some() || self.hovered;
        let mut lasting: Vec<&Activity> = self
            .activities
            .iter()
            .filter(|a| !a.kind.is_transient())
            .filter(|a| peeking || a.dormant_at.is_none_or(|at| at > now))
            .collect();
        lasting.sort_by_key(|a| a.order(now, self.focused));
        lasting
    }

    /// The highest priority activity that shows without being asked for.
    fn awake(&self, now: Millis) -> Option<&Activity> {
        let awake = self.activities.iter().filter(|a| a.dormant_at.is_none_or(|at| at > now));
        awake.min_by_key(|a| a.order(now, self.focused))
    }

    /// What is on display, which includes dormant activities during a peek.
    fn top(&self, now: Millis) -> Option<&Activity> {
        let peeking = self.peek_until.is_some() || self.hovered;
        let any = self.activities.iter().min_by_key(|a| a.order(now, self.focused));
        self.awake(now).or(if peeking { any } else { None })
    }

    fn expire(&mut self, now: Millis) {
        self.activities.retain(|a| a.expires_at.is_none_or(|at| at > now));
    }

    fn next_deadline(&self, now: Millis, timer_in_pill: bool) -> Option<Millis> {
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
            .chain(self.collapse_at)
            .chain(self.peek_until)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(frame.activity.unwrap().trailing, "Açık");
        assert_eq!(frame.wake_at, None);
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
}
