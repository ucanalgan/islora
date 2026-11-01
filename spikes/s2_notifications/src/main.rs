//! S2: paket kimliğiyle `UserNotificationListener` olayları geliyor mu?
//!
//! Kullanım:
//!   s2_notifications            durumu yazar, 30 saniye olay dinler
//!   s2_notifications --request  önce Windows'tan erişim izni ister
//!
//! Bildirim metni yazdırılmaz, yalnızca uygulama adı ve kimlik numarası.

use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{NotificationKinds, UserNotificationChangedEventArgs};
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostQuitMessage, SetTimer, TranslateMessage, WM_TIMER,
};
use windows::core::{PWSTR, Result};
use windows_future::{AsyncOperationCompletedHandler, AsyncStatus};

const LISTEN_MS: u32 = 30_000;

fn main() -> Result<()> {
    unsafe { RoInitialize(RO_INIT_SINGLETHREADED)? };

    println!("kimlik: {}", package_name());

    let listener = match UserNotificationListener::Current() {
        Ok(listener) => listener,
        Err(error) => {
            println!("Current hatasi: {:#x} {}", error.code().0, error.message());
            return Ok(());
        }
    };

    match listener.GetAccessStatus() {
        Ok(status) => println!("erisim: {}", status_name(status)),
        Err(error) => println!("GetAccessStatus hatasi: {:#x} {}", error.code().0, error.message()),
    }

    let request = if std::env::args().any(|a| a == "--request") {
        let operation = listener.RequestAccessAsync()?;
        operation.SetCompleted(&AsyncOperationCompletedHandler::new(|op, status| {
            if status == AsyncStatus::Completed {
                if let Some(op) = op.as_ref() {
                    println!("izin sonucu: {}", status_name(op.GetResults()?));
                }
            } else {
                println!("izin istegi bitti: {}", status.0);
            }
            Ok(())
        }))?;
        Some(operation)
    } else {
        None
    };

    match listener.GetNotificationsAsync(NotificationKinds::Toast).and_then(|op| op.join()) {
        Ok(list) => println!("mevcut bildirim: {}", list.Size()?),
        Err(error) => println!("GetNotifications hatasi: {:#x} {}", error.code().0, error.message()),
    }

    let events = TypedEventHandler::<UserNotificationListener, UserNotificationChangedEventArgs>::new(
        |sender, args| {
            let (Some(sender), Some(args)) = (sender.as_ref(), args.as_ref()) else {
                return Ok(());
            };
            let id = args.UserNotificationId()?;
            let app = sender
                .GetNotification(id)
                .and_then(|n| n.AppInfo()?.DisplayInfo()?.DisplayName())
                .map(|n| n.to_string())
                .unwrap_or_else(|_| "?".into());
            println!("OLAY tur={} id={} uygulama={}", args.ChangeKind()?.0, id, app);
            Ok(())
        },
    );
    match listener.NotificationChanged(&events) {
        Ok(_) => println!("olay aboneligi: tamam"),
        Err(error) => println!("olay aboneligi hatasi: {:#x} {}", error.code().0, error.message()),
    }

    unsafe {
        SetTimer(None, 0, LISTEN_MS, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_TIMER {
                PostQuitMessage(0);
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    drop(request);
    println!("bitti");
    Ok(())
}

fn package_name() -> String {
    let mut len = 0u32;
    let first = unsafe { GetCurrentPackageFullName(&mut len, None) };
    if first != ERROR_INSUFFICIENT_BUFFER {
        return format!("yok (kod {})", first.0);
    }
    let mut buffer = vec![0u16; len as usize];
    let _ = unsafe { GetCurrentPackageFullName(&mut len, Some(PWSTR(buffer.as_mut_ptr()))) };
    String::from_utf16_lossy(&buffer[..len.saturating_sub(1) as usize])
}

fn status_name(status: UserNotificationListenerAccessStatus) -> &'static str {
    match status {
        UserNotificationListenerAccessStatus::Allowed => "izin verildi",
        UserNotificationListenerAccessStatus::Denied => "reddedildi",
        _ => "belirtilmedi",
    }
}
