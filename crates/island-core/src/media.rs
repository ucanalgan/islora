use std::sync::Arc;

use crate::activity::{Activity, Icon, Kind, Millis};

/// A paused session stops drawing attention after this long, but stays
/// reachable: a peek or the shortcut brings it back so it can be resumed.
pub const PAUSED_LINGER_MS: Millis = 30_000;
/// How long a new track is announced for.
pub const TRACK_NOTICE_MS: Millis = 3_000;
/// How far the buttons beside play jump in a player without tracks to skip.
pub const JUMP_BACK_MS: u64 = 10_000;
pub const JUMP_FORWARD_MS: u64 = 30_000;
/// Players report their position roughly. A difference this small is that, not
/// a jump, and following it would make the time on screen twitch.
const POSITION_NOISE_MS: u64 = 3_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

/// What the buttons either side of play do, which depends on what the player
/// offers rather than on which app it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sides {
    /// The previous and next track, as in music players and playlists.
    Tracks,
    /// Back and forward within what plays, as for a single video.
    Jumps,
    /// Nothing the player accepts, so the buttons only show what they would be.
    Unavailable,
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
    /// Whether the player said anything about the position. A change such as
    /// shuffle arrives with an old position, which means nothing.
    pub new_position: bool,
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
    /// A song rather than a video or a recording. Browsers call everything
    /// music, so this rests on what came with it: an album, or a player that
    /// can shuffle and repeat.
    pub is_music: bool,
    pub can_previous: bool,
    pub can_next: bool,
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

    pub fn sides(&self) -> Sides {
        if self.can_previous || self.can_next {
            Sides::Tracks
        } else if self.can_seek && self.duration_ms > 0 {
            Sides::Jumps
        } else {
            Sides::Unavailable
        }
    }

    /// Where a jump back or forward lands, within the track.
    pub fn jump_target(&self, forward: bool, now: Millis) -> u64 {
        let position = self.position_at(now);
        if forward {
            (position + JUMP_FORWARD_MS).min(self.duration_ms)
        } else {
            position.saturating_sub(JUMP_BACK_MS)
        }
    }

    pub fn is_same_track(&self, other: &MediaSnapshot) -> bool {
        self.title == other.title && self.artist == other.artist
    }

    /// Carries on counting from `earlier`, unless this report really moved the
    /// track somewhere else: a jump, a new track, or a pause or resume.
    pub fn continue_from(&mut self, earlier: &MediaSnapshot, now: Millis) {
        let same_run = self.is_same_track(earlier)
            && self.playing == earlier.playing
            && self.rate == earlier.rate;
        let close = self.position_at(now).abs_diff(earlier.position_at(now)) <= POSITION_NOISE_MS;
        if same_run && (close || !self.new_position) {
            self.position_ms = earlier.position_ms;
            self.sampled_at = earlier.sampled_at;
        }
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
            new_position: true,
            shuffle: None,
            repeat: None,
            is_music: true,
            can_previous: true,
            can_next: true,
        }
    }

    #[test]
    fn the_buttons_follow_what_the_player_offers() {
        assert_eq!(snapshot(true).sides(), Sides::Tracks);
        let video = MediaSnapshot { can_previous: false, can_next: false, ..snapshot(true) };
        assert_eq!(video.sides(), Sides::Jumps);
        let live = MediaSnapshot { can_seek: false, ..video.clone() };
        assert_eq!(live.sides(), Sides::Unavailable);
        // Only next, as at the start of a playlist, still means tracks.
        let first = MediaSnapshot { can_previous: false, ..snapshot(true) };
        assert_eq!(first.sides(), Sides::Tracks);
    }

    #[test]
    fn jumps_stay_within_the_track() {
        let paused = snapshot(false);
        assert_eq!(paused.jump_target(false, 5_000), 0);
        assert_eq!(paused.jump_target(true, 5_000), 40_000);
        let near_end = MediaSnapshot { position_ms: 90_000, ..paused };
        assert_eq!(near_end.jump_target(true, 5_000), 100_000);
        assert_eq!(near_end.jump_target(false, 5_000), 80_000);
    }

    #[test]
    fn a_report_without_position_news_keeps_the_count() {
        let earlier = snapshot(true);
        let mut report = MediaSnapshot { position_ms: 2_000, sampled_at: 9_000, new_position: false, ..snapshot(true) };
        report.continue_from(&earlier, 9_000);
        assert_eq!(report.position_at(9_000), 18_000);
    }

    #[test]
    fn a_small_difference_in_a_position_report_is_noise() {
        let earlier = snapshot(true);
        let mut report = MediaSnapshot { position_ms: 16_500, sampled_at: 9_000, ..snapshot(true) };
        report.continue_from(&earlier, 9_000);
        assert_eq!(report.position_at(9_000), 18_000);
    }

    #[test]
    fn a_real_jump_is_followed() {
        let earlier = snapshot(true);
        let mut report = MediaSnapshot { position_ms: 60_000, sampled_at: 9_000, ..snapshot(true) };
        report.continue_from(&earlier, 9_000);
        assert_eq!(report.position_at(9_000), 60_000);
    }

    #[test]
    fn pausing_takes_the_position_the_player_gives() {
        let earlier = snapshot(true);
        let mut report = MediaSnapshot { position_ms: 17_000, sampled_at: 9_000, ..snapshot(false) };
        report.continue_from(&earlier, 9_000);
        assert_eq!(report.position_at(9_000), 17_000);
    }

    #[test]
    fn a_new_track_starts_from_its_own_position() {
        let earlier = snapshot(true);
        let mut report = MediaSnapshot {
            title: "Other".into(),
            position_ms: 0,
            sampled_at: 9_000,
            new_position: false,
            ..snapshot(true)
        };
        report.continue_from(&earlier, 9_000);
        assert_eq!(report.position_at(9_000), 0);
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
