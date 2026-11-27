//! Recovery artifacts next to the primary configuration and, from 2D-2 S3 on, the candidates for a restore.

use std::io;

use island_core::config_state::{ArtifactInventory, ProtectedBackupName, StoredFailure};

use super::{config_commit, config_fs::ConfigDir, settings};

pub(crate) fn inventory(dir: &ConfigDir) -> io::Result<ArtifactInventory> {
    let mut inventory = ArtifactInventory::default();
    for name in dir.entries().map_err(io::Error::from)? {
        let Some(original) = name.to_str() else {
            inventory.other_evidence = true;
            inventory.ambiguous_leftovers = true;
            continue;
        };
        let lower = original.to_ascii_lowercase();
        if lower == "config.toml.bak" {
            inventory.rolling_backup = true;
        }
        if matches!(
            lower.as_str(),
            "config.toml.bak.new" | "config.toml.recover"
        ) || (lower.starts_with("config.pre-migration") && lower.ends_with(".bak"))
        {
            inventory.ambiguous_leftovers = true;
        }
        if lower == "config.toml.new" {
            inventory.other_evidence = true;
        }
        if lower == config_commit::FAILURE_RECORD_NAME {
            inventory.failure_record_present = true;
        }
        if settings::is_stale_draft_name(&name) {
            inventory.leftover_drafts += 1;
        }
        if let Some(backup) = ProtectedBackupName::parse(original) {
            inventory.protected_backups.push(backup);
        }
    }
    if let StoredFailure::Valid(record) = config_commit::read_failure(dir) {
        inventory.recorded_failure = Some(record.source);
    }
    Ok(inventory)
}

/// For observation paths that cannot fail: an unreadable folder listing counts as
/// evidence that blocks any restore, never as an empty folder.
pub(crate) fn inventory_or_closed(dir: &ConfigDir) -> ArtifactInventory {
    close_on_error(inventory(dir))
}

fn close_on_error(result: io::Result<ArtifactInventory>) -> ArtifactInventory {
    result.unwrap_or_else(|_| closed())
}

pub(crate) fn closed() -> ArtifactInventory {
    ArtifactInventory {
        other_evidence: true,
        ambiguous_leftovers: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use island_core::config_state::{ChangeFingerprint, FailureRecord, FailureStage};
    use std::fs;

    fn folder() -> (settings::tests::Temp, ConfigDir) {
        let path = settings::tests::temp_base();
        let dir = ConfigDir::open(&path).unwrap();
        (path, dir)
    }

    #[test]
    fn inventory_classifies_every_known_name() {
        let (path, dir) = folder();
        let backup = ProtectedBackupName::new(1, 2, ChangeFingerprint::of(b"backup")).unwrap();
        for name in [
            "CONFIG.TOML.BAK",
            "CONFIG.TOML.BAK.NEW",
            "CONFIG.TOML.RECOVER",
            "CONFIG.PRE-MIGRATION-x.BAK",
            "CONFIG.TOML.NEW",
            "CONFIG.TOML.FAILURE",
            "config.toml.1-2-3.tmp",
            &backup.file_name(),
        ] {
            fs::write(path.join(name), b"x").unwrap();
        }
        let found = inventory(&dir).unwrap();
        assert!(found.rolling_backup);
        assert_eq!(found.protected_backups, vec![backup]);
        assert_eq!(found.leftover_drafts, 1);
        assert!(found.ambiguous_leftovers);
        assert!(found.failure_record_present);
        assert!(found.other_evidence);
        assert_eq!(found.recorded_failure, None);
    }

    #[test]
    fn inventory_ambiguous_names_set_only_ambiguous() {
        for name in [
            "config.toml.bak.new",
            "config.toml.recover",
            "config.pre-migration-x.bak",
        ] {
            let (path, dir) = folder();
            fs::write(path.join(name), b"x").unwrap();
            assert_eq!(
                inventory(&dir).unwrap(),
                ArtifactInventory {
                    ambiguous_leftovers: true,
                    ..Default::default()
                }
            );
        }
    }

    #[test]
    fn inventory_uppercase_ambiguous_names_set_only_ambiguous() {
        for name in [
            "CONFIG.TOML.BAK.NEW",
            "CONFIG.TOML.RECOVER",
            "CONFIG.PRE-MIGRATION-X.BAK",
        ] {
            let (path, dir) = folder();
            fs::write(path.join(name), b"x").unwrap();
            assert_eq!(
                inventory(&dir).unwrap(),
                ArtifactInventory {
                    ambiguous_leftovers: true,
                    ..Default::default()
                }
            );
        }
    }

    #[test]
    fn inventory_stale_draft_variants_count_only_as_drafts() {
        for name in ["config.toml.bak.1-2-3.tmp", "config.toml.failure.1-2-3.tmp"] {
            let (path, dir) = folder();
            fs::write(path.join(name), b"x").unwrap();
            assert_eq!(
                inventory(&dir).unwrap(),
                ArtifactInventory {
                    leftover_drafts: 1,
                    ..Default::default()
                }
            );
        }
    }

    #[test]
    fn inventory_non_unicode_name_is_ambiguous_evidence() {
        use std::os::windows::ffi::OsStringExt;
        let (path, dir) = folder();
        let name = std::ffi::OsString::from_wide(&[
            b'x' as u16,
            0xd800,
            b'.' as u16,
            b't' as u16,
            b'x' as u16,
            b't' as u16,
        ]);
        if let Err(error) = fs::write(path.join(name), b"x") {
            eprintln!("skipping non-Unicode filename test: {error}");
            return;
        }
        assert_eq!(
            inventory(&dir).unwrap(),
            ArtifactInventory {
                other_evidence: true,
                ambiguous_leftovers: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn inventory_config_toml_new_is_evidence_not_ambiguous() {
        let (path, dir) = folder();
        fs::write(path.join("config.toml.new"), b"x").unwrap();
        assert_eq!(
            inventory(&dir).unwrap(),
            ArtifactInventory {
                other_evidence: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn inventory_failure_record_counts_once() {
        let (path, dir) = folder();
        let record = FailureRecord {
            source: ChangeFingerprint::of(b"source"),
            from_schema: 1,
            stage: FailureStage::Backup,
            attempts: 1,
        };
        fs::write(
            path.join(config_commit::FAILURE_RECORD_NAME),
            record.encode(),
        )
        .unwrap();
        let found = inventory(&dir).unwrap();
        assert!(found.failure_record_present);
        assert_eq!(found.leftover_drafts, 0);
        assert_eq!(found.recorded_failure, Some(record.source));
        fs::write(path.join(config_commit::FAILURE_RECORD_NAME), b"corrupt").unwrap();
        let found = inventory(&dir).unwrap();
        assert!(found.failure_record_present);
        assert_eq!(found.leftover_drafts, 0);
        assert_eq!(found.recorded_failure, None);
    }

    #[test]
    fn inventory_empty_folder_has_no_evidence() {
        let (_, dir) = folder();
        assert_eq!(inventory(&dir).unwrap(), ArtifactInventory::default());
    }

    #[test]
    fn inventory_or_closed_blocks_on_listing_error() {
        let found = close_on_error(Err(io::Error::other("listing failed")));
        assert_eq!(
            found,
            ArtifactInventory {
                other_evidence: true,
                ambiguous_leftovers: true,
                ..Default::default()
            }
        );
        assert!(found.has_recovery_artifacts());
    }
}
