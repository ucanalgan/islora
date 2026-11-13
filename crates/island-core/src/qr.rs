//! The square code a phone camera reads to open the download page.

use qrcodegen::{QrCode, QrCodeEcc};

/// Light modules around the code, which cameras need to find its edge. The
/// standard asks for four; two is what most code generators use on screens.
pub const QUIET_ZONE: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrMatrix {
    size: usize,
    dark: Vec<bool>,
}

impl QrMatrix {
    /// The smallest code that holds `text`. Low correction keeps it small,
    /// and the library raises it whenever that costs no extra size.
    pub fn encode(text: &str) -> Option<Self> {
        let code = QrCode::encode_text(text, QrCodeEcc::Low).ok()?;
        let size = code.size() as usize;
        let dark = (0..size * size)
            .map(|index| code.get_module((index % size) as i32, (index / size) as i32))
            .collect();
        Some(Self { size, dark })
    }

    /// Modules on each side, without the quiet zone.
    pub fn size(&self) -> usize {
        self.size
    }

    pub fn is_dark(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.dark[y * self.size + x]
    }

    /// Dark modules of one row joined into runs, as (first column, length),
    /// so a row draws as a few rectangles instead of one per module.
    pub fn runs(&self, y: usize) -> Vec<(usize, usize)> {
        let mut runs = Vec::new();
        let mut x = 0;
        while x < self.size {
            if self.is_dark(x, y) {
                let start = x;
                while x < self.size && self.is_dark(x, y) {
                    x += 1;
                }
                runs.push((start, x - start));
            } else {
                x += 1;
            }
        }
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_fits_a_small_code() {
        let code =
            QrMatrix::encode("http://192.168.100.200:65535/abcdefghijklmnopqrst").expect("fits");
        // Version 3 is 29 modules a side, small enough to read from a laptop screen.
        assert!(code.size() <= 29, "{}", code.size());
        // Every code starts with a finder pattern in its top left corner.
        assert!(
            code.is_dark(0, 0) && code.is_dark(6, 0) && !code.is_dark(1, 1) && code.is_dark(2, 2)
        );
    }

    #[test]
    fn runs_cover_exactly_the_dark_modules() {
        let code = QrMatrix::encode("http://10.0.0.2:5000/x").expect("fits");
        for y in 0..code.size() {
            let mut from_runs = vec![false; code.size()];
            for (start, length) in code.runs(y) {
                from_runs[start..start + length].fill(true);
            }
            let direct: Vec<bool> = (0..code.size()).map(|x| code.is_dark(x, y)).collect();
            assert_eq!(from_runs, direct, "row {y}");
        }
    }
}
