pub mod airpods;
pub mod audio;
pub mod bluetooth;
pub mod calendar;
pub mod clipboard;
pub mod clock_timers;
pub mod direct_link;
pub mod drives;
pub mod firewall;
pub mod focus;
pub mod fullscreen;
pub mod imaging;
pub mod keys;
pub mod media;
pub mod network;
pub mod notifications;
pub mod power;
pub mod privacy;
pub mod quick_brightness;
pub mod quick_radios;
mod registry;
pub mod screenshots;
pub mod sharing;

use std::sync::mpsc::{self, Receiver, Sender};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use island_core::Indicator;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const MSG_INDICATOR: u32 = WM_APP + 5;
/// The computer's network changed, once it settled.
pub const MSG_NETWORK_CHANGED: u32 = WM_APP + 22;
/// AirPods said how many of their earbuds are in an ear, or are gone.
pub const MSG_EAR_CHANGED: u32 = WM_APP + 29;

/// What AirPods tell about their earbuds and where the sound goes, which is
/// no notice but what pauses music.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EarEvent {
    /// How many earbuds are in an ear. Every report is sent and none is
    /// merged, as it takes two reports to be sure of a change.
    Worn(u8),
    /// The container ids of the connected AirPods, which say which audio
    /// output is theirs. Empty when not known.
    Containers(Vec<u128>),
    /// The AirPods are gone.
    Gone,
}

/// Lets callbacks on any thread raise an indicator. They arrive on system
/// threads, while the island may only be touched from the UI thread.
#[derive(Clone)]
pub struct IndicatorSink {
    sender: Sender<Indicator>,
    ear: Sender<EarEvent>,
    /// Whether the app wants the reports of worn earbuds at all. They come
    /// several times a second, so they are not sent when they would be
    /// ignored.
    ear_wanted: Arc<AtomicBool>,
    window: isize,
}

impl IndicatorSink {
    pub fn channel(hwnd: HWND) -> (Self, Receiver<Indicator>, Receiver<EarEvent>) {
        let (sender, receiver) = mpsc::channel();
        let (ear, ear_receiver) = mpsc::channel();
        (
            Self {
                sender,
                ear,
                ear_wanted: Arc::new(AtomicBool::new(true)),
                window: hwnd.0 as isize,
            },
            receiver,
            ear_receiver,
        )
    }

    /// The switch for the reports of worn earbuds, which the app turns on and
    /// off.
    pub fn ear_switch(&self) -> Arc<AtomicBool> {
        self.ear_wanted.clone()
    }

    /// Tells the island something about AirPods' earbuds.
    pub fn ear(&self, event: EarEvent) {
        if matches!(event, EarEvent::Worn(_)) && !self.ear_wanted.load(Ordering::Relaxed) {
            return;
        }
        if self.ear.send(event).is_ok() {
            let window = HWND(self.window as _);
            unsafe {
                let _ = PostMessageW(Some(window), MSG_EAR_CHANGED, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// Tells the island the network changed, for a send waiting on Wi-Fi.
    pub fn network_changed(&self) {
        let window = HWND(self.window as _);
        unsafe {
            let _ = PostMessageW(Some(window), MSG_NETWORK_CHANGED, WPARAM(0), LPARAM(0));
        }
    }

    pub fn send(&self, indicator: Indicator) {
        if self.sender.send(indicator).is_ok() {
            let window = HWND(self.window as _);
            unsafe {
                let _ = PostMessageW(Some(window), MSG_INDICATOR, WPARAM(0), LPARAM(0));
            }
        }
    }
}
