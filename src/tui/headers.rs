//! The request Headers tab is a text editor with one `Name: value` per line, like `curl -H`:
//! editing a line sets a header, deleting a line removes it, a new line appends one.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::client::Header;

pub fn to_lines(headers: &[Header]) -> Vec<String> {
    headers
        .iter()
        .map(|h| format!("{}: {}", h.name, h.value))
        .collect()
}

/// `Ok(None)` for a blank line, `Err` when the line is not `Name: value`.
pub fn parse_line(line: &str) -> Result<Option<Header>, String> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    let (name, value) = line
        .split_once(':')
        .ok_or_else(|| "expected `Name: value`".to_owned())?;
    let name = name.trim();
    if name.is_empty() {
        return Err("the header name is empty".into());
    }
    if let Some(bad) = name.chars().find(|c| !is_token_char(*c)) {
        return Err(format!("invalid character `{bad}` in the header name"));
    }
    Ok(Some(Header::new(name, value.trim())))
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

/// All headers, or the first invalid line with its 1-based number.
pub fn parse(lines: &[String]) -> Result<Vec<Header>, String> {
    let mut headers = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        match parse_line(line) {
            Ok(Some(h)) => headers.push(h),
            Ok(None) => {}
            Err(reason) => return Err(format!("header line {}: {reason}", i + 1)),
        }
    }
    Ok(headers)
}

/// Valid headers only; invalid lines are skipped (they are shown in red while editing).
pub fn parse_lenient(lines: &[String]) -> Vec<Header> {
    lines
        .iter()
        .filter_map(|l| parse_line(l).ok().flatten())
        .collect()
}

pub fn highlight_line(line: &str) -> Line<'static> {
    match parse_line(line) {
        Ok(Some(_)) => {
            let (name, value) = line.split_once(':').expect("valid header has a colon");
            Line::from(vec![
                Span::styled(name.to_owned(), Style::new().fg(Color::Cyan)),
                Span::styled(":", Style::new().fg(Color::DarkGray)),
                Span::raw(value.to_owned()),
            ])
        }
        Ok(None) => Line::raw(line.to_owned()),
        Err(_) => Line::styled(line.to_owned(), Style::new().fg(Color::Red)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_and_trimmed_value() {
        assert_eq!(
            parse_line("Content-Type:  application/json ").unwrap(),
            Some(Header::new("Content-Type", "application/json"))
        );
        // only the first colon splits, so values may contain colons
        assert_eq!(
            parse_line("Authorization: Bearer a:b").unwrap(),
            Some(Header::new("Authorization", "Bearer a:b"))
        );
        assert_eq!(
            parse_line("X-Empty:").unwrap(),
            Some(Header::new("X-Empty", ""))
        );
        assert_eq!(parse_line("   ").unwrap(), None);
    }

    #[test]
    fn rejects_malformed_lines() {
        assert!(parse_line("no colon here").is_err());
        assert!(parse_line(": value").is_err());
        assert!(parse_line("Bad Name: v").is_err());
    }

    #[test]
    fn parse_reports_the_first_bad_line_number() {
        let lines = [
            "A: 1".to_owned(),
            "".to_owned(),
            "oops".to_owned(),
            "B: 2".to_owned(),
        ];
        let err = parse(&lines).unwrap_err();
        assert!(err.contains("line 3"), "{err}");
        assert_eq!(
            parse_lenient(&lines),
            vec![Header::new("A", "1"), Header::new("B", "2")]
        );
    }

    #[test]
    fn round_trips_and_keeps_order_and_duplicates() {
        let headers = vec![
            Header::new("Set-Cookie", "a=1"),
            Header::new("X-A", "b"),
            Header::new("Set-Cookie", "b=2"),
        ];
        assert_eq!(parse(&to_lines(&headers)).unwrap(), headers);
    }

    #[test]
    fn invalid_lines_render_red_valid_ones_styled() {
        assert_eq!(highlight_line("oops").style.fg, Some(Color::Red));
        let ok = highlight_line("Accept: */*");
        assert_eq!(ok.spans[0].style.fg, Some(Color::Cyan));
        let text: String = ok.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Accept: */*");
    }
}
