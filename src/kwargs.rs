//! Parsing for the string-valued device arguments the SoapySDR plugin
//! and the seify backend accept.
//!
//! Both front-ends take their options as `key=value` strings. One parser
//! serves both so the same spelling means the same thing in each: were
//! `listen_only=yes` listen-only through SoapySDR and not through seify,
//! seify would retune a device the caller asked it to leave alone. A
//! value the parser does not know is an error rather than a guess in
//! either direction.

use crate::{Error, Result};

/// Parse the boolean device argument `key` from `value`.
///
/// Accepts `true`/`1`/`yes`/`on` and `false`/`0`/`no`/`off`, ignoring
/// ASCII case and surrounding whitespace. Anything else — the empty
/// string included — is [`Error::Config`] naming the key and the value.
pub fn parse_bool(key: &str, value: &str) -> Result<bool> {
    let trimmed = value.trim();
    const TRUE: [&str; 4] = ["true", "1", "yes", "on"];
    const FALSE: [&str; 4] = ["false", "0", "no", "off"];
    if TRUE.iter().any(|t| trimmed.eq_ignore_ascii_case(t)) {
        Ok(true)
    } else if FALSE.iter().any(|f| trimmed.eq_ignore_ascii_case(f)) {
        Ok(false)
    } else {
        Err(Error::Config(format!(
            "device argument {key}={value:?} is not a boolean; \
             use true/1/yes/on or false/0/no/off"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_spellings_parse_in_any_case() {
        for value in ["true", "1", "yes", "on", "True", "YES", " on ", "TRUE"] {
            assert!(parse_bool("listen_only", value).unwrap(), "{value:?}");
        }
        for value in ["false", "0", "no", "off", "False", "NO", " off\t"] {
            assert!(!parse_bool("listen_only", value).unwrap(), "{value:?}");
        }
    }

    /// Neither direction is a safe guess: an unknown `listen_only` read
    /// as false retunes a shared device, and an unknown `tx` read as
    /// true opens a transmitter.
    #[test]
    fn an_unknown_value_is_an_error_naming_the_argument() {
        for value in ["", " ", "maybe", "2", "y", "enable", "truee", "-1"] {
            let err = parse_bool("tx", value).expect_err(value);
            let text = err.to_string();
            assert!(matches!(err, Error::Config(_)), "{value:?}");
            assert!(text.contains("tx="), "{text}");
        }
    }
}
