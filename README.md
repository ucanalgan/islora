# Islora

A live island for Windows 11. Islora shows what your PC is doing in a small black or glass pill at the top of the screen, and hides when there is nothing to show.

<table>
  <tr>
    <td><img src="assets/screenshots/island.png" alt="Music in the pill with the camera indicator in a bubble beside it"></td>
    <td><img src="assets/screenshots/notch.png" alt="The notch style, attached to the top edge of the screen"></td>
  </tr>
  <tr>
    <td><img src="assets/screenshots/music.png" alt="Expanded music controls"></td>
    <td><img src="assets/screenshots/timer.png" alt="Expanded running timer"></td>
  </tr>
  <tr>
    <td><img src="assets/screenshots/drop.png" alt="Files dragged to the island, over the three places they can go"></td>
    <td><img src="assets/screenshots/phone.png" alt="A QR code for a phone to download a file over the local network"></td>
  </tr>
  <tr>
    <td><img src="assets/screenshots/glass.png" alt="The glass material, showing the colours behind it"></td>
    <td><img src="assets/screenshots/settings.png" alt="The settings window on its appearance page, with the progress style for sending"></td>
  </tr>
</table>

> [!NOTE]
> Islora is at an early stage. The interface is in Turkish only, and Islora has only been run on a Windows 11 Insider build so far. The timer and calendar screens will change.

## Download

Get `islora.exe` from the [latest release](https://github.com/ucanalgan/islora/releases/latest), put it in any folder and run it. There is no installer. It needs 64-bit Windows 11.

The file is not signed, so SmartScreen may warn you the first time. Choose **More info**, then **Run anyway**.

## What it shows

- **Music and video:** cover, title, artist, progress and playback buttons for any player that reports to Windows, such as Spotify, or YouTube and YouTube Music in a browser. You can seek, shuffle and repeat when the player allows it. Where a player has no previous or next, as for a single video, the buttons beside play jump 10 seconds back and 30 forward. Songs are told from videos by what comes with them, such as an album, a square cover, the controls on offer and the title, not by which app or site plays them, so a new song is announced and the next video is not.
- **Timers:** Islora's own countdown, Windows Focus sessions in both directions, and running timers from the Clock app. Clock timers are read-only, as Windows offers no public way to create them.
- **Calendar:** events from a private iCal address, as a bubble 30 minutes before they start, in the pill 5 minutes before, and as an alert when they begin.
- **Files:** drag files to the top of the screen and three places open. **Raf** keeps them at hand to drag somewhere else later. **Telefona gönder** shows a code that a phone camera scans to download them over your network, with no app or account on the phone. **Dönüştür** saves pictures as JPG or PNG, or makes them smaller, next to the original.
- **System changes:** volume, brightness, Caps Lock, Num Lock, charger, low battery, Energy Saver, Bluetooth, sound output, microphone, camera, internet, Wi-Fi, VPN, airplane mode, USB drives, displays, screenshots and screen clips, copied items, new songs and keyboard language.

Two things can run at once: the main one sits in the pill and the second in a bubble beside it.

The island hides during fullscreen apps and leaves itself out of screenshots, screen recordings and screen sharing.

## Using it

| To | Do this |
|---|---|
| Open the island | Press `Win+Alt+Space`, or move the pointer to the middle of the top screen edge. If that shortcut is taken, `Ctrl+Shift+Alt+Space` is used instead. |
| Expand or collapse it | Click the pill. Moving the pointer away also collapses it. |
| Switch to the second thing | Click the bubble. |
| Keep, send or convert files | Drag them to the middle of the top screen edge, or onto the island, and drop them on one of the three places. Files on the shelf wait in a bubble beside the island: click it to drag them out again. |
| Start a timer or see your next events | Open the home view with the timer icon or by pressing the shortcut twice. |
| Change settings | Open Islora again from the Start menu or its exe, or use the gear in the home view or the tray menu. The window works with the keyboard too: Tab and the arrows move, Enter or Space press, Esc closes. |

## Settings

The settings window opens by itself on first run, and changes apply at once. Everything in it is stored in `%APPDATA%\Islora\config.toml`, which you can also edit by hand. After editing the file, choose **Takvimi ve ayarları yenile** on the window's Genel page.

| Setting | Meaning | Default |
|---|---|---|
| `calendar_ics_url` | Private iCal address of your calendar. Treat it like a password. | empty |
| `timer_presets` | The four durations in the home view, in minutes. | `5, 15, 25, 45` |
| `start_mode` | What those buttons start: `focus` for a Windows Focus session, `timer` for Islora's own countdown. | `focus` |
| `material` | `black`, or `glass` for a frosted view of what is behind the pill. Glass turns black in Energy Saver and with Windows transparency off. | `black` |
| `island_style` | `island` floats below the top edge, `notch` is attached to it. | `island` |
| `notices_off` | Notices to hide, separated by commas. The file lists the names. | empty |
| `progress_style` | How the open view shows a send to a phone: `ring`, `edge` for light running along the island's edge, or `fill`. | `ring` |
| `sound_wave` | `off` stills the wave in the small pill, which is lighter on the system. Energy Saver stills it too. | `on` |

## Privacy

Islora has no account and no telemetry. On its own it makes one kind of network request: it downloads the calendar address you set, every 15 minutes.

While you send files to a phone, and only then, Islora runs a small web server on your computer that only devices on the same network can reach. It answers only at an address with a one-time code in it, which the QR code carries, offers only the files you dropped, and stops once they are downloaded or after 5 minutes without a download. The files go straight from your computer to the phone and never through the internet.

The shelf keeps where your files are, never copies, and forgets them when Islora quits. Converted pictures are saved next to the original, which is left as it was.

Islora never reads what you copy, only whether it was text, an image or a file, and it skips copies marked by password managers. Of the keyboard it notices only Caps Lock, Num Lock, the language shortcut and the screen clip shortcut, and keeps nothing you type. From the Clock app's settings it reads the timers and nothing else.

## System load

Islora is written in Rust with no runtime or web view. Nothing polls: every source waits for a Windows event, and shape animations run in the Windows compositor, so the app itself does no work per frame. With nothing to show, v0.0.3 used 0 ms of CPU over 15 seconds and about 8 MB of private memory, the same as before files could be dropped on it. The web server for a phone and the picture work run only while they are needed.

The exception is the sound wave in the small pill while music plays. It loops, so the compositor (`dwm.exe`) redraws it on every frame, which costs about half a CPU core at 144 Hz. Turn off **Ses dalgası** in the settings to stop it. It also stops by itself while Windows Energy Saver is on.

## Known limits

- App notifications are not shown yet.
- Only the primary display is used.
- The Windows volume flyout and notification banners still appear beside the island.
- The first time you send to a phone, Windows asks whether Islora may use the network. Allow it on private networks, or the phone cannot connect. On a network Windows treats as public, phones usually cannot connect at all.
- Telling songs from videos rests on what each site sends. A page that sends nothing counts as a video, and an odd title can be misread.
- A screen clip shows a preview only when the Snipping Tool saves it as a file. Otherwise the island says it was copied.
- Converting HEIC pictures needs Microsoft's HEIF Image Extensions, and AVIF the AV1 Video Extension, both free in the Microsoft Store.
- Some notices are unit tested but have not been checked against real events yet. The [release notes](https://github.com/ucanalgan/islora/releases/latest) list them.

## Building

You need current stable Rust and the MSVC build tools with the Windows SDK.

```powershell
cargo build --release
cargo test -p island-core
```

The app is `target\release\islora.exe`. `crates/island-core` holds the logic and has no Windows dependency, so it can be tested on its own. `crates/island-app` holds the window, drawing and event sources. `spikes` keeps the early experiments and is not part of the build.

## License

No license has been chosen yet, so all rights are reserved for now.
