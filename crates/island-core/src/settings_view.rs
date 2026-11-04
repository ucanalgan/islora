//! What the settings window shows and where, in DIPs. Nothing here draws, so the
//! layout and what each click means can be tested without a window.

use crate::layout::{IslandStyle, Material, StartMode};

pub const WINDOW_WIDTH: f32 = 760.0;
pub const WINDOW_HEIGHT: f32 = 540.0;
pub const SIDEBAR_WIDTH: f32 = 190.0;

const LEFT: f32 = SIDEBAR_WIDTH + 28.0;
const WIDTH: f32 = WINDOW_WIDTH - LEFT - 28.0;
const TOP: f32 = 72.0;
const GAP: f32 = 10.0;
const ROW_HEIGHT: f32 = 46.0;
const CHOICE_HEIGHT: f32 = 44.0;
const BUTTON_HEIGHT: f32 = 40.0;
const STEP_SIZE: f32 = 32.0;

/// How far one press of a duration's buttons moves it, in minutes.
const PRESET_STEP: u64 = 5;
const LONGEST_PRESET: u64 = 720;

/// Where the small picture of the island goes on the appearance page.
pub const PREVIEW: Rect = Rect { x: LEFT, y: TOP, width: WIDTH, height: 112.0 };
/// The box for the calendar address, which Windows draws itself.
pub const ADDRESS_FIELD: Rect = Rect { x: LEFT, y: TOP + 26.0, width: WIDTH, height: 40.0 };

/// Every notice that can be turned off: its name in the settings file and its label.
pub const NOTICES: [(&str, &str); 16] = [
    ("audio_output", "Ses çıkışı"),
    ("microphone", "Mikrofon"),
    ("microphone_use", "Mikrofon kullanımı"),
    ("camera", "Kamera"),
    ("internet", "İnternet"),
    ("wifi", "Wi-Fi"),
    ("vpn", "VPN"),
    ("airplane", "Uçak modu"),
    ("bluetooth_battery", "Bluetooth pili"),
    ("usb", "USB"),
    ("display", "Ekranlar"),
    ("screenshot", "Ekran görüntüsü"),
    ("clipboard", "Kopyalama"),
    ("track_change", "Şarkı değişimi"),
    ("battery_saver", "Enerji tasarrufu"),
    ("keyboard_language", "Klavye dili"),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Appearance,
    Notices,
    Calendar,
    Durations,
    General,
}

impl Page {
    pub const ALL: [Page; 5] =
        [Page::Appearance, Page::Notices, Page::Calendar, Page::Durations, Page::General];

    pub fn label(self) -> &'static str {
        match self {
            Page::Appearance => "Görünüm",
            Page::Notices => "Bildirimler",
            Page::Calendar => "Takvim",
            Page::Durations => "Süreler",
            Page::General => "Genel",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// What a click on a control asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Open(Page),
    Material(Material),
    Style(IslandStyle),
    SoundWave,
    Notice(usize),
    Preset { index: usize, longer: bool },
    Mode(StartMode),
    SaveCalendar,
    Autostart,
    Demo,
    Reload,
    EditFile,
    Quit,
}

/// What the keyboard is on: a control, or the box for the calendar address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Control(Action),
    Address,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Tab { selected: bool },
    Choice { selected: bool },
    Switch { on: bool },
    Button,
    Step,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    pub action: Action,
    pub look: Look,
    pub bounds: Rect,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub text: String,
    pub bounds: Rect,
    pub size: f32,
    pub bold: bool,
    pub opacity: f32,
    pub centered: bool,
}

/// What the window reflects. The app fills this from what is really in effect.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsState {
    pub page: Page,
    pub material: Material,
    pub style: IslandStyle,
    pub sound_wave: bool,
    pub notices_off: Vec<String>,
    pub presets: [u64; 4],
    pub mode: StartMode,
    pub autostart: bool,
    pub demo_running: bool,
    /// The answer to the last attempt to save the calendar address.
    pub calendar_note: Option<&'static str>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub controls: Vec<Control>,
    pub labels: Vec<Label>,
}

impl View {
    pub fn action_at(&self, x: f32, y: f32) -> Option<Action> {
        self.controls.iter().find(|control| control.bounds.contains(x, y)).map(|control| control.action)
    }

    pub fn bounds_of(&self, action: Action) -> Option<Rect> {
        self.controls.iter().find(|control| control.action == action).map(|control| control.bounds)
    }

    /// What Tab walks through, in reading order: the pages, then the page.
    pub fn focus_order(&self, page: Page) -> Vec<Focus> {
        let mut order: Vec<Focus> = self.controls.iter().map(|control| Focus::Control(control.action)).collect();
        if page == Page::Calendar {
            let save = order.iter().position(|focus| *focus == Focus::Control(Action::SaveCalendar));
            order.insert(save.unwrap_or(order.len()), Focus::Address);
        }
        order
    }

    /// Where the keyboard goes next, round from the last back to the first.
    pub fn step_focus(&self, page: Page, current: Option<Focus>, forward: bool) -> Option<Focus> {
        let order = self.focus_order(page);
        let count = order.len();
        if count == 0 {
            return None;
        }
        let next = match current.and_then(|focus| order.iter().position(|entry| *entry == focus)) {
            Some(index) if forward => (index + 1) % count,
            Some(index) => (index + count - 1) % count,
            None if forward => 0,
            None => count - 1,
        };
        Some(order[next])
    }

    fn control(&mut self, action: Action, look: Look, bounds: Rect, label: impl Into<String>) {
        self.controls.push(Control { action, look, bounds, label: label.into() });
    }

    fn caption(&mut self, text: impl Into<String>, y: f32) {
        let bounds = Rect { x: LEFT, y, width: WIDTH, height: 20.0 };
        self.labels.push(Label { text: text.into(), bounds, size: 12.5, bold: false, opacity: 0.65, centered: false });
    }

    /// Two equal choices side by side.
    fn choices<const N: usize>(&mut self, y: f32, items: [(Action, bool, &str); N]) {
        let width = (WIDTH - GAP * (N - 1) as f32) / N as f32;
        for (index, (action, selected, label)) in items.into_iter().enumerate() {
            let x = LEFT + index as f32 * (width + GAP);
            let bounds = Rect { x, y, width, height: CHOICE_HEIGHT };
            self.control(action, Look::Choice { selected }, bounds, label);
        }
    }
}

pub fn view(state: &SettingsState) -> View {
    let mut view = View::default();
    view.labels.push(Label {
        text: "Islora".to_owned(),
        bounds: Rect { x: 26.0, y: 22.0, width: SIDEBAR_WIDTH - 40.0, height: 32.0 },
        size: 20.0,
        bold: true,
        opacity: 1.0,
        centered: false,
    });
    for (index, page) in Page::ALL.into_iter().enumerate() {
        let bounds = Rect { x: 14.0, y: 66.0 + index as f32 * 42.0, width: SIDEBAR_WIDTH - 28.0, height: 38.0 };
        view.control(Action::Open(page), Look::Tab { selected: page == state.page }, bounds, page.label());
    }
    view.labels.push(Label {
        text: state.page.label().to_owned(),
        bounds: Rect { x: LEFT, y: 22.0, width: WIDTH, height: 36.0 },
        size: 24.0,
        bold: true,
        opacity: 1.0,
        centered: false,
    });
    match state.page {
        Page::Appearance => appearance(&mut view, state),
        Page::Notices => notices(&mut view, state),
        Page::Calendar => calendar(&mut view, state),
        Page::Durations => durations(&mut view, state),
        Page::General => general(&mut view, state),
    }
    view
}

fn appearance(view: &mut View, state: &SettingsState) {
    view.caption("Seçimlerin gerçek island'a da hemen uygulanır", PREVIEW.y + PREVIEW.height + 6.0);
    view.caption("Tasarım", 214.0);
    view.choices(
        236.0,
        [
            (Action::Material(Material::Black), state.material == Material::Black, "Siyah"),
            (Action::Material(Material::Glass), state.material == Material::Glass, "Cam"),
        ],
    );
    view.caption("Biçim", 288.0);
    view.choices(
        310.0,
        [
            (Action::Style(IslandStyle::Island), state.style == IslandStyle::Island, "Island"),
            (Action::Style(IslandStyle::Notch), state.style == IslandStyle::Notch, "Notch"),
        ],
    );
    let row = Rect { x: LEFT, y: 366.0, width: WIDTH, height: ROW_HEIGHT };
    view.control(Action::SoundWave, Look::Switch { on: state.sound_wave }, row, "Ses dalgası");
    view.caption("Kapalıyken sabit çubuklar görünür. Enerji tasarrufunda dalga kendiliğinden durur", 416.0);
    view.caption("Cam, enerji tasarrufunda ve Windows şeffaflığı kapalıyken Siyah görünür", 438.0);
}

fn notices(view: &mut View, state: &SettingsState) {
    const COLUMNS: usize = 2;
    let width = (WIDTH - GAP) / COLUMNS as f32;
    let rows = NOTICES.len().div_ceil(COLUMNS);
    for (index, (setting, label)) in NOTICES.into_iter().enumerate() {
        let column = index / rows;
        let row = index % rows;
        let bounds = Rect {
            x: LEFT + column as f32 * (width + GAP),
            y: TOP + row as f32 * (ROW_HEIGHT + 6.0),
            width,
            height: ROW_HEIGHT,
        };
        let on = !state.notices_off.iter().any(|name| name == setting);
        view.control(Action::Notice(index), Look::Switch { on }, bounds, label);
    }
}

fn calendar(view: &mut View, state: &SettingsState) {
    view.caption("Takvim adresi", TOP);
    view.caption("Google Takvim'de: Ayarlar > takviminin adı > Takvimi entegre et >", TOP + 72.0);
    view.caption("iCal biçiminde gizli adres. Bu adres şifre gibidir, kimseyle paylaşma.", TOP + 92.0);
    let button = Rect { x: LEFT, y: TOP + 126.0, width: 190.0, height: BUTTON_HEIGHT };
    view.control(Action::SaveCalendar, Look::Button, button, "Kaydet ve yenile");
    if let Some(note) = state.calendar_note {
        view.caption(note, TOP + 176.0);
    }
}

fn durations(view: &mut View, state: &SettingsState) {
    view.caption("Ana görünümdeki süre düğmeleri", TOP);
    let width = (WIDTH - GAP * 3.0) / 4.0;
    for (index, minutes) in state.presets.into_iter().enumerate() {
        let x = LEFT + index as f32 * (width + GAP);
        let y = TOP + 26.0;
        let shorter = Rect { x, y, width: STEP_SIZE, height: STEP_SIZE };
        let longer = Rect { x: x + width - STEP_SIZE, y, width: STEP_SIZE, height: STEP_SIZE };
        view.control(Action::Preset { index, longer: false }, Look::Step, shorter, "−");
        view.control(Action::Preset { index, longer: true }, Look::Step, longer, "+");
        view.labels.push(Label {
            text: format!("{minutes} dk"),
            bounds: Rect { x: x + STEP_SIZE, y, width: width - STEP_SIZE * 2.0, height: STEP_SIZE },
            size: 14.0,
            bold: true,
            opacity: 1.0,
            centered: true,
        });
    }
    view.caption("Düğmeler ne başlatsın", TOP + 84.0);
    view.choices(
        TOP + 108.0,
        [
            (Action::Mode(StartMode::Focus), state.mode == StartMode::Focus, "Odak"),
            (Action::Mode(StartMode::Timer), state.mode == StartMode::Timer, "Zamanlayıcı"),
        ],
    );
    view.caption("Odak, Windows Odak oturumu başlatır ve bildirimleri susturabilir", TOP + 160.0);
}

fn general(view: &mut View, state: &SettingsState) {
    let row = Rect { x: LEFT, y: TOP, width: WIDTH, height: ROW_HEIGHT };
    view.control(Action::Autostart, Look::Switch { on: state.autostart }, row, "Windows ile başlat");
    let demo = if state.demo_running { "Demoyu durdur" } else { "Demoyu başlat" };
    let buttons = [
        (Action::Demo, demo),
        (Action::Reload, "Takvimi ve ayarları yenile"),
        (Action::EditFile, "Ayar dosyasını aç"),
        (Action::Quit, "Islora'dan çık"),
    ];
    for (index, (action, label)) in buttons.into_iter().enumerate() {
        let y = TOP + 66.0 + index as f32 * (BUTTON_HEIGHT + GAP);
        let bounds = Rect { x: LEFT, y, width: 240.0, height: BUTTON_HEIGHT };
        view.control(action, Look::Button, bounds, label);
    }
    view.caption(format!("Islora {}", env!("CARGO_PKG_VERSION")), WINDOW_HEIGHT - 44.0);
}

/// The next duration in that direction, always on a whole step.
pub fn stepped_preset(minutes: u64, longer: bool) -> u64 {
    let steps = minutes / PRESET_STEP;
    let next = if longer {
        steps + 1
    } else if minutes.is_multiple_of(PRESET_STEP) {
        steps.saturating_sub(1)
    } else {
        steps
    };
    (next * PRESET_STEP).clamp(PRESET_STEP, LONGEST_PRESET)
}

/// The list with that notice switched over, kept in the order it was in.
pub fn toggled_notice(off: &[String], name: &str) -> Vec<String> {
    if off.iter().any(|entry| entry == name) {
        off.iter().filter(|entry| *entry != name).cloned().collect()
    } else {
        off.iter().cloned().chain([name.to_owned()]).collect()
    }
}

/// Whether this can be a calendar address. Blank clears the setting.
pub fn is_calendar_address(text: &str) -> bool {
    text.is_empty()
        || (text.starts_with("https://") || text.starts_with("http://"))
            && !text.contains(|c: char| c == '"' || c.is_whitespace() || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(page: Page) -> SettingsState {
        SettingsState {
            page,
            material: Material::Black,
            style: IslandStyle::Island,
            sound_wave: true,
            notices_off: Vec::new(),
            presets: [5, 15, 25, 45],
            mode: StartMode::Focus,
            autostart: false,
            demo_running: false,
            calendar_note: Some("Kaydedildi"),
        }
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
    }

    #[test]
    fn every_page_stays_inside_the_window_without_overlaps() {
        for page in Page::ALL {
            let view = view(&state(page));
            let mut rects: Vec<Rect> = view.controls.iter().map(|control| control.bounds).collect();
            rects.extend(view.labels.iter().map(|label| label.bounds));
            if page == Page::Appearance {
                rects.push(PREVIEW);
            }
            if page == Page::Calendar {
                rects.push(ADDRESS_FIELD);
            }
            for (index, rect) in rects.iter().enumerate() {
                assert!(rect.x >= 0.0 && rect.x + rect.width <= WINDOW_WIDTH, "{page:?} {rect:?}");
                assert!(rect.y >= 0.0 && rect.y + rect.height <= WINDOW_HEIGHT, "{page:?} {rect:?}");
                for other in &rects[index + 1..] {
                    assert!(!overlaps(*rect, *other), "{page:?} {rect:?} {other:?}");
                }
            }
        }
    }

    #[test]
    fn the_sidebar_opens_every_page_from_every_page() {
        for from in Page::ALL {
            let view = view(&state(from));
            for to in Page::ALL {
                let tab = view.controls.iter().find(|control| control.action == Action::Open(to)).unwrap();
                assert_eq!(tab.look, Look::Tab { selected: to == from });
                let (x, y) = (tab.bounds.x + 5.0, tab.bounds.y + 5.0);
                assert_eq!(view.action_at(x, y), Some(Action::Open(to)));
            }
        }
    }

    #[test]
    fn tab_walks_the_pages_then_the_page_and_comes_round() {
        let current = state(Page::Appearance);
        let view = view(&current);
        let first = view.step_focus(current.page, None, true);
        assert_eq!(first, Some(Focus::Control(Action::Open(Page::Appearance))));
        let order = view.focus_order(current.page);
        assert_eq!(order[Page::ALL.len()], Focus::Control(Action::Material(Material::Black)));
        let last = *order.last().unwrap();
        assert_eq!(view.step_focus(current.page, Some(last), true), first);
        assert_eq!(view.step_focus(current.page, first, false), Some(last));
        assert_eq!(view.step_focus(current.page, None, false), Some(last));
    }

    #[test]
    fn the_address_box_comes_before_its_save_button() {
        let current = state(Page::Calendar);
        let order = view(&current).focus_order(current.page);
        let address = order.iter().position(|focus| *focus == Focus::Address).unwrap();
        assert_eq!(order[address + 1], Focus::Control(Action::SaveCalendar));
        assert!(!view(&state(Page::General)).focus_order(Page::General).contains(&Focus::Address));
    }

    #[test]
    fn a_click_on_nothing_does_nothing() {
        let view = view(&state(Page::Appearance));
        assert_eq!(view.action_at(PREVIEW.x + 5.0, PREVIEW.y + 5.0), None);
        assert_eq!(view.action_at(-1.0, -1.0), None);
    }

    #[test]
    fn the_appearance_page_shows_what_is_in_effect() {
        let mut current = state(Page::Appearance);
        current.material = Material::Glass;
        current.style = IslandStyle::Notch;
        current.sound_wave = false;
        let view = view(&current);
        let look = |action| view.controls.iter().find(|control| control.action == action).unwrap().look;
        assert_eq!(look(Action::Material(Material::Glass)), Look::Choice { selected: true });
        assert_eq!(look(Action::Material(Material::Black)), Look::Choice { selected: false });
        assert_eq!(look(Action::Style(IslandStyle::Notch)), Look::Choice { selected: true });
        assert_eq!(look(Action::Style(IslandStyle::Island)), Look::Choice { selected: false });
        assert_eq!(look(Action::SoundWave), Look::Switch { on: false });
    }

    #[test]
    fn a_turned_off_notice_shows_as_off() {
        let mut current = state(Page::Notices);
        current.notices_off = vec!["wifi".to_owned()];
        let view = view(&current);
        let wifi = NOTICES.iter().position(|(setting, _)| *setting == "wifi").unwrap();
        let switches = view.controls.iter().filter_map(|control| match (control.action, control.look) {
            (Action::Notice(index), Look::Switch { on }) => Some((index, on)),
            _ => None,
        });
        for (index, on) in switches {
            assert_eq!(on, index != wifi);
        }
    }

    #[test]
    fn every_notice_has_its_own_setting_name() {
        let mut names: Vec<_> = NOTICES.iter().map(|(setting, _)| *setting).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), NOTICES.len());
    }

    #[test]
    fn durations_move_in_whole_steps_within_their_limits() {
        assert_eq!(stepped_preset(25, true), 30);
        assert_eq!(stepped_preset(25, false), 20);
        assert_eq!(stepped_preset(7, true), 10);
        assert_eq!(stepped_preset(7, false), 5);
        assert_eq!(stepped_preset(5, false), 5);
        assert_eq!(stepped_preset(1, false), 5);
        assert_eq!(stepped_preset(720, true), 720);
    }

    #[test]
    fn toggling_a_notice_keeps_the_others() {
        let off = vec!["wifi".to_owned(), "usb".to_owned()];
        assert_eq!(toggled_notice(&off, "usb"), vec!["wifi".to_owned()]);
        assert_eq!(toggled_notice(&off, "vpn"), vec!["wifi".to_owned(), "usb".to_owned(), "vpn".to_owned()]);
    }

    #[test]
    fn a_calendar_address_must_be_one_line_on_the_web() {
        assert!(is_calendar_address(""));
        assert!(is_calendar_address("https://example.com/basic.ics"));
        assert!(!is_calendar_address("example.com/basic.ics"));
        assert!(!is_calendar_address("https://example.com/a b.ics"));
        assert!(!is_calendar_address("https://example.com/a\".ics"));
    }
}
