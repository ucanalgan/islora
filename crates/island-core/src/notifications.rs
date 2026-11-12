//! Small, private queue of Windows notifications waiting for the island.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use crate::{Activity, Icon, Kind, Millis};

const MAX_PENDING: usize = 12;
const SHOW_MS: Millis = 6_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub id: u32,
    pub app_id: String,
    pub app: String,
    pub title: String,
    pub body: String,
    pub logo: Option<Arc<[u8]>>,
}

impl Notice {
    pub fn activity(&self, now: Millis) -> Activity {
        Activity {
            title: if self.title.is_empty() {
                self.app.clone()
            } else {
                format!("{} · {}", self.app, self.title)
            },
            detail: self.body.clone(),
            icon: Some(Icon::Message),
            preview: self.logo.clone(),
            expires_at: Some(now + SHOW_MS),
            ..Activity::new(Kind::ExternalNotification)
        }
    }
}

#[derive(Default)]
pub struct NotificationQueue {
    pending: VecDeque<Notice>,
    known: HashSet<u32>,
    muted: HashSet<String>,
    focus: bool,
}

impl NotificationQueue {
    pub fn set_muted(&mut self, apps: impl IntoIterator<Item = String>) {
        self.muted = apps.into_iter().collect();
        self.pending
            .retain(|notice| !self.muted.contains(&notice.app_id));
    }

    pub fn set_focus(&mut self, focus: bool) {
        self.focus = focus;
        if focus {
            self.pending.clear();
        }
    }

    /// Old items populate `known` at startup, without replaying the entire
    /// Windows notification center as new alerts.
    pub fn seed(&mut self, ids: impl IntoIterator<Item = u32>) {
        self.known.extend(ids);
    }

    pub fn add(&mut self, notice: Notice) {
        if !self.known.insert(notice.id) || self.focus || self.muted.contains(&notice.app_id) {
            return;
        }
        if self.pending.len() == MAX_PENDING {
            self.pending.pop_front();
        }
        self.pending.push_back(notice);
    }

    pub fn remove(&mut self, id: u32) {
        self.known.remove(&id);
        self.pending.retain(|notice| notice.id != id);
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.known.clear();
    }

    pub fn pop(&mut self) -> Option<Notice> {
        self.pending.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(id: u32, app: &str) -> Notice {
        Notice {
            id,
            app_id: app.into(),
            app: app.into(),
            title: "title".into(),
            body: "body".into(),
            logo: None,
        }
    }

    #[test]
    fn old_notifications_are_not_replayed_and_new_ones_are_ordered() {
        let mut queue = NotificationQueue::default();
        queue.seed([1]);
        queue.add(notice(1, "chat"));
        queue.add(notice(2, "chat"));
        queue.add(notice(3, "mail"));
        queue.add(notice(2, "chat"));
        assert_eq!(queue.pop().unwrap().id, 2);
        assert_eq!(queue.pop().unwrap().id, 3);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn focus_and_mute_clear_pending_without_replaying_it_later() {
        let mut queue = NotificationQueue::default();
        queue.add(notice(1, "chat"));
        queue.set_focus(true);
        queue.add(notice(2, "mail"));
        queue.set_focus(false);
        queue.add(notice(2, "mail"));
        queue.set_muted(["chat".into()]);
        queue.add(notice(3, "chat"));
        queue.add(notice(4, "mail"));
        assert_eq!(queue.pop().unwrap().id, 4);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn removal_forgets_a_pending_notification() {
        let mut queue = NotificationQueue::default();
        queue.add(notice(5, "chat"));
        queue.remove(5);
        assert!(queue.pop().is_none());
        queue.add(notice(5, "chat"));
        assert_eq!(queue.pop().unwrap().id, 5);
    }
}
