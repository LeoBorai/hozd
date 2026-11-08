//! Authorization header values: HTTP Basic and JWT (decoded and verified locally).

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Sha256, Sha384, Sha512};

/// `Basic <base64(user:password)>`, RFC 7617.
pub fn basic_value(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
}

/// The `(user, password)` of a `Basic` credential; `None` when `value` is anything else.
pub fn parse_basic(value: &str) -> Option<(String, String)> {
    let (scheme, encoded) = value.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = STANDARD.decode(encoded.trim()).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, password) = text.split_once(':')?;
    Some((user.to_owned(), password.to_owned()))
}

/// Why a text is not a JWT.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JwtError {
    #[error("a JWT has 3 dot-separated parts, found {0}")]
    Parts(usize),
    #[error("the {0} is not base64url")]
    Encoding(&'static str),
    #[error("the {0} is not a JSON object")]
    Json(&'static str),
}

/// What checking the signature found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    /// The signature matches the secret.
    Verified,
    /// The signature does not match the secret.
    Mismatch,
    /// The algorithm needs a key this check cannot use (RS256, ES256, none, ...).
    Unsupported(String),
}

/// A decoded JWT. Nothing is trusted: this only reads the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jwt {
    pub header: serde_json::Value,
    pub payload: serde_json::Value,
    signing_input: String,
    signature: Vec<u8>,
}

impl Jwt {
    pub fn parse(token: &str) -> Result<Self, JwtError> {
        let token = token.trim();
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(JwtError::Parts(parts.len()));
        }
        let object = |part: &str, what: &'static str| {
            let bytes = URL_SAFE_NO_PAD
                .decode(part)
                .map_err(|_| JwtError::Encoding(what))?;
            match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(v @ serde_json::Value::Object(_)) => Ok(v),
                _ => Err(JwtError::Json(what)),
            }
        };
        let header = object(parts[0], "header")?;
        let payload = object(parts[1], "payload")?;
        let signature = URL_SAFE_NO_PAD
            .decode(parts[2])
            .map_err(|_| JwtError::Encoding("signature"))?;
        Ok(Self {
            header,
            payload,
            signing_input: format!("{}.{}", parts[0], parts[1]),
            signature,
        })
    }

    /// The `alg` of the header, empty when missing.
    pub fn algorithm(&self) -> &str {
        self.header
            .get("alg")
            .and_then(|a| a.as_str())
            .unwrap_or("")
    }

    pub fn header_json(&self) -> String {
        serde_json::to_string_pretty(&self.header).unwrap_or_default()
    }

    pub fn payload_json(&self) -> String {
        serde_json::to_string_pretty(&self.payload).unwrap_or_default()
    }

    /// Checks the signature against `secret` (HS256, HS384 and HS512).
    pub fn verify(&self, secret: &[u8]) -> Verification {
        let input = self.signing_input.as_bytes();
        let ok = match self.algorithm() {
            "HS256" => hmac_matches::<Sha256>(secret, input, &self.signature),
            "HS384" => hmac_matches::<Sha384>(secret, input, &self.signature),
            "HS512" => hmac_matches::<Sha512>(secret, input, &self.signature),
            other => return Verification::Unsupported(other.to_owned()),
        };
        if ok {
            Verification::Verified
        } else {
            Verification::Mismatch
        }
    }
}

fn hmac_matches<D: hmac::digest::block_api::EagerHash>(
    secret: &[u8],
    input: &[u8],
    signature: &[u8],
) -> bool {
    let Ok(mut mac) = Hmac::<D>::new_from_slice(secret) else {
        return false;
    };
    mac.update(input);
    mac.verify_slice(signature).is_ok()
}

/// The secret as bytes: the text itself, or decoded from base64url when `base64url`.
pub fn secret_bytes(text: &str, base64url: bool) -> Option<Vec<u8>> {
    if base64url {
        URL_SAFE_NO_PAD
            .decode(text.trim().trim_end_matches('='))
            .ok()
    } else {
        Some(text.as_bytes().to_vec())
    }
}

/// `prefix token`, or the token alone without a prefix.
pub fn token_value(prefix: &str, token: &str) -> String {
    let (prefix, token) = (prefix.trim(), token.trim());
    if prefix.is_empty() {
        token.to_owned()
    } else {
        format!("{prefix} {token}")
    }
}

/// Splits an `Authorization` value into its prefix and a decodable JWT.
pub fn parse_token(value: &str) -> Option<(String, String)> {
    let value = value.trim();
    let (prefix, token) = match value.split_once(' ') {
        Some((prefix, token)) => (prefix, token.trim()),
        None => ("", value),
    };
    Jwt::parse(token).ok()?;
    Some((prefix.to_owned(), token.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // the jwt.io example, secret `a-string-secret-at-least-256-bits-long`
    const TOKEN: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiYWRtaW4iOnRydWUsImlhdCI6MTUxNjIzOTAyMn0.KMUFsIDTnFmyG3nMiGM6H9FNFUROf3wh7SmqJp-QV30";
    const SECRET: &str = "a-string-secret-at-least-256-bits-long";

    #[test]
    fn basic_round_trips() {
        let value = basic_value("Aladdin", "open sesame");
        assert_eq!(value, "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
        assert_eq!(
            parse_basic(&value),
            Some(("Aladdin".into(), "open sesame".into()))
        );
        assert_eq!(parse_basic("Bearer abc"), None);
        assert_eq!(parse_basic("Basic !!!"), None);
    }

    #[test]
    fn basic_password_may_contain_colons() {
        let value = basic_value("u", "a:b");
        assert_eq!(parse_basic(&value), Some(("u".into(), "a:b".into())));
    }

    #[test]
    fn decodes_header_and_payload() {
        let jwt = Jwt::parse(TOKEN).unwrap();
        assert_eq!(jwt.algorithm(), "HS256");
        assert_eq!(jwt.payload["name"], "John Doe");
        assert!(jwt.header_json().contains("\"typ\": \"JWT\""));
    }

    #[test]
    fn verifies_the_signature() {
        let jwt = Jwt::parse(TOKEN).unwrap();
        assert_eq!(jwt.verify(SECRET.as_bytes()), Verification::Verified);
        assert_eq!(jwt.verify(b"wrong"), Verification::Mismatch);
    }

    #[test]
    fn other_algorithms_are_unsupported() {
        // {"alg":"RS256"}.{}.sig
        let token = "eyJhbGciOiJSUzI1NiJ9.e30.c2ln";
        let jwt = Jwt::parse(token).unwrap();
        assert_eq!(jwt.verify(b"k"), Verification::Unsupported("RS256".into()));
    }

    #[test]
    fn rejects_what_is_not_a_jwt() {
        assert_eq!(Jwt::parse("abc"), Err(JwtError::Parts(1)));
        assert_eq!(
            Jwt::parse("bm90anNvbg.e30.c2ln"),
            Err(JwtError::Json("header"))
        );
        assert_eq!(Jwt::parse("!.b.c"), Err(JwtError::Encoding("header")));
    }

    #[test]
    fn base64url_secret_is_decoded() {
        assert_eq!(secret_bytes("c2VjcmV0", true), Some(b"secret".to_vec()));
        assert_eq!(secret_bytes("***", true), None);
        assert_eq!(secret_bytes("c2VjcmV0", false), Some(b"c2VjcmV0".to_vec()));
    }

    #[test]
    fn token_values_round_trip() {
        let value = token_value("Bearer", TOKEN);
        assert_eq!(
            parse_token(&value),
            Some(("Bearer".into(), TOKEN.to_owned()))
        );
        assert_eq!(token_value("", TOKEN), TOKEN);
        assert_eq!(parse_token(TOKEN), Some((String::new(), TOKEN.to_owned())));
        assert_eq!(parse_token("Bearer opaque"), None);
    }
}
