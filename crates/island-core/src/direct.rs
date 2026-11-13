//! A Wi-Fi network of the island's own, for a phone that is not on the same
//! network as the computer. The phone joins it by scanning a code, as it would
//! join any network, and needs no app for it.

/// Letters that read without doubt, in either case: no 0 and O, no 1, i and l.
const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
const NAME_LETTERS: usize = 4;
/// Longer than the 8 that WPA2 asks for. Nobody types it: the code carries it.
const PASSPHRASE_LETTERS: usize = 12;

fn letters(random: &[u8], count: usize) -> String {
    random
        .iter()
        .take(count)
        .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()] as char)
        .collect()
}

/// Such as `Islora-K4FX`, different each time so that an old network on the
/// phone never stands in for the new one.
pub fn network_name(random: &[u8]) -> String {
    format!("Islora-{}", letters(random, NAME_LETTERS).to_uppercase())
}

pub fn passphrase(random: &[u8]) -> String {
    letters(random, PASSPHRASE_LETTERS)
}

/// Random bytes enough for a name and a passphrase.
pub const RANDOM_BYTES: usize = NAME_LETTERS + PASSPHRASE_LETTERS;

/// What a code says for a phone camera to offer joining a network.
pub fn join_code_text(network: &str, passphrase: &str) -> String {
    format!(
        "WIFI:T:WPA;S:{};P:{};;",
        escape(network),
        escape(passphrase)
    )
}

/// The format sets these apart with a backslash.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(character, '\\' | ';' | ',' | ':' | '"') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_passphrases_use_only_clear_letters() {
        let random: Vec<u8> = (0..=255).collect();
        let name = network_name(&random);
        assert!(name.starts_with("Islora-") && name.len() == "Islora-".len() + NAME_LETTERS);
        let secret = passphrase(&random[100..]);
        assert_eq!(secret.len(), PASSPHRASE_LETTERS);
        assert!(secret.bytes().all(|letter| ALPHABET.contains(&letter)));
        assert!(!secret.contains(['0', 'o', '1', 'i', 'l']));
    }

    #[test]
    fn a_join_code_is_what_phone_cameras_read() {
        assert_eq!(
            join_code_text("Islora-K4FX", "abcdefgh2345"),
            "WIFI:T:WPA;S:Islora-K4FX;P:abcdefgh2345;;"
        );
        assert_eq!(
            join_code_text("a;b", "c:d\"e"),
            "WIFI:T:WPA;S:a\\;b;P:c\\:d\\\"e;;"
        );
    }
}
