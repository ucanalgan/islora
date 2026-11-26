//! Pure naming and retention rules for protected configuration backups.

/// Used only to notice that a file changed between two observations.
/// It is not a security or integrity check and must never be used to trust content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChangeFingerprint(u64);

impl ChangeFingerprint {
    pub fn of(bytes: &[u8]) -> Self {
        Self(bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        }))
    }

    pub fn value(self) -> u64 {
        self.0
    }

    pub fn from_value(value: u64) -> Self {
        Self(value)
    }
}

pub const MAX_PROTECTED_SEQUENCE: u32 = 999_999;
pub const DEFAULT_PROTECTED_KEEP: usize = 3;

/// A canonical name for a protected, never-overwritten backup.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProtectedBackupName {
    sequence: u32,
    source_schema: u32,
    fingerprint: ChangeFingerprint,
}

impl ProtectedBackupName {
    pub fn new(sequence: u32, source_schema: u32, fingerprint: ChangeFingerprint) -> Option<Self> {
        (1..=MAX_PROTECTED_SEQUENCE)
            .contains(&sequence)
            .then_some(Self {
                sequence,
                source_schema,
                fingerprint,
            })
    }

    pub fn sequence(&self) -> u32 {
        self.sequence
    }

    pub fn source_schema(&self) -> u32 {
        self.source_schema
    }

    pub fn fingerprint(&self) -> ChangeFingerprint {
        self.fingerprint
    }

    pub fn file_name(&self) -> String {
        format!(
            "config.protected.{:06}.v{}.{:016x}.bak",
            self.sequence,
            self.source_schema,
            self.fingerprint.value()
        )
    }

    /// Accept only names in the exact canonical form.
    pub fn parse(name: &str) -> Option<Self> {
        let mut parts = name.split('.');
        if parts.next()? != "config" || parts.next()? != "protected" {
            return None;
        }
        let sequence = parts.next()?;
        let schema = parts.next()?.strip_prefix('v')?;
        let fingerprint = parts.next()?;
        if parts.next()? != "bak"
            || parts.next().is_some()
            || sequence.len() != 6
            || !sequence.bytes().all(|b| b.is_ascii_digit())
            || schema.is_empty()
            || (schema.len() > 1 && schema.starts_with('0'))
            || !schema.bytes().all(|b| b.is_ascii_digit())
            || fingerprint.len() != 16
            || !fingerprint
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        Self::new(
            sequence.parse().ok()?,
            schema.parse().ok()?,
            ChangeFingerprint::from_value(u64::from_str_radix(fingerprint, 16).ok()?),
        )
    }
}

/// Return the next unused sequence without filling gaps or wrapping.
pub fn next_sequence<'a>(
    existing: impl IntoIterator<Item = &'a ProtectedBackupName>,
) -> Option<u32> {
    match existing.into_iter().map(|name| name.sequence).max() {
        None => Some(1),
        Some(MAX_PROTECTED_SEQUENCE) => None,
        Some(sequence) => Some(sequence + 1),
    }
}

/// Return distinct excess names from oldest to newest, keeping at least one.
/// Names sharing a sequence (possible only with a planted file, since `next_sequence`
/// never repeats a number) are ordered by schema and then fingerprint.
pub fn retention_excess(existing: &[ProtectedBackupName], keep: usize) -> Vec<ProtectedBackupName> {
    let mut names = existing.to_vec();
    names
        .sort_unstable_by_key(|name| (name.sequence, name.source_schema, name.fingerprint.value()));
    names.dedup();
    names.truncate(names.len().saturating_sub(keep.max(1)));
    names
}

/// What the platform observed at the primary configuration path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrimaryObservation {
    /// The configuration file does not exist.
    Missing,
    /// The file exists but could not be read.
    Unreadable,
    /// The file is invalid or contradictory.
    Invalid { fingerprint: ChangeFingerprint },
    /// The file has a schema newer than this build understands.
    Future {
        schema: u32,
        fingerprint: ChangeFingerprint,
    },
    /// The file uses the schema this build writes.
    Current { fingerprint: ChangeFingerprint },
    /// The file has an older, migratable schema.
    Legacy {
        schema: u32,
        fingerprint: ChangeFingerprint,
    },
}

/// Recovery artifacts observed next to the primary configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtifactInventory {
    /// Whether a rolling backup exists.
    pub rolling_backup: bool,
    /// Protected backups found next to the primary configuration.
    pub protected_backups: Vec<ProtectedBackupName>,
    /// Number of leftover draft files.
    pub leftover_drafts: usize,
    /// Fingerprint recorded for a previous migration failure, if present.
    pub recorded_failure: Option<ChangeFingerprint>,
}

impl ArtifactInventory {
    /// Return whether any recovery artifact is present.
    pub fn has_recovery_artifacts(&self) -> bool {
        self.rolling_backup
            || !self.protected_backups.is_empty()
            || self.leftover_drafts > 0
            || self.recorded_failure.is_some()
    }
}

/// How the application can operate after startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeMode {
    /// Normal operation, including writes.
    Normal,
    /// Read-only operation for a newer schema.
    ReadOnly,
    /// Compatibility mode for legacy, invalid or unreadable primaries (migration disabled, failed or suppressed); settings writes need readable text and ready storage.
    LegacyCompatible,
    /// Recovery is required before normal operation.
    Recovery,
}

/// Why startup must use legacy compatibility.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LegacyReason {
    /// The primary file is invalid or contradictory.
    Invalid,
    /// The primary file could not be read.
    Unreadable,
    /// Migration already failed for this source.
    RetrySuppressed,
}

/// The action chosen for startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartupOutcome {
    /// Create defaults for a new installation.
    NewInstall,
    /// Use the current file as is.
    Ready,
    /// Migrate the identified legacy source.
    Migrate {
        from_schema: u32,
        source: ChangeFingerprint,
    },
    /// Use a newer schema read-only.
    FutureReadOnly { schema: u32 },
    /// Open in compatibility mode without touching the file.
    LegacyCompatible { reason: LegacyReason },
    /// Recover a missing primary from existing artifacts.
    Recover,
}

impl StartupOutcome {
    /// Return the runtime mode associated with this outcome.
    pub fn mode(&self) -> RuntimeMode {
        match self {
            Self::NewInstall | Self::Ready | Self::Migrate { .. } => RuntimeMode::Normal,
            Self::FutureReadOnly { .. } => RuntimeMode::ReadOnly,
            Self::LegacyCompatible { .. } => RuntimeMode::LegacyCompatible,
            Self::Recover => RuntimeMode::Recovery,
        }
    }

    /// Return whether this outcome permits writes.
    pub fn allows_writes(&self) -> bool {
        matches!(self, Self::NewInstall | Self::Ready | Self::Migrate { .. })
    }
}

/// Decide startup according to these rules, in precedence order:
///
/// a. A missing primary requires recovery if any recovery artifact exists;
///    otherwise it is a new installation.
/// b. A future schema is always read-only, regardless of artifacts or failures.
/// c. An invalid or unreadable primary uses legacy compatibility, regardless of
///    artifacts or failures, without touching the file.
/// d. A recorded failure for the same legacy fingerprint suppresses migration;
///    a failure for a different fingerprint does not.
/// e. Any other legacy source is migrated. A current file is ready regardless
///    of artifacts or failures.
pub fn plan_startup(primary: &PrimaryObservation, artifacts: &ArtifactInventory) -> StartupOutcome {
    match primary {
        PrimaryObservation::Missing if artifacts.has_recovery_artifacts() => {
            StartupOutcome::Recover
        }
        PrimaryObservation::Missing => StartupOutcome::NewInstall,
        PrimaryObservation::Future { schema, .. } => {
            StartupOutcome::FutureReadOnly { schema: *schema }
        }
        PrimaryObservation::Invalid { .. } => StartupOutcome::LegacyCompatible {
            reason: LegacyReason::Invalid,
        },
        PrimaryObservation::Unreadable => StartupOutcome::LegacyCompatible {
            reason: LegacyReason::Unreadable,
        },
        PrimaryObservation::Legacy { fingerprint, .. }
            if artifacts.recorded_failure == Some(*fingerprint) =>
        {
            StartupOutcome::LegacyCompatible {
                reason: LegacyReason::RetrySuppressed,
            }
        }
        PrimaryObservation::Legacy {
            schema,
            fingerprint,
        } => StartupOutcome::Migrate {
            from_schema: *schema,
            source: *fingerprint,
        },
        PrimaryObservation::Current { .. } => StartupOutcome::Ready,
    }
}

/// The migration step that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureStage {
    /// Creating the backup failed.
    Backup,
    /// Committing the migration failed.
    Commit,
    /// Verifying the migration failed.
    Verify,
}

/// A value-free record of a failed migration. Attempts must be at least one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureRecord {
    /// Fingerprint of the source file.
    pub source: ChangeFingerprint,
    /// Schema of the source file.
    pub from_schema: u32,
    /// Step that failed.
    pub stage: FailureStage,
    /// Number of attempts, at least one.
    pub attempts: u32,
}

/// Maximum accepted size of a failure record in bytes.
pub const FAILURE_RECORD_MAX_BYTES: usize = 256;

/// A failure to decode a record, without retaining any input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureRecordError {
    /// The record exceeds the size limit.
    TooLarge,
    /// The record is not UTF-8.
    NotUtf8,
    /// The record is not canonical.
    Malformed,
    /// The record declares another numeric version.
    UnsupportedVersion,
}

impl FailureRecord {
    /// Encode the record in canonical form; attempts must be at least one.
    pub fn encode(&self) -> String {
        let stage = match self.stage {
            FailureStage::Backup => "backup",
            FailureStage::Commit => "commit",
            FailureStage::Verify => "verify",
        };
        format!(
            "islora-migration-failure 1\nsource={:016x}\nfrom_schema={}\nstage={}\nattempts={}\n",
            self.source.value(),
            self.from_schema,
            stage,
            self.attempts
        )
    }

    /// Decode only the canonical form, without retaining rejected input.
    pub fn decode(bytes: &[u8]) -> Result<Self, FailureRecordError> {
        use FailureRecordError::{Malformed, NotUtf8, TooLarge, UnsupportedVersion};
        if bytes.len() > FAILURE_RECORD_MAX_BYTES {
            return Err(TooLarge);
        }
        let text = std::str::from_utf8(bytes).map_err(|_| NotUtf8)?;
        let first = text.split('\n').next().ok_or(Malformed)?;
        if let Some(version) = first.strip_prefix("islora-migration-failure ")
            && !version.is_empty()
            && version.bytes().all(|b| b.is_ascii_digit())
            && version != "1"
        {
            return Err(UnsupportedVersion);
        }
        let mut lines = text.split('\n');
        if lines.next() != Some("islora-migration-failure 1") {
            return Err(Malformed);
        }
        let source = lines
            .next()
            .and_then(|s| s.strip_prefix("source="))
            .ok_or(Malformed)?;
        if source.len() != 16
            || !source
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Malformed);
        }
        let source = u64::from_str_radix(source, 16).map_err(|_| Malformed)?;
        let number = |s: &str| -> Result<u32, FailureRecordError> {
            if s.is_empty()
                || (s.len() > 1 && s.starts_with('0'))
                || !s.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(Malformed);
            }
            s.parse().map_err(|_| Malformed)
        };
        let from_schema = number(
            lines
                .next()
                .and_then(|s| s.strip_prefix("from_schema="))
                .ok_or(Malformed)?,
        )?;
        let stage = match lines.next() {
            Some("stage=backup") => FailureStage::Backup,
            Some("stage=commit") => FailureStage::Commit,
            Some("stage=verify") => FailureStage::Verify,
            _ => return Err(Malformed),
        };
        let attempts = number(
            lines
                .next()
                .and_then(|s| s.strip_prefix("attempts="))
                .ok_or(Malformed)?,
        )?;
        if attempts == 0 || lines.next() != Some("") || lines.next().is_some() {
            return Err(Malformed);
        }
        Ok(Self {
            source: ChangeFingerprint::from_value(source),
            from_schema,
            stage,
            attempts,
        })
    }
}

/// What the platform found at the failure-record path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredFailure {
    /// No record exists.
    None,
    /// A record was decoded successfully.
    Valid(FailureRecord),
    /// The record could not be read or decoded.
    Corrupt,
}

/// Whether a migration retry is allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryDecision {
    /// No previous failure was recorded.
    NoFailureRecorded,
    /// The source changed since the failure.
    SourceChanged,
    /// Retry is suppressed.
    Suppressed,
}

impl RetryDecision {
    /// Return whether a migration attempt is allowed.
    pub fn allows_attempt(self) -> bool {
        self != Self::Suppressed
    }
}

/// Decide whether to retry. Corrupt records fail closed and suppress retries;
/// they are cleared only by a successful recovery elsewhere.
pub fn retry_decision(stored: &StoredFailure, current_source: ChangeFingerprint) -> RetryDecision {
    match stored {
        StoredFailure::None => RetryDecision::NoFailureRecorded,
        StoredFailure::Valid(record) if record.source != current_source => {
            RetryDecision::SourceChanged
        }
        StoredFailure::Valid(_) | StoredFailure::Corrupt => RetryDecision::Suppressed,
    }
}

/// The location of a recovery candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateSource {
    /// The rolling backup.
    RollingBackup,
    /// A protected backup.
    Protected(ProtectedBackupName),
}

/// The observed validity of a recovery candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateValidity {
    /// A readable, valid configuration.
    Valid {
        schema: u32,
        fingerprint: ChangeFingerprint,
    },
    /// A newer schema.
    Future,
    /// Invalid content.
    Invalid,
    /// Unreadable content.
    Unreadable,
}

/// A recovery source and its observed validity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryCandidate {
    /// The backup location.
    pub source: CandidateSource,
    /// The validation result.
    pub validity: CandidateValidity,
}

/// The result of selecting a recovery candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateSelection {
    /// Restore this source.
    Restore(CandidateSource),
    /// Valid sources have different content fingerprints.
    Ambiguous,
    /// No valid source exists.
    NoneValid,
}

/// Select a valid candidate only when all valid fingerprints agree.
pub fn select_candidate(candidates: &[RecoveryCandidate]) -> CandidateSelection {
    let mut fingerprint = None;
    let mut selected: Option<&CandidateSource> = None;
    for candidate in candidates {
        let CandidateValidity::Valid {
            fingerprint: current,
            ..
        } = candidate.validity
        else {
            continue;
        };
        if fingerprint.is_some_and(|previous| previous != current) {
            return CandidateSelection::Ambiguous;
        }
        fingerprint = Some(current);
        selected = match (selected, &candidate.source) {
            (None, source) => Some(source),
            (Some(CandidateSource::RollingBackup), _) => selected,
            (_, CandidateSource::RollingBackup) => Some(&candidate.source),
            (Some(CandidateSource::Protected(old)), CandidateSource::Protected(new)) => {
                let key = |name: &ProtectedBackupName| {
                    (
                        name.sequence(),
                        name.source_schema(),
                        name.fingerprint().value(),
                    )
                };
                if key(new) > key(old) {
                    Some(&candidate.source)
                } else {
                    selected
                }
            }
        };
    }
    selected
        .cloned()
        .map_or(CandidateSelection::NoneValid, CandidateSelection::Restore)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backup(sequence: u32) -> ProtectedBackupName {
        ProtectedBackupName::new(sequence, 3, ChangeFingerprint::from_value(0)).unwrap()
    }

    #[test]
    fn fingerprints() {
        assert_eq!(ChangeFingerprint::of(b"").value(), 0xcbf29ce484222325);
        assert_eq!(ChangeFingerprint::of(b"a").value(), 0xaf63dc4c8601ec8c);
        assert_eq!(ChangeFingerprint::of(b"foobar").value(), 0x85944171f73967e8);
        assert_ne!(
            ChangeFingerprint::of(b"foobar"),
            ChangeFingerprint::of(b"foobas")
        );
    }

    #[test]
    fn names_round_trip() {
        for (sequence, schema, fingerprint) in [
            (1, 0, 0),
            (MAX_PROTECTED_SEQUENCE, u32::MAX, u64::MAX),
            (42, 3, 0x00a1b2c3d4e5f607),
        ] {
            let name = ProtectedBackupName::new(
                sequence,
                schema,
                ChangeFingerprint::from_value(fingerprint),
            )
            .unwrap();
            assert_eq!(
                (
                    name.sequence(),
                    name.source_schema(),
                    name.fingerprint().value()
                ),
                (sequence, schema, fingerprint)
            );
            assert_eq!(ProtectedBackupName::parse(&name.file_name()), Some(name));
        }
        assert_eq!(
            backup(1).file_name(),
            "config.protected.000001.v3.0000000000000000.bak"
        );
        assert!(ProtectedBackupName::new(0, 0, ChangeFingerprint::from_value(0)).is_none());
        assert!(
            ProtectedBackupName::new(
                MAX_PROTECTED_SEQUENCE + 1,
                0,
                ChangeFingerprint::from_value(0)
            )
            .is_none()
        );
    }

    #[test]
    fn rejects_noncanonical_names() {
        for name in [
            "config.toml.bak",
            "config.pre-migration.bak",
            "Config.protected.000001.v3.0000000000000000.bak",
            "config.Protected.000001.v3.0000000000000000.bak",
            "config.protected.000001.v3.0000000000000000.BAK",
            "config.protected.000001.V3.0000000000000000.bak",
            "config.protected.-00001.v3.0000000000000000.bak",
            "config.protected.000001.v3 .0000000000000000.bak",
            "config.protected.000001. v3.0000000000000000.bak",
            "config.protected.000001.v3.000000000000000A.bak",
            "config.protected.+00001.v3.0000000000000000.bak",
            "config.protected.0000001.v3.0000000000000000.bak",
            "config.protected.00001.v3.0000000000000000.bak",
            "config.protected.000000.v3.0000000000000000.bak",
            "config.protected.000001.v03.0000000000000000.bak",
            "config.protected.000001.v.0000000000000000.bak",
            "config.protected.000001.v4294967296.0000000000000000.bak",
            "config.protected.000001.v+3.0000000000000000.bak",
            "config.protected.000001.v-3.0000000000000000.bak",
            "config.protected.00000\u{FF11}.v3.0000000000000000.bak",
            "config.protected.000001.v\u{0663}.0000000000000000.bak",
            "config.protected.000001.v3.000000000000000.bak",
            "config.protected.000001.v3.00000000000000000.bak",
            "config.protected.000001.v3.000000000000000g.bak",
            "config.protected.000001.v3.0000000000000000.bak.",
            "config.protected.000001.v3.0000000000000000.bak ",
            " config.protected.000001.v3.0000000000000000.bak",
            "dir/config.protected.000001.v3.0000000000000000.bak",
            "dir\\config.protected.000001.v3.0000000000000000.bak",
            "config.protected.000001.v3.0000000000000000.txt",
            "config.protected.000001.v3.0000000000000000.extra.bak",
            "config.protected.000001.v3.bak",
        ] {
            assert!(
                ProtectedBackupName::parse(name).is_none(),
                "accepted {name:?}"
            );
        }
    }

    #[test]
    fn sequences_do_not_fill_gaps() {
        assert_eq!(next_sequence(&[]), Some(1));
        assert_eq!(next_sequence(&[backup(5), backup(1)]), Some(6));
        assert_eq!(
            next_sequence(&[backup(1), backup(MAX_PROTECTED_SEQUENCE)]),
            None
        );
    }

    #[test]
    fn retention_keeps_newest_distinct_names() {
        let names = vec![backup(5), backup(1), backup(3), backup(2), backup(4)];
        assert!(retention_excess(&names[..2], 3).is_empty());
        assert!(retention_excess(&names[..3], 3).is_empty());
        assert_eq!(retention_excess(&names, 3), vec![backup(1), backup(2)]);
        assert_eq!(
            retention_excess(&names, 0),
            vec![backup(1), backup(2), backup(3), backup(4)]
        );
        assert_eq!(
            retention_excess(&[backup(3), backup(1), backup(1), backup(2), backup(3)], 2),
            vec![backup(1)]
        );
        let other = ProtectedBackupName::new(3, 4, ChangeFingerprint::from_value(0)).unwrap();
        assert_eq!(
            retention_excess(&[other.clone(), backup(3)], 1),
            retention_excess(&[backup(3), other], 1)
        );
    }

    fn inventories(f: ChangeFingerprint) -> [ArtifactInventory; 7] {
        let other = ChangeFingerprint::from_value(2);
        [
            ArtifactInventory::default(),
            ArtifactInventory {
                rolling_backup: true,
                ..Default::default()
            },
            ArtifactInventory {
                protected_backups: vec![backup(1)],
                ..Default::default()
            },
            ArtifactInventory {
                leftover_drafts: 1,
                ..Default::default()
            },
            ArtifactInventory {
                recorded_failure: Some(f),
                ..Default::default()
            },
            ArtifactInventory {
                recorded_failure: Some(other),
                ..Default::default()
            },
            ArtifactInventory {
                rolling_backup: true,
                protected_backups: vec![backup(1)],
                leftover_drafts: 1,
                recorded_failure: Some(f),
            },
        ]
    }

    #[test]
    fn startup_decision_matrix() {
        use LegacyReason::{Invalid, RetrySuppressed, Unreadable};
        use RuntimeMode::{LegacyCompatible as Compatibility, Normal, ReadOnly, Recovery};
        use StartupOutcome::*;
        let f = ChangeFingerprint::from_value(1);
        let new = (NewInstall, Normal, true);
        let ready = (Ready, Normal, true);
        let migrate = (
            Migrate {
                from_schema: 2,
                source: f,
            },
            Normal,
            true,
        );
        let future = (FutureReadOnly { schema: 5 }, ReadOnly, false);
        let invalid = (LegacyCompatible { reason: Invalid }, Compatibility, false);
        let unreadable = (
            LegacyCompatible { reason: Unreadable },
            Compatibility,
            false,
        );
        let suppressed = (
            LegacyCompatible {
                reason: RetrySuppressed,
            },
            Compatibility,
            false,
        );
        let recover = (Recover, Recovery, false);
        // Columns: empty, rolling, protected, draft, matching failure, different failure, all.
        #[rustfmt::skip]
        let rows = [
            (PrimaryObservation::Missing, [ &new, &recover, &recover, &recover, &recover, &recover, &recover ]),
            (PrimaryObservation::Unreadable, [ &unreadable, &unreadable, &unreadable, &unreadable, &unreadable, &unreadable, &unreadable ]),
            (PrimaryObservation::Invalid { fingerprint: f }, [ &invalid, &invalid, &invalid, &invalid, &invalid, &invalid, &invalid ]),
            (PrimaryObservation::Future { schema: 5, fingerprint: f }, [ &future, &future, &future, &future, &future, &future, &future ]),
            (PrimaryObservation::Current { fingerprint: f }, [ &ready, &ready, &ready, &ready, &ready, &ready, &ready ]),
            (PrimaryObservation::Legacy { schema: 2, fingerprint: f }, [ &migrate, &migrate, &migrate, &migrate, &suppressed, &migrate, &suppressed ]),
        ];
        for (primary, expected) in rows {
            for (artifacts, (outcome, mode, writes)) in inventories(f).into_iter().zip(expected) {
                let actual = plan_startup(&primary, &artifacts);
                assert_eq!(&actual, outcome, "{primary:?} with {artifacts:?}");
                assert_eq!(&actual.mode(), mode, "{primary:?} with {artifacts:?}");
                assert_eq!(
                    actual.allows_writes(),
                    *writes,
                    "{primary:?} with {artifacts:?}"
                );
            }
        }
    }

    #[test]
    fn missing_with_only_recorded_failure_recovers() {
        let f = ChangeFingerprint::from_value(1);
        assert_eq!(
            plan_startup(&PrimaryObservation::Missing, &inventories(f)[4]),
            StartupOutcome::Recover
        );
    }

    #[test]
    fn future_with_matching_failure_is_read_only() {
        let f = ChangeFingerprint::from_value(1);
        assert_eq!(
            plan_startup(
                &PrimaryObservation::Future {
                    schema: 5,
                    fingerprint: f
                },
                &inventories(f)[4]
            ),
            StartupOutcome::FutureReadOnly { schema: 5 }
        );
    }

    #[test]
    fn invalid_with_matching_failure_uses_compatibility() {
        let f = ChangeFingerprint::from_value(1);
        assert_eq!(
            plan_startup(
                &PrimaryObservation::Invalid { fingerprint: f },
                &inventories(f)[4]
            ),
            StartupOutcome::LegacyCompatible {
                reason: LegacyReason::Invalid
            }
        );
    }

    #[test]
    fn invalid_with_artifacts_does_not_recover() {
        let f = ChangeFingerprint::from_value(1);
        assert_eq!(
            plan_startup(
                &PrimaryObservation::Invalid { fingerprint: f },
                &inventories(f)[6]
            ),
            StartupOutcome::LegacyCompatible {
                reason: LegacyReason::Invalid
            }
        );
    }

    #[test]
    fn legacy_with_different_failure_migrates() {
        let f = ChangeFingerprint::from_value(1);
        assert_eq!(
            plan_startup(
                &PrimaryObservation::Legacy {
                    schema: 2,
                    fingerprint: f
                },
                &inventories(f)[5]
            ),
            StartupOutcome::Migrate {
                from_schema: 2,
                source: f
            }
        );
    }

    #[test]
    fn legacy_with_matching_failure_and_all_artifacts_suppresses_retry() {
        let f = ChangeFingerprint::from_value(1);
        assert_eq!(
            plan_startup(
                &PrimaryObservation::Legacy {
                    schema: 2,
                    fingerprint: f
                },
                &inventories(f)[6]
            ),
            StartupOutcome::LegacyCompatible {
                reason: LegacyReason::RetrySuppressed
            }
        );
    }

    #[test]
    fn recovery_artifacts_include_each_single_field() {
        let inventories = inventories(ChangeFingerprint::from_value(1));
        assert!(!inventories[0].has_recovery_artifacts());
        for inventory in &inventories[1..=5] {
            assert!(inventory.has_recovery_artifacts());
        }
    }

    #[test]
    fn non_normal_outcomes_disallow_writes() {
        for outcome in [
            StartupOutcome::FutureReadOnly { schema: 5 },
            StartupOutcome::LegacyCompatible {
                reason: LegacyReason::Invalid,
            },
            StartupOutcome::LegacyCompatible {
                reason: LegacyReason::Unreadable,
            },
            StartupOutcome::LegacyCompatible {
                reason: LegacyReason::RetrySuppressed,
            },
            StartupOutcome::Recover,
        ] {
            assert_ne!(outcome.mode(), RuntimeMode::Normal);
            assert!(!outcome.allows_writes());
        }
    }

    fn record(source: u64, from_schema: u32, stage: FailureStage, attempts: u32) -> FailureRecord {
        FailureRecord {
            source: ChangeFingerprint::from_value(source),
            from_schema,
            stage,
            attempts,
        }
    }

    #[test]
    fn failure_record_encoding_and_round_trips() {
        let example = record(0xabc, 3, FailureStage::Commit, 2);
        assert_eq!(
            example.encode(),
            "islora-migration-failure 1\nsource=0000000000000abc\nfrom_schema=3\nstage=commit\nattempts=2\n"
        );
        for item in [
            example,
            record(0, 0, FailureStage::Backup, 1),
            record(u64::MAX, u32::MAX, FailureStage::Verify, u32::MAX),
        ] {
            let encoded = item.encode();
            assert_eq!(FailureRecord::decode(encoded.as_bytes()), Ok(item));
            assert_eq!(
                FailureRecord::decode(encoded.as_bytes()).unwrap().encode(),
                encoded
            );
        }
    }

    #[test]
    fn failure_record_rejects_noncanonical_input() {
        use FailureRecordError::*;
        let good = record(10, 3, FailureStage::Backup, 1).encode();
        assert_eq!(FailureRecord::decode(&[b'x'; 257]), Err(TooLarge));
        assert_eq!(FailureRecord::decode(&[b'x'; 256]), Err(Malformed));
        assert_eq!(FailureRecord::decode(&[0xff; 257]), Err(TooLarge));
        assert_eq!(
            FailureRecord::decode(b"islora-migration-failure 2\n\xff\n"),
            Err(NotUtf8)
        );
        assert_eq!(
            FailureRecord::decode(b"islora-migration-failure 2\nsource=zz\n"),
            Err(UnsupportedVersion)
        );
        assert_eq!(FailureRecord::decode(&[0xff]), Err(NotUtf8));
        assert_eq!(
            FailureRecord::decode(good.replace("failure 1", "failure 2").as_bytes()),
            Err(UnsupportedVersion)
        );
        assert_eq!(
            FailureRecord::decode(good.replace("failure 1", "failure 01").as_bytes()),
            Err(UnsupportedVersion)
        );
        for bad in [
            good.replace('\n', "\r\n"),
            good.trim_end().to_owned(),
            format!("{good}extra\n"),
            good.replace("stage=backup\n", ""),
            good.replace("from_schema=3\nstage=backup", "stage=backup\nfrom_schema=3"),
            good.replace("stage=backup", "stage=backup\nstage=backup"),
            good.replace("stage=backup", "theme=dark"),
            good.replace("source=", "source ="),
            good.replace("000000000000000a", "000000000000000A"),
            good.replace("from_schema=3", "from_schema=+3"),
            good.replace("attempts=1", "attempts=-1"),
            good.replace("from_schema=3", "from_schema=03"),
            good.replace("attempts=1", "attempts=01"),
            good.replace("attempts=1", "attempts=0"),
            good.replace("from_schema=3", "from_schema=4294967296"),
            good.replace("attempts=1", "attempts=4294967296"),
            format!("\u{feff}{good}"),
            good.replace("from_schema=3", "from_schema=\u{0663}"),
            good.replace("stage=backup", "stage=other"),
        ] {
            assert_eq!(
                FailureRecord::decode(bad.as_bytes()),
                Err(Malformed),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn failure_debug_does_not_echo_input() {
        for error in [
            FailureRecordError::TooLarge,
            FailureRecordError::NotUtf8,
            FailureRecordError::Malformed,
            FailureRecordError::UnsupportedVersion,
        ] {
            assert!(!format!("{error:?}").contains("dark"));
        }
        let bad = record(0, 0, FailureStage::Backup, 1)
            .encode()
            .replace("stage=backup", "theme=dark");
        assert!(
            !format!("{:?}", FailureRecord::decode(bad.as_bytes()).unwrap_err()).contains("dark")
        );
        let decoded =
            FailureRecord::decode(record(0, 0, FailureStage::Backup, 1).encode().as_bytes())
                .unwrap();
        assert!(!format!("{decoded:?}").contains("dark"));
    }

    #[test]
    fn retry_decisions_match_startup() {
        let f = ChangeFingerprint::from_value(1);
        assert_eq!(
            retry_decision(&StoredFailure::None, f),
            RetryDecision::NoFailureRecorded
        );
        assert_eq!(
            retry_decision(&StoredFailure::Corrupt, f),
            RetryDecision::Suppressed
        );
        for source in [1, 2] {
            let failure = record(source, 2, FailureStage::Verify, 1);
            let decision = retry_decision(&StoredFailure::Valid(failure.clone()), f);
            assert_eq!(
                decision,
                if source == 1 {
                    RetryDecision::Suppressed
                } else {
                    RetryDecision::SourceChanged
                }
            );
            let outcome = plan_startup(
                &PrimaryObservation::Legacy {
                    schema: 2,
                    fingerprint: f,
                },
                &ArtifactInventory {
                    recorded_failure: Some(failure.source),
                    ..Default::default()
                },
            );
            assert_eq!(
                matches!(
                    outcome,
                    StartupOutcome::LegacyCompatible {
                        reason: LegacyReason::RetrySuppressed
                    }
                ),
                !decision.allows_attempt()
            );
        }
        assert!(RetryDecision::NoFailureRecorded.allows_attempt());
        assert!(RetryDecision::SourceChanged.allows_attempt());
        assert!(!RetryDecision::Suppressed.allows_attempt());
    }

    #[test]
    fn recovery_candidate_selection() {
        let valid = |source, fingerprint| RecoveryCandidate {
            source,
            validity: CandidateValidity::Valid {
                schema: 3,
                fingerprint: ChangeFingerprint::from_value(fingerprint),
            },
        };
        let rolling = valid(CandidateSource::RollingBackup, 1);
        let low = valid(CandidateSource::Protected(backup(1)), 1);
        let high = valid(CandidateSource::Protected(backup(2)), 1);
        let other = valid(CandidateSource::Protected(backup(3)), 2);
        let future = RecoveryCandidate {
            source: CandidateSource::Protected(backup(4)),
            validity: CandidateValidity::Future,
        };
        let invalid = RecoveryCandidate {
            source: CandidateSource::Protected(backup(5)),
            validity: CandidateValidity::Invalid,
        };
        let unreadable = RecoveryCandidate {
            source: CandidateSource::Protected(backup(6)),
            validity: CandidateValidity::Unreadable,
        };
        for (mut candidates, expected) in [
            (vec![], CandidateSelection::NoneValid),
            (
                vec![future.clone(), invalid, unreadable],
                CandidateSelection::NoneValid,
            ),
            (
                vec![low.clone()],
                CandidateSelection::Restore(low.source.clone()),
            ),
            (
                vec![low.clone(), rolling.clone()],
                CandidateSelection::Restore(CandidateSource::RollingBackup),
            ),
            (
                vec![low.clone(), high.clone()],
                CandidateSelection::Restore(high.source.clone()),
            ),
            (vec![low.clone(), other], CandidateSelection::Ambiguous),
            (
                vec![low.clone(), future],
                CandidateSelection::Restore(low.source.clone()),
            ),
        ] {
            assert_eq!(select_candidate(&candidates), expected);
            candidates.reverse();
            assert_eq!(select_candidate(&candidates), expected);
        }
        let tie = valid(
            CandidateSource::Protected(
                ProtectedBackupName::new(1, 4, ChangeFingerprint::from_value(0)).unwrap(),
            ),
            1,
        );
        assert_eq!(
            select_candidate(&[low.clone(), tie.clone()]),
            select_candidate(&[tie, low])
        );
    }
}
