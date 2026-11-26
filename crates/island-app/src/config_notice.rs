//! Maps the startup migration report and config mode to what the user sees (2C-4).

// ConfigStatus is wired into settings in 2C-4 S3.
#![cfg_attr(not(test), allow(dead_code))]

use crate::platform::config_migrate::{FallbackReason, MigrationReport};
use island_core::config_state::RuntimeMode;
use island_core::{Activity, Icon, Kind, Millis};

pub(crate) const STARTUP_NOTICE_MS: Millis = 6_000;

pub(crate) fn startup_activity(notice: StartupNotice, now: Millis) -> Activity {
    let (title, detail) = notice.text();
    Activity {
        title: title.to_owned(),
        detail: detail.to_owned(),
        icon: Some(Icon::Warning),
        expires_at: Some(now + STARTUP_NOTICE_MS),
        ..Activity::new(Kind::Notification)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartupNotice {
    MigrationFailed,
    Recovery,
    ReadOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigStatus {
    MigrationFailed,
    Legacy,
    Recovery,
    ReadOnly,
    RestartNeeded,
}

pub(crate) fn startup_notice(
    report: Option<&MigrationReport>,
    mode: &RuntimeMode,
) -> Option<StartupNotice> {
    match (mode, report) {
        (RuntimeMode::ReadOnly, _) => Some(StartupNotice::ReadOnly),
        (RuntimeMode::Recovery, _) => Some(StartupNotice::Recovery),
        (
            RuntimeMode::Normal | RuntimeMode::LegacyCompatible,
            Some(MigrationReport::Fallback(FallbackReason::Failed(_))),
        ) => Some(StartupNotice::MigrationFailed),
        (
            RuntimeMode::Normal | RuntimeMode::LegacyCompatible,
            None
            | Some(MigrationReport::NotNeeded | MigrationReport::Migrated { .. })
            | Some(MigrationReport::Fallback(
                FallbackReason::Busy
                | FallbackReason::Unprepared
                | FallbackReason::Io(_)
                | FallbackReason::SourceChanged
                | FallbackReason::Suppressed
                | FallbackReason::Unpreparable,
            )),
        ) => None,
    }
}

pub(crate) fn status(
    report: Option<&MigrationReport>,
    mode: &RuntimeMode,
    writes_allowed: bool,
) -> Option<ConfigStatus> {
    match (mode, report, writes_allowed) {
        (RuntimeMode::Recovery, _, _) => Some(ConfigStatus::Recovery),
        (RuntimeMode::ReadOnly, _, _) => Some(ConfigStatus::ReadOnly),
        (
            RuntimeMode::LegacyCompatible,
            Some(MigrationReport::Fallback(FallbackReason::Failed(_) | FallbackReason::Suppressed)),
            _,
        ) => Some(ConfigStatus::MigrationFailed),
        (RuntimeMode::LegacyCompatible, _, _) => Some(ConfigStatus::Legacy),
        (RuntimeMode::Normal, _, false) => Some(ConfigStatus::RestartNeeded),
        (RuntimeMode::Normal, _, true) => None,
    }
}

impl StartupNotice {
    pub(crate) fn text(self) -> (&'static str, &'static str) {
        match self {
            Self::MigrationFailed => (
                "Ayarlar ta\u{15f}\u{131}namad\u{131}",
                "Eski ayarlarla devam ediliyor",
            ),
            Self::Recovery => (
                "Ayar dosyas\u{131} onar\u{131}m bekliyor",
                "De\u{11f}i\u{15f}iklikler kaydedilmiyor",
            ),
            Self::ReadOnly => (
                "Ayarlar salt okunur",
                "Dosya daha yeni bir Islora s\u{fc}r\u{fc}m\u{fc}ne ait",
            ),
        }
    }
}
impl ConfigStatus {
    pub(crate) fn lines(self) -> (&'static str, &'static str) {
        match self {
            Self::MigrationFailed => (
                "Ayarlar yeni bi\u{e7}ime ta\u{15f}\u{131}namad\u{131}; dosyan\u{131}z de\u{11f}i\u{15f}medi.",
                "Eski ayarlarla \u{e7}al\u{131}\u{15f}\u{131}l\u{131}yor. Dosya de\u{11f}i\u{15f}irse sonraki a\u{e7}\u{131}l\u{131}\u{15f}ta yeniden denenir.",
            ),
            Self::Legacy => (
                "Ayar dosyas\u{131} eski ya da hatal\u{131}; uyumluluk modunda \u{e7}al\u{131}\u{15f}\u{131}l\u{131}yor.",
                "Dosyay\u{131} \"Ayar dosyas\u{131}n\u{131} a\u{e7}\" ile denetleyebilirsiniz.",
            ),
            Self::Recovery => (
                "Ayar dosyas\u{131} onar\u{131}m bekliyor; de\u{11f}i\u{15f}iklikler kaydedilmiyor.",
                "Dosyay\u{131} \"Ayar dosyas\u{131}n\u{131} a\u{e7}\" ile d\u{fc}zeltip Islora'y\u{131} yeniden ba\u{15f}lat\u{131}n.",
            ),
            Self::ReadOnly => (
                "Ayar dosyas\u{131} daha yeni bir Islora s\u{fc}r\u{fc}m\u{fc}ne ait; salt okunur.",
                "De\u{11f}i\u{15f}iklik yapmak i\u{e7}in Islora'y\u{131} g\u{fc}ncelleyin.",
            ),
            Self::RestartNeeded => (
                "Ayarlar \u{15f}u anda kaydedilemiyor.",
                "Islora'y\u{131} yeniden ba\u{15f}lat\u{131}n.",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::config_migrate::FallbackReason;
    use island_core::config_state::{ChangeFingerprint, FailureStage, ProtectedBackupName};
    use std::io::ErrorKind;

    #[test]
    fn startup_activity_is_a_short_warning_notification() {
        for notice in [
            StartupNotice::MigrationFailed,
            StartupNotice::Recovery,
            StartupNotice::ReadOnly,
        ] {
            let (title, detail) = notice.text();
            let activity = startup_activity(notice, 1_000);
            assert_eq!(activity.kind, island_core::Kind::Notification);
            assert_eq!(activity.icon, Some(island_core::Icon::Warning));
            assert_eq!(activity.title, title);
            assert_eq!(activity.detail, detail);
            assert_eq!(activity.expires_at, Some(7_000));
        }
    }

    #[test]
    fn startup_notice_ms_is_six_seconds() {
        assert_eq!(STARTUP_NOTICE_MS, 6_000);
    }

    fn reports() -> Vec<Option<MigrationReport>> {
        use FallbackReason::*;
        vec![
            None,
            Some(MigrationReport::NotNeeded),
            Some(MigrationReport::Migrated {
                backup: ProtectedBackupName::new(1, 3, ChangeFingerprint::from_value(1)).unwrap(),
            }),
            Some(MigrationReport::Fallback(Busy)),
            Some(MigrationReport::Fallback(Unprepared)),
            Some(MigrationReport::Fallback(Io(ErrorKind::NotFound))),
            Some(MigrationReport::Fallback(SourceChanged)),
            Some(MigrationReport::Fallback(Suppressed)),
            Some(MigrationReport::Fallback(Unpreparable)),
            Some(MigrationReport::Fallback(Failed(FailureStage::Backup))),
            Some(MigrationReport::Fallback(Failed(FailureStage::Commit))),
            Some(MigrationReport::Fallback(Failed(FailureStage::Verify))),
        ]
    }

    fn modes() -> [RuntimeMode; 4] {
        [
            RuntimeMode::Normal,
            RuntimeMode::ReadOnly,
            RuntimeMode::LegacyCompatible,
            RuntimeMode::Recovery,
        ]
    }

    #[test]
    fn startup_notice_table() {
        let none = None;
        let not_needed = Some(MigrationReport::NotNeeded);
        let migrated = Some(MigrationReport::Migrated {
            backup: ProtectedBackupName::new(1, 3, ChangeFingerprint::from_value(1)).unwrap(),
        });
        let busy = Some(MigrationReport::Fallback(FallbackReason::Busy));
        let unprepared = Some(MigrationReport::Fallback(FallbackReason::Unprepared));
        let io = Some(MigrationReport::Fallback(FallbackReason::Io(
            ErrorKind::NotFound,
        )));
        let source_changed = Some(MigrationReport::Fallback(FallbackReason::SourceChanged));
        let suppressed = Some(MigrationReport::Fallback(FallbackReason::Suppressed));
        let unpreparable = Some(MigrationReport::Fallback(FallbackReason::Unpreparable));
        let failed_backup = Some(MigrationReport::Fallback(FallbackReason::Failed(
            FailureStage::Backup,
        )));
        let failed_commit = Some(MigrationReport::Fallback(FallbackReason::Failed(
            FailureStage::Commit,
        )));
        let failed_verify = Some(MigrationReport::Fallback(FallbackReason::Failed(
            FailureStage::Verify,
        )));
        #[rustfmt::skip]
        let table = [
            (RuntimeMode::Normal, &none, None),
            (RuntimeMode::Normal, &not_needed, None),
            (RuntimeMode::Normal, &migrated, None),
            (RuntimeMode::Normal, &busy, None),
            (RuntimeMode::Normal, &unprepared, None),
            (RuntimeMode::Normal, &io, None),
            (RuntimeMode::Normal, &source_changed, None),
            (RuntimeMode::Normal, &suppressed, None),
            (RuntimeMode::Normal, &unpreparable, None),
            (RuntimeMode::Normal, &failed_backup, Some(StartupNotice::MigrationFailed)),
            (RuntimeMode::Normal, &failed_commit, Some(StartupNotice::MigrationFailed)),
            (RuntimeMode::Normal, &failed_verify, Some(StartupNotice::MigrationFailed)),
            (RuntimeMode::ReadOnly, &none, Some(StartupNotice::ReadOnly)),
            (RuntimeMode::ReadOnly, &not_needed, Some(StartupNotice::ReadOnly)),
            (RuntimeMode::ReadOnly, &migrated, Some(StartupNotice::ReadOnly)),
            (RuntimeMode::ReadOnly, &busy, Some(StartupNotice::ReadOnly)),
            (RuntimeMode::ReadOnly, &unprepared, Some(StartupNotice::ReadOnly)),
            (RuntimeMode::ReadOnly, &io, Some(StartupNotice::ReadOnly)),
            (RuntimeMode::ReadOnly, &source_changed, Some(StartupNotice::ReadOnly)),
            (RuntimeMode::ReadOnly, &suppressed, Some(StartupNotice::ReadOnly)),
            (RuntimeMode::ReadOnly, &unpreparable, Some(StartupNotice::ReadOnly)),
            (RuntimeMode::ReadOnly, &failed_backup, Some(StartupNotice::ReadOnly)),
            (RuntimeMode::ReadOnly, &failed_commit, Some(StartupNotice::ReadOnly)),
            (RuntimeMode::ReadOnly, &failed_verify, Some(StartupNotice::ReadOnly)),
            (RuntimeMode::LegacyCompatible, &none, None),
            (RuntimeMode::LegacyCompatible, &not_needed, None),
            (RuntimeMode::LegacyCompatible, &migrated, None),
            (RuntimeMode::LegacyCompatible, &busy, None),
            (RuntimeMode::LegacyCompatible, &unprepared, None),
            (RuntimeMode::LegacyCompatible, &io, None),
            (RuntimeMode::LegacyCompatible, &source_changed, None),
            (RuntimeMode::LegacyCompatible, &suppressed, None),
            (RuntimeMode::LegacyCompatible, &unpreparable, None),
            (RuntimeMode::LegacyCompatible, &failed_backup, Some(StartupNotice::MigrationFailed)),
            (RuntimeMode::LegacyCompatible, &failed_commit, Some(StartupNotice::MigrationFailed)),
            (RuntimeMode::LegacyCompatible, &failed_verify, Some(StartupNotice::MigrationFailed)),
            (RuntimeMode::Recovery, &none, Some(StartupNotice::Recovery)),
            (RuntimeMode::Recovery, &not_needed, Some(StartupNotice::Recovery)),
            (RuntimeMode::Recovery, &migrated, Some(StartupNotice::Recovery)),
            (RuntimeMode::Recovery, &busy, Some(StartupNotice::Recovery)),
            (RuntimeMode::Recovery, &unprepared, Some(StartupNotice::Recovery)),
            (RuntimeMode::Recovery, &io, Some(StartupNotice::Recovery)),
            (RuntimeMode::Recovery, &source_changed, Some(StartupNotice::Recovery)),
            (RuntimeMode::Recovery, &suppressed, Some(StartupNotice::Recovery)),
            (RuntimeMode::Recovery, &unpreparable, Some(StartupNotice::Recovery)),
            (RuntimeMode::Recovery, &failed_backup, Some(StartupNotice::Recovery)),
            (RuntimeMode::Recovery, &failed_commit, Some(StartupNotice::Recovery)),
            (RuntimeMode::Recovery, &failed_verify, Some(StartupNotice::Recovery)),
        ];
        assert_eq!(table.len(), modes().len() * reports().len());
        for mode in modes() {
            for report in reports() {
                assert_eq!(
                    table
                        .iter()
                        .filter(|(m, r, _)| m == &mode && *r == &report)
                        .count(),
                    1
                );
            }
        }
        for (mode, report, expected) in table {
            assert_eq!(
                startup_notice(report.as_ref(), &mode),
                expected,
                "{mode:?} {report:?}"
            );
        }
    }

    #[test]
    fn status_table() {
        let none = None;
        let not_needed = Some(MigrationReport::NotNeeded);
        let migrated = Some(MigrationReport::Migrated {
            backup: ProtectedBackupName::new(1, 3, ChangeFingerprint::from_value(1)).unwrap(),
        });
        let busy = Some(MigrationReport::Fallback(FallbackReason::Busy));
        let unprepared = Some(MigrationReport::Fallback(FallbackReason::Unprepared));
        let io = Some(MigrationReport::Fallback(FallbackReason::Io(
            ErrorKind::NotFound,
        )));
        let source_changed = Some(MigrationReport::Fallback(FallbackReason::SourceChanged));
        let suppressed = Some(MigrationReport::Fallback(FallbackReason::Suppressed));
        let unpreparable = Some(MigrationReport::Fallback(FallbackReason::Unpreparable));
        let failed_backup = Some(MigrationReport::Fallback(FallbackReason::Failed(
            FailureStage::Backup,
        )));
        let failed_commit = Some(MigrationReport::Fallback(FallbackReason::Failed(
            FailureStage::Commit,
        )));
        let failed_verify = Some(MigrationReport::Fallback(FallbackReason::Failed(
            FailureStage::Verify,
        )));
        #[rustfmt::skip]
        let table = [
            (RuntimeMode::Normal, &none, true, None),
            (RuntimeMode::Normal, &none, false, Some(ConfigStatus::RestartNeeded)),
            (RuntimeMode::Normal, &not_needed, true, None),
            (RuntimeMode::Normal, &not_needed, false, Some(ConfigStatus::RestartNeeded)),
            (RuntimeMode::Normal, &migrated, true, None),
            (RuntimeMode::Normal, &migrated, false, Some(ConfigStatus::RestartNeeded)),
            (RuntimeMode::Normal, &busy, true, None),
            (RuntimeMode::Normal, &busy, false, Some(ConfigStatus::RestartNeeded)),
            (RuntimeMode::Normal, &unprepared, true, None),
            (RuntimeMode::Normal, &unprepared, false, Some(ConfigStatus::RestartNeeded)),
            (RuntimeMode::Normal, &io, true, None),
            (RuntimeMode::Normal, &io, false, Some(ConfigStatus::RestartNeeded)),
            (RuntimeMode::Normal, &source_changed, true, None),
            (RuntimeMode::Normal, &source_changed, false, Some(ConfigStatus::RestartNeeded)),
            (RuntimeMode::Normal, &suppressed, true, None),
            (RuntimeMode::Normal, &suppressed, false, Some(ConfigStatus::RestartNeeded)),
            (RuntimeMode::Normal, &unpreparable, true, None),
            (RuntimeMode::Normal, &unpreparable, false, Some(ConfigStatus::RestartNeeded)),
            (RuntimeMode::Normal, &failed_backup, true, None),
            (RuntimeMode::Normal, &failed_backup, false, Some(ConfigStatus::RestartNeeded)),
            (RuntimeMode::Normal, &failed_commit, true, None),
            (RuntimeMode::Normal, &failed_commit, false, Some(ConfigStatus::RestartNeeded)),
            (RuntimeMode::Normal, &failed_verify, true, None),
            (RuntimeMode::Normal, &failed_verify, false, Some(ConfigStatus::RestartNeeded)),
            (RuntimeMode::ReadOnly, &none, true, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &none, false, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &not_needed, true, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &not_needed, false, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &migrated, true, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &migrated, false, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &busy, true, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &busy, false, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &unprepared, true, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &unprepared, false, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &io, true, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &io, false, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &source_changed, true, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &source_changed, false, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &suppressed, true, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &suppressed, false, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &unpreparable, true, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &unpreparable, false, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &failed_backup, true, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &failed_backup, false, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &failed_commit, true, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &failed_commit, false, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &failed_verify, true, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::ReadOnly, &failed_verify, false, Some(ConfigStatus::ReadOnly)),
            (RuntimeMode::LegacyCompatible, &none, true, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &none, false, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &not_needed, true, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &not_needed, false, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &migrated, true, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &migrated, false, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &busy, true, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &busy, false, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &unprepared, true, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &unprepared, false, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &io, true, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &io, false, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &source_changed, true, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &source_changed, false, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &suppressed, true, Some(ConfigStatus::MigrationFailed)),
            (RuntimeMode::LegacyCompatible, &suppressed, false, Some(ConfigStatus::MigrationFailed)),
            (RuntimeMode::LegacyCompatible, &unpreparable, true, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &unpreparable, false, Some(ConfigStatus::Legacy)),
            (RuntimeMode::LegacyCompatible, &failed_backup, true, Some(ConfigStatus::MigrationFailed)),
            (RuntimeMode::LegacyCompatible, &failed_backup, false, Some(ConfigStatus::MigrationFailed)),
            (RuntimeMode::LegacyCompatible, &failed_commit, true, Some(ConfigStatus::MigrationFailed)),
            (RuntimeMode::LegacyCompatible, &failed_commit, false, Some(ConfigStatus::MigrationFailed)),
            (RuntimeMode::LegacyCompatible, &failed_verify, true, Some(ConfigStatus::MigrationFailed)),
            (RuntimeMode::LegacyCompatible, &failed_verify, false, Some(ConfigStatus::MigrationFailed)),
            (RuntimeMode::Recovery, &none, true, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &none, false, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &not_needed, true, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &not_needed, false, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &migrated, true, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &migrated, false, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &busy, true, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &busy, false, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &unprepared, true, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &unprepared, false, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &io, true, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &io, false, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &source_changed, true, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &source_changed, false, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &suppressed, true, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &suppressed, false, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &unpreparable, true, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &unpreparable, false, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &failed_backup, true, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &failed_backup, false, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &failed_commit, true, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &failed_commit, false, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &failed_verify, true, Some(ConfigStatus::Recovery)),
            (RuntimeMode::Recovery, &failed_verify, false, Some(ConfigStatus::Recovery)),
        ];
        assert_eq!(table.len(), modes().len() * reports().len() * 2);
        for mode in modes() {
            for report in reports() {
                for writes in [true, false] {
                    assert_eq!(
                        table
                            .iter()
                            .filter(|(m, r, w, _)| m == &mode && *r == &report && *w == writes)
                            .count(),
                        1
                    );
                }
            }
        }
        for (mode, report, writes, expected) in table {
            assert_eq!(
                status(report.as_ref(), &mode, writes),
                expected,
                "{mode:?} {report:?} {writes}"
            );
        }
    }

    #[test]
    fn success_and_transient_fallbacks_never_notify() {
        let reports = [
            Some(MigrationReport::Migrated {
                backup: ProtectedBackupName::new(1, 3, ChangeFingerprint::from_value(1)).unwrap(),
            }),
            Some(MigrationReport::Fallback(FallbackReason::Suppressed)),
            Some(MigrationReport::Fallback(FallbackReason::Busy)),
            Some(MigrationReport::Fallback(FallbackReason::SourceChanged)),
            Some(MigrationReport::Fallback(FallbackReason::Io(
                ErrorKind::NotFound,
            ))),
        ];
        for mode in [RuntimeMode::Normal, RuntimeMode::LegacyCompatible] {
            for report in &reports {
                assert_eq!(startup_notice(report.as_ref(), &mode), None);
            }
        }
    }

    #[test]
    fn texts_are_short_and_value_free() {
        let notices = [
            StartupNotice::MigrationFailed,
            StartupNotice::Recovery,
            StartupNotice::ReadOnly,
        ];
        let statuses = [
            ConfigStatus::MigrationFailed,
            ConfigStatus::Legacy,
            ConfigStatus::Recovery,
            ConfigStatus::ReadOnly,
            ConfigStatus::RestartNeeded,
        ];
        for (a, b) in notices
            .into_iter()
            .map(StartupNotice::text)
            .chain(statuses.into_iter().map(ConfigStatus::lines))
        {
            for line in [a, b] {
                assert!(!line.is_empty());
                assert!(
                    line.chars().count() <= 80,
                    "{}: {line}",
                    line.chars().count()
                );
                assert!(!line.chars().any(|c| c.is_ascii_digit()));
                assert!(
                    !line.contains("config.toml")
                        && !line.contains('\\')
                        && !line.contains(".toml")
                );
            }
        }
    }

    #[test]
    fn texts_decode_exactly() {
        assert_eq!(
            StartupNotice::MigrationFailed.text(),
            (
                "Ayarlar ta\u{15f}\u{131}namad\u{131}",
                "Eski ayarlarla devam ediliyor"
            )
        );
        assert_eq!(
            StartupNotice::Recovery.text(),
            (
                "Ayar dosyas\u{131} onar\u{131}m bekliyor",
                "De\u{11f}i\u{15f}iklikler kaydedilmiyor"
            )
        );
        assert_eq!(
            StartupNotice::ReadOnly.text(),
            (
                "Ayarlar salt okunur",
                "Dosya daha yeni bir Islora s\u{fc}r\u{fc}m\u{fc}ne ait"
            )
        );
        assert_eq!(
            ConfigStatus::MigrationFailed.lines(),
            (
                "Ayarlar yeni bi\u{e7}ime ta\u{15f}\u{131}namad\u{131}; dosyan\u{131}z de\u{11f}i\u{15f}medi.",
                "Eski ayarlarla \u{e7}al\u{131}\u{15f}\u{131}l\u{131}yor. Dosya de\u{11f}i\u{15f}irse sonraki a\u{e7}\u{131}l\u{131}\u{15f}ta yeniden denenir."
            )
        );
        assert_eq!(
            ConfigStatus::Legacy.lines(),
            (
                "Ayar dosyas\u{131} eski ya da hatal\u{131}; uyumluluk modunda \u{e7}al\u{131}\u{15f}\u{131}l\u{131}yor.",
                "Dosyay\u{131} \"Ayar dosyas\u{131}n\u{131} a\u{e7}\" ile denetleyebilirsiniz."
            )
        );
        assert_eq!(
            ConfigStatus::Recovery.lines(),
            (
                "Ayar dosyas\u{131} onar\u{131}m bekliyor; de\u{11f}i\u{15f}iklikler kaydedilmiyor.",
                "Dosyay\u{131} \"Ayar dosyas\u{131}n\u{131} a\u{e7}\" ile d\u{fc}zeltip Islora'y\u{131} yeniden ba\u{15f}lat\u{131}n."
            )
        );
        assert_eq!(
            ConfigStatus::ReadOnly.lines(),
            (
                "Ayar dosyas\u{131} daha yeni bir Islora s\u{fc}r\u{fc}m\u{fc}ne ait; salt okunur.",
                "De\u{11f}i\u{15f}iklik yapmak i\u{e7}in Islora'y\u{131} g\u{fc}ncelleyin."
            )
        );
        assert_eq!(
            ConfigStatus::RestartNeeded.lines(),
            (
                "Ayarlar \u{15f}u anda kaydedilemiyor.",
                "Islora'y\u{131} yeniden ba\u{15f}lat\u{131}n."
            )
        );
    }
}
