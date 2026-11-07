//! The internet connection, the Wi-Fi network, VPNs and airplane mode.

use std::sync::mpsc;
use std::time::Duration;

use island_core::indicator::is_vpn_adapter;
use island_core::{Indicator, NetworkState, NetworkTracker};
use windows::Networking::Connectivity::{
    NetworkConnectivityLevel, NetworkInformation, NetworkStatusChangedEventHandler,
};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::AF_UNSPEC;
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_DWORD, RegGetValueW, RegOpenKeyExW,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::{PCWSTR, w};

use super::{IndicatorSink, registry};

/// A network takes a moment to settle, and asking too soon reports it as lost.
const SETTLE: Duration = Duration::from_secs(3);
const ADAPTER_LIST_BYTES: usize = 32 * 1024;
const RADIO_STATE: PCWSTR = w!("SYSTEM\\CurrentControlSet\\Control\\RadioManagement\\SystemRadioState");

pub fn start(sink: IndicatorSink) {
    let airplane_sink = sink.clone();
    let network = std::thread::Builder::new().name("network".into()).spawn(move || {
        follow_network(&sink);
    });
    let airplane = std::thread::Builder::new().name("airplane mode".into()).spawn(move || {
        follow_airplane_mode(&airplane_sink);
    });
    if network.is_err() || airplane.is_err() {
        eprintln!("network threads failed to start");
    }
}

fn follow_network(sink: &IndicatorSink) {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let (changed, changes) = mpsc::channel();
    let handler = NetworkStatusChangedEventHandler::new(move |_| {
        let _ = changed.send(());
        Ok(())
    });
    if NetworkInformation::NetworkStatusChanged(&handler).is_err() {
        return;
    }

    let mut tracker = NetworkTracker::new();
    tracker.update(read_state());
    while changes.recv().is_ok() {
        std::thread::sleep(SETTLE);
        changes.try_iter().for_each(drop);
        for indicator in tracker.update(read_state()) {
            sink.send(indicator);
        }
        sink.network_changed();
    }
}

fn read_state() -> NetworkState {
    let profile = NetworkInformation::GetInternetConnectionProfile().ok();
    let level = profile.as_ref().and_then(|profile| profile.GetNetworkConnectivityLevel().ok());
    // Behind a sign-in page counts as connected: there is a network, it just
    // wants something first, and calling that a lost connection would be wrong.
    let online = matches!(
        level,
        Some(NetworkConnectivityLevel::InternetAccess | NetworkConnectivityLevel::ConstrainedInternetAccess)
    );
    let wifi = profile
        .filter(|profile| profile.IsWlanConnectionProfile().unwrap_or(false))
        .and_then(|profile| profile.ProfileName().ok())
        .map(|name| name.to_string());
    NetworkState { online, wifi, vpn: vpn_name() }
}

fn vpn_name() -> Option<String> {
    let mut list = vec![0_u64; ADAPTER_LIST_BYTES / size_of::<u64>()];
    let mut size = ADAPTER_LIST_BYTES as u32;
    let first = list.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let status = unsafe { GetAdaptersAddresses(u32::from(AF_UNSPEC.0), flags, None, Some(first), &mut size) };
    if status != ERROR_SUCCESS.0 {
        return None;
    }

    let mut adapter = first.cast_const();
    while let Some(current) = unsafe { adapter.as_ref() } {
        let name = unsafe { current.FriendlyName.to_string() }.unwrap_or_default();
        let description = unsafe { current.Description.to_string() }.unwrap_or_default();
        let is_up = current.OperStatus == IfOperStatusUp;
        if is_up && is_vpn_adapter(current.IfType, &format!("{name} {description}")) {
            return Some(name);
        }
        adapter = current.Next;
    }
    None
}

/// Windows offers no event for this, but keeps it in a value that changes with it.
fn follow_airplane_mode(sink: &IndicatorSink) {
    let mut key = HKEY::default();
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, RADIO_STATE, None, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return;
    }
    let mut known = airplane_mode(key);
    registry::follow(key, || {
        let now = airplane_mode(key);
        if let (Some(on), true) = (now, now != known) {
            sink.send(Indicator::AirplaneMode { on });
        }
        known = now;
    });
}

fn airplane_mode(key: HKEY) -> Option<bool> {
    let mut value = 0_u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            key,
            None,
            None,
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value != 0)
}
