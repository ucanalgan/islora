//! A silent media player for tests. It registers a real system media session
//! with a title, artist and cover, and prints every command it receives, so the
//! island's media controls can be checked without a real player or sound.
//!
//! test_player <silent.wav> <cover.png> <seconds to run>

use windows::Foundation::{TypedEventHandler, Uri};
use windows::Media::Core::MediaSource;
use windows::Media::MediaPlaybackType;
use windows::Media::Playback::{
    MediaPlaybackCommandManagerAutoRepeatModeReceivedEventArgs,
    MediaPlaybackCommandManagerNextReceivedEventArgs,
    MediaPlaybackCommandManagerPositionReceivedEventArgs,
    MediaPlaybackCommandManagerPreviousReceivedEventArgs,
    MediaPlaybackCommandManagerShuffleReceivedEventArgs, MediaPlaybackItem, MediaPlaybackSession,
    MediaPlaybackState, MediaPlayer,
};
use windows::Storage::StorageFile;
use windows::Storage::Streams::RandomAccessStreamReference;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostQuitMessage, SetTimer, TranslateMessage, WM_TIMER,
};
use windows::core::{HSTRING, Result, h};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (wav, cover, seconds) = (&args[1], &args[2], args[3].parse::<u32>().unwrap_or(30));
    unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };

    let player = MediaPlayer::new()?;
    let uri = Uri::CreateUri(&HSTRING::from(format!("file:///{}", wav.replace('\\', "/"))))?;
    let item = MediaPlaybackItem::Create(&MediaSource::CreateFromUri(&uri)?)?;

    // With the command manager on, the player publishes what the item says
    // and ignores the display updater.
    let display = item.GetDisplayProperties()?;
    display.SetType(MediaPlaybackType::Music)?;
    let music = display.MusicProperties()?;
    music.SetTitle(h!("Deneme Şarkısı Uzun Bir Başlıkla"))?;
    music.SetArtist(h!("Test Sanatçısı"))?;
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(cover))?.join()?;
    display.SetThumbnail(&RandomAccessStreamReference::CreateFromFile(&file)?)?;
    item.ApplyDisplayProperties(&display)?;
    player.SetSource(&item)?;

    let commands = player.CommandManager()?;
    commands.NextBehavior()?.SetEnablingRule(windows::Media::Playback::MediaCommandEnablingRule::Always)?;
    commands.PreviousBehavior()?.SetEnablingRule(windows::Media::Playback::MediaCommandEnablingRule::Always)?;
    commands.PositionBehavior()?.SetEnablingRule(windows::Media::Playback::MediaCommandEnablingRule::Always)?;
    commands.PositionReceived(&TypedEventHandler::<_, MediaPlaybackCommandManagerPositionReceivedEventArgs>::new(|_, args| {
        if let Some(args) = args.as_ref() {
            println!("command position {}", args.Position()?.Duration / 10_000_000);
        }
        Ok(())
    }))?;
    commands.NextReceived(&TypedEventHandler::<_, MediaPlaybackCommandManagerNextReceivedEventArgs>::new(|_, _| {
        println!("command next");
        Ok(())
    }))?;
    commands.PreviousReceived(&TypedEventHandler::<_, MediaPlaybackCommandManagerPreviousReceivedEventArgs>::new(|_, _| {
        println!("command previous");
        Ok(())
    }))?;

    commands.ShuffleBehavior()?.SetEnablingRule(windows::Media::Playback::MediaCommandEnablingRule::Always)?;
    commands.AutoRepeatModeBehavior()?.SetEnablingRule(windows::Media::Playback::MediaCommandEnablingRule::Always)?;
    let shuffle_controls = player.SystemMediaTransportControls()?;
    commands.ShuffleReceived(&TypedEventHandler::<_, MediaPlaybackCommandManagerShuffleReceivedEventArgs>::new(move |_, args| {
        if let Some(args) = args.as_ref() {
            let requested = args.IsShuffleRequested()?;
            println!("command shuffle {requested}");
            shuffle_controls.SetShuffleEnabled(requested)?;
        }
        Ok(())
    }))?;
    let repeat_controls = player.SystemMediaTransportControls()?;
    commands.AutoRepeatModeReceived(&TypedEventHandler::<_, MediaPlaybackCommandManagerAutoRepeatModeReceivedEventArgs>::new(move |_, args| {
        if let Some(args) = args.as_ref() {
            let requested = args.AutoRepeatMode()?;
            println!("command repeat {}", requested.0);
            repeat_controls.SetAutoRepeatMode(requested)?;
        }
        Ok(())
    }))?;

    player.PlaybackSession()?.PlaybackStateChanged(&TypedEventHandler::<MediaPlaybackSession, _>::new(|session, _| {
        if let Some(session) = session.as_ref() {
            let state = session.PlaybackState()?;
            if state == MediaPlaybackState::Playing {
                println!("state playing");
            } else if state == MediaPlaybackState::Paused {
                println!("state paused");
            }
        }
        Ok(())
    }))?;

    player.Play()?;
    println!("started");

    unsafe {
        SetTimer(None, 0, seconds * 1000, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_TIMER {
                PostQuitMessage(0);
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
