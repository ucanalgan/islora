//! A Wi-Fi network of the island's own, for a phone that is not on the
//! computer's network. Windows' Wi-Fi Direct legacy mode makes the Wi-Fi card
//! act as a small access point that any phone joins like a normal network,
//! while the computer stays on its own Wi-Fi. It lives only during a send.

use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Devices::WiFiDirect::{
    WiFiDirectAdvertisementPublisher, WiFiDirectAdvertisementPublisherStatus,
    WiFiDirectAdvertisementPublisherStatusChangedEventArgs, WiFiDirectError,
};
use windows::Foundation::TypedEventHandler;
use windows::Security::Credentials::PasswordCredential;
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, LPARAM, WPARAM};
use windows::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    GetAdaptersAddresses, GetIpNetTable2, IP_ADAPTER_ADDRESSES_LH, MIB_IPNET_TABLE2,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::{AF_INET, NlnsIncomplete, NlnsUnreachable, SOCKADDR_IN};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::HSTRING;

pub const MSG_DIRECT: u32 = WM_APP + 21;

/// How long Windows may take to start the network.
const START_WAIT: Duration = Duration::from_secs(10);
/// The network's address arrives a moment after it starts.
const ADDRESS_WAIT: Duration = Duration::from_secs(8);
const ADDRESS_STEP: Duration = Duration::from_millis(250);
/// How often to look for a phone on the network while waiting for one. Windows
/// has no event for a device joining, and this runs only during a send.
const JOIN_STEP: Duration = Duration::from_secs(1);
const ADAPTER_LIST_BYTES: usize = 32 * 1024;

pub enum DirectEvent {
    /// The network is up, with this address for the computer on it.
    Ready(Ipv4Addr),
    /// A phone joined.
    Joined,
    /// Wi-Fi is off, so no network of the island's own either.
    RadioOff,
    Failed(&'static str),
}

/// What Windows says when Wi-Fi is off.
const RADIO_OFF: &str = "Wi-Fi kapalı";

pub struct DirectLink {
    stop: Sender<()>,
    events: Receiver<DirectEvent>,
    thread: Option<JoinHandle<()>>,
}

impl DirectLink {
    pub fn start(window: HWND, network: String, passphrase: String) -> Self {
        let (stop, stopped) = channel();
        let (sender, events) = channel();
        let window = window.0 as isize;
        let thread = std::thread::Builder::new()
            .name("direct link".into())
            .spawn(move || {
                run(&network, &passphrase, &stopped, &|event| {
                    report(&sender, window, event)
                })
            })
            .ok();
        Self {
            stop,
            events,
            thread,
        }
    }

    pub fn events(&self) -> Vec<DirectEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for DirectLink {
    /// Asks the thread to close the network and does not wait for it: Windows
    /// can take seconds to start one, which must not hold up the island.
    fn drop(&mut self) {
        let _ = self.stop.send(());
        drop(self.thread.take());
    }
}

fn report(sender: &Sender<DirectEvent>, window: isize, event: DirectEvent) {
    if sender.send(event).is_ok() {
        let _ = unsafe { PostMessageW(Some(HWND(window as _)), MSG_DIRECT, WPARAM(0), LPARAM(0)) };
    }
}

fn run(network: &str, passphrase: &str, stopped: &Receiver<()>, report: &dyn Fn(DirectEvent)) {
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let before = direct_adapters();
    let publisher = match start(network, passphrase) {
        Ok(publisher) => publisher,
        Err(RADIO_OFF) => {
            report(DirectEvent::RadioOff);
            return;
        }
        Err(reason) => {
            report(DirectEvent::Failed(reason));
            return;
        }
    };
    let found = wait_for_address(&before, stopped);
    let Some((index, address)) = found else {
        let _ = publisher.Stop();
        if stopped.try_recv().is_err() {
            report(DirectEvent::Failed("Ağın adresi alınamadı"));
        }
        return;
    };
    report(DirectEvent::Ready(address));

    let mut joined = false;
    while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(JOIN_STEP) {
        if !joined && has_neighbour(index, address) {
            joined = true;
            report(DirectEvent::Joined);
        }
    }
    let _ = publisher.Stop();
}

/// Starts the network and waits until Windows says it is up.
fn start(
    network: &str,
    passphrase: &str,
) -> Result<WiFiDirectAdvertisementPublisher, &'static str> {
    let failed = |_| "Windows doğrudan bağlantıyı başlatamadı";
    let publisher = WiFiDirectAdvertisementPublisher::new().map_err(failed)?;
    let advertisement = publisher.Advertisement().map_err(failed)?;
    advertisement
        .SetIsAutonomousGroupOwnerEnabled(true)
        .map_err(failed)?;
    let legacy = advertisement.LegacySettings().map_err(failed)?;
    legacy.SetIsEnabled(true).map_err(failed)?;
    legacy.SetSsid(&HSTRING::from(network)).map_err(failed)?;
    let credential = PasswordCredential::new().map_err(failed)?;
    credential
        .SetPassword(&HSTRING::from(passphrase))
        .map_err(failed)?;
    legacy.SetPassphrase(&credential).map_err(failed)?;

    let (sender, statuses) = channel();
    publisher
        .StatusChanged(&TypedEventHandler::<
            WiFiDirectAdvertisementPublisher,
            WiFiDirectAdvertisementPublisherStatusChangedEventArgs,
        >::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                let _ = sender.send((args.Status()?, args.Error()?));
            }
            Ok(())
        }))
        .map_err(failed)?;
    publisher.Start().map_err(failed)?;

    let deadline = Instant::now() + START_WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match statuses.recv_timeout(left) {
            Ok((WiFiDirectAdvertisementPublisherStatus::Started, _)) => return Ok(publisher),
            Ok((WiFiDirectAdvertisementPublisherStatus::Aborted, error)) => {
                return Err(reason(error));
            }
            Ok(_) => {}
            Err(_) => {
                let _ = publisher.Stop();
                return Err("Windows ağı zamanında başlatamadı");
            }
        }
    }
}

fn reason(error: WiFiDirectError) -> &'static str {
    match error {
        WiFiDirectError::RadioNotAvailable => RADIO_OFF,
        // The Mobile Hotspot, or another Wi-Fi Direct link, has the card.
        WiFiDirectError::ResourceInUse => "Wi-Fi kartı meşgul, Mobil erişim noktası açık olabilir",
        _ => "Bu Wi-Fi kartı kendi ağını kuramıyor",
    }
}

/// The computer's address on Windows' Mobile Hotspot, which runs on a Wi-Fi
/// Direct adapter too. Only asked while no network of the island's own is up.
pub fn hotspot_address() -> Option<Ipv4Addr> {
    direct_adapters()
        .into_iter()
        .find_map(|(_, address)| address)
}

/// Adapters Windows uses for Wi-Fi Direct, with their index and IPv4 address
/// once they have one.
fn direct_adapters() -> Vec<(u32, Option<Ipv4Addr>)> {
    let mut list = vec![0_u64; ADAPTER_LIST_BYTES / size_of::<u64>()];
    let mut size = ADAPTER_LIST_BYTES as u32;
    let first = list.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let status =
        unsafe { GetAdaptersAddresses(u32::from(AF_INET.0), flags, None, Some(first), &mut size) };
    if status != ERROR_SUCCESS.0 {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut adapter = first.cast_const();
    while let Some(current) = unsafe { adapter.as_ref() } {
        let description = unsafe { current.Description.to_string() }.unwrap_or_default();
        if description.contains("Wi-Fi Direct") && current.OperStatus == IfOperStatusUp {
            let index = unsafe { current.Anonymous1.Anonymous.IfIndex };
            found.push((index, first_ipv4(current)));
        }
        adapter = current.Next;
    }
    found
}

fn first_ipv4(adapter: &IP_ADAPTER_ADDRESSES_LH) -> Option<Ipv4Addr> {
    let mut unicast = adapter.FirstUnicastAddress;
    while let Some(entry) = unsafe { unicast.as_ref() } {
        let socket = entry.Address.lpSockaddr;
        if let Some(address) = unsafe { socket.as_ref() }
            && address.sa_family == AF_INET
        {
            let ipv4 = unsafe { &*socket.cast::<SOCKADDR_IN>() };
            let octets = unsafe { ipv4.sin_addr.S_un.S_addr }.to_ne_bytes();
            let address = Ipv4Addr::from(octets);
            if !address.is_link_local() {
                return Some(address);
            }
        }
        unicast = entry.Next;
    }
    None
}

/// The address of the network just started, preferring an adapter that was
/// not up before, as an older Wi-Fi Direct link may still be around.
fn wait_for_address(
    before: &[(u32, Option<Ipv4Addr>)],
    stopped: &Receiver<()>,
) -> Option<(u32, Ipv4Addr)> {
    let known: HashSet<u32> = before
        .iter()
        .filter(|(_, address)| address.is_some())
        .map(|(index, _)| *index)
        .collect();
    let deadline = Instant::now() + ADDRESS_WAIT;
    while Instant::now() < deadline {
        let adapters = direct_adapters();
        let with_address = adapters
            .iter()
            .filter_map(|(index, address)| Some((*index, (*address)?)));
        let mut candidates: Vec<(u32, Ipv4Addr)> = with_address.collect();
        candidates.sort_by_key(|(index, _)| known.contains(index));
        if let Some(found) = candidates.first() {
            return Some(*found);
        }
        if stopped.recv_timeout(ADDRESS_STEP).is_ok() {
            return None;
        }
    }
    None
}

/// Whether another device has an address on the network: a phone that joined
/// asks for one, and Windows then knows it as a neighbour.
fn has_neighbour(index: u32, own: Ipv4Addr) -> bool {
    let mut table: *mut MIB_IPNET_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIpNetTable2(AF_INET, &mut table) } != ERROR_SUCCESS || table.is_null() {
        return false;
    }
    let rows = unsafe {
        let count = (*table).NumEntries as usize;
        std::slice::from_raw_parts((*table).Table.as_ptr(), count)
    };
    let found = rows.iter().any(|row| {
        if row.InterfaceIndex != index
            || row.State == NlnsUnreachable
            || row.State == NlnsIncomplete
        {
            return false;
        }
        let octets = unsafe { row.Address.Ipv4.sin_addr.S_un.S_addr }.to_ne_bytes();
        let address = Ipv4Addr::from(octets);
        address != own && !address.is_broadcast() && !address.is_multicast() && octets[3] != 255
    });
    unsafe { FreeMibTable(table.cast()) };
    found
}
