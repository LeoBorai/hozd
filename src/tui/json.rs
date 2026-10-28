use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Key,
    String,
    Number,
    Literal,
    Punct,
    Plain,
}

impl Kind {
    pub fn style(self) -> Style {
        match self {
            Kind::Key => Style::new().fg(Color::Cyan),
            Kind::String => Style::new().fg(Color::Green),
            Kind::Number => Style::new().fg(Color::Yellow),
            Kind::Literal => Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            Kind::Punct => Style::new().fg(Color::DarkGray),
            Kind::Plain => Style::new(),
        }
    }
}

/// Splits one line of (possibly invalid) JSON into styled tokens. Concatenating the
/// token texts always yields the input line, so invalid JSON just renders plain.
pub fn tokens(line: &str) -> Vec<(Kind, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<(Kind, String)> = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        let start = i;
        let kind = match c {
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
                i = (i + 1).min(chars.len());
                let next = chars[i..].iter().find(|c| !c.is_whitespace());
                if next == Some(&':') {
                    Kind::Key
                } else {
                    Kind::String
                }
            }
            '{' | '}' | '[' | ']' | ',' | ':' => {
                i += 1;
                Kind::Punct
            }
            c if c.is_ascii_digit() || c == '-' => {
                i += 1;
                while i < chars.len()
                    && (chars[i].is_ascii_digit()
                        || matches!(chars[i], '.' | 'e' | 'E' | '+' | '-'))
                {
                    i += 1;
                }
                Kind::Number
            }
            c if c.is_ascii_alphabetic() => {
                while i < chars.len() && chars[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if matches!(word.as_str(), "true" | "false" | "null") {
                    Kind::Literal
                } else {
                    Kind::Plain
                }
            }
            _ => {
                i += 1;
                Kind::Plain
            }
        };
        let text: String = chars[start..i.min(chars.len())].iter().collect();
        match out.last_mut() {
            Some((k, t)) if *k == Kind::Plain && kind == Kind::Plain => t.push_str(&text),
            _ => out.push((kind, text)),
        }
    }
    out
}

pub fn highlight_line(line: &str) -> Line<'static> {
    Line::from(
        tokens(line)
            .into_iter()
            .map(|(kind, text)| Span::styled(text, kind.style()))
            .collect::<Vec<_>>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use Kind::{Key, Literal, Number, Plain, Punct};
    const STR: Kind = Kind::String;

    fn kinds(line: &str) -> Vec<(Kind, String)> {
        tokens(line)
            .into_iter()
            .filter(|(k, t)| !(*k == Plain && t.trim().is_empty()))
            .collect()
    }

    #[test]
    fn key_value_line() {
        assert_eq!(
            kinds(r#"  "name": "ada", "n": -1.5e3, "ok": true, "x": null"#),
            vec![
                (Key, "\"name\"".into()),
                (Punct, ":".into()),
                (STR, "\"ada\"".into()),
                (Punct, ",".into()),
                (Key, "\"n\"".into()),
                (Punct, ":".into()),
                (Number, "-1.5e3".into()),
                (Punct, ",".into()),
                (Key, "\"ok\"".into()),
                (Punct, ":".into()),
                (Literal, "true".into()),
                (Punct, ",".into()),
                (Key, "\"x\"".into()),
                (Punct, ":".into()),
                (Literal, "null".into()),
            ]
        );
    }

    #[test]
    fn nested_and_escaped() {
        assert_eq!(
            kinds(r#"{"a": ["q\"uote", {}]}"#),
            vec![
                (Punct, "{".into()),
                (Key, "\"a\"".into()),
                (Punct, ":".into()),
                (Punct, "[".into()),
                (STR, "\"q\\\"uote\"".into()),
                (Punct, ",".into()),
                (Punct, "{".into()),
                (Punct, "}".into()),
                (Punct, "]".into()),
                (Punct, "}".into()),
            ]
        );
    }

    #[test]
    fn invalid_json_never_loses_text() {
        for line in [
            "",
            "not json at all",
            "\"unterminated",
            "{ ] ,, 12abc \u{e9}\u{4e2d}",
        ] {
            let joined: String = tokens(line).into_iter().map(|(_, t)| t).collect();
            assert_eq!(joined, line);
        }
        assert_eq!(kinds("hello"), vec![(Plain, "hello".into())]);
    }
}
