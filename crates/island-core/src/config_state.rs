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
pub fn retention_excess(existing: &[ProtectedBackupName], keep: usize) -> Vec<ProtectedBackupName> {
    let mut names = existing.to_vec();
    names
        .sort_unstable_by_key(|name| (name.sequence, name.source_schema, name.fingerprint.value()));
    names.dedup();
    names.truncate(names.len().saturating_sub(keep.max(1)));
    names
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
            "config.protected.00000１.v3.0000000000000000.bak",
            "config.protected.000001.v٣.0000000000000000.bak",
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
}
