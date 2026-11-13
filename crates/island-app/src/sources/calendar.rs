//! Keeps the coming events fresh from a calendar's private iCalendar address.
//!
//! The only periodic work in the app, and the only one that uses the network.
//! The address is a secret: it stays in the settings file and is never logged.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use island_core::config::read_value;
use island_core::ics::{self, Event};
use windows::Foundation::Uri;
use windows::Web::Http::HttpClient;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{HSTRING, Result};

use crate::platform::settings;

pub const MSG_CALENDAR: u32 = WM_APP + 7;

const REFRESH_EVERY: Duration = Duration::from_secs(15 * 60);
/// A laptop waking up has no network for a moment, and a quarter of an hour is
/// too long to go without events because of that.
const RETRY_AFTER: [Duration; 2] = [Duration::from_secs(60), Duration::from_secs(5 * 60)];
/// Waking up only refreshes events older than this.
const STALE_AFTER: Duration = Duration::from_secs(5 * 60);
const LOOK_BEHIND_MS: i64 = 86_400_000;
const LOOK_AHEAD_MS: i64 = 14 * 86_400_000;

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub enum CalendarUpdate {
    Events(Vec<Event>),
    NoAddress,
    /// The events from before stay valid until a download works again.
    Failed,
}

enum Request {
    Now,
    IfStale,
}

pub struct CalendarSource {
    requests: Sender<Request>,
    updates: Receiver<CalendarUpdate>,
}

impl CalendarSource {
    /// `test_file` reads a local file instead of the address in the settings.
    pub fn start(hwnd: HWND, test_file: Option<String>) -> Self {
        let (requests, inbox) = mpsc::channel();
        let (publish, updates) = mpsc::channel();
        let window = hwnd.0 as isize;
        let spawned = std::thread::Builder::new()
            .name("calendar".into())
            .spawn(move || {
                run(window, test_file, inbox, publish);
            });
        if let Err(error) = spawned {
            eprintln!("calendar thread failed to start: {error}");
        }
        Self { requests, updates }
    }

    pub fn refresh(&self) {
        let _ = self.requests.send(Request::Now);
    }

    pub fn refresh_if_stale(&self) {
        let _ = self.requests.send(Request::IfStale);
    }

    pub fn latest(&self) -> Option<CalendarUpdate> {
        self.updates.try_iter().last()
    }
}

fn run(
    window: isize,
    test_file: Option<String>,
    inbox: Receiver<Request>,
    publish: Sender<CalendarUpdate>,
) {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let mut next = Instant::now();
    let mut fresh_since: Option<Instant> = None;
    let mut failures = 0;
    loop {
        match inbox.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(Request::IfStale) if fresh_since.is_some_and(|at| at.elapsed() < STALE_AFTER) => {
                continue;
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        // Settings are read each time, so a newly added address works at once.
        let update = load(test_file.as_deref());
        if matches!(update, CalendarUpdate::Failed) {
            next = Instant::now() + RETRY_AFTER.get(failures).copied().unwrap_or(REFRESH_EVERY);
            failures += 1;
        } else {
            fresh_since = Some(Instant::now());
            next = Instant::now() + REFRESH_EVERY;
            failures = 0;
        }
        if publish.send(update).is_err() {
            return;
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(window as _)), MSG_CALENDAR, WPARAM(0), LPARAM(0));
        }
    }
}

fn load(test_file: Option<&str>) -> CalendarUpdate {
    let text = match test_file {
        Some(path) => std::fs::read_to_string(path).ok(),
        None => {
            let address = settings::read().and_then(|text| read_value(&text, "calendar_ics_url"));
            let Some(address) = address else {
                return CalendarUpdate::NoAddress;
            };
            // Offline is normal. The error text could contain the secret address.
            download(&address)
                .inspect_err(|_| eprintln!("calendar download failed"))
                .ok()
        }
    };
    let Some(text) = text else {
        return CalendarUpdate::Failed;
    };
    let now = unix_now();
    CalendarUpdate::Events(ics::parse(&text, now - LOOK_BEHIND_MS, now + LOOK_AHEAD_MS))
}

fn download(address: &str) -> Result<String> {
    let uri = Uri::CreateUri(&HSTRING::from(address))?;
    Ok(HttpClient::new()?.GetStringAsync(&uri)?.join()?.to_string())
}
