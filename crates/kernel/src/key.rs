use std::fmt;
use std::str::FromStr;

use serde::de::{Deserializer, Error as _};
use serde::{Deserialize, Serialize, Serializer};

const KEY_BYTES: usize = 32;

/// A 32-byte digest, written as 64 lowercase hexadecimal characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key([u8; KEY_BYTES]);

/// The text was not 64 lowercase hexadecimal characters.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a key is 64 lowercase hexadecimal characters")]
pub struct KeyParseError;

impl Key {
    /// Return the key that holds `bytes`.
    #[must_use]
    pub const fn new(bytes: [u8; KEY_BYTES]) -> Self {
        Self(bytes)
    }

    /// Return the digest bytes.
    #[must_use]
    pub const fn bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl FromStr for Key {
    type Err = KeyParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let digits = text.as_bytes();
        if digits.len() != KEY_BYTES * 2 {
            return Err(KeyParseError);
        }
        let mut bytes = [0; KEY_BYTES];
        for (byte, pair) in bytes.iter_mut().zip(digits.chunks_exact(2)) {
            *byte = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Ok(Self(bytes))
    }
}

const fn nibble(digit: u8) -> Result<u8, KeyParseError> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        _ => Err(KeyParseError),
    }
}

impl Serialize for Key {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::{Key, KeyParseError};

    // sha256 of the empty string, as Python's hashlib.sha256(b"").hexdigest() writes it.
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn a_key_writes_and_reads_lowercase_hex() {
        let key: Key = EMPTY.parse().unwrap();
        assert_eq!(key.bytes()[..2], [0xe3, 0xb0]);
        assert_eq!(key.to_string(), EMPTY);
    }

    #[test]
    fn a_key_round_trips_as_a_json_string() {
        let key: Key = EMPTY.parse().unwrap();
        let text = serde_json::to_string(&key).unwrap();
        assert_eq!(text, format!("\"{EMPTY}\""));
        assert_eq!(serde_json::from_str::<Key>(&text).unwrap(), key);
    }

    #[test]
    fn a_key_reads_from_an_owned_json_value() {
        let key = serde_json::from_value::<Key>(serde_json::json!(EMPTY)).unwrap();
        assert_eq!(key, EMPTY.parse::<Key>().unwrap());
        let refused = serde_json::from_value::<Key>(serde_json::json!(EMPTY.to_uppercase()));
        assert_eq!(refused.unwrap_err().to_string(), KeyParseError.to_string());
    }

    #[test]
    fn a_key_reads_from_a_json_reader() {
        let text = format!("\"{EMPTY}\"");
        let key = serde_json::from_reader::<_, Key>(text.as_bytes()).unwrap();
        assert_eq!(key, EMPTY.parse::<Key>().unwrap());
    }

    #[test]
    fn a_key_refuses_the_wrong_length_uppercase_and_non_hex() {
        let uppercase = EMPTY.to_uppercase();
        let not_hex = EMPTY.replace('e', "g");
        for text in [&EMPTY[..62], uppercase.as_str(), not_hex.as_str()] {
            assert_eq!(text.parse::<Key>(), Err(KeyParseError));
        }
    }
}
