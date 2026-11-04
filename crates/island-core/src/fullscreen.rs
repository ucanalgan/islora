//! Whether something is full screen. Windows reports this for the app to hide,
//! and is trusted, except that it says the same about the bare desktop once
//! nothing else is open, and about the taskbar in front of it.

#[derive(Clone, Copy, Default)]
pub struct FullscreenWatch {
    reported: bool,
    desktop_in_front: bool,
}

impl FullscreenWatch {
    /// Windows said something is, or no longer is, full screen.
    pub fn report(&mut self, on: bool, desktop_in_front: bool) {
        self.reported = on;
        self.desktop_in_front = desktop_in_front;
    }

    /// Another window came to the front, which Windows may not report: it sees no
    /// change from the desktop, which it calls full screen, to an app that is.
    pub fn front_changed(&mut self, desktop_in_front: bool) {
        self.desktop_in_front = desktop_in_front;
    }

    pub fn active(&self) -> bool {
        self.reported && !self.desktop_in_front
    }
}

/// The windows of the desktop and the taskbar, which Windows wrongly counts as
/// full screen. Anything else in front, even a small window on another
/// screen, leaves Windows' word standing.
pub fn is_desktop_class(name: &str) -> bool {
    matches!(name, "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_windows_reports_counts_with_an_app_in_front() {
        let mut watch = FullscreenWatch::default();
        watch.report(true, false);
        assert!(watch.active());
        watch.report(false, false);
        assert!(!watch.active());
    }

    #[test]
    fn the_bare_desktop_is_not_full_screen_whatever_windows_says() {
        let mut watch = FullscreenWatch::default();
        watch.report(true, true);
        assert!(!watch.active());
    }

    #[test]
    fn an_app_going_full_screen_from_the_desktop_counts_without_a_new_report() {
        let mut watch = FullscreenWatch::default();
        watch.report(true, true);
        watch.front_changed(false);
        assert!(watch.active());
    }

    #[test]
    fn leaving_a_full_screen_app_for_the_desktop_brings_the_island_back() {
        let mut watch = FullscreenWatch::default();
        watch.report(true, false);
        watch.front_changed(true);
        assert!(!watch.active());
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
