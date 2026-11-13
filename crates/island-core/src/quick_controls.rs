//! Quick Controls decisions and hit targets, independent of Windows and drawing.

use crate::activity::Millis;
use crate::geometry::Rect;

pub const DEFAULT_INACTIVITY_MS: Millis = 8_000;
pub const PANEL_WIDTH: f32 = 400.0;
pub const PANEL_HEIGHT: f32 = 170.0;
const BUTTON_SIZE: f32 = 52.0;
const BUTTON_GAP: f32 = 10.0;
const BUTTON_LEFT: f32 = 19.0;
const BUTTON_TOP: f32 = 66.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Wifi,
    Bluetooth,
    Volume,
    Brightness,
    Timer,
    Calendar,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RadioState {
    On,
    Off,
    #[default]
    Unknown,
}

impl Control {
    pub const ALL: [Self; 6] = [
        Self::Wifi,
        Self::Bluetooth,
        Self::Volume,
        Self::Brightness,
        Self::Timer,
        Self::Calendar,
    ];

    pub fn bounds(self) -> Rect {
        let index = Self::ALL
            .iter()
            .position(|control| *control == self)
            .unwrap_or(0);
        Rect {
            x: BUTTON_LEFT + index as f32 * (BUTTON_SIZE + BUTTON_GAP),
            y: BUTTON_TOP,
            width: BUTTON_SIZE,
            height: BUTTON_SIZE,
        }
    }
}

pub const SETTINGS_BOUNDS: Rect = Rect {
    x: 350.0,
    y: 14.0,
    width: 34.0,
    height: 34.0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Control(Control),
    Settings,
    Back,
    Slider,
    Mute,
    CalendarSetup,
    TimerMode(crate::layout::StartMode),
    TimerPreset(u8),
}

impl Target {
    pub const ALL: [Self; 7] = [
        Self::Control(Control::Wifi),
        Self::Control(Control::Bluetooth),
        Self::Control(Control::Volume),
        Self::Control(Control::Brightness),
        Self::Control(Control::Timer),
        Self::Control(Control::Calendar),
        Self::Settings,
    ];

    pub fn at(x: f32, y: f32) -> Option<Self> {
        if SETTINGS_BOUNDS.contains(x, y) {
            return Some(Self::Settings);
        }
        Control::ALL
            .into_iter()
            .find(|control| control.bounds().contains(x, y))
            .map(Self::Control)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Main,
    Timer,
    Calendar,
    Volume,
    Brightness,
}

pub const BACK_BOUNDS: Rect = Rect {
    x: 18.0,
    y: 14.0,
    width: 34.0,
    height: 34.0,
};
pub const SLIDER_BOUNDS: Rect = Rect {
    x: 35.0,
    y: 75.0,
    width: 330.0,
    height: 36.0,
};
pub const MUTE_BOUNDS: Rect = Rect {
    x: 150.0,
    y: 126.0,
    width: 100.0,
    height: 30.0,
};
pub const CALENDAR_SETUP_BOUNDS: Rect = Rect {
    x: 120.0,
    y: 110.0,
    width: 160.0,
    height: 34.0,
};

/// Returns a rounded percentage for a click within the level control.
pub fn slider_percent(x: f32, y: f32) -> Option<u8> {
    SLIDER_BOUNDS.contains(x, y).then(|| {
        (((x - SLIDER_BOUNDS.x) / SLIDER_BOUNDS.width) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u8
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickControls {
    pub page: Page,
    pub focused: Option<Target>,
    inactivity: Option<Millis>,
    deadline: Option<Millis>,
    interacting: bool,
}

impl QuickControls {
    /// `None` keeps the panel open until explicitly dismissed.
    pub fn new(now: Millis, inactivity: Option<Millis>) -> Self {
        Self {
            page: Page::Main,
            focused: None,
            inactivity,
            deadline: inactivity.map(|delay| now.saturating_add(delay)),
            interacting: false,
        }
    }

    pub fn deadline(&self) -> Option<Millis> {
        self.deadline
    }

    pub fn is_interacting(&self) -> bool {
        self.interacting
    }

    pub fn touch(&mut self, now: Millis) {
        self.deadline = if self.interacting {
            None
        } else {
            self.inactivity.map(|delay| now.saturating_add(delay))
        };
    }

    pub fn set_interacting(&mut self, interacting: bool, now: Millis) {
        self.interacting = interacting;
        self.touch(now);
    }

    pub fn set_page(&mut self, page: Page, now: Millis) {
        self.page = page;
        self.focused = None;
        self.touch(now);
    }

    /// Navigation is kept in reading order; the caller paints focus only after keyboard use.
    pub fn move_focus(&mut self, forward: bool, now: Millis) {
        self.move_focus_in(&Target::ALL, forward, now);
    }

    /// Move among the controls actually offered by the current page.
    pub fn move_focus_in(&mut self, targets: &[Target], forward: bool, now: Millis) {
        if targets.is_empty() {
            self.focused = None;
            return;
        }
        let current = self
            .focused
            .and_then(|target| targets.iter().position(|item| *item == target));
        let index = match current {
            Some(index) if forward => (index + 1) % targets.len(),
            Some(index) => (index + targets.len() - 1) % targets.len(),
            None if forward => 0,
            None => targets.len() - 1,
        };
        self.focused = Some(targets[index]);
        self.touch(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_targets_fit_and_keep_the_requested_order() {
        for (index, control) in Control::ALL.into_iter().enumerate() {
            let bounds = control.bounds();
            assert!(bounds.x >= 0.0 && bounds.x + bounds.width <= PANEL_WIDTH);
            assert!(bounds.y >= 0.0 && bounds.y + bounds.height <= PANEL_HEIGHT);
            assert_eq!(
                Target::at(
                    bounds.x + bounds.width / 2.0,
                    bounds.y + bounds.height / 2.0
                ),
                Some(Target::Control(control))
            );
            if index > 0 {
                assert!(Control::ALL[index - 1].bounds().x + BUTTON_SIZE < bounds.x);
            }
        }
        assert_eq!(
            Target::at(SETTINGS_BOUNDS.x + 1.0, SETTINGS_BOUNDS.y + 1.0),
            Some(Target::Settings)
        );
        assert_eq!(Target::at(0.0, 0.0), None);
    }

    #[test]
    fn interaction_pauses_and_restarts_inactivity() {
        let mut state = QuickControls::new(100, Some(1_000));
        assert_eq!(state.deadline(), Some(1_100));
        state.set_interacting(true, 500);
        assert_eq!(state.deadline(), None);
        state.set_interacting(false, 5_000);
        assert_eq!(state.deadline(), Some(6_000));
        state.move_focus(true, 5_500);
        assert_eq!(state.focused, Some(Target::Control(Control::Wifi)));
        assert_eq!(state.deadline(), Some(6_500));
    }

    #[test]
    fn keyboard_wraps_in_both_directions_and_timeout_can_be_disabled() {
        let mut state = QuickControls::new(0, None);
        state.move_focus(false, 1);
        assert_eq!(state.focused, Some(Target::Settings));
        state.move_focus(true, 2);
        assert_eq!(state.focused, Some(Target::Control(Control::Wifi)));
        assert_eq!(state.deadline(), None);
    }

    #[test]
    fn keyboard_focus_tracks_only_the_current_pages_actions() {
        let mut state = QuickControls::new(0, Some(1_000));
        state.set_page(Page::Volume, 10);
        let volume = [Target::Back, Target::Slider, Target::Mute, Target::Settings];
        state.move_focus_in(&volume, true, 20);
        assert_eq!(state.focused, Some(Target::Back));
        state.move_focus_in(&volume, true, 30);
        assert_eq!(state.focused, Some(Target::Slider));
        state.set_page(Page::Calendar, 40);
        state.move_focus_in(&[Target::Back, Target::Settings], false, 50);
        assert_eq!(state.focused, Some(Target::Settings));
        assert_eq!(state.deadline(), Some(1_050));
    }

    #[test]
    fn level_slider_maps_only_its_hit_target_to_percentages() {
        let y = SLIDER_BOUNDS.y + SLIDER_BOUNDS.height / 2.0;
        assert_eq!(slider_percent(SLIDER_BOUNDS.x, y), Some(0));
        assert_eq!(
            slider_percent(SLIDER_BOUNDS.x + SLIDER_BOUNDS.width / 2.0, y),
            Some(50)
        );
        assert_eq!(
            slider_percent(SLIDER_BOUNDS.x + SLIDER_BOUNDS.width - 0.1, y),
            Some(100)
        );
        assert_eq!(slider_percent(SLIDER_BOUNDS.x - 1.0, y), None);
        assert_eq!(slider_percent(SLIDER_BOUNDS.x, SLIDER_BOUNDS.y - 1.0), None);
    }
}
