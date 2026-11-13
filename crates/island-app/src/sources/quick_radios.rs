//! One-shot radio status reads for Quick Controls; no radio state is changed.

use island_core::quick_controls::RadioState as ShownState;
use std::sync::mpsc::{self, Receiver, Sender};
use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const MSG_QUICK_RADIOS: u32 = WM_APP + 26;

pub struct QuickRadios {
    window: isize,
    sender: Sender<(ShownState, ShownState)>,
    updates: Receiver<(ShownState, ShownState)>,
}

impl QuickRadios {
    pub fn new(hwnd: HWND) -> Self {
        let (sender, updates) = mpsc::channel();
        Self {
            window: hwnd.0 as isize,
            sender,
            updates,
        }
    }

    pub fn refresh(&self) {
        let sender = self.sender.clone();
        let window = self.window;
        if let Err(error) = std::thread::Builder::new()
            .name("quick-radios".into())
            .spawn(move || {
                let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
                let mut wifi = None;
                let mut bluetooth = None;
                match Radio::GetRadiosAsync().and_then(|op| op.join()) {
                    Ok(radios) => {
                        for radio in radios {
                            let state = match radio.State() {
                                Ok(RadioState::On) => ShownState::On,
                                Ok(RadioState::Off | RadioState::Disabled) => ShownState::Off,
                                _ => ShownState::Unknown,
                            };
                            match radio.Kind() {
                                Ok(RadioKind::WiFi) => wifi = merge(wifi, state),
                                Ok(RadioKind::Bluetooth) => bluetooth = merge(bluetooth, state),
                                _ => {}
                            }
                        }
                    }
                    Err(error) => eprintln!("radio status unavailable: {error}"),
                }
                if sender
                    .send((wifi.unwrap_or_default(), bluetooth.unwrap_or_default()))
                    .is_ok()
                {
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(window as _)),
                            MSG_QUICK_RADIOS,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                }
            })
        {
            eprintln!("radio status worker failed: {error}");
        }
    }

    pub fn latest(&self) -> Option<(ShownState, ShownState)> {
        self.updates.try_iter().last()
    }
}

fn merge(a: Option<ShownState>, b: ShownState) -> Option<ShownState> {
    Some(match (a, b) {
        (Some(ShownState::On), _) | (_, ShownState::On) => ShownState::On,
        (Some(ShownState::Unknown), _) | (_, ShownState::Unknown) => ShownState::Unknown,
        _ => ShownState::Off,
    })
}
