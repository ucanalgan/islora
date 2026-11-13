//! The shelf keeps files at hand to drag somewhere else later. It holds where
//! they are, never a copy, and forgets them when the app quits.

use std::sync::Arc;

use crate::drop::SHELF_TILES;
use crate::files::file_name;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShelfItem {
    pub path: String,
    pub name: String,
    /// A small encoded picture of the file, once Windows has made one.
    pub thumbnail: Option<Arc<[u8]>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shelf {
    items: Vec<ShelfItem>,
    /// The first file in view, when there are more than fit.
    first: usize,
}

/// What the shelf view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShelfView {
    pub items: Vec<ShelfItem>,
    pub first: usize,
    pub total: usize,
    /// Such as `Raf · 3 dosya`.
    pub title: String,
    /// Which files are in view, such as `6–7 / 7`, when not all fit.
    pub page: String,
}

impl Shelf {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[ShelfItem] {
        &self.items
    }

    pub fn first(&self) -> usize {
        self.first
    }

    /// Adds files not already there, and returns those it added. The view
    /// moves to the end, where they arrive.
    pub fn add(&mut self, paths: &[String]) -> Vec<String> {
        let mut added = Vec::new();
        for path in paths {
            let known = self
                .items
                .iter()
                .any(|item| item.path.eq_ignore_ascii_case(path));
            if !known
                && !added
                    .iter()
                    .any(|new: &String| new.eq_ignore_ascii_case(path))
            {
                added.push(path.clone());
                self.items.push(ShelfItem {
                    path: path.clone(),
                    name: file_name(path).to_owned(),
                    thumbnail: None,
                });
            }
        }
        self.first = self.items.len().saturating_sub(SHELF_TILES);
        added
    }

    pub fn set_thumbnail(&mut self, path: &str, thumbnail: Arc<[u8]>) {
        if let Some(item) = self.items.iter_mut().find(|item| item.path == path) {
            item.thumbnail = Some(thumbnail);
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            self.items.remove(index);
            self.keep_in_view();
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.first = 0;
    }

    /// Forgets files that were moved or deleted since.
    pub fn retain(&mut self, exists: impl Fn(&str) -> bool) {
        self.items.retain(|item| exists(&item.path));
        self.keep_in_view();
    }

    /// Moves the view by `pages` screens of files, forward or back.
    pub fn scroll(&mut self, pages: i32) {
        let step = SHELF_TILES as i64 * i64::from(pages);
        let last = self.items.len().saturating_sub(SHELF_TILES) as i64;
        self.first = (self.first as i64 + step).clamp(0, last) as usize;
    }

    fn keep_in_view(&mut self) {
        self.first = self.first.min(self.items.len().saturating_sub(SHELF_TILES));
    }

    pub fn view(&self) -> ShelfView {
        let total = self.items.len();
        let end = (self.first + SHELF_TILES).min(total);
        ShelfView {
            items: self.items[self.first..end].to_vec(),
            first: self.first,
            total,
            title: format!("Raf · {total} dosya"),
            page: if total > SHELF_TILES {
                format!("{}–{end} / {total}", self.first + 1)
            } else {
                String::new()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| format!(r"C:\a\{name}")).collect()
    }

    #[test]
    fn a_file_is_kept_once() {
        let mut shelf = Shelf::default();
        assert_eq!(shelf.add(&paths(&["a.txt", "b.jpg"])).len(), 2);
        assert_eq!(
            shelf.add(&[r"C:\A\A.TXT".to_owned(), r"C:\a\c.pdf".to_owned()]),
            vec![r"C:\a\c.pdf".to_owned()]
        );
        assert_eq!(shelf.len(), 3);
        assert_eq!(shelf.items()[2].name, "c.pdf");
    }

    #[test]
    fn new_files_come_into_view() {
        let mut shelf = Shelf::default();
        shelf.add(&paths(&["1", "2", "3", "4", "5", "6", "7"]));
        let view = shelf.view();
        assert_eq!((view.first, view.total, view.items.len()), (2, 7, 5));
        assert_eq!(view.items[4].name, "7");
        assert_eq!(
            (view.title.as_str(), view.page.as_str()),
            ("Raf · 7 dosya", "3–7 / 7")
        );
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        let mut shelf = Shelf::default();
        shelf.add(&paths(&["1", "2", "3", "4", "5", "6", "7"]));
        shelf.scroll(-1);
        assert_eq!(shelf.first(), 0);
        shelf.scroll(-1);
        assert_eq!(shelf.first(), 0);
        shelf.scroll(1);
        assert_eq!(shelf.first(), 2);
    }

    #[test]
    fn removing_keeps_the_view_full() {
        let mut shelf = Shelf::default();
        shelf.add(&paths(&["1", "2", "3", "4", "5", "6"]));
        assert_eq!(shelf.first(), 1);
        shelf.remove(5);
        assert_eq!(shelf.first(), 0);
        assert_eq!(shelf.view().items.len(), 5);
        shelf.remove(99);
        assert_eq!(shelf.len(), 5);
    }

    #[test]
    fn moved_files_are_forgotten() {
        let mut shelf = Shelf::default();
        shelf.add(&paths(&["kalan", "giden"]));
        shelf.retain(|path| !path.ends_with("giden"));
        assert_eq!(shelf.len(), 1);
        assert_eq!(shelf.items()[0].name, "kalan");
    }

    #[test]
    fn a_thumbnail_finds_its_file() {
        let mut shelf = Shelf::default();
        shelf.add(&paths(&["a.png"]));
        shelf.set_thumbnail(r"C:\a\a.png", Arc::from(vec![1_u8, 2, 3]));
        assert!(shelf.items()[0].thumbnail.is_some());
    }
}
