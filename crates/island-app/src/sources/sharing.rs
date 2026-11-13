//! Sending files to a phone over the local network. A small web server runs on
//! this computer only while a send is under way: it listens on the address of
//! the Wi-Fi or Ethernet adapter, answers only under a one-time token, and
//! stops once the files are down or nobody came for them.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use island_core::firewall;
use island_core::http::{self, MAX_HEAD, Method, Route, Span};
use island_core::transfer::WAIT_MS;
use windows::Networking::Connectivity::{NetworkConnectivityLevel, NetworkInformation};
use windows::Networking::HostNameType;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Networking::NetworkListManager::{
    INetworkConnection, INetworkListManager, NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED,
    NLM_NETWORK_CATEGORY_PRIVATE, NetworkListManager,
};
use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::GUID;

pub const MSG_SHARING: u32 = WM_APP + 19;

const WIFI: u32 = 71;
const ETHERNET: u32 = 6;
const CHUNK: usize = 64 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// A phone that stops reading for this long has gone.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// More at once would only be someone poking at the address.
const MOST_CONNECTIONS: usize = 8;

/// The network a phone has to join to reach this computer.
pub struct LocalNetwork {
    pub address: Ipv4Addr,
    pub name: String,
    pub wifi: bool,
    /// Which of the firewall's profiles applies to it: public, private or domain.
    pub profile: u32,
}

/// The Wi-Fi or Ethernet connection in use, never a VPN, whose address a
/// phone on the same network cannot reach.
pub fn local_network() -> Option<LocalNetwork> {
    let profiles = NetworkInformation::GetConnectionProfiles().ok()?;
    let mut best: Option<(u8, GUID, String, bool)> = None;
    for profile in profiles {
        let level = profile
            .GetNetworkConnectivityLevel()
            .unwrap_or(NetworkConnectivityLevel::None);
        if level == NetworkConnectivityLevel::None {
            continue;
        }
        let Ok(adapter) = profile.NetworkAdapter() else {
            continue;
        };
        let kind = adapter.IanaInterfaceType().unwrap_or(0);
        if kind != WIFI && kind != ETHERNET {
            continue;
        }
        let wifi = kind == WIFI;
        // Wi-Fi first, as that is where phones are, and online ahead of local only.
        let online = matches!(
            level,
            NetworkConnectivityLevel::InternetAccess
                | NetworkConnectivityLevel::ConstrainedInternetAccess
        );
        let rank = u8::from(!wifi) * 2 + u8::from(!online);
        let Ok(id) = adapter.NetworkAdapterId() else {
            continue;
        };
        let name = profile
            .ProfileName()
            .map(|name| name.to_string())
            .unwrap_or_default();
        if best.as_ref().is_none_or(|(known, ..)| rank < *known) {
            best = Some((rank, id, name, wifi));
        }
    }
    let (_, id, name, wifi) = best?;
    Some(LocalNetwork {
        address: address_of(id)?,
        name,
        wifi,
        profile: profile_of(id),
    })
}

/// This computer only, for tests.
pub fn loopback() -> LocalNetwork {
    LocalNetwork {
        address: Ipv4Addr::LOCALHOST,
        name: "Bu bilgisayar".into(),
        wifi: false,
        profile: firewall::PRIVATE,
    }
}

fn address_of(adapter: GUID) -> Option<Ipv4Addr> {
    let hosts = NetworkInformation::GetHostNames().ok()?;
    hosts.into_iter().find_map(|host| {
        if host.Type().ok()? != HostNameType::Ipv4 {
            return None;
        }
        let owner = host
            .IPInformation()
            .ok()?
            .NetworkAdapter()
            .ok()?
            .NetworkAdapterId()
            .ok()?;
        let address: Ipv4Addr = host.CanonicalName().ok()?.to_string().parse().ok()?;
        (owner == adapter && !address.is_link_local()).then_some(address)
    })
}

/// The firewall profile of the adapter's network. Unknown counts as public,
/// the strictest.
fn profile_of(adapter: GUID) -> u32 {
    let check = || -> windows::core::Result<u32> {
        unsafe {
            let manager: INetworkListManager =
                CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL)?;
            let connections = manager.GetNetworkConnections()?;
            loop {
                let mut next: [Option<INetworkConnection>; 1] = [None];
                let mut fetched = 0;
                connections.Next(&mut next, Some(&mut fetched))?;
                let Some(connection) = next[0].take().filter(|_| fetched == 1) else {
                    return Ok(firewall::PUBLIC);
                };
                if connection.GetAdapterId()? == adapter {
                    return Ok(match connection.GetNetwork()?.GetCategory()? {
                        NLM_NETWORK_CATEGORY_PRIVATE => firewall::PRIVATE,
                        NLM_NETWORK_CATEGORY_DOMAIN_AUTHENTICATED => firewall::DOMAIN,
                        _ => firewall::PUBLIC,
                    });
                }
            }
        }
    };
    check().unwrap_or(firewall::PUBLIC)
}

pub enum SharingEvent {
    /// A phone opened the page, so it found its way to the computer.
    Visited,
    /// Bytes sent so far, of all the files together.
    Progress(u64),
    /// A download broke off before the end. The code waits again.
    BrokeOff,
    Done,
    /// Nobody came for the files in time.
    Expired,
}

pub struct SharedFile {
    pub path: String,
    pub name: String,
    pub size: u64,
}

struct Progress {
    /// Bytes sent of each file, at most its size.
    sent: Vec<u64>,
    done: Vec<bool>,
    downloads: usize,
    connections: usize,
    /// When the server gives up, unless a download is under way.
    deadline: Option<Instant>,
    reported_percent: u64,
}

struct Shared {
    token: String,
    files: Vec<SharedFile>,
    total: u64,
    stop: AtomicBool,
    /// A phone got through, which is told once.
    visited: AtomicBool,
    progress: Mutex<Progress>,
    wake: Condvar,
    window: isize,
    events: Mutex<Sender<SharingEvent>>,
}

impl Shared {
    fn report(&self, event: SharingEvent) {
        if let Ok(events) = self.events.lock()
            && events.send(event).is_ok()
        {
            let _ = unsafe {
                PostMessageW(
                    Some(HWND(self.window as _)),
                    MSG_SHARING,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

pub struct Sharing {
    pub url: String,
    shared: Arc<Shared>,
    address: SocketAddr,
    events: Receiver<SharingEvent>,
}

impl Sharing {
    pub fn start(window: HWND, files: Vec<SharedFile>, address: Ipv4Addr) -> io::Result<Self> {
        let listener = TcpListener::bind((address, 0))?;
        let address = listener.local_addr()?;
        let token = http::token(&random_bytes()?);
        let url = format!("http://{}:{}/{token}", address.ip(), address.port());
        let (sender, events) = channel();
        let count = files.len();
        let shared = Arc::new(Shared {
            token,
            total: files.iter().map(|file| file.size).sum(),
            files,
            stop: AtomicBool::new(false),
            visited: AtomicBool::new(false),
            progress: Mutex::new(Progress {
                sent: vec![0; count],
                done: vec![false; count],
                downloads: 0,
                connections: 0,
                deadline: Some(Instant::now() + Duration::from_millis(WAIT_MS)),
                reported_percent: 0,
            }),
            wake: Condvar::new(),
            window: window.0 as isize,
            events: Mutex::new(sender),
        });
        let accepting = shared.clone();
        std::thread::spawn(move || accept(listener, accepting));
        let watching = shared.clone();
        std::thread::spawn(move || watch(watching, address));
        Ok(Self {
            url,
            shared,
            address,
            events,
        })
    }

    pub fn events(&self) -> Vec<SharingEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for Sharing {
    fn drop(&mut self) {
        stop(&self.shared, self.address);
    }
}

fn stop(shared: &Shared, address: SocketAddr) {
    if shared.stop.swap(true, Ordering::Relaxed) {
        return;
    }
    shared.wake.notify_all();
    // The listener waits in accept, which only a connection wakes.
    let _ = TcpStream::connect_timeout(&address, Duration::from_secs(1));
}

fn random_bytes() -> io::Result<[u8; 13]> {
    let mut bytes = [0_u8; 13];
    unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
        .ok()
        .map_err(|error| io::Error::other(error.message()))?;
    Ok(bytes)
}

/// Random bytes from Windows' own generator, for names and passwords.
pub fn random(count: usize) -> Option<Vec<u8>> {
    let mut bytes = vec![0_u8; count];
    unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
        .ok()
        .ok()?;
    Some(bytes)
}

fn accept(listener: TcpListener, shared: Arc<Shared>) {
    for stream in listener.incoming() {
        if shared.stopped() {
            break;
        }
        let Ok(stream) = stream else { continue };
        {
            let Ok(mut progress) = shared.progress.lock() else {
                break;
            };
            if progress.connections >= MOST_CONNECTIONS {
                continue;
            }
            progress.connections += 1;
        }
        let serving = shared.clone();
        std::thread::spawn(move || {
            let _ = serve(stream, &serving);
            if let Ok(mut progress) = serving.progress.lock() {
                progress.connections -= 1;
            }
        });
    }
}

/// Stops the server at its deadline. A download under way holds it off.
fn watch(shared: Arc<Shared>, address: SocketAddr) {
    let Ok(mut progress) = shared.progress.lock() else {
        return;
    };
    loop {
        if shared.stopped() {
            return;
        }
        match progress.deadline {
            Some(deadline) if Instant::now() >= deadline => break,
            Some(deadline) => {
                let wait = deadline.saturating_duration_since(Instant::now());
                let Ok((next, _)) = shared.wake.wait_timeout(progress, wait) else {
                    return;
                };
                progress = next;
            }
            None => {
                let Ok(next) = shared.wake.wait(progress) else {
                    return;
                };
                progress = next;
            }
        }
    }
    drop(progress);
    shared.report(SharingEvent::Expired);
    stop(&shared, address);
}

fn read_head(stream: &mut TcpStream) -> io::Result<String> {
    let mut head = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        if head.len() > MAX_HEAD {
            return Err(io::Error::other("head too long"));
        }
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        head.extend_from_slice(&buffer[..read]);
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

fn serve(mut stream: TcpStream, shared: &Shared) -> io::Result<()> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let head = read_head(&mut stream)?;
    let Some(request) = http::parse_request(&head) else {
        return answer(&mut stream, 400, "text/plain", b"");
    };
    if request.method == Method::Other {
        return answer(&mut stream, 405, "text/plain", b"");
    }
    match http::route(&request.path, &shared.token, shared.files.len()) {
        Route::NotFound => answer(&mut stream, 404, "text/plain", b""),
        Route::Page => {
            if !shared.visited.swap(true, Ordering::Relaxed) {
                shared.report(SharingEvent::Visited);
            }
            let listed: Vec<(String, u64)> = shared
                .files
                .iter()
                .map(|file| (file.name.clone(), file.size))
                .collect();
            let page = http::page(&shared.token, &listed);
            let body = if request.method == Method::Head {
                &[][..]
            } else {
                page.as_bytes()
            };
            let head = http::response_head(
                200,
                &[
                    ("Content-Type", "text/html; charset=utf-8".into()),
                    ("Content-Length", page.len().to_string()),
                ],
            );
            stream.write_all(head.as_bytes())?;
            stream.write_all(body)
        }
        Route::File(index) => send_file(
            &mut stream,
            shared,
            index,
            request.method,
            request.range.as_deref(),
        ),
    }
}

fn answer(stream: &mut TcpStream, status: u16, kind: &str, body: &[u8]) -> io::Result<()> {
    let head = http::response_head(
        status,
        &[
            ("Content-Type", kind.into()),
            ("Content-Length", body.len().to_string()),
        ],
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)
}

fn send_file(
    stream: &mut impl Write,
    shared: &Shared,
    index: usize,
    method: Method,
    range: Option<&str>,
) -> io::Result<()> {
    let file = &shared.files[index];
    let mut source = std::fs::File::open(&file.path)?;
    let size = source.metadata()?.len();
    let (status, first, last) = match http::parse_range(range, size) {
        Span::Whole => (200, 0, size.saturating_sub(1)),
        Span::Part { first, last } => (206, first, last),
        Span::Unsatisfiable => {
            let head = http::response_head(
                416,
                &[
                    ("Content-Range", format!("bytes */{size}")),
                    ("Content-Length", "0".into()),
                ],
            );
            return stream.write_all(head.as_bytes());
        }
    };
    let length = if size == 0 { 0 } else { last - first + 1 };
    let mut headers = vec![
        ("Content-Type", http::content_type(&file.name).to_owned()),
        ("Content-Length", length.to_string()),
        ("Content-Disposition", http::content_disposition(&file.name)),
        ("Accept-Ranges", "bytes".to_owned()),
    ];
    if status == 206 {
        headers.push(("Content-Range", format!("bytes {first}-{last}/{size}")));
    }
    stream.write_all(http::response_head(status, &headers).as_bytes())?;
    if method == Method::Head {
        return Ok(());
    }

    if length == 0 {
        // A real empty download completes, but it must not decrement another
        // active download or treat a metadata-only HEAD request as a download.
        started(shared);
        finish(shared, index, true);
        return Ok(());
    }

    started(shared);
    let result = copy_span(stream, shared, &mut source, index, first, length);
    finish(shared, index, result.is_ok() && first + length >= size);
    result
}

fn copy_span(
    stream: &mut impl Write,
    shared: &Shared,
    source: &mut std::fs::File,
    index: usize,
    first: u64,
    length: u64,
) -> io::Result<()> {
    use std::io::Seek;
    source.seek(io::SeekFrom::Start(first))?;
    let mut buffer = vec![0_u8; CHUNK];
    let mut left = length;
    while left > 0 {
        if shared.stopped() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let wanted = left.min(CHUNK as u64) as usize;
        let read = source.read(&mut buffer[..wanted])?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        stream.write_all(&buffer[..read])?;
        left -= read as u64;
        sent(shared, index, read as u64);
    }
    stream.flush()
}

fn started(shared: &Shared) {
    if let Ok(mut progress) = shared.progress.lock() {
        progress.downloads += 1;
        progress.deadline = None;
    }
    shared.wake.notify_all();
}

/// Counts bytes as they go, and tells the island at each whole percent.
fn sent(shared: &Shared, index: usize, bytes: u64) {
    let Ok(mut progress) = shared.progress.lock() else {
        return;
    };
    let size = shared.files[index].size;
    progress.sent[index] = (progress.sent[index] + bytes).min(size);
    let total: u64 = progress.sent.iter().sum();
    let percent = (total * 100).checked_div(shared.total).unwrap_or(100);
    if percent != progress.reported_percent {
        progress.reported_percent = percent;
        drop(progress);
        shared.report(SharingEvent::Progress(total));
    }
}

fn finish(shared: &Shared, index: usize, reached_the_end: bool) {
    let Ok(mut progress) = shared.progress.lock() else {
        return;
    };
    progress.downloads = progress.downloads.saturating_sub(1);
    if reached_the_end && progress.sent[index] >= shared.files[index].size {
        progress.done[index] = true;
    }
    let all_done = progress.done.iter().all(|done| *done);
    let idle = progress.downloads == 0;
    if idle && !all_done {
        progress.deadline = Some(Instant::now() + Duration::from_millis(WAIT_MS));
    }
    drop(progress);
    shared.wake.notify_all();
    if all_done {
        shared.report(SharingEvent::Done);
        // Stopping here would cut off the last bytes still in flight, so the
        // island stops the server when it hears of this.
    } else if idle && !reached_the_end {
        shared.report(SharingEvent::BrokeOff);
    }
}

/// How the island names the network, such as `Wi-Fi · Ev`.
pub fn describe(network: &LocalNetwork) -> String {
    let kind = if network.wifi { "Wi-Fi" } else { "Ethernet" };
    if network.name.is_empty() {
        kind.to_owned()
    } else {
        format!("{kind} · {}", network.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_for_empty_file_does_not_complete_or_interrupt_a_download() {
        let path =
            std::env::temp_dir().join(format!("islora-empty-transfer-{}", std::process::id()));
        std::fs::write(&path, []).unwrap();
        let (sender, events) = channel();
        let shared = Shared {
            token: "test".into(),
            files: vec![SharedFile {
                path: path.to_string_lossy().into_owned(),
                name: "empty.txt".into(),
                size: 0,
            }],
            total: 0,
            stop: AtomicBool::new(false),
            visited: AtomicBool::new(false),
            progress: Mutex::new(Progress {
                sent: vec![0],
                done: vec![false],
                downloads: 1,
                connections: 0,
                deadline: None,
                reported_percent: 0,
            }),
            wake: Condvar::new(),
            window: 0,
            events: Mutex::new(sender),
        };
        let mut response = Vec::new();
        send_file(&mut response, &shared, 0, Method::Head, None).unwrap();
        {
            let progress = shared.progress.lock().unwrap();
            assert_eq!(progress.downloads, 1);
            assert!(!progress.done[0]);
        }
        assert!(events.try_recv().is_err());
        response.clear();
        send_file(&mut response, &shared, 0, Method::Get, None).unwrap();
        let progress = shared.progress.lock().unwrap();
        assert_eq!(progress.downloads, 1);
        assert!(progress.done[0]);
        assert!(matches!(events.try_recv(), Ok(SharingEvent::Done)));
        drop(progress);
        std::fs::remove_file(path).unwrap();
    }
}
