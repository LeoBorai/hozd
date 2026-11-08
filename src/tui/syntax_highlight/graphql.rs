//! A small GraphQL highlighter for the request editor. It is a tokenizer, not a parser:
//! every character of the input ends up in some token, so invalid documents still render
//! in full. The only state carried between lines is whether a `"""` block string is open.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Keyword,
    Type,
    Field,
    Argument,
    Variable,
    Directive,
    String,
    Number,
    Literal,
    Comment,
    Punct,
    Plain,
}

impl Kind {
    pub fn style(self) -> Style {
        match self {
            Kind::Keyword => Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD),
            Kind::Type => Style::new().fg(Color::LightBlue),
            Kind::Field => Style::new().fg(Color::Cyan),
            Kind::Argument => Style::new().fg(Color::LightCyan),
            Kind::Variable => Style::new().fg(Color::LightRed),
            Kind::Directive => Style::new().fg(Color::LightMagenta),
            Kind::String => Style::new().fg(Color::Green),
            Kind::Number | Kind::Literal => Style::new().fg(Color::Yellow),
            Kind::Comment => Style::new()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
            Kind::Punct => Style::new().fg(Color::DarkGray),
            Kind::Plain => Style::new(),
        }
    }
}

const KEYWORDS: &[&str] = &[
    "query",
    "mutation",
    "subscription",
    "fragment",
    "on",
    "schema",
    "scalar",
    "type",
    "interface",
    "union",
    "enum",
    "input",
    "extend",
    "directive",
    "implements",
    "repeatable",
];

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_name(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Scans to the end of a `"""` block string starting at `i`. Returns the index after the
/// closing quotes and whether they were found on this line.
fn block_string_end(chars: &[char], mut i: usize) -> (usize, bool) {
    while i < chars.len() {
        if chars[i] == '\\' && chars[i..].starts_with(&['\\', '"', '"', '"']) {
            i += 4;
        } else if chars[i..].starts_with(&['"', '"', '"']) {
            return (i + 3, true);
        } else {
            i += 1;
        }
    }
    (chars.len(), false)
}

/// Splits one line of GraphQL into styled tokens. `in_block` says whether the line starts
/// inside a `"""` block string and is updated for the next line. Concatenating the token
/// texts always yields the input line.
pub fn tokens(line: &str, in_block: &mut bool) -> Vec<(Kind, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<(Kind, String)> = Vec::new();
    let mut i = 0;
    let push = |out: &mut Vec<(Kind, String)>, kind: Kind, text: String| match out.last_mut() {
        Some((k, t)) if *k == kind && matches!(kind, Kind::Plain | Kind::Punct) => {
            t.push_str(&text)
        }
        _ => out.push((kind, text)),
    };

    if *in_block {
        let (end, closed) = block_string_end(&chars, 0);
        *in_block = !closed;
        push(&mut out, Kind::String, chars[..end].iter().collect());
        i = end;
    }

    while i < chars.len() {
        let c = chars[i];
        let start = i;
        let kind = match c {
            '#' => {
                i = chars.len();
                Kind::Comment
            }
            '"' if chars[i..].starts_with(&['"', '"', '"']) => {
                let (end, closed) = block_string_end(&chars, i + 3);
                *in_block = !closed;
                i = end;
                Kind::String
            }
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
                i = (i + 1).min(chars.len());
                Kind::String
            }
            '$' | '@' if chars.get(i + 1).is_some_and(|&n| is_name_start(n)) => {
                i += 1;
                while i < chars.len() && is_name(chars[i]) {
                    i += 1;
                }
                if c == '$' {
                    Kind::Variable
                } else {
                    Kind::Directive
                }
            }
            c if c.is_ascii_digit()
                || (c == '-' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) =>
            {
                i += 1;
                while i < chars.len()
                    && (chars[i].is_ascii_digit()
                        || matches!(chars[i], '.' | 'e' | 'E' | '+' | '-'))
                {
                    i += 1;
                }
                Kind::Number
            }
            c if is_name_start(c) => {
                while i < chars.len() && is_name(chars[i]) {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                let next = chars[i..].iter().find(|c| !c.is_whitespace());
                if next == Some(&':') {
                    // Argument names, aliases and field definitions.
                    Kind::Argument
                } else if matches!(word.as_str(), "true" | "false" | "null") {
                    Kind::Literal
                } else if KEYWORDS.contains(&word.as_str()) {
                    Kind::Keyword
                } else if word.starts_with(|c: char| c.is_ascii_uppercase()) {
                    Kind::Type
                } else {
                    Kind::Field
                }
            }
            '{' | '}' | '(' | ')' | '[' | ']' | ':' | ',' | '=' | '!' | '|' | '&' | '.' => {
                i += 1;
                Kind::Punct
            }
            _ => {
                i += 1;
                Kind::Plain
            }
        };
        push(
            &mut out,
            kind,
            chars[start..i.min(chars.len())].iter().collect(),
        );
    }
    out
}

/// Highlights a whole document, carrying block-string state across lines.
pub fn highlight_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<Line<'static>> {
    let mut in_block = false;
    lines
        .into_iter()
        .map(|l| {
            Line::from(
                tokens(l, &mut in_block)
                    .into_iter()
                    .map(|(kind, text)| Span::styled(text, kind.style()))
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use Kind::{
        Argument, Comment, Directive, Field, Keyword, Literal, Number, Plain, Punct, Type, Variable,
    };
    const STR: Kind = Kind::String;

    fn kinds(line: &str) -> Vec<(Kind, String)> {
        tokens(line, &mut false)
            .into_iter()
            .filter(|(k, t)| !(*k == Plain && t.trim().is_empty()))
            .collect()
    }

    #[test]
    fn operation_with_variables_and_arguments() {
        assert_eq!(
            kinds(
                r#"query User($id: ID!, $n: Int = -5) { user(id: $id, tag: "x") @include(if: true) { ...F } }"#
            ),
            vec![
                (Keyword, "query".into()),
                (Type, "User".into()),
                (Punct, "(".into()),
                (Variable, "$id".into()),
                (Punct, ":".into()),
                (Type, "ID".into()),
                (Punct, "!,".into()),
                (Variable, "$n".into()),
                (Punct, ":".into()),
                (Type, "Int".into()),
                (Punct, "=".into()),
                (Number, "-5".into()),
                (Punct, ")".into()),
                (Punct, "{".into()),
                (Field, "user".into()),
                (Punct, "(".into()),
                (Argument, "id".into()),
                (Punct, ":".into()),
                (Variable, "$id".into()),
                (Punct, ",".into()),
                (Argument, "tag".into()),
                (Punct, ":".into()),
                (STR, "\"x\"".into()),
                (Punct, ")".into()),
                (Directive, "@include".into()),
                (Punct, "(".into()),
                (Argument, "if".into()),
                (Punct, ":".into()),
                (Literal, "true".into()),
                (Punct, ")".into()),
                (Punct, "{".into()),
                (Punct, "...".into()),
                (Type, "F".into()),
                (Punct, "}".into()),
                (Punct, "}".into()),
            ]
        );
    }

    #[test]
    fn comments_and_block_strings_span_lines() {
        let mut in_block = false;
        let first = tokens(r#"  name # the "name""#, &mut in_block);
        assert_eq!(first.last().unwrap(), &(Comment, "# the \"name\"".into()));
        assert!(!in_block);

        let open = tokens(r#"  """starts"#, &mut in_block);
        assert_eq!(open.last().unwrap(), &(STR, "\"\"\"starts".into()));
        assert!(in_block);
        let middle = tokens("still { text }", &mut in_block);
        assert_eq!(middle, vec![(STR, "still { text }".into())]);
        let close = tokens(r#"ends""" field"#, &mut in_block);
        assert_eq!(close[0], (STR, "ends\"\"\"".into()));
        assert_eq!(close.last().unwrap(), &(Field, "field".into()));
        assert!(!in_block);
    }

    #[test]
    fn never_loses_text() {
        let mut in_block = false;
        for line in [
            "",
            "\"unterminated",
            "$ @ - 12abc \u{e9}\u{4e2d} ~",
            "\"\"\" open \\\"\"\" still",
        ] {
            let joined: String = tokens(line, &mut in_block)
                .into_iter()
                .map(|(_, t)| t)
                .collect();
            assert_eq!(joined, line);
        }
    }
}
