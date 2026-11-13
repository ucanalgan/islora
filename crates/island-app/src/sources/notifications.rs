//! Reads Windows app notifications after the user grants access.
//! A package identity gives us change events; a plain exe checks occasionally.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use island_core::notifications::Notice;
use windows::Foundation::{Size, TypedEventHandler};
use windows::Storage::Streams::DataReader;
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{
    KnownNotificationBindings, NotificationBinding, NotificationKinds, UserNotification,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::Result;
use windows_future::{AsyncOperationCompletedHandler, AsyncStatus};

pub const MSG_NOTIFICATIONS: u32 = WM_APP + 25;
const CHECK_EVERY: Duration = Duration::from_secs(5);
const MAX_LOGO_BYTES: u64 = 256 * 1024;

enum Signal {
    Refresh,
    Stop,
}

pub enum Update {
    Seed(Vec<u32>),
    Added(Notice),
    Removed(u32),
}

pub struct NotificationSource {
    signals: Sender<Signal>,
    updates: Receiver<Update>,
}

impl NotificationSource {
    pub fn start(hwnd: HWND) -> Self {
        let (signals, inbox) = mpsc::channel();
        let (publish, updates) = mpsc::channel();
        let worker_signals = signals.clone();
        let window = hwnd.0 as isize;
        let spawned = std::thread::Builder::new()
            .name("notifications".into())
            .spawn(move || {
                if let Err(error) = run(window, inbox, worker_signals, publish) {
                    eprintln!("notification source stopped: {error}");
                }
            });
        if let Err(error) = spawned {
            eprintln!("notification thread failed to start: {error}");
        }
        Self { signals, updates }
    }

    pub fn updates(&self) -> impl Iterator<Item = Update> + '_ {
        self.updates.try_iter()
    }

    /// Invoke from the window thread so Windows can display its consent prompt.
    pub fn request_access(&self) -> Result<()> {
        let listener = UserNotificationListener::Current()?;
        let signals = self.signals.clone();
        listener
            .RequestAccessAsync()?
            .SetCompleted(&AsyncOperationCompletedHandler::new(move |op, status| {
                if status == AsyncStatus::Completed
                    && op.as_ref().is_some_and(|op| {
                        op.GetResults().ok() == Some(UserNotificationListenerAccessStatus::Allowed)
                    })
                {
                    let _ = signals.send(Signal::Refresh);
                }
                Ok(())
            }))
    }
}

impl Drop for NotificationSource {
    fn drop(&mut self) {
        let _ = self.signals.send(Signal::Stop);
    }
}

fn run(
    window: isize,
    inbox: Receiver<Signal>,
    signals: Sender<Signal>,
    publish: Sender<Update>,
) -> Result<()> {
    unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
    let listener = UserNotificationListener::Current()?;
    let token = listener
        .NotificationChanged(&TypedEventHandler::new(move |_, _| {
            let _ = signals.send(Signal::Refresh);
            Ok(())
        }))
        .ok();
    let mut known = HashMap::<u32, Notice>::new();
    let mut seeded = false;
    loop {
        if listener.GetAccessStatus()? == UserNotificationListenerAccessStatus::Allowed {
            if let Ok(list) = listener
                .GetNotificationsAsync(NotificationKinds::Toast)?
                .join()
            {
                let mut current = HashMap::new();
                for item in list {
                    if let Ok(id) = item.Id()
                        && let Some(notice) = known.get(&id).cloned().or_else(|| read_notice(&item))
                    {
                        current.insert(id, notice);
                    }
                }
                if !seeded {
                    publish_update(
                        window,
                        &publish,
                        Update::Seed(current.keys().copied().collect()),
                    );
                    seeded = true;
                } else {
                    for (id, notice) in &current {
                        if !known.contains_key(id) {
                            publish_update(window, &publish, Update::Added(notice.clone()));
                        }
                    }
                    for id in known.keys() {
                        if !current.contains_key(id) {
                            publish_update(window, &publish, Update::Removed(*id));
                        }
                    }
                }
                known = current;
            }
        } else {
            known.clear();
            seeded = false;
        }
        match inbox.recv_timeout(CHECK_EVERY) {
            Ok(Signal::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Signal::Refresh) | Err(RecvTimeoutError::Timeout) => {}
        }
    }
    if let Some(token) = token {
        let _ = listener.RemoveNotificationChanged(token);
    }
    Ok(())
}

fn publish_update(window: isize, sender: &Sender<Update>, update: Update) {
    if sender.send(update).is_ok() {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(window as _)),
                MSG_NOTIFICATIONS,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

fn read_notice(item: &UserNotification) -> Option<Notice> {
    let app_info = item.AppInfo().ok()?;
    let display = app_info.DisplayInfo().ok();
    let visual = item.Notification().ok()?.Visual().ok()?;
    let generic = KnownNotificationBindings::ToastGeneric()
        .ok()
        .and_then(|name| visual.GetBinding(&name).ok())
        .and_then(|binding| binding_texts(&binding));
    let texts = generic
        .filter(|texts| !texts.is_empty())
        .or_else(|| {
            visual
                .Bindings()
                .ok()?
                .into_iter()
                .filter_map(|binding| binding_texts(&binding))
                .find(|texts| !texts.is_empty())
        })
        .unwrap_or_default();
    let app = display
        .as_ref()
        .and_then(|display| display.DisplayName().ok())
        .map_or_else(|| "Windows".to_owned(), |name| name.to_string());
    Some(Notice {
        id: item.Id().ok()?,
        app_id: app_info
            .AppUserModelId()
            .or_else(|_| app_info.Id())
            .ok()?
            .to_string(),
        app,
        title: texts.first().cloned().unwrap_or_default(),
        body: if texts.len() > 1 {
            texts[1..].join(" · ")
        } else {
            "Yeni bildirim".into()
        },
        logo: display.as_ref().and_then(read_logo),
    })
}

fn binding_texts(binding: &NotificationBinding) -> Option<Vec<String>> {
    Some(
        binding
            .GetTextElements()
            .ok()?
            .into_iter()
            .filter_map(|text| text.Text().ok().map(|value| value.to_string()))
            .filter(|text| !text.trim().is_empty())
            .collect(),
    )
}

fn read_logo(display: &windows::ApplicationModel::AppDisplayInfo) -> Option<Arc<[u8]>> {
    let logo = display
        .GetLogo(Size {
            Width: 48.0,
            Height: 48.0,
        })
        .ok()?;
    let stream = logo.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > MAX_LOGO_BYTES {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size as u32).ok()?.join().ok()?;
    let mut bytes = vec![0; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(bytes.into())
}
