//! Telling a song from a video by what came with it, not by which app or site
//! plays it.
//!
//! Browsers report everything as music to Windows, and pass on only what a
//! site gives them: a title, an artist, an album, a picture, and the buttons it
//! handles. Firefox drops the album too. So no single field decides; these
//! rules weigh them in order, and the first that fits wins.

/// How a picture is shaped, read from its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtShape {
    None,
    /// Album covers, which are square.
    Square,
    /// Video frames, which are wide, 4:3 included: YouTube's larger
    /// thumbnails hold a 16:9 frame between black bars.
    Wide,
    Other,
}

/// What a player says it is playing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reported {
    Music,
    Video,
    Unknown,
}

/// Everything the decision rests on.
#[derive(Clone, Copy, Debug)]
pub struct Signals<'a> {
    /// Only apps that say video mean it. Browsers say music for everything.
    pub reported: Reported,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: bool,
    /// A track number, a track count, a genre or an album artist, which only
    /// music apps fill in. Browsers never do.
    pub music_details: bool,
    pub shuffle_or_repeat: bool,
    /// Channel up and down, as on a TV app.
    pub channels: bool,
    /// Previous or next track.
    pub tracks: bool,
    /// No length and no seeking, as for a live stream.
    pub live: bool,
    pub duration_ms: u64,
    pub art: ArtShape,
}

/// Songs and episodes are rarely longer. Past this, a picture or a playlist
/// alone no longer makes something music.
const SONG_LENGTH_MS: u64 = 15 * 60_000;

/// Words that mark a song in a video's title, Turkish ones included. Live
/// radio streams use the last ones.
const SONG_WORDS: [&str; 18] = [
    "official video",
    "official music video",
    "official audio",
    "music video",
    "lyric video",
    "lyrics",
    "(audio)",
    "[audio]",
    "visualizer",
    "ft.",
    "feat.",
    "remix",
    "klip",
    "şarkı sözleri",
    "sözleri",
    "lofi",
    "lo-fi",
    "radio",
];

/// Whether the title or the artist names a song: song words in the title, the
/// artist's name before a dash in it, or a channel that YouTube makes for an
/// artist's songs.
pub fn names_a_song(title: &str, artist: &str) -> bool {
    let title = fold(title);
    let artist = fold(artist.trim());
    let words = SONG_WORDS.iter().any(|word| title.contains(&fold(word)));
    let channel = artist.ends_with(" - topic") || artist.contains("vevo");
    let artist_first = artist.chars().count() >= 2
        && [" - ", " – ", " — "]
            .iter()
            .any(|dash| title.starts_with(&format!("{artist}{dash}")));
    words || channel || artist_first
}

/// Lower case that matches across Turkish and English: `İ` turns into `i`
/// with a dot that is then dropped, and `ı` counts as `i`, so "BİR" and "BIR"
/// both match "bir".
fn fold(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| *c != '\u{307}')
        .map(|c| if c == 'ı' { 'i' } else { c })
        .collect()
}

/// Whether to treat what plays as a song.
pub fn is_music(signals: &Signals) -> bool {
    // An app that says video, or switches channels, means it.
    if signals.reported == Reported::Video || signals.channels {
        return false;
    }
    // What only music apps and song pages give, or a song named in the title.
    let song_fields = signals.music_details || signals.shuffle_or_repeat || signals.album;
    if song_fields || names_a_song(signals.title, signals.artist) {
        return true;
    }
    // A live stream that names no song is a broadcast.
    if signals.live {
        return false;
    }
    // Short and with an album's square cover, or a queue of short tracks
    // without video frames for pictures.
    let short = signals.duration_ms > 0 && signals.duration_ms <= SONG_LENGTH_MS;
    let queue = signals.tracks && signals.art != ArtShape::Wide;
    short && (signals.art == ArtShape::Square || queue)
}

/// The artist as it should read. YouTube names the channels it makes for an
/// artist's songs "Name - Topic".
pub fn display_artist(artist: &str) -> &str {
    let trimmed = artist.trim();
    match trimmed.len().checked_sub(" - Topic".len()) {
        Some(cut)
            if trimmed.is_char_boundary(cut) && trimmed[cut..].eq_ignore_ascii_case(" - topic") =>
        {
            trimmed[..cut].trim_end()
        }
        _ => trimmed,
    }
}

/// The shape of a PNG or JPEG from its header, without decoding it.
pub fn art_shape(encoded: &[u8]) -> ArtShape {
    let Some((width, height)) = image_size(encoded) else {
        return ArtShape::None;
    };
    if width == 0 || height == 0 {
        return ArtShape::None;
    }
    let ratio = width as f32 / height as f32;
    if (0.9..=1.12).contains(&ratio) {
        ArtShape::Square
    } else if ratio >= 1.3 {
        ArtShape::Wide
    } else {
        ArtShape::Other
    }
}

fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.starts_with(PNG) && bytes.get(12..16) == Some(b"IHDR") {
        let number = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
        return Some((number(16)?, number(20)?));
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return jpeg_size(bytes);
    }
    None
}

/// Walks the JPEG segments to the frame header, which holds the size.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    loop {
        while *bytes.get(at)? != 0xFF {
            at += 1;
        }
        while *bytes.get(at)? == 0xFF {
            at += 1;
        }
        let marker = *bytes.get(at)?;
        at += 1;
        // Markers without a length.
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        let length = u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?) as usize;
        let frame = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if frame {
            let height = u16::from_be_bytes(bytes.get(at + 3..at + 5)?.try_into().ok()?);
            let width = u16::from_be_bytes(bytes.get(at + 5..at + 7)?.try_into().ok()?);
            return Some((u32::from(width), u32::from(height)));
        }
        if length < 2 {
            return None;
        }
        at += length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: u64 = 60_000;

    /// A browser tab with nothing but a title, the way a site that tells
    /// Windows little shows up.
    fn browser(title: &'static str) -> Signals<'static> {
        Signals {
            reported: Reported::Music,
            title,
            artist: "",
            album: false,
            music_details: false,
            shuffle_or_repeat: false,
            channels: false,
            tracks: false,
            live: false,
            duration_ms: 8 * MINUTE,
            art: ArtShape::None,
        }
    }

    #[test]
    fn a_youtube_video_is_a_video() {
        // As measured: a channel as the artist, no album, a 16:9 frame, no tracks.
        let video = Signals {
            artist: "Bir Kanal",
            art: ArtShape::Wide,
            ..browser("Evde ekmek nasıl yapılır")
        };
        assert!(!is_music(&video));
    }

    #[test]
    fn youtube_music_is_music_in_every_browser() {
        // Chrome and Edge pass the album on.
        let chrome = Signals {
            artist: "Sanatçı",
            album: true,
            tracks: true,
            art: ArtShape::Square,
            duration_ms: 156_000,
            ..browser("Şarkı")
        };
        assert!(is_music(&chrome));
        // Firefox does not, which leaves the square cover and the queue.
        let firefox = Signals {
            album: false,
            ..chrome
        };
        assert!(is_music(&firefox));
    }

    #[test]
    fn a_music_video_on_youtube_is_music() {
        let marked = Signals {
            artist: "Tarkan",
            art: ArtShape::Wide,
            ..browser("Tarkan - Kuzu Kuzu (Official Video)")
        };
        assert!(is_music(&marked));
        let named = Signals {
            artist: "Sezen Aksu",
            art: ArtShape::Wide,
            ..browser("Sezen Aksu - Gidiyorum")
        };
        assert!(is_music(&named));
        let topic = Signals {
            artist: "Sezen Aksu - Topic",
            art: ArtShape::Wide,
            ..browser("Gidiyorum")
        };
        assert!(is_music(&topic));
        let klip = Signals {
            art: ArtShape::Wide,
            ..browser("Yeni Şarkı | Resmi Klip")
        };
        assert!(is_music(&klip));
    }

    #[test]
    fn a_live_stream_is_a_video_unless_it_is_radio() {
        let stream = Signals {
            artist: "Yayıncı",
            live: true,
            duration_ms: 0,
            art: ArtShape::Square,
            ..browser("Bugün oyun oynuyoruz")
        };
        assert!(!is_music(&stream));
        let radio = Signals {
            live: true,
            duration_ms: 0,
            art: ArtShape::Wide,
            ..browser("lofi hip hop radio - beats to sleep/chill to")
        };
        assert!(is_music(&radio));
    }

    #[test]
    fn a_long_episode_with_a_next_button_is_a_video() {
        let episode = Signals {
            tracks: true,
            art: ArtShape::Wide,
            duration_ms: 45 * MINUTE,
            ..browser("Bölüm 3")
        };
        assert!(!is_music(&episode));
        // A square logo for a picture does not make a film music.
        let film = Signals {
            art: ArtShape::Square,
            duration_ms: 110 * MINUTE,
            ..browser("Film")
        };
        assert!(!is_music(&film));
    }

    #[test]
    fn a_playlist_of_videos_stays_video() {
        let recipe = Signals {
            tracks: true,
            art: ArtShape::Wide,
            duration_ms: 9 * MINUTE,
            ..browser("Tarif 4: Ev ekmeği")
        };
        assert!(!is_music(&recipe));
    }

    #[test]
    fn music_apps_are_believed() {
        let player = Signals {
            reported: Reported::Music,
            music_details: true,
            duration_ms: 20 * MINUTE,
            ..browser("Uzun parça")
        };
        assert!(is_music(&player));
        let shuffling = Signals {
            shuffle_or_repeat: true,
            ..browser("Parça")
        };
        assert!(is_music(&shuffling));
    }

    #[test]
    fn an_app_that_says_video_is_believed() {
        let film = Signals {
            reported: Reported::Video,
            album: true,
            ..browser("Film")
        };
        assert!(!is_music(&film));
        let tv = Signals {
            channels: true,
            album: true,
            ..browser("Kanal 1")
        };
        assert!(!is_music(&tv));
    }

    #[test]
    fn a_page_that_says_nothing_is_a_video() {
        assert!(!is_music(&browser("Bir sayfa")));
    }

    #[test]
    fn ordinary_titles_are_not_songs() {
        assert!(!names_a_song("Evde ekmek nasıl yapılır", "Bir Kanal"));
        assert!(!names_a_song("Bir Kanal ile tanışın", "Bir Kanal"));
        assert!(names_a_song("BİR KANAL - Yeni Şarkı", "Bir Kanal"));
    }

    #[test]
    fn topic_channels_read_as_the_artist() {
        assert_eq!(display_artist("Sezen Aksu - Topic"), "Sezen Aksu");
        assert_eq!(display_artist("Sezen Aksu"), "Sezen Aksu");
        assert_eq!(display_artist("Ğ - topic"), "Ğ");
        assert_eq!(display_artist(""), "");
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    }

    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x4A, 0x46];
        bytes.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&[0x03, 0, 0, 0]);
        bytes
    }

    #[test]
    fn covers_are_measured_from_their_header() {
        assert_eq!(art_shape(&png(544, 544)), ArtShape::Square);
        assert_eq!(art_shape(&png(320, 180)), ArtShape::Wide);
        assert_eq!(art_shape(&jpeg(640, 640)), ArtShape::Square);
        // YouTube's 4:3 thumbnail holds a 16:9 frame between black bars.
        assert_eq!(art_shape(&jpeg(480, 360)), ArtShape::Wide);
        assert_eq!(art_shape(&jpeg(300, 400)), ArtShape::Other);
        assert_eq!(art_shape(&jpeg(1280, 720)), ArtShape::Wide);
        assert_eq!(art_shape(b"not a picture"), ArtShape::None);
        assert_eq!(art_shape(&[]), ArtShape::None);
        assert_eq!(art_shape(&png(0, 10)), ArtShape::None);
    }
}
