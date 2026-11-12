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
mod registry;
pub mod screenshots;
pub mod sharing;

use std::sync::mpsc::{self, Receiver, Sender};

use island_core::Indicator;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const MSG_INDICATOR: u32 = WM_APP + 5;
/// The computer's network changed, once it settled.
pub const MSG_NETWORK_CHANGED: u32 = WM_APP + 22;

/// Lets callbacks on any thread raise an indicator. They arrive on system
/// threads, while the island may only be touched from the UI thread.
#[derive(Clone)]
pub struct IndicatorSink {
    sender: Sender<Indicator>,
    window: isize,
}

impl IndicatorSink {
    pub fn channel(hwnd: HWND) -> (Self, Receiver<Indicator>) {
        let (sender, receiver) = mpsc::channel();
        (Self { sender, window: hwnd.0 as isize }, receiver)
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
