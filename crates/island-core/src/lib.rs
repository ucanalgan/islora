//! Platform-independent island logic. Nothing here touches Windows, so every
//! behaviour can be unit tested with a fake clock.

pub mod activity;
pub mod clock_timer;
pub mod config;
pub mod convert;
pub mod direct;
pub mod drop;
pub mod files;
pub mod firewall;
pub mod focus;
pub mod fullscreen;
pub mod http;
pub mod ics;
pub mod indicator;
pub mod layout;
pub mod media;
pub mod media_kind;
pub mod qr;
pub mod schedule;
pub mod scheduler;
pub mod settings_view;
pub mod shelf;
pub mod timer;
pub mod transfer;

pub use activity::{Activity, Bubble, Icon, Kind, Millis, TimerView};
pub use fullscreen::FullscreenWatch;
pub use indicator::{Indicator, NetworkState, NetworkTracker, SystemTracker};
pub use layout::{IslandStyle, Material, MediaButton, Pill, ProgressStyle, StartMode, TaskLayout, TimerButton};
pub use media::MediaSnapshot;
pub use schedule::Upcoming;
pub use scheduler::{
    CalendarState, ConvertView, FocusView, Frame, HomeView, Island, PanelView, Presentation, Suppression,
};
pub use timer::Timer;
