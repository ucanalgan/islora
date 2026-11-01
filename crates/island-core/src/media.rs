use std::sync::Arc;

use crate::activity::{Activity, Icon, Kind, Millis};

/// A paused session stops drawing attention after this long, but stays
/// reachable: a peek or the shortcut brings it back so it can be resumed.
pub const PAUSED_LINGER_MS: Millis = 30_000;
/// How long a new track is announced for.
pub const TRACK_NOTICE_MS: Millis = 3_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl RepeatMode {
    /// The order one button steps through.
    pub fn next(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }
}

/// What a media session reported, with its position read at `sampled_at`.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaSnapshot {
    /// `None` while the player does not offer it, as for video.
    pub shuffle: Option<bool>,
    /// `None` while the player does not offer it, as for video.
    pub repeat: Option<RepeatMode>,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub rate: f32,
    pub sampled_at: Millis,
    /// Whether the player accepts jumps to a position.
    pub can_seek: bool,
    /// Encoded image as the player supplied it.
    pub artwork: Option<Arc<[u8]>>,
}

impl MediaSnapshot {
    /// Players report their position rarely, so it is projected forward.
    pub fn position_at(&self, now: Millis) -> u64 {
        let position = if self.playing {
            let elapsed = now.saturating_sub(self.sampled_at) as f32 * self.rate;
            self.position_ms + elapsed as u64
        } else {
            self.position_ms
        };
        position.min(self.duration_ms)
    }

    /// Progress from 0 to 1, or `None` for sources without a known length.
    pub fn progress_at(&self, now: Millis) -> Option<f32> {
        (self.duration_ms > 0).then(|| self.position_at(now) as f32 / self.duration_ms as f32)
    }

    pub fn is_same_track(&self, other: &MediaSnapshot) -> bool {
        self.title == other.title && self.artist == other.artist
    }

    /// The passing notice that this track has started.
    pub fn track_notice(&self, expires_at: Millis) -> Activity {
        Activity {
            title: self.title.clone(),
            detail: self.artist.clone(),
            expires_at: Some(expires_at),
            icon: Some(Icon::Music),
            preview: self.artwork.clone(),
            ..Activity::new(Kind::NowPlaying)
        }
    }

    pub fn into_activity(self, now: Millis) -> Activity {
        Activity {
            leading: self.title.clone(),
            title: self.title.clone(),
            detail: self.artist.clone(),
            dormant_at: (!self.playing).then_some(now + PAUSED_LINGER_MS),
            media: Some(self),
            ..Activity::new(Kind::Media)
        }
    }
}

/// Formats a length the way players do: `2:41`, or `1:02:41` past an hour.
pub fn format_time(ms: u64) -> String {
    let seconds = ms / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(playing: bool) -> MediaSnapshot {
        MediaSnapshot {
            title: "Song".into(),
            artist: "Artist".into(),
            playing,
            position_ms: 10_000,
            duration_ms: 100_000,
            rate: 1.0,
            sampled_at: 1_000,
            can_seek: true,
            artwork: None,
            shuffle: None,
            repeat: None,
        }
    }

    #[test]
    fn repeat_steps_through_every_mode_and_back() {
        assert_eq!(RepeatMode::Off.next(), RepeatMode::All);
        assert_eq!(RepeatMode::All.next(), RepeatMode::One);
        assert_eq!(RepeatMode::One.next(), RepeatMode::Off);
    }

    #[test]
    fn position_advances_only_while_playing() {
        assert_eq!(snapshot(true).position_at(6_000), 15_000);
        assert_eq!(snapshot(false).position_at(6_000), 10_000);
    }

    #[test]
    fn position_follows_the_playback_rate() {
        let fast = MediaSnapshot { rate: 2.0, ..snapshot(true) };
        assert_eq!(fast.position_at(6_000), 20_000);
    }

    #[test]
    fn position_never_passes_the_end() {
        assert_eq!(snapshot(true).position_at(10_000_000), 100_000);
    }

    #[test]
    fn progress_is_unknown_without_a_duration() {
        let live = MediaSnapshot { duration_ms: 0, ..snapshot(true) };
        assert_eq!(live.progress_at(2_000), None);
        assert_eq!(snapshot(false).progress_at(2_000), Some(0.1));
    }

    #[test]
    fn only_a_paused_session_goes_dormant() {
        assert_eq!(snapshot(true).into_activity(5_000).dormant_at, None);
        assert_eq!(snapshot(false).into_activity(5_000).dormant_at, Some(5_000 + PAUSED_LINGER_MS));
    }

    #[test]
    fn times_format_like_a_player() {
        assert_eq!(format_time(0), "0:00");
        assert_eq!(format_time(161_000), "2:41");
        assert_eq!(format_time(3_761_000), "1:02:41");
    }
}
