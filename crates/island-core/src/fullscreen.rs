//! Whether something really fills the screen. Windows reports this for the app to
//! hide, but it says the same about the bare desktop once nothing else is open.

#[derive(Clone, Copy, Default)]
pub struct FullscreenWatch {
    reported: bool,
    front_fills_screen: bool,
}

impl FullscreenWatch {
    /// Windows said something is, or no longer is, full screen.
    pub fn report(&mut self, on: bool, front_fills_screen: bool) {
        self.reported = on;
        self.front_fills_screen = front_fills_screen;
    }

    /// Another window came to the front, which Windows may not report: it sees no
    /// change from the desktop, which it calls full screen, to an app that is.
    pub fn front_changed(&mut self, front_fills_screen: bool) {
        self.front_fills_screen = front_fills_screen;
    }

    pub fn active(&self) -> bool {
        self.reported && self.front_fills_screen
    }
}

/// The two window classes that draw the desktop. They fill the screen like an app.
pub fn is_desktop_class(name: &str) -> bool {
    matches!(name, "Progman" | "WorkerW")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_filling_the_screen_is_full_screen() {
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
    fn an_app_going_full_screen_from_the_desktop_counts_without_a_new_report() {
        let mut watch = FullscreenWatch::default();
        watch.report(true, false);
        watch.front_changed(true);
        assert!(watch.active());
    }

    #[test]
    fn leaving_a_full_screen_app_for_the_desktop_brings_the_island_back() {
        let mut watch = FullscreenWatch::default();
        watch.report(true, true);
        watch.front_changed(false);
        assert!(!watch.active());
    }

    #[test]
    fn only_the_desktop_classes_are_the_desktop() {
        assert!(is_desktop_class("Progman"));
        assert!(is_desktop_class("WorkerW"));
        assert!(!is_desktop_class("Shell_TrayWnd"));
        assert!(!is_desktop_class("Chrome_WidgetWin_1"));
        assert!(!is_desktop_class(""));
    }
}
