//! The Auth tab: scheme picker, Basic inputs, free text and the JWT layout (token on the
//! left; decoded header, decoded payload and signature check on the right).

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::tui::app::{App, Focus};
use crate::tui::auth::AuthEditor;
use crate::tui::auth::{Check, CheckState, Field, Scheme};
use crate::tui::syntax_highlight::json::highlight_line;

use super::*;

const LABEL: u16 = 10;
/// The colors of the JWT parts: header, payload, signature.
const HEADER_COLOR: Color = Color::Magenta;
const PAYLOAD_COLOR: Color = Color::Rgb(160, 110, 255);
const SIGNATURE_COLOR: Color = Color::Cyan;

/// Where the clickable parts of the Auth tab are.
pub struct AuthLayout {
    pub schemes: Vec<(Scheme, Rect)>,
    pub fields: Vec<(Field, Rect)>,
    /// Decoded header, decoded payload and signature boxes; JWT only.
    pub panels: Option<[Rect; 3]>,
    /// Wrapped token (a field) and its two checks; JWT only.
    pub checks: Vec<Rect>,
    /// Everything below the scheme line.
    pub content: Rect,
    /// The secret's check line; JWT only.
    pub secret_check: Option<Rect>,
    /// Free line for hints (Basic preview, None hint).
    pub note: Option<Rect>,
}

pub fn auth_layout(inner: Rect, auth: &AuthEditor) -> AuthLayout {
    let mut layout = AuthLayout {
        schemes: Vec::new(),
        fields: Vec::new(),
        panels: None,
        checks: Vec::new(),
        content: Rect::default(),
        secret_check: None,
        note: None,
    };
    if inner.width == 0 || inner.height < 3 {
        return layout;
    }
    let mut x = inner.x + 8;
    for scheme in Scheme::ALL {
        let width = scheme.label().len() as u16 + 2;
        let rect = Rect::new(x, inner.y, width, 1).intersection(inner);
        layout.schemes.push((scheme, rect));
        x = x.saturating_add(width + 1);
    }
    let content = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
    layout.content = content;
    let row = |y: u16, label: u16| {
        Rect::new(
            content.x + label,
            content.y + y,
            content.width.saturating_sub(label),
            1,
        )
    };
    match auth.scheme {
        Scheme::None => layout.note = Some(row(0, 0)),
        Scheme::Basic => {
            layout.fields = vec![
                (Field::User, row(0, LABEL)),
                (Field::Password, row(1, LABEL)),
            ];
            layout.note = Some(row(3, 0));
        }
        Scheme::Raw => layout.fields = vec![(Field::Raw, row(0, 15))],
        Scheme::Jwt => {
            let [left, right] =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .spacing(1)
                    .areas(content);
            let prefix = Rect::new(left.x + 7, left.y, left.width.saturating_sub(7), 1);
            let token_height = left.height.saturating_sub(5);
            let token = Rect::new(left.x, left.y + 3, left.width, token_height);
            layout.fields.push((Field::Prefix, prefix));
            layout.fields.push((Field::Token, token));
            for i in 0..2 {
                layout.checks.push(Rect::new(
                    left.x,
                    left.y + left.height.saturating_sub(2 - i),
                    left.width,
                    1,
                ));
            }
            let [header, payload, signature] = Layout::vertical([
                Constraint::Fill(1),
                Constraint::Fill(2),
                Constraint::Length(5),
            ])
            .areas(right);
            layout.panels = Some([header, payload, signature]);
            let sig = Rect::new(
                signature.x + 1,
                signature.y + 1,
                signature.width.saturating_sub(2),
                signature.height.saturating_sub(2),
            );
            let line = |y: u16, label: u16| {
                Rect::new(
                    sig.x + label,
                    sig.y + y,
                    sig.width.saturating_sub(label),
                    1.min(sig.height.saturating_sub(y)),
                )
            };
            layout.fields.push((Field::Secret, line(0, 7)));
            layout.fields.push((Field::Encoded, line(1, 0)));
            layout.secret_check = Some(line(2, 0));
        }
    }
    layout
}

/// The first visible row of the wrapped token, so the cursor stays in view.
fn token_scroll(width: u16, cursor: usize, height: u16) -> usize {
    (cursor / width.max(1) as usize).saturating_sub((height as usize).saturating_sub(1))
}

/// The character of the token under `at`, for a click in the token area.
pub fn token_index(area: Rect, at: Position, cursor: usize) -> usize {
    let width = area.width.max(1) as usize;
    (token_scroll(area.width, cursor, area.height) + (at.y - area.y) as usize) * width
        + (at.x - area.x) as usize
}

fn mark(check: &Check) -> Line<'static> {
    let (symbol, color) = match check.state {
        CheckState::Pass => ("✓", Color::Green),
        CheckState::Fail => ("✗", Color::Red),
        CheckState::Idle => ("·", Color::DarkGray),
    };
    Line::from(vec![
        Span::styled(format!("{symbol} "), Style::new().fg(color)),
        Span::styled(check.text.clone(), Style::new().fg(color)),
    ])
}

/// The token wrapped to `width`, each part in its color.
fn token_lines(token: &str, width: u16, selected: Option<(usize, usize)>) -> Vec<Line<'static>> {
    let width = width.max(1) as usize;
    let chars: Vec<char> = token.chars().collect();
    let mut part = 0;
    let mut styled: Vec<(char, Style)> = Vec::with_capacity(chars.len());
    for (i, c) in chars.into_iter().enumerate() {
        let mut style = if c == '.' {
            part += 1;
            Style::new().fg(Color::White)
        } else {
            Style::new().fg([HEADER_COLOR, PAYLOAD_COLOR, SIGNATURE_COLOR][part.min(2)])
        };
        if selected.is_some_and(|(from, to)| i >= from && i < to) {
            style = style.patch(super::editor::SELECTION);
        }
        styled.push((c, style));
    }
    styled
        .chunks(width)
        .map(|row| {
            let mut spans: Vec<Span> = Vec::new();
            let mut run = String::new();
            let mut run_style = row[0].1;
            for (c, style) in row {
                if *style != run_style {
                    spans.push(Span::styled(std::mem::take(&mut run), run_style));
                    run_style = *style;
                }
                run.push(*c);
            }
            spans.push(Span::styled(run, run_style));
            Line::from(spans)
        })
        .collect()
}

pub(crate) fn draw_auth(frame: &mut Frame, app: &App, inner: Rect) {
    let auth = &app.auth;
    let layout = auth_layout(inner, auth);
    if layout.content.height == 0 {
        return;
    }
    let dim = Style::new().fg(Color::DarkGray);
    let active = Style::new()
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        .fg(Color::Yellow);
    frame.render_widget(
        Paragraph::new(Line::styled("Scheme", dim)),
        Rect::new(inner.x, inner.y, 7.min(inner.width), 1),
    );
    for (scheme, rect) in &layout.schemes {
        let style = if *scheme == auth.scheme {
            active
        } else {
            Style::new()
        };
        frame.render_widget(
            Paragraph::new(Line::styled(format!(" {} ", scheme.label()), style)),
            *rect,
        );
    }
    let hint = "Ctrl+R switches";
    let hint_x = layout
        .schemes
        .last()
        .map_or(inner.x, |(_, r)| r.right() + 2);
    if hint_x + hint.len() as u16 <= inner.right() {
        frame.render_widget(
            Paragraph::new(Line::styled(hint, dim)),
            Rect::new(hint_x, inner.y, hint.len() as u16, 1),
        );
    }

    let focused = app.focus == Focus::Auth;
    let input = |frame: &mut Frame, field: Field, rect: Rect, placeholder: &str, syntax| {
        let Some(ta) = auth.input(field) else { return };
        let here = focused && auth.field == field;
        if here {
            frame.render_widget(
                Block::default().style(Style::new().bg(Color::DarkGray)),
                rect,
            );
        }
        if ta.lines()[0].is_empty() && !here {
            frame.render_widget(
                Paragraph::new(Line::styled(placeholder.to_owned(), dim)),
                rect,
            );
        } else {
            render_editor(frame, ta, rect, here, syntax);
        }
    };
    let label = |frame: &mut Frame, text: &str, rect: Rect| {
        frame.render_widget(
            Paragraph::new(Line::styled(text.to_owned(), dim)),
            Rect::new(
                rect.x.saturating_sub(text.len() as u16 + 1).max(inner.x),
                rect.y,
                text.len() as u16,
                1,
            ),
        );
    };
    let field_rect = |field: Field| {
        layout
            .fields
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, r)| *r)
    };

    match auth.scheme {
        Scheme::None => {
            if let Some(note) = layout.note {
                frame.render_widget(
                    Paragraph::new(Line::styled(
                        "No Authorization header is added. Pick a scheme above (Ctrl+R).",
                        dim,
                    )),
                    note,
                );
            }
        }
        Scheme::Basic => {
            for (field, name, placeholder) in [
                (Field::User, "Username", "user"),
                (Field::Password, "Password", "password"),
            ] {
                if let Some(rect) = field_rect(field) {
                    label(frame, name, rect);
                    input(frame, field, rect, placeholder, Syntax::Plain);
                }
            }
            if let (Some(note), Some(value)) = (layout.note, auth.header_value()) {
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled("Authorization: ", Style::new().fg(Color::Cyan)),
                        Span::raw(value),
                    ])),
                    note,
                );
            }
        }
        Scheme::Raw => {
            if let Some(rect) = field_rect(Field::Raw) {
                label(frame, "Authorization", rect);
                input(
                    frame,
                    Field::Raw,
                    rect,
                    "Digest realm=..., or any value",
                    Syntax::Plain,
                );
            }
        }
        Scheme::Jwt => draw_jwt(frame, app, &layout, input, label, dim),
    }
}

fn draw_jwt(
    frame: &mut Frame,
    app: &App,
    layout: &AuthLayout,
    input: impl Fn(&mut Frame, Field, Rect, &str, Syntax),
    label: impl Fn(&mut Frame, &str, Rect),
    dim: Style,
) {
    let auth = &app.auth;
    let focused = app.focus == Focus::Auth;
    let checks = auth.checks();
    let rect_of = |field: Field| {
        layout
            .fields
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, r)| *r)
    };
    if let Some(prefix) = rect_of(Field::Prefix) {
        label(frame, "Prefix", prefix);
        input(
            frame,
            Field::Prefix,
            prefix,
            "Bearer (optional)",
            Syntax::Plain,
        );
    }
    if let Some(token) = rect_of(Field::Token) {
        let title = Rect::new(token.x, token.y.saturating_sub(1), token.width, 1);
        frame.render_widget(
            Paragraph::new(Line::styled("JSON Web Token (JWT)", dim)),
            title,
        );
        if token.height > 0 {
            let text = auth.token.lines().join("");
            let here = focused && auth.field == Field::Token;
            if text.is_empty() {
                frame.render_widget(Paragraph::new(Line::styled("Paste a JWT", dim)), token);
            } else {
                let selected = auth
                    .token
                    .selection_range()
                    .filter(|_| here)
                    .map(|(from, to)| (from.1, to.1));
                let lines = token_lines(&text, token.width, selected);
                let scroll = token_scroll(token.width, auth.token.cursor().1, token.height);
                frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), token);
            }
            if here {
                let cursor = auth.token.cursor().1;
                let width = token.width.max(1) as usize;
                let scroll = token_scroll(token.width, cursor, token.height);
                frame.set_cursor_position(Position::new(
                    token.x + (cursor % width) as u16,
                    token.y + (cursor / width - scroll) as u16,
                ));
            }
        }
    }
    for (rect, check) in layout
        .checks
        .iter()
        .zip([&checks.valid_jwt, &checks.signature])
    {
        frame.render_widget(Paragraph::new(mark(check)), *rect);
    }

    let Some([header, payload, signature]) = layout.panels else {
        return;
    };
    let jwt = auth.jwt().and_then(Result::ok);
    for (area, title, color, json) in [
        (
            header,
            "Decoded Header",
            HEADER_COLOR,
            jwt.as_ref().map(|j| j.header_json()),
        ),
        (
            payload,
            "Decoded Payload",
            PAYLOAD_COLOR,
            jwt.as_ref().map(|j| j.payload_json()),
        ),
    ] {
        let block = block(title, false).border_style(Style::new().fg(color));
        let body = block.inner(area);
        frame.render_widget(block, area);
        let lines: Vec<Line> = match &json {
            Some(json) => json.lines().map(highlight_line).collect(),
            None => vec![Line::styled(
                match &checks.valid_jwt.state {
                    CheckState::Fail => "The token cannot be decoded",
                    _ => "Nothing to decode",
                },
                dim,
            )],
        };
        frame.render_widget(Paragraph::new(lines), body);
    }

    let block =
        block("JWT Signature Verification", false).border_style(Style::new().fg(SIGNATURE_COLOR));
    frame.render_widget(block, signature);
    if let Some(secret) = rect_of(Field::Secret) {
        label(frame, "Secret", secret);
        input(
            frame,
            Field::Secret,
            secret,
            "secret used to sign the JWT",
            Syntax::Plain,
        );
    }
    if let Some(encoded) = rect_of(Field::Encoded) {
        let here = focused && auth.field == Field::Encoded;
        let style = if here {
            Style::new().bg(Color::DarkGray)
        } else {
            Style::new()
        };
        let boxed = if auth.encoded { "[x]" } else { "[ ]" };
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("{boxed} secret is base64url encoded (Space)"),
                style,
            )),
            encoded,
        );
    }
    if let Some(rect) = layout.secret_check {
        frame.render_widget(Paragraph::new(mark(&checks.secret)), rect);
    }
}
