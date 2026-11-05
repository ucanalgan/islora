//! The panel that opens when files are dragged to the island, and the views
//! that follow a drop. Positions are in DIPs inside the expanded pill.

use crate::convert::Target;
use crate::files::{DroppedFile, summary};
use crate::layout::Pill;
pub use crate::settings_view::Rect;

const MARGIN: f32 = 18.0;
const GAP: f32 = 10.0;
const COLUMN: f32 = (Pill::EXPANDED.width - MARGIN * 2.0 - GAP * 2.0) / 3.0;

/// The line on top that says what is being carried.
pub const HEADER: Rect = Rect { x: 24.0, y: 12.0, width: Pill::EXPANDED.width - 48.0, height: 22.0 };

const ZONE_TOP: f32 = 40.0;
const ZONE_HEIGHT: f32 = 114.0;

/// Where a drop can go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// Keeps the files at hand, to drag somewhere else later.
    Shelf,
    /// Hands the files to a phone through a code it scans.
    Phone,
    /// Saves pictures in another format beside the original.
    Convert,
}

impl Zone {
    pub const ALL: [Zone; 3] = [Zone::Shelf, Zone::Phone, Zone::Convert];

    pub fn label(self) -> &'static str {
        match self {
            Zone::Shelf => "Raf",
            Zone::Phone => "Telefona gönder",
            Zone::Convert => "Dönüştür",
        }
    }

    fn column(self) -> usize {
        match self {
            Zone::Shelf => 0,
            Zone::Phone => 1,
            Zone::Convert => 2,
        }
    }

    pub fn bounds(self) -> Rect {
        let x = MARGIN + self.column() as f32 * (COLUMN + GAP);
        Rect { x, y: ZONE_TOP, width: COLUMN, height: ZONE_HEIGHT }
    }

    pub fn at(x: f32, y: f32) -> Option<Zone> {
        Zone::ALL.into_iter().find(|zone| zone.bounds().contains(x, y))
    }
}

/// What the panel shows while files are dragged over it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropView {
    pub header: String,
    /// The zone under the pointer, which lights up.
    pub hovered: Option<Zone>,
    /// Why a zone cannot take these files, if it cannot.
    pub phone_refusal: Option<&'static str>,
    pub convert_refusal: Option<&'static str>,
}

impl DropView {
    /// The panel for these files, with the zones that cannot take them marked.
    pub fn for_files(files: &[DroppedFile]) -> Self {
        let only_folders = !files.is_empty() && files.iter().all(|file| file.is_folder);
        let any_image = files.iter().any(DroppedFile::is_image);
        DropView {
            header: summary(files),
            hovered: None,
            phone_refusal: only_folders.then_some("Klasörler gönderilemez"),
            convert_refusal: (!any_image).then_some("Yalnızca görseller"),
        }
    }

    pub fn refusal(&self, zone: Zone) -> Option<&'static str> {
        match zone {
            Zone::Shelf => None,
            Zone::Phone => self.phone_refusal,
            Zone::Convert => self.convert_refusal,
        }
    }

    /// The zone a drop at this point would go to, if it takes the files.
    pub fn accepting(&self, x: f32, y: f32) -> Option<Zone> {
        Zone::at(x, y).filter(|zone| self.refusal(*zone).is_none())
    }
}

/// How many files the shelf shows at once.
pub const SHELF_TILES: usize = 5;
const TILE_WIDTH: f32 = 68.0;
const TILE_GAP: f32 = 6.0;
const TILE_LEFT: f32 = (Pill::EXPANDED.width - TILE_WIDTH * SHELF_TILES as f32 - TILE_GAP * (SHELF_TILES - 1) as f32) / 2.0;
pub const THUMBNAIL: f32 = 56.0;
const THUMBNAIL_TOP: f32 = 44.0;
const REMOVE_SIZE: f32 = 20.0;

pub const SHELF_CLEAR: Rect = Rect { x: 296.0, y: 10.0, width: 80.0, height: 26.0 };
pub const SHELF_BACK: Rect = Rect { x: 18.0, y: 128.0, width: 32.0, height: 28.0 };
pub const SHELF_FORWARD: Rect = Rect { x: Pill::EXPANDED.width - 50.0, y: 128.0, width: 32.0, height: 28.0 };

/// The place of a file on the shelf, counted from the first one in view.
pub fn tile(slot: usize) -> Rect {
    let x = TILE_LEFT + slot as f32 * (TILE_WIDTH + TILE_GAP);
    Rect { x, y: THUMBNAIL_TOP, width: TILE_WIDTH, height: 82.0 }
}

pub fn thumbnail(slot: usize) -> Rect {
    let tile = tile(slot);
    Rect { x: tile.x + (TILE_WIDTH - THUMBNAIL) / 2.0, y: THUMBNAIL_TOP, width: THUMBNAIL, height: THUMBNAIL }
}

/// The small cross on a file's corner that takes it off the shelf.
pub fn remove_button(slot: usize) -> Rect {
    let picture = thumbnail(slot);
    let x = picture.x + picture.width - REMOVE_SIZE + 6.0;
    Rect { x, y: picture.y - 6.0, width: REMOVE_SIZE, height: REMOVE_SIZE }
}

pub fn tile_name(slot: usize) -> Rect {
    let tile = tile(slot);
    Rect { x: tile.x, y: THUMBNAIL_TOP + THUMBNAIL + 4.0, width: TILE_WIDTH, height: 18.0 }
}

/// What a click on the shelf view lands on. Files are counted from the first
/// one on the shelf, not from the first one in view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShelfHit {
    File(usize),
    Remove(usize),
    Clear,
    Back,
    Forward,
}

pub fn shelf_hit(x: f32, y: f32, first: usize, count: usize) -> Option<ShelfHit> {
    if SHELF_CLEAR.contains(x, y) {
        return Some(ShelfHit::Clear);
    }
    if count > SHELF_TILES {
        if SHELF_BACK.contains(x, y) && first > 0 {
            return Some(ShelfHit::Back);
        }
        if SHELF_FORWARD.contains(x, y) && first + SHELF_TILES < count {
            return Some(ShelfHit::Forward);
        }
    }
    let shown = count.saturating_sub(first).min(SHELF_TILES);
    // The cross overlaps the picture, so it is checked first.
    if let Some(slot) = (0..shown).find(|&slot| remove_button(slot).contains(x, y)) {
        return Some(ShelfHit::Remove(first + slot));
    }
    (0..shown).find(|&slot| tile(slot).contains(x, y)).map(|slot| ShelfHit::File(first + slot))
}

const TARGET_TOP: f32 = 42.0;
const TARGET_HEIGHT: f32 = 84.0;

impl Target {
    pub fn bounds(self) -> Rect {
        let column = Target::ALL.iter().position(|target| *target == self).unwrap_or(0);
        let x = MARGIN + column as f32 * (COLUMN + GAP);
        Rect { x, y: TARGET_TOP, width: COLUMN, height: TARGET_HEIGHT }
    }

    pub fn at(x: f32, y: f32) -> Option<Target> {
        Target::ALL.into_iter().find(|target| target.bounds().contains(x, y))
    }
}

/// The note under the choices, or what is happening once one was made.
pub const CONVERT_NOTE: Rect = Rect { x: 24.0, y: 134.0, width: Pill::EXPANDED.width - 48.0, height: 20.0 };

/// The code for the phone, or what replaces it once the phone is on its way.
pub const TRANSFER_CODE: Rect = Rect { x: 20.0, y: 17.0, width: 136.0, height: 136.0 };
pub const TRANSFER_TEXT_LEFT: f32 = 174.0;
pub const TRANSFER_STOP: Rect = Rect { x: TRANSFER_TEXT_LEFT, y: 120.0, width: 90.0, height: 32.0 };

pub fn on_transfer_stop(x: f32, y: f32) -> bool {
    TRANSFER_STOP.contains(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside_pill(rect: Rect) -> bool {
        rect.x >= 0.0
            && rect.y >= 0.0
            && rect.x + rect.width <= Pill::EXPANDED.width
            && rect.y + rect.height <= Pill::EXPANDED.height
    }

    #[test]
    fn the_zones_sit_side_by_side_inside_the_pill() {
        for zone in Zone::ALL {
            let bounds = zone.bounds();
            assert!(inside_pill(bounds), "{zone:?}");
            let middle = (bounds.x + bounds.width / 2.0, bounds.y + bounds.height / 2.0);
            assert_eq!(Zone::at(middle.0, middle.1), Some(zone));
        }
        assert_eq!(Zone::at(Zone::Shelf.bounds().x + COLUMN + GAP / 2.0, 80.0), None);
        assert_eq!(Zone::at(200.0, HEADER.y + 4.0), None);
    }

    #[test]
    fn a_refused_zone_takes_no_drop() {
        let view = DropView {
            header: "not.pdf · 1 KB".into(),
            hovered: None,
            phone_refusal: None,
            convert_refusal: Some("Yalnızca görseller"),
        };
        let convert = Zone::Convert.bounds();
        assert_eq!(view.accepting(convert.x + 10.0, convert.y + 10.0), None);
        let phone = Zone::Phone.bounds();
        assert_eq!(view.accepting(phone.x + 10.0, phone.y + 10.0), Some(Zone::Phone));
    }

    #[test]
    fn zones_refuse_what_they_cannot_use() {
        let file = |path: &str, is_folder| DroppedFile { path: path.into(), size: 10, is_folder };
        let pictures = DropView::for_files(&[file("a.jpg", false), file("b.pdf", false)]);
        assert_eq!((pictures.phone_refusal, pictures.convert_refusal), (None, None));
        let papers = DropView::for_files(&[file("b.pdf", false)]);
        assert_eq!(papers.convert_refusal, Some("Yalnızca görseller"));
        let folders = DropView::for_files(&[file(r"C:\a\Klasör", true)]);
        assert_eq!(folders.phone_refusal, Some("Klasörler gönderilemez"));
        assert_eq!(folders.refusal(Zone::Shelf), None);
    }

    #[test]
    fn the_shelf_counts_clicks_from_its_first_file() {
        let picture = thumbnail(1);
        let middle = (picture.x + picture.width / 2.0, picture.y + picture.height / 2.0);
        assert_eq!(shelf_hit(middle.0, middle.1, 0, 3), Some(ShelfHit::File(1)));
        assert_eq!(shelf_hit(middle.0, middle.1, 4, 7), Some(ShelfHit::File(5)));
        let cross = remove_button(1);
        assert_eq!(shelf_hit(cross.x + 10.0, cross.y + 10.0, 0, 3), Some(ShelfHit::Remove(1)));
        // Nothing is there past the last file.
        let empty = thumbnail(4);
        assert_eq!(shelf_hit(empty.x + 5.0, empty.y + 5.0, 0, 3), None);
        assert_eq!(shelf_hit(SHELF_CLEAR.x + 5.0, SHELF_CLEAR.y + 5.0, 0, 3), Some(ShelfHit::Clear));
    }

    #[test]
    fn the_shelf_pages_only_where_there_is_more() {
        let back = (SHELF_BACK.x + 5.0, SHELF_BACK.y + 5.0);
        let forward = (SHELF_FORWARD.x + 5.0, SHELF_FORWARD.y + 5.0);
        assert_eq!(shelf_hit(forward.0, forward.1, 0, 4), None);
        assert_eq!(shelf_hit(forward.0, forward.1, 0, 7), Some(ShelfHit::Forward));
        assert_eq!(shelf_hit(back.0, back.1, 0, 7), None);
        assert_eq!(shelf_hit(back.0, back.1, 2, 7), Some(ShelfHit::Back));
        assert_eq!(shelf_hit(forward.0, forward.1, 2, 7), None);
    }

    #[test]
    fn every_part_of_the_views_fits_in_the_pill() {
        for slot in 0..SHELF_TILES {
            assert!(inside_pill(tile(slot)), "tile {slot}");
            assert!(inside_pill(remove_button(slot)), "cross {slot}");
        }
        for target in Target::ALL {
            assert!(inside_pill(target.bounds()), "{target:?}");
        }
        for rect in [SHELF_CLEAR, SHELF_BACK, SHELF_FORWARD, CONVERT_NOTE, TRANSFER_CODE, TRANSFER_STOP, HEADER] {
            assert!(inside_pill(rect), "{rect:?}");
        }
    }
}
