//! Whether something is full screen where the island is. Windows reports full
//! screen for the whole session: for an app on any screen, for the bare desktop
//! once nothing else is open, and for an invisible overlay that spans a screen.
//! Its word is needed before the island hides, but the island hides only when
//! a window fills the island's own screen in front of it.

#[derive(Clone, Copy, Default)]
pub struct FullscreenWatch {
    reported: bool,
    covers_island: bool,
}

impl FullscreenWatch {
    /// Windows said something is, or no longer is, full screen.
    pub fn report(&mut self, on: bool, covers_island: bool) {
        self.reported = on;
        self.covers_island = covers_island;
    }

    /// Another window came to the front, which Windows may not report: it sees no
    /// change from the desktop, which it calls full screen, to an app that is.
    /// What Windows says now still counts, as a screen it does not notify about.
    pub fn front_changed(&mut self, reported: bool, covers_island: bool) {
        self.reported |= reported;
        self.covers_island = covers_island;
    }

    pub fn active(&self) -> bool {
        self.reported && self.covers_island
    }
}

/// A rectangle in virtual screen pixels, right and bottom exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ScreenRect {
    pub fn intersects(self, other: ScreenRect) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }

    pub fn contains(self, other: ScreenRect) -> bool {
        self.left <= other.left
            && self.top <= other.top
            && self.right >= other.right
            && self.bottom >= other.bottom
    }
}

/// Whether the window in front of the island fills the island's screen. A
/// maximized window with a title bar fills it too when the taskbar hides
/// itself, and is no full screen app.
pub fn fills_screen(window: ScreenRect, screen: ScreenRect, maximized_with_caption: bool) -> bool {
    !maximized_with_caption && window.contains(screen)
}

/// The windows of the desktop and the taskbar, which Windows wrongly counts as
/// full screen. They are looked past for the window in front of the island.
pub fn is_desktop_class(name: &str) -> bool {
    matches!(
        name,
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    const ABOVE: ScreenRect = ScreenRect {
        left: 4,
        top: -1080,
        right: 1924,
        bottom: 0,
    };

    #[test]
    fn what_windows_reports_counts_with_the_island_covered() {
        let mut watch = FullscreenWatch::default();
        watch.report(true, true);
        assert!(watch.active());
        watch.report(false, true);
        assert!(!watch.active());
    }

    #[test]
    fn the_bare_desktop_is_not_full_screen_whatever_windows_says() {
        let mut watch = FullscreenWatch::default();
        watch.report(true, false);
        assert!(!watch.active());
    }

    #[test]
    fn full_screen_on_another_screen_leaves_the_island_alone() {
        let mut watch = FullscreenWatch::default();
        watch.report(true, false);
        watch.front_changed(true, false);
        assert!(!watch.active());
    }

    #[test]
    fn an_app_going_full_screen_from_the_desktop_counts_without_a_new_report() {
        let mut watch = FullscreenWatch::default();
        watch.report(true, false);
        watch.front_changed(false, true);
        assert!(watch.active());
    }

    #[test]
    fn a_screen_windows_does_not_notify_about_counts_by_what_it_says_now() {
        let mut watch = FullscreenWatch::default();
        watch.front_changed(true, true);
        assert!(watch.active());
    }

    #[test]
    fn leaving_a_full_screen_app_brings_the_island_back() {
        let mut watch = FullscreenWatch::default();
        watch.report(true, true);
        watch.front_changed(true, false);
        assert!(!watch.active());
    }

    #[test]
    fn only_a_window_over_the_whole_screen_fills_it() {
        assert!(fills_screen(PRIMARY, PRIMARY, false));
        let game_above = ABOVE;
        assert!(fills_screen(game_above, ABOVE, false));
        assert!(!fills_screen(PRIMARY, ABOVE, false));
        let browser = ScreenRect {
            left: -4,
            top: -1088,
            right: 1932,
            bottom: -40,
        };
        assert!(!fills_screen(browser, ABOVE, false));
    }

    #[test]
    fn a_maximized_window_over_a_hiding_taskbar_is_no_full_screen_app() {
        let maximized = ScreenRect {
            left: -8,
            top: -8,
            right: 1928,
            bottom: 1088,
        };
        assert!(!fills_screen(maximized, PRIMARY, true));
    }

    #[test]
    fn screens_side_by_side_do_not_intersect() {
        assert!(!PRIMARY.intersects(ABOVE));
        let island = ScreenRect {
            left: 734,
            top: -1080,
            right: 1194,
            bottom: -860,
        };
        assert!(island.intersects(ABOVE));
        assert!(!island.intersects(PRIMARY));
    }

    #[test]
    fn only_the_desktop_and_taskbar_count_as_the_desktop() {
        assert!(is_desktop_class("Progman"));
        assert!(is_desktop_class("WorkerW"));
        assert!(is_desktop_class("Shell_TrayWnd"));
        assert!(is_desktop_class("Shell_SecondaryTrayWnd"));
        assert!(!is_desktop_class("Chrome_WidgetWin_1"));
        assert!(!is_desktop_class("ApplicationFrameWindow"));
        assert!(!is_desktop_class(""));
    }
}
