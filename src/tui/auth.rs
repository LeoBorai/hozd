//! The request Auth tab: what goes into the `Authorization` header.
//!
//! The editor only keeps the pieces (user and password, a JWT and its secret, or free
//! text); [`AuthEditor::header_value`] puts them together when the request is built, and
//! [`AuthEditor::from_header`] takes a stored `Authorization` header apart again.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui_textarea::{Input, TextArea};

use crate::client::{
    Jwt, JwtError, Verification, basic_value, parse_basic, parse_token, secret_bytes, token_value,
};

use super::headers::single_line;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    None,
    Basic,
    Jwt,
    /// The header value as free text.
    Raw,
}

impl Scheme {
    pub const ALL: [Scheme; 4] = [Scheme::None, Scheme::Basic, Scheme::Jwt, Scheme::Raw];

    pub fn label(self) -> &'static str {
        match self {
            Scheme::None => "None",
            Scheme::Basic => "Basic",
            Scheme::Jwt => "JWT",
            Scheme::Raw => "Free text",
        }
    }

    fn next(self) -> Scheme {
        let i = Scheme::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Scheme::ALL[(i + 1) % Scheme::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    User,
    Password,
    Prefix,
    Token,
    Secret,
    /// The "base64url encoded" switch of the secret.
    Encoded,
    Raw,
}

/// One line under the token: a mark and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub state: CheckState,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Pass,
    Fail,
    /// Nothing to check yet.
    Idle,
}

impl Check {
    fn new(state: CheckState, text: impl Into<String>) -> Self {
        Self {
            state,
            text: text.into(),
        }
    }
}

/// The three checks of the JWT layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checks {
    pub valid_jwt: Check,
    pub signature: Check,
    pub secret: Check,
}

pub struct AuthEditor {
    pub scheme: Scheme,
    pub field: Field,
    pub user: TextArea<'static>,
    pub password: TextArea<'static>,
    pub prefix: TextArea<'static>,
    pub token: TextArea<'static>,
    pub secret: TextArea<'static>,
    /// Whether the secret is base64url encoded.
    pub encoded: bool,
    pub raw: TextArea<'static>,
}

impl Default for AuthEditor {
    fn default() -> Self {
        Self {
            scheme: Scheme::None,
            field: Field::User,
            user: single_line(""),
            password: single_line(""),
            prefix: single_line("Bearer"),
            token: single_line(""),
            secret: single_line(""),
            encoded: false,
            raw: single_line(""),
        }
    }
}

fn text(ta: &TextArea<'static>) -> String {
    ta.lines().join("")
}

impl AuthEditor {
    /// The editor for a stored `Authorization` value: Basic and JWT values are taken
    /// apart, anything else is kept as free text.
    pub fn from_header(value: &str) -> Self {
        let mut editor = Self::default();
        if let Some((user, password)) = parse_basic(value) {
            editor.scheme = Scheme::Basic;
            editor.user = single_line(&user);
            editor.password = single_line(&password);
        } else if let Some((prefix, token)) = parse_token(value) {
            editor.scheme = Scheme::Jwt;
            editor.prefix = single_line(&prefix);
            editor.token = single_line(&token);
        } else {
            editor.scheme = Scheme::Raw;
            editor.raw = single_line(value.trim());
        }
        editor.field = editor.fields()[0];
        editor
    }

    /// The `Authorization` value to send; `None` when nothing is set.
    pub fn header_value(&self) -> Option<String> {
        let value = match self.scheme {
            Scheme::None => return None,
            Scheme::Basic => {
                let (user, password) = (text(&self.user), text(&self.password));
                if user.is_empty() && password.is_empty() {
                    return None;
                }
                basic_value(&user, &password)
            }
            Scheme::Jwt => {
                let token = text(&self.token);
                if token.trim().is_empty() {
                    return None;
                }
                token_value(&text(&self.prefix), &token)
            }
            Scheme::Raw => text(&self.raw).trim().to_owned(),
        };
        (!value.is_empty()).then_some(value)
    }

    /// The inputs of the selected scheme, top to bottom.
    pub fn fields(&self) -> &'static [Field] {
        match self.scheme {
            Scheme::None => &[],
            Scheme::Basic => &[Field::User, Field::Password],
            Scheme::Jwt => &[Field::Prefix, Field::Token, Field::Secret, Field::Encoded],
            Scheme::Raw => &[Field::Raw],
        }
    }

    pub fn set_scheme(&mut self, scheme: Scheme) {
        self.scheme = scheme;
        if let Some(first) = self.fields().first() {
            self.field = *first;
        }
    }

    /// The input under the cursor; `None` on the switch.
    pub fn input(&self, field: Field) -> Option<&TextArea<'static>> {
        match field {
            Field::User => Some(&self.user),
            Field::Password => Some(&self.password),
            Field::Prefix => Some(&self.prefix),
            Field::Token => Some(&self.token),
            Field::Secret => Some(&self.secret),
            Field::Raw => Some(&self.raw),
            Field::Encoded => None,
        }
    }

    fn input_mut(&mut self, field: Field) -> Option<&mut TextArea<'static>> {
        match field {
            Field::User => Some(&mut self.user),
            Field::Password => Some(&mut self.password),
            Field::Prefix => Some(&mut self.prefix),
            Field::Token => Some(&mut self.token),
            Field::Secret => Some(&mut self.secret),
            Field::Raw => Some(&mut self.raw),
            Field::Encoded => None,
        }
    }

    /// Selects `field` with the cursor `x` characters in.
    pub fn select(&mut self, field: Field, x: usize) {
        if !self.fields().contains(&field) {
            return;
        }
        self.field = field;
        if let Some(ta) = self.input_mut(field) {
            ta.move_cursor(ratatui_textarea::CursorMove::Jump(0, x as u16));
        } else {
            self.encoded = !self.encoded;
        }
    }

    fn step(&mut self, delta: isize) {
        let fields = self.fields();
        let Some(at) = fields.iter().position(|f| *f == self.field) else {
            return;
        };
        let to = at as isize + delta;
        if (0..fields.len() as isize).contains(&to) {
            self.field = fields[to as usize];
        }
    }

    pub fn key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('r') if ctrl => self.set_scheme(self.scheme.next()),
            KeyCode::Down | KeyCode::Enter => self.step(1),
            KeyCode::Up => self.step(-1),
            KeyCode::Char(' ') if self.field == Field::Encoded => self.encoded = !self.encoded,
            _ => {
                let field = self.field;
                if let Some(ta) = self.input_mut(field) {
                    ta.input(Input::from(key));
                    // a pasted token is one line, however it was wrapped
                    if ta.lines().len() > 1 {
                        let joined: String = ta
                            .lines()
                            .iter()
                            .flat_map(|l| l.split_whitespace())
                            .collect();
                        *ta = single_line(&joined);
                    }
                }
            }
        }
    }

    /// The decoded token, `None` while the token input is empty.
    pub fn jwt(&self) -> Option<Result<Jwt, JwtError>> {
        let token = text(&self.token);
        (!token.trim().is_empty()).then(|| Jwt::parse(&token))
    }

    /// What the token and the secret say about each other.
    pub fn checks(&self) -> Checks {
        use CheckState::*;
        let (valid_jwt, jwt) = match self.jwt() {
            None => (Check::new(Idle, "Paste a JWT"), None),
            Some(Err(e)) => (Check::new(Fail, format!("Invalid JWT: {e}")), None),
            Some(Ok(jwt)) => (Check::new(Pass, "Valid JWT"), Some(jwt)),
        };
        let secret = text(&self.secret);
        let (signature, secret_check) = match jwt {
            None => (
                Check::new(Idle, "Signature not verified"),
                Check::new(Idle, "Secret not checked"),
            ),
            Some(_) if secret.is_empty() => (
                Check::new(Idle, "Signature not verified"),
                Check::new(Idle, "Enter the secret"),
            ),
            Some(jwt) => match secret_bytes(&secret, self.encoded) {
                None => (
                    Check::new(Fail, "Signature not verified"),
                    Check::new(Fail, "Invalid secret: not base64url"),
                ),
                Some(bytes) => match jwt.verify(&bytes) {
                    Verification::Verified => (
                        Check::new(Pass, "Signature Verified"),
                        Check::new(Pass, "Valid secret"),
                    ),
                    Verification::Mismatch => (
                        Check::new(Fail, "Signature not verified"),
                        Check::new(Fail, "Invalid secret"),
                    ),
                    Verification::BadKey => (
                        Check::new(Fail, "Signature not verified"),
                        Check::new(Fail, "Invalid key: expected an RSA public key in PEM"),
                    ),
                    Verification::Unsupported(alg) => (
                        Check::new(Idle, format!("Signature not verified: {alg} unsupported")),
                        Check::new(Idle, "Secret not checked"),
                    ),
                },
            },
        };
        Checks {
            valid_jwt,
            signature,
            secret: secret_check,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiYWRtaW4iOnRydWUsImlhdCI6MTUxNjIzOTAyMn0.KMUFsIDTnFmyG3nMiGM6H9FNFUROf3wh7SmqJp-QV30";

    #[test]
    fn nothing_set_sends_nothing() {
        assert_eq!(AuthEditor::default().header_value(), None);
    }

    #[test]
    fn stored_values_pick_their_scheme() {
        let basic = AuthEditor::from_header("Basic dTpw");
        assert_eq!(basic.scheme, Scheme::Basic);
        assert_eq!(basic.header_value().as_deref(), Some("Basic dTpw"));

        let jwt = AuthEditor::from_header(&format!("Bearer {TOKEN}"));
        assert_eq!(jwt.scheme, Scheme::Jwt);
        assert_eq!(text(&jwt.prefix), "Bearer");

        let raw = AuthEditor::from_header("Digest abc");
        assert_eq!(raw.scheme, Scheme::Raw);
        assert_eq!(raw.header_value().as_deref(), Some("Digest abc"));
    }

    #[test]
    fn checks_follow_the_secret() {
        let mut editor = AuthEditor::from_header(&format!("Bearer {TOKEN}"));
        assert_eq!(editor.checks().valid_jwt.state, CheckState::Pass);
        assert_eq!(editor.checks().signature.state, CheckState::Idle);
        editor.secret = single_line("a-string-secret-at-least-256-bits-long");
        assert_eq!(editor.checks().signature.state, CheckState::Pass);
        assert_eq!(editor.checks().secret.state, CheckState::Pass);
        editor.secret = single_line("nope");
        assert_eq!(editor.checks().signature.state, CheckState::Fail);
        assert_eq!(editor.checks().secret.text, "Invalid secret");
    }
}
