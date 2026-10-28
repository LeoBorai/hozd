//! A small HTML pretty-printer and highlighter for reading responses. It is a formatter for
//! humans, not a parser: it never fails, and malformed markup is indented as best it can be.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

pub fn looks_like_html(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('<') && t.contains('>')
}

#[derive(Debug)]
enum Tok {
    /// `<!doctype ...>`, `<?xml ...?>`, `<!-- ... -->`
    Decl(String),
    Text(String),
    Open {
        text: String,
        empty: bool,
    },
    Close(String),
    /// Body of `<script>` / `<style>`.
    Code(String),
    /// A whole `<pre>` / `<textarea>` element, whose whitespace is significant.
    Verbatim(String),
}

/// Index of the `>` that ends the tag starting at `start`, skipping quoted attribute values.
fn tag_end(src: &str, start: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (i, &b) in src.as_bytes().iter().enumerate().skip(start + 1) {
        match (quote, b) {
            (Some(q), b) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(b),
            (None, b'>') => return Some(i),
            _ => {}
        }
    }
    None
}

fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('<')
        .trim_start_matches('/')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | ':'))
        .collect::<String>()
        .to_ascii_lowercase()
}

fn tokenize(src: &str) -> Vec<Tok> {
    let bytes = src.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;

    while i < src.len() {
        if bytes[i] != b'<' {
            let next = src[i..].find('<').map_or(src.len(), |p| i + p);
            toks.push(Tok::Text(src[i..next].to_owned()));
            i = next;
            continue;
        }

        if src[i..].starts_with("<!--") {
            let end = src[i + 4..]
                .find("-->")
                .map_or(src.len(), |p| i + 4 + p + 3);
            toks.push(Tok::Decl(src[i..end].to_owned()));
            i = end;
            continue;
        }

        let starts_tag = bytes
            .get(i + 1)
            .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'/' | b'!' | b'?'));
        let end = if starts_tag { tag_end(src, i) } else { None };
        let Some(end) = end else {
            // a lone `<` (as in `a < b`): plain text up to the next `<`
            let next = src[i + 1..].find('<').map_or(src.len(), |p| i + 1 + p);
            toks.push(Tok::Text(src[i..next].to_owned()));
            i = next;
            continue;
        };

        let text = &src[i..=end];
        i = end + 1;
        if text.starts_with("<!") || text.starts_with("<?") {
            toks.push(Tok::Decl(text.to_owned()));
        } else if text.starts_with("</") {
            toks.push(Tok::Close(text.to_owned()));
        } else {
            let name = tag_name(text);
            let empty = text.ends_with("/>") || VOID.contains(&name.as_str());
            if !empty && matches!(name.as_str(), "script" | "style" | "pre" | "textarea") {
                let close = format!("</{name}");
                let rest = src[i..].to_ascii_lowercase();
                let content_end = rest.find(&close).map_or(src.len(), |p| i + p);
                if matches!(name.as_str(), "pre" | "textarea") {
                    let close_end = src[content_end..]
                        .find('>')
                        .map_or(src.len(), |p| content_end + p + 1);
                    toks.push(Tok::Verbatim(
                        src[end + 1 - text.len()..close_end].to_owned(),
                    ));
                    i = close_end;
                    continue;
                }
                toks.push(Tok::Open {
                    text: text.to_owned(),
                    empty,
                });
                toks.push(Tok::Code(src[i..content_end].to_owned()));
                i = content_end;
                continue;
            }
            toks.push(Tok::Open {
                text: text.to_owned(),
                empty,
            });
        }
    }
    // a lone `<` splits text in two; join the pieces back into one node
    let mut merged: Vec<Tok> = Vec::with_capacity(toks.len());
    for tok in toks {
        match (merged.last_mut(), tok) {
            (Some(Tok::Text(prev)), Tok::Text(next)) => prev.push_str(&next),
            (_, tok) => merged.push(tok),
        }
    }
    merged
}

/// Re-indents `<script>`/`<style>` content: strips the common indentation, keeps the rest.
fn dedent(code: &str) -> Vec<String> {
    let lines: Vec<&str> = code.lines().collect();
    let common = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out: Vec<String> = lines
        .iter()
        .map(|l| {
            l.get(common.min(l.len())..)
                .unwrap_or("")
                .trim_end()
                .to_owned()
        })
        .collect();
    while out.first().is_some_and(|l| l.is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out
}

/// One tag or text node per line, two spaces per nesting level.
pub fn format(src: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut line = |depth: usize, text: &str| {
        out.push_str(&"  ".repeat(depth));
        out.push_str(text);
        out.push('\n');
    };

    for tok in tokenize(src) {
        match tok {
            Tok::Decl(t) | Tok::Verbatim(t) => line(depth, &t),
            Tok::Text(t) => {
                let collapsed = t.split_whitespace().collect::<Vec<_>>().join(" ");
                if !collapsed.is_empty() {
                    line(depth, &collapsed);
                }
            }
            Tok::Open { text, empty, .. } => {
                line(depth, &text);
                if !empty {
                    depth += 1;
                }
            }
            Tok::Close(t) => {
                depth = depth.saturating_sub(1);
                line(depth, &t);
            }
            Tok::Code(code) => {
                for l in dedent(&code) {
                    line(depth, &l);
                }
            }
        }
    }
    out.trim_end().to_owned()
}

pub fn highlight_line(line: &str) -> Line<'static> {
    let tag = Style::new().fg(Color::Cyan);
    let comment = Style::new().fg(Color::DarkGray);
    let mut spans = Vec::new();
    let mut rest = line;

    while !rest.is_empty() {
        match rest.find('<') {
            None => {
                spans.push(Span::raw(rest.to_owned()));
                break;
            }
            Some(0) => {
                let is_comment = rest.starts_with("<!--");
                let end = rest.find('>').map_or(rest.len(), |p| p + 1);
                let style = if is_comment { comment } else { tag };
                spans.push(Span::styled(rest[..end].to_owned(), style));
                rest = &rest[end..];
            }
            Some(p) => {
                spans.push(Span::raw(rest[..p].to_owned()));
                rest = &rest[p..];
            }
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nests_and_puts_each_node_on_a_line() {
        let html = "<!DOCTYPE html><html><head><title>T</title></head><body><p>Hi  there <b>x</b></p></body></html>";
        assert_eq!(
            format(html),
            "<!DOCTYPE html>\n<html>\n  <head>\n    <title>\n      T\n    </title>\n  </head>\n  <body>\n    <p>\n      Hi there\n      <b>\n        x\n      </b>\n    </p>\n  </body>\n</html>"
        );
    }

    #[test]
    fn void_and_self_closing_tags_do_not_indent() {
        let out = format(r#"<div><br><img src="a>b.png" alt='x>y'><input/><span></span></div>"#);
        assert_eq!(
            out,
            "<div>\n  <br>\n  <img src=\"a>b.png\" alt='x>y'>\n  <input/>\n  <span>\n  </span>\n</div>"
        );
    }

    #[test]
    fn comments_are_kept_whole() {
        assert_eq!(
            format("<p><!-- a > b --></p>"),
            "<p>\n  <!-- a > b -->\n</p>"
        );
    }

    #[test]
    fn script_and_style_keep_relative_indentation() {
        let out = format("<script>\n      var a = 1;\n        if (a) { b(); }\n    </script>");
        assert_eq!(
            out,
            "<script>\n  var a = 1;\n    if (a) { b(); }\n</script>"
        );
    }

    #[test]
    fn pre_is_verbatim() {
        let out = format("<div><pre>  a\n    b  </pre></div>");
        assert_eq!(out, "<div>\n  <pre>  a\n    b  </pre>\n</div>");
    }

    #[test]
    fn lone_less_than_is_text() {
        assert_eq!(
            format("<p>a < b and c > d</p>"),
            "<p>\n  a < b and c > d\n</p>"
        );
    }

    #[test]
    fn malformed_markup_never_panics() {
        for src in [
            "</div></div>",
            "<div",
            "<a href=\"x",
            "<",
            "<!--",
            "<script>x",
            "<pre>x",
            "",
        ] {
            let _ = format(src);
        }
    }

    #[test]
    fn formatting_twice_changes_nothing() {
        let once = format("<ul><li>a</li><li>b <i>c</i></li></ul>");
        assert_eq!(format(&once), once);
    }

    #[test]
    fn detection() {
        assert!(looks_like_html("  <html></html>"));
        assert!(!looks_like_html("{\"a\": \"<b>\"}"));
        assert!(!looks_like_html("plain text"));
    }

    #[test]
    fn highlight_colors_tags_and_keeps_the_text() {
        let line = highlight_line("<a href=\"x\">link</a>");
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "<a href=\"x\">link</a>");
        assert_eq!(line.spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(line.spans[1].style.fg, None);
    }
}
