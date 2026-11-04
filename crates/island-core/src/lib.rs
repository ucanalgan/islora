//! Platform-independent island logic. Nothing here touches Windows, so every
//! behaviour can be unit tested with a fake clock.

pub mod activity;
pub mod clock_timer;
pub mod config;
pub mod focus;
pub mod fullscreen;
pub mod ics;
pub mod indicator;
pub mod layout;
pub mod media;
pub mod schedule;
pub mod scheduler;
pub mod settings_view;
pub mod timer;

pub use activity::{Activity, Bubble, Icon, Kind, Millis, TimerView};
pub use fullscreen::FullscreenWatch;
pub use indicator::{Indicator, NetworkState, NetworkTracker, SystemTracker};
pub use layout::{IslandStyle, Material, MediaButton, Pill, StartMode, TimerButton};
pub use media::MediaSnapshot;
pub use schedule::Upcoming;
pub use scheduler::{CalendarState, FocusView, Frame, HomeView, Island, Presentation, Suppression};
pub use timer::Timer;
