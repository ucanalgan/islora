//! Sending files to a phone: the code waits to be scanned, the files go, and
//! the island says so. The server itself lives in the app.

use std::sync::Arc;

use crate::activity::{Activity, Bubble, Icon, Kind, Millis};
use crate::files::format_size;
use crate::layout::ProgressStyle;
use crate::qr::QrMatrix;

/// How long the code waits to be scanned, and waits again after a download
/// that broke off.
pub const WAIT_MS: Millis = 5 * 60_000;
/// How long "sent" stays before the island closes.
pub const DONE_MS: Millis = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Waiting { until: Millis },
    Sending { sent: u64 },
    Done { at: Millis },
}

/// The island's own network, for a phone that is not on the computer's.
#[derive(Clone, Debug, PartialEq)]
pub enum Direct {
    /// Windows is starting the network.
    Preparing,
    /// A code to join the network, shown until the phone is on it.
    Ready { code: Arc<QrMatrix>, network: String },
    /// The phone is on the network, so the download code shows.
    Joined { network: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transfer {
    /// The file's name, or how many there are, for the compact pill.
    pub name: String,
    /// Name and size, for the open view.
    pub summary: String,
    pub total: u64,
    pub code: Arc<QrMatrix>,
    /// Which network the phone has to be on, such as `Wi-Fi · Ev`.
    pub network: String,
    /// Why the phone might not get through, such as a public network.
    pub warning: Option<String>,
    pub stage: Stage,
    pub direct: Option<Direct>,
}

/// What the open view shows at this stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    /// The code, with the time it has left, such as `4:12`.
    Waiting { left: String },
    Sending { permille: u16, amount: String },
    Done,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransferView {
    pub code: Arc<QrMatrix>,
    pub summary: String,
    pub network: String,
    pub warning: Option<String>,
    pub shown: Shown,
    pub style: ProgressStyle,
    pub direct: Option<Direct>,
}

/// Minutes and seconds, rounded up so it never shows 0:00 while still open.
fn minutes_and_seconds(ms: Millis) -> String {
    let seconds = ms.div_ceil(1000);
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

impl Transfer {
    pub fn permille(&self) -> u16 {
        match self.stage {
            Stage::Sending { sent } if self.total > 0 => ((sent.min(self.total) as f64 / self.total as f64) * 1000.0) as u16,
            Stage::Done { .. } => 1000,
            _ => 0,
        }
    }

    pub fn activity(&self, now: Millis, style: ProgressStyle) -> Activity {
        let shown = match self.stage {
            Stage::Waiting { until } => Shown::Waiting { left: minutes_and_seconds(until.saturating_sub(now)) },
            Stage::Sending { sent } => Shown::Sending {
                permille: self.permille(),
                amount: format!("{} / {}", format_size(sent.min(self.total)), format_size(self.total)),
            },
            Stage::Done { .. } => Shown::Done,
        };
        let view = TransferView {
            code: self.code.clone(),
            summary: self.summary.clone(),
            network: self.network.clone(),
            warning: self.warning.clone(),
            shown: shown.clone(),
            style,
            direct: self.direct.clone(),
        };
        let percent = (self.permille() / 10) as u8;
        let (leading, trailing, icon, badge, expires_at) = match shown {
            Shown::Waiting { .. } => (self.name.clone(), "QR".to_owned(), Icon::Phone, Bubble::Symbol(Icon::Phone), None),
            Shown::Sending { .. } => (
                self.name.clone(),
                format!("%{percent}"),
                Icon::Phone,
                Bubble::Count { value: percent.to_string(), icon: None, ring: Some(percent) },
                None,
            ),
            Shown::Done => {
                let done_at = match self.stage {
                    Stage::Done { at } => at,
                    _ => now,
                };
                ("Gönderildi".to_owned(), String::new(), Icon::Check, Bubble::Symbol(Icon::Check), Some(done_at + DONE_MS))
            }
        };
        Activity {
            leading,
            trailing,
            title: "Telefona gönder".into(),
            detail: self.summary.clone(),
            icon: Some(icon),
            badge: Some(badge),
            expires_at,
            transfer: Some(view),
            ..Activity::new(Kind::Transfer)
        }
    }

    /// When the screen has to change next without anything else happening.
    pub fn next_moment(&self, now: Millis, counting_down: bool) -> Option<Millis> {
        match self.stage {
            Stage::Waiting { until } if counting_down => {
                let into_second = until.saturating_sub(now) % 1000;
                Some(now + if into_second == 0 { 1000 } else { into_second }).map(|next| next.min(until))
            }
            Stage::Waiting { until } => Some(until),
            Stage::Done { at } => Some(at + DONE_MS),
            Stage::Sending { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn transfer(stage: Stage) -> Transfer {
        Transfer {
            name: "tatil.jpg".into(),
            summary: "tatil.jpg · 2,0 MB".into(),
            total: 2 * 1024 * 1024,
            code: Arc::new(QrMatrix::encode("http://10.0.0.2:5000/x").expect("fits")),
            network: "Wi-Fi · Ev".into(),
            warning: None,
            stage,
            direct: None,
        }
    }

    #[test]
    fn waiting_shows_the_time_left() {
        let activity = transfer(Stage::Waiting { until: WAIT_MS }).activity(1_500, ProgressStyle::Ring);
        assert_eq!(activity.kind, Kind::Transfer);
        assert_eq!((activity.leading.as_str(), activity.trailing.as_str()), ("tatil.jpg", "QR"));
        let view = activity.transfer.expect("view");
        assert_eq!(view.shown, Shown::Waiting { left: "4:59".into() });
    }

    #[test]
    fn sending_shows_how_far_it_got() {
        let activity = transfer(Stage::Sending { sent: 1024 * 1024 }).activity(0, ProgressStyle::Edge);
        assert_eq!(activity.trailing, "%50");
        assert_eq!(activity.badge, Some(Bubble::Count { value: "50".into(), icon: None, ring: Some(50) }));
        let view = activity.transfer.expect("view");
        assert_eq!(view.shown, Shown::Sending { permille: 500, amount: "1,0 MB / 2,0 MB".into() });
        assert_eq!(view.style, ProgressStyle::Edge);
    }

    #[test]
    fn done_says_so_then_leaves() {
        let activity = transfer(Stage::Done { at: 10_000 }).activity(10_000, ProgressStyle::Ring);
        assert_eq!(activity.leading, "Gönderildi");
        assert_eq!(activity.icon, Some(Icon::Check));
        assert_eq!(activity.expires_at, Some(10_000 + DONE_MS));
    }

    #[test]
    fn the_countdown_wakes_on_whole_seconds() {
        let waiting = transfer(Stage::Waiting { until: 10_000 });
        assert_eq!(waiting.next_moment(1_500, true), Some(2_000));
        assert_eq!(waiting.next_moment(2_000, true), Some(3_000));
        assert_eq!(waiting.next_moment(9_800, true), Some(10_000));
        assert_eq!(waiting.next_moment(1_500, false), Some(10_000));
        assert_eq!(transfer(Stage::Sending { sent: 1 }).next_moment(0, true), None);
    }

    #[test]
    fn more_bytes_than_expected_still_reads_as_full() {
        let over = transfer(Stage::Sending { sent: 3 * 1024 * 1024 });
        assert_eq!(over.permille(), 1000);
    }
}
