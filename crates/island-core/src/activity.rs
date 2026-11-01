use std::sync::Arc;

use crate::media::MediaSnapshot;

/// Milliseconds on a monotonic clock supplied by the caller.
pub type Millis = u64;

/// Declaration order is display priority: earlier variants win.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Alert,
    Notification,
    TimerFinished,
    NowPlaying,
    Media,
    Camera,
    Focus,
    TimerRunning,
    ClockTimer,
    Upcoming,
}

impl Kind {
    /// Transient kinds interrupt whatever is showing and leave on their own.
    pub fn is_transient(self) -> bool {
        matches!(self, Kind::Alert | Kind::Notification | Kind::TimerFinished | Kind::NowPlaying)
    }
}

/// Symbols the island can draw. Each maps to a system font glyph in the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Volume,
    Muted,
    Brightness,
    Keyboard,
    Charging,
    Battery,
    BatteryLow,
    Bluetooth,
    Timer,
    Focus,
    Calendar,
    Music,
    Speaker,
    Microphone,
    MicrophoneOff,
    Camera,
    Internet,
    InternetOff,
    Wifi,
    Vpn,
    Airplane,
    Usb,
    Monitor,
    Clipboard,
}

/// What an activity shows when it only gets the small bubble beside the pill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bubble {
    Artwork(Arc<[u8]>),
    Symbol(Icon),
    /// A short number, with an optional symbol or a ring filled to this percent.
    Count { value: String, icon: Option<Icon>, ring: Option<u8> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerView {
    pub running: bool,
    /// Whether it can be paused and cancelled from the island.
    pub has_controls: bool,
}

/// One thing the island can show. There is at most one activity per [`Kind`]:
/// a newer one replaces the older.
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub kind: Kind,
    /// Left side of the compact pill.
    pub leading: String,
    /// Right side of the compact pill.
    pub trailing: String,
    pub title: String,
    pub detail: String,
    /// Removed entirely from this moment on.
    pub expires_at: Option<Millis>,
    /// From this moment on it stops showing by itself, yet stays reachable
    /// through a peek.
    pub dormant_at: Option<Millis>,
    pub media: Option<MediaSnapshot>,
    pub icon: Option<Icon>,
    /// A percentage to show as a bar, which also selects the slim pill.
    pub level: Option<u8>,
    /// From this moment on it outranks everything that is not an interruption.
    pub urgent_at: Option<Millis>,
    /// How it looks as a bubble, unless derived from its media.
    pub badge: Option<Bubble>,
    pub timer: Option<TimerView>,
    /// An encoded image shown instead of the symbol on an alert.
    pub preview: Option<Arc<[u8]>>,
    /// A file that a click on the alert opens.
    pub file: Option<String>,
    /// A small confirmation, which uses the compact pill instead of an alert.
    pub brief: bool,
}

impl Activity {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            leading: String::new(),
            trailing: String::new(),
            title: String::new(),
            detail: String::new(),
            expires_at: None,
            dormant_at: None,
            media: None,
            icon: None,
            level: None,
            urgent_at: None,
            badge: None,
            timer: None,
            preview: None,
            file: None,
            brief: false,
        }
    }

    pub fn bubble(&self) -> Bubble {
        if let Some(badge) = &self.badge {
            return badge.clone();
        }
        match self.media.as_ref().and_then(|media| media.artwork.clone()) {
            Some(artwork) => Bubble::Artwork(artwork),
            None if self.media.is_some() => Bubble::Symbol(Icon::Music),
            None => Bubble::Symbol(self.icon.unwrap_or(Icon::Music)),
        }
    }

    fn rank(&self, now: Millis) -> (u8, Kind) {
        let group = if self.kind.is_transient() {
            0
        } else if self.urgent_at.is_some_and(|at| at <= now) {
            1
        } else {
            2
        };
        (group, self.kind)
    }

    /// Lower comes first. `focused` is what the user chose to open.
    pub fn order(&self, now: Millis, focused: Option<Kind>) -> (u8, u8, Kind) {
        let (group, kind) = self.rank(now);
        let chosen = focused == Some(self.kind) && group > 0;
        (group.min(1), if chosen { 0 } else { group + 1 }, kind)
    }
}
