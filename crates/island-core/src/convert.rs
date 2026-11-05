//! Saving pictures in another format: what the choices are, and what the new
//! file is called. The new file goes beside the original, which stays as it was.

use crate::files::{extension, file_name};

/// The longest side of a picture made smaller, in pixels.
pub const SMALLER_SIDE: u32 = 1600;
pub const SMALLER_QUALITY: f32 = 0.82;
pub const JPG_QUALITY: f32 = 0.92;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Jpg,
    Png,
    /// A JPG no larger than [`SMALLER_SIDE`] on its longest side.
    Smaller,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::Jpg, Target::Png, Target::Smaller];

    pub fn label(self) -> &'static str {
        match self {
            Target::Jpg => "JPG",
            Target::Png => "PNG",
            Target::Smaller => "Küçült",
        }
    }

    /// A word on what the choice is good for.
    pub fn hint(self) -> &'static str {
        match self {
            Target::Jpg => "Her yerde açılır",
            Target::Png => "Kayıpsız, saydam",
            Target::Smaller => "Paylaşmak için",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Target::Jpg | Target::Smaller => "jpg",
            Target::Png => "png",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Target::Smaller => " (küçük)",
            _ => "",
        }
    }
}

/// Where the converted copy of `source` goes. A number is added rather than
/// replacing anything, the original included.
pub fn output_path(source: &str, target: Target, exists: impl Fn(&str) -> bool) -> String {
    let name = file_name(source);
    let folder = &source[..source.len() - name.len()];
    let old_extension = extension(source);
    let stem = if old_extension.is_empty() { name } else { &name[..name.len() - old_extension.len() - 1] };
    let base = format!("{folder}{stem}{}", target.suffix());
    let taken = |candidate: &str| candidate.eq_ignore_ascii_case(source) || exists(candidate);

    let first = format!("{base}.{}", target.extension());
    if !taken(&first) {
        return first;
    }
    (2..)
        .map(|number| format!("{base} ({number}).{}", target.extension()))
        .find(|candidate| !taken(candidate))
        .unwrap_or(first)
}

/// The size of a picture made smaller, keeping its shape. Pictures already
/// small enough keep their size.
pub fn smaller_size(width: u32, height: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= SMALLER_SIDE || longest == 0 {
        return (width, height);
    }
    let scale = SMALLER_SIDE as f64 / longest as f64;
    let shrink = |side: u32| ((side as f64 * scale).round() as u32).max(1);
    (shrink(width), shrink(height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nothing_there(_: &str) -> bool {
        false
    }

    #[test]
    fn the_copy_goes_beside_the_original() {
        assert_eq!(output_path(r"C:\Foto\tatil.heic", Target::Jpg, nothing_there), r"C:\Foto\tatil.jpg");
        assert_eq!(output_path(r"C:\Foto\tatil.heic", Target::Png, nothing_there), r"C:\Foto\tatil.png");
        assert_eq!(
            output_path(r"C:\Foto\tatil.heic", Target::Smaller, nothing_there),
            r"C:\Foto\tatil (küçük).jpg"
        );
    }

    #[test]
    fn nothing_is_ever_replaced() {
        // The original itself, whatever the case of its name.
        assert_eq!(output_path(r"C:\Foto\ekran.JPG", Target::Jpg, nothing_there), r"C:\Foto\ekran (2).jpg");
        let taken = |path: &str| path == r"C:\Foto\a.png" || path == r"C:\Foto\a (2).png";
        assert_eq!(output_path(r"C:\Foto\a.bmp", Target::Png, taken), r"C:\Foto\a (3).png");
    }

    #[test]
    fn dots_inside_a_name_stay() {
        assert_eq!(output_path(r"C:\a\v1.2 son.webp", Target::Png, nothing_there), r"C:\a\v1.2 son.png");
        assert_eq!(output_path(r"C:\a\adsız", Target::Jpg, nothing_there), r"C:\a\adsız.jpg");
    }

    #[test]
    fn smaller_keeps_the_shape() {
        assert_eq!(smaller_size(4032, 3024), (1600, 1200));
        assert_eq!(smaller_size(3024, 4032), (1200, 1600));
        assert_eq!(smaller_size(800, 600), (800, 600));
        assert_eq!(smaller_size(16000, 10), (1600, 1));
    }
}
