//! Follows the media session Windows considers current.
//!
//! WinRT calls here can take a while, so they run on their own thread, which
//! blocks on a channel and costs nothing until something actually changes.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use island_core::media::RepeatMode;
use island_core::media_kind::{self, ArtShape, Reported, Signals, art_shape, display_artist};
use island_core::{MediaButton, MediaSnapshot, Millis};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionMediaProperties as Properties,
    GlobalSystemMediaTransportControlsSessionPlaybackInfo as PlaybackInfo,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Media::{MediaPlaybackAutoRepeatMode, MediaPlaybackType};
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::Result;

pub const MSG_MEDIA: u32 = WM_APP + 4;

const TICKS_PER_MS: i64 = 10_000;
/// Seconds between the Windows epoch (1601) and the Unix epoch (1970).
const EPOCH_GAP_SECONDS: u64 = 11_644_473_600;
/// Refuses absurd thumbnails rather than holding them in memory.
const MAX_ARTWORK_BYTES: u64 = 8 * 1024 * 1024;
/// Browsers name a new track before they swap its picture, sometimes without
/// saying so, so the picture is read once more this long after a change.
const ARTWORK_SETTLE: Duration = Duration::from_millis(1_500);

enum Signal {
    SessionChanged,
    MetadataChanged,
    StateChanged,
    TimelineChanged,
    Press(MediaButton),
    Seek(u64),
}

pub struct MediaSource {
    signals: Sender<Signal>,
    updates: Receiver<Option<MediaSnapshot>>,
}

impl MediaSource {
    /// `clock` is the app's time origin, so snapshots share its timeline.
    ///
    /// With `only_session`, only the session of that app is followed and every
    /// other one is ignored. Tests use it so they can never control the user's
    /// real player, which Windows may otherwise make current at any moment.
    pub fn start(hwnd: HWND, clock: Instant, only_session: Option<String>) -> Self {
        let (signals, inbox) = mpsc::channel();
        let (publish, updates) = mpsc::channel();
        let worker_signals = signals.clone();
        let window = hwnd.0 as isize;
        let spawned = std::thread::Builder::new()
            .name("media".into())
            .spawn(move || {
                let worker = Worker {
                    signals: worker_signals,
                    inbox,
                    publish,
                    window,
                    clock,
                    only_session,
                };
                if let Err(error) = worker.run() {
                    eprintln!("media source stopped: {error}");
                }
            });
        if let Err(error) = spawned {
            eprintln!("media thread failed to start: {error}");
        }
        Self { signals, updates }
    }

    pub fn press(&self, button: MediaButton) {
        let _ = self.signals.send(Signal::Press(button));
    }

    pub fn seek(&self, position_ms: u64) {
        let _ = self.signals.send(Signal::Seek(position_ms));
    }

    /// The newest update, if any arrived. `Some(None)` means media ended.
    pub fn latest(&self) -> Option<Option<MediaSnapshot>> {
        self.updates.try_iter().last()
    }
}

/// A session with our handlers attached, detached again on drop.
struct Tracked {
    session: Session,
    metadata_token: i64,
    playback_token: i64,
    timeline_token: i64,
}

impl Tracked {
    fn new(session: Session, signals: &Sender<Signal>) -> Result<Self> {
        let metadata = signals.clone();
        let metadata_token =
            session.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                let _ = metadata.send(Signal::MetadataChanged);
                Ok(())
            }))?;
        let playback = signals.clone();
        let playback_token =
            session.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                let _ = playback.send(Signal::StateChanged);
                Ok(())
            }))?;
        let timeline = signals.clone();
        let timeline_token =
            session.TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
                let _ = timeline.send(Signal::TimelineChanged);
                Ok(())
            }))?;
        Ok(Self {
            session,
            metadata_token,
            playback_token,
            timeline_token,
        })
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        let _ = self
            .session
            .RemoveMediaPropertiesChanged(self.metadata_token);
        let _ = self.session.RemovePlaybackInfoChanged(self.playback_token);
        let _ = self
            .session
            .RemoveTimelinePropertiesChanged(self.timeline_token);
    }
}

struct Metadata {
    title: String,
    /// As the player gave it, which the decision about music needs.
    artist: String,
    album: bool,
    /// Fields only music apps fill in: a track number or count, a genre, an
    /// album artist.
    music_details: bool,
    /// What the player says it plays, if it says.
    reported: Option<MediaPlaybackType>,
    artwork: Option<Arc<[u8]>>,
    art: ArtShape,
}

impl Default for Metadata {
    fn default() -> Self {
        Self {
            title: String::new(),
            artist: String::new(),
            album: false,
            music_details: false,
            reported: None,
            artwork: None,
            art: ArtShape::None,
        }
    }
}

struct Worker {
    signals: Sender<Signal>,
    inbox: Receiver<Signal>,
    publish: Sender<Option<MediaSnapshot>>,
    window: isize,
    clock: Instant,
    only_session: Option<String>,
}

impl Worker {
    fn run(self) -> Result<()> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
        let manager = Manager::RequestAsync()?.join()?;

        let current = self.signals.clone();
        manager.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
            let _ = current.send(Signal::SessionChanged);
            Ok(())
        }))?;
        let sessions = self.signals.clone();
        manager.SessionsChanged(&TypedEventHandler::new(move |_, _| {
            let _ = sessions.send(Signal::SessionChanged);
            Ok(())
        }))?;
        let _ = self.signals.send(Signal::SessionChanged);

        let mut tracked: Option<Tracked> = None;
        let mut metadata = Metadata::default();
        let mut look_again: Option<Instant> = None;

        loop {
            // Asleep until something happens, or until the picture is due a second look.
            let first = match look_again {
                Some(at) => match self
                    .inbox
                    .recv_timeout(at.saturating_duration_since(Instant::now()))
                {
                    Ok(signal) => Some(signal),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match self.inbox.recv() {
                    Ok(signal) => Some(signal),
                    Err(_) => break,
                },
            };
            // Players send bursts of events; handle each burst once. A second
            // look at the picture is a metadata change of its own.
            let mut session_changed = false;
            let mut metadata_changed = first.is_none();
            if first.is_none() {
                look_again = None;
            }
            let mut timeline_changed = false;
            let mut presses = Vec::new();
            let mut seek = None;
            for signal in first.into_iter().chain(self.inbox.try_iter()) {
                match signal {
                    Signal::SessionChanged => session_changed = true,
                    Signal::MetadataChanged => metadata_changed = true,
                    Signal::StateChanged => {}
                    Signal::TimelineChanged => timeline_changed = true,
                    Signal::Press(button) => presses.push(button),
                    Signal::Seek(position_ms) => seek = Some(position_ms),
                }
            }

            if session_changed {
                tracked = self
                    .followed_session(&manager)
                    .and_then(|session| Tracked::new(session, &self.signals).ok());
                metadata = Metadata::default();
                metadata_changed = true;
            }

            let Some(session) = tracked.as_ref().map(|t| &t.session) else {
                self.send(None);
                continue;
            };
            for button in presses {
                press(session, button);
            }
            if metadata_changed {
                let before = metadata.title.clone();
                metadata = read_metadata(session, &metadata).unwrap_or_default();
                if metadata.title != before {
                    look_again = Some(Instant::now() + ARTWORK_SETTLE);
                }
            }
            // Reading now would still return the old position. The player
            // reports the new one by itself, unless it refused.
            if seek.is_some_and(|position_ms| seek_to(session, position_ms)) && !metadata_changed {
                continue;
            }
            let new_position = timeline_changed || metadata_changed;
            self.send(
                self.snapshot(session, &metadata, new_position)
                    .unwrap_or(None),
            );
        }
        Ok(())
    }

    fn followed_session(&self, manager: &Manager) -> Option<Session> {
        let Some(only) = &self.only_session else {
            return manager.GetCurrentSession().ok();
        };
        let sessions = manager.GetSessions().ok()?;
        sessions.into_iter().find(|session| {
            session
                .SourceAppUserModelId()
                .is_ok_and(|id| id == only.as_str())
        })
    }

    fn send(&self, update: Option<MediaSnapshot>) {
        if self.publish.send(update).is_ok() {
            let window = HWND(self.window as _);
            unsafe {
                let _ = PostMessageW(Some(window), MSG_MEDIA, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn snapshot(
        &self,
        session: &Session,
        metadata: &Metadata,
        new_position: bool,
    ) -> Result<Option<MediaSnapshot>> {
        let info = session.GetPlaybackInfo()?;
        let status = info.PlaybackStatus()?;
        if metadata.title.is_empty() || matches!(status, Status::Closed | Status::Stopped) {
            return Ok(None);
        }
        let playing = status == Status::Playing;
        let rate = info
            .PlaybackRate()
            .and_then(|rate| rate.Value())
            .unwrap_or(1.0) as f32;

        let timeline = session.GetTimelineProperties()?;
        let start = timeline.StartTime()?.Duration;
        let end = timeline.EndTime()?.Duration;
        let position = timeline.Position()?.Duration;
        let updated = timeline.LastUpdatedTime()?.UniversalTime;

        let mut position_ms = (position - start).max(0) / TICKS_PER_MS;
        if playing && updated > 0 {
            // The reported position is as of `updated`, which may be seconds ago.
            let since_update = (windows_time_now() - updated).max(0) / TICKS_PER_MS;
            position_ms += (since_update as f32 * rate) as i64;
        }

        let controls = info.Controls()?;
        let kind = info
            .PlaybackType()
            .and_then(|kind| kind.Value())
            .ok()
            .or(metadata.reported);
        let reported = match kind {
            Some(MediaPlaybackType::Video) => Reported::Video,
            Some(MediaPlaybackType::Music) => Reported::Music,
            _ => Reported::Unknown,
        };
        let video = reported == Reported::Video;
        let shuffle = !video && controls.IsShuffleEnabled()?;
        let repeat = !video && controls.IsRepeatEnabled()?;
        let can_previous = controls.IsPreviousEnabled()?;
        let can_next = controls.IsNextEnabled()?;
        let can_seek = controls.IsPlaybackPositionEnabled()?;
        let duration_ms = ((end - start).max(0) / TICKS_PER_MS) as u64;
        let is_music = media_kind::is_music(&Signals {
            reported,
            title: &metadata.title,
            artist: &metadata.artist,
            album: metadata.album,
            music_details: metadata.music_details,
            shuffle_or_repeat: shuffle || repeat,
            channels: controls.IsChannelUpEnabled()? || controls.IsChannelDownEnabled()?,
            tracks: can_previous || can_next,
            live: duration_ms == 0 && !can_seek,
            duration_ms,
            art: metadata.art,
        });
        Ok(Some(MediaSnapshot {
            new_position,
            shuffle: shuffle.then(|| shuffle_of(&info)),
            repeat: repeat.then(|| repeat_of(&info)),
            is_music,
            can_previous,
            can_next,
            title: metadata.title.clone(),
            artist: display_artist(&metadata.artist).to_owned(),
            playing,
            position_ms: position_ms as u64,
            duration_ms,
            rate,
            sampled_at: self.clock.elapsed().as_millis() as Millis,
            can_seek,
            artwork: metadata.artwork.clone(),
        }))
    }
}

fn windows_time_now() -> i64 {
    let since_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (since_unix.as_secs() + EPOCH_GAP_SECONDS) as i64 * 10_000_000
        + since_unix.subsec_nanos() as i64 / 100
}

fn shuffle_of(info: &PlaybackInfo) -> bool {
    info.IsShuffleActive()
        .and_then(|active| active.Value())
        .unwrap_or(false)
}

fn repeat_of(info: &PlaybackInfo) -> RepeatMode {
    match info.AutoRepeatMode().and_then(|mode| mode.Value()) {
        Ok(MediaPlaybackAutoRepeatMode::List) => RepeatMode::All,
        Ok(MediaPlaybackAutoRepeatMode::Track) => RepeatMode::One,
        _ => RepeatMode::Off,
    }
}

fn press(session: &Session, button: MediaButton) {
    let operation = match button {
        MediaButton::Previous => session.TrySkipPreviousAsync(),
        MediaButton::PlayPause => session.TryTogglePlayPauseAsync(),
        MediaButton::Next => session.TrySkipNextAsync(),
        MediaButton::Shuffle => session
            .GetPlaybackInfo()
            .and_then(|info| session.TryChangeShuffleActiveAsync(!shuffle_of(&info))),
        MediaButton::Repeat => session.GetPlaybackInfo().and_then(|info| {
            session.TryChangeAutoRepeatModeAsync(match repeat_of(&info).next() {
                RepeatMode::Off => MediaPlaybackAutoRepeatMode::None,
                RepeatMode::All => MediaPlaybackAutoRepeatMode::List,
                RepeatMode::One => MediaPlaybackAutoRepeatMode::Track,
            })
        }),
    };
    if let Err(error) = operation.and_then(|operation| operation.join()) {
        eprintln!("media command failed: {error}");
    }
}

/// Returns whether the player accepted the jump.
fn seek_to(session: &Session, position_ms: u64) -> bool {
    let start = session.GetTimelineProperties().and_then(|t| t.StartTime());
    let ticks = start.map_or(0, |start| start.Duration) + position_ms as i64 * TICKS_PER_MS;
    match session
        .TryChangePlaybackPositionAsync(ticks)
        .and_then(|operation| operation.join())
    {
        Ok(accepted) => accepted,
        Err(error) => {
            eprintln!("seek failed: {error}");
            false
        }
    }
}

fn read_metadata(session: &Session, previous: &Metadata) -> Result<Metadata> {
    let properties = session.TryGetMediaPropertiesAsync()?.join()?;
    let title = properties.Title()?.to_string();
    let artist = properties.Artist()?.to_string();
    let album = properties.AlbumTitle().is_ok_and(|album| !album.is_empty());
    let music_details = properties.TrackNumber().unwrap_or(0) > 0
        || properties.AlbumTrackCount().unwrap_or(0) > 0
        || properties
            .AlbumArtist()
            .is_ok_and(|artist| !artist.is_empty())
        || properties
            .Genres()
            .and_then(|genres| genres.Size())
            .unwrap_or(0)
            > 0;
    let reported = properties.PlaybackType().and_then(|kind| kind.Value()).ok();

    // The picture is read every time, as players often send the new text
    // with the old picture and the new picture only afterwards. An unchanged
    // picture keeps its old copy, so nothing repaints for it.
    let same_track = title == previous.title && artist == previous.artist;
    let artwork = match (read_artwork(&properties), &previous.artwork) {
        (Some(fresh), Some(known)) if fresh == *known => Some(known.clone()),
        (Some(fresh), _) => Some(fresh),
        // A picture that is late for the same track keeps the one it had.
        (None, Some(known)) if same_track => Some(known.clone()),
        (None, _) => None,
    };
    let art = artwork.as_deref().map_or(ArtShape::None, art_shape);
    Ok(Metadata {
        title,
        artist,
        album,
        music_details,
        reported,
        artwork,
        art,
    })
}

fn read_artwork(properties: &Properties) -> Option<Arc<[u8]>> {
    let stream = properties
        .Thumbnail()
        .ok()?
        .OpenReadAsync()
        .ok()?
        .join()
        .ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > MAX_ARTWORK_BYTES {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size as u32).ok()?.join().ok()?;
    let mut bytes = vec![0; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(bytes.into())
}
