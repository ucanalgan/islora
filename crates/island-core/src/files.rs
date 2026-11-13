//! Files dropped on the island: what they are called, how big they are, and
//! whether they are pictures.

/// Picture formats Windows can read, some of them only with a Store extension.
const IMAGE_EXTENSIONS: [&str; 15] = [
    "jpg", "jpeg", "jpe", "jfif", "png", "bmp", "gif", "tif", "tiff", "webp", "heic", "heif",
    "avif", "jxr", "ico",
];

/// One dropped file, as far as the island needs to know it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedFile {
    pub path: String,
    pub size: u64,
    pub is_folder: bool,
}

impl DroppedFile {
    pub fn name(&self) -> &str {
        file_name(&self.path)
    }

    pub fn is_image(&self) -> bool {
        !self.is_folder && is_image(&self.path)
    }
}

/// The last part of a path, with either kind of slash.
pub fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['\\', '/']);
    trimmed.rsplit(['\\', '/']).next().unwrap_or(trimmed)
}

/// What follows the last dot of the name, in lower case, or nothing.
pub fn extension(path: &str) -> String {
    let name = file_name(path);
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => extension.to_lowercase(),
        _ => String::new(),
    }
}

pub fn is_image(path: &str) -> bool {
    IMAGE_EXTENSIONS.contains(&extension(path).as_str())
}

/// A size as Windows shows it, in steps of 1024 with a Turkish decimal comma.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} bayt");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let text = if value < 10.0 {
        let tenths = (value * 10.0).round() / 10.0;
        if tenths >= 10.0 {
            "10".to_owned()
        } else {
            format!("{tenths:.1}").replace('.', ",")
        }
    } else {
        format!("{}", value.round() as u64)
    };
    format!("{text} {}", UNITS[unit])
}

/// One line about what is being carried: the name and size of a single file,
/// or how many there are and their size together.
pub fn summary(files: &[DroppedFile]) -> String {
    let total: u64 = files.iter().map(|file| file.size).sum();
    match files {
        [] => String::new(),
        [only] if only.is_folder => only.name().to_owned(),
        [only] => format!("{} · {}", only.name(), format_size(only.size)),
        several => format!("{} dosya · {}", several.len(), format_size(total)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64) -> DroppedFile {
        DroppedFile {
            path: path.to_owned(),
            size,
            is_folder: false,
        }
    }

    #[test]
    fn names_come_from_the_end_of_the_path() {
        assert_eq!(file_name(r"C:\Users\a\Resimler\tatil.jpg"), "tatil.jpg");
        assert_eq!(file_name("C:/a/b.txt"), "b.txt");
        assert_eq!(file_name(r"C:\a\klasör\"), "klasör");
        assert_eq!(file_name("tek"), "tek");
    }

    #[test]
    fn pictures_are_known_by_their_extension() {
        assert!(is_image(r"C:\a\Foto.JPG"));
        assert!(is_image(r"C:\a\ekran.png"));
        assert!(is_image(r"C:\a\IMG_0001.HEIC"));
        assert!(!is_image(r"C:\a\not.pdf"));
        assert!(!is_image(r"C:\a\.png"));
        assert!(!is_image(r"C:\a\png"));
        assert_eq!(extension(r"C:\a\arşiv.tar.GZ"), "gz");
    }

    #[test]
    fn sizes_read_like_explorer() {
        assert_eq!(format_size(0), "0 bayt");
        assert_eq!(format_size(1023), "1023 bayt");
        assert_eq!(format_size(1024), "1,0 KB");
        assert_eq!(format_size(2_516_582), "2,4 MB");
        assert_eq!(format_size(340 * 1024), "340 KB");
        assert_eq!(format_size(10_485_000), "10 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3,0 GB");
    }

    #[test]
    fn a_summary_names_one_file_and_counts_several() {
        assert_eq!(
            summary(&[file(r"C:\a\tatil.jpg", 2_516_582)]),
            "tatil.jpg · 2,4 MB"
        );
        let three = [
            file("a.jpg", 1024 * 1024),
            file("b.jpg", 1024 * 1024),
            file("c.pdf", 1024 * 1024),
        ];
        assert_eq!(summary(&three), "3 dosya · 3,0 MB");
        let folder = DroppedFile {
            path: r"C:\a\Belgeler".into(),
            size: 0,
            is_folder: true,
        };
        assert_eq!(summary(&[folder]), "Belgeler");
        assert_eq!(summary(&[]), "");
    }
}
