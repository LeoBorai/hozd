//! The JSON body editor: one row per field, indented by depth, each with a type.
//!
//! The rows are a flattened tree: a row's children are the rows after it that are deeper
//! than it. Objects are made of `key: value` rows, arrays of bare value rows.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui_textarea::TextArea;

use crate::client::JsonValue;

use super::headers::{cursor_col, len, single_line};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonKind {
    String,
    Number,
    Bool,
    Null,
    Object,
    Array,
}

impl JsonKind {
    pub const ALL: [JsonKind; 6] = [
        JsonKind::String,
        JsonKind::Number,
        JsonKind::Bool,
        JsonKind::Null,
        JsonKind::Object,
        JsonKind::Array,
    ];

    pub fn label(self) -> &'static str {
        match self {
            JsonKind::String => "string",
            JsonKind::Number => "number",
            JsonKind::Bool => "boolean",
            JsonKind::Null => "null",
            JsonKind::Object => "object",
            JsonKind::Array => "array",
        }
    }

    pub fn is_container(self) -> bool {
        matches!(self, JsonKind::Object | JsonKind::Array)
    }

    /// Whether the value is typed in a text input.
    pub fn is_typed(self) -> bool {
        matches!(self, JsonKind::String | JsonKind::Number)
    }

    /// Whether the row has a value to move the cursor to.
    pub fn has_value(self) -> bool {
        self.is_typed() || self == JsonKind::Bool
    }

    pub fn next(self) -> JsonKind {
        let i = Self::ALL.iter().position(|k| *k == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonCol {
    Key,
    Value,
}

pub struct JsonRow {
    pub depth: usize,
    pub key: TextArea<'static>,
    pub kind: JsonKind,
    /// The text of a string or number; `true` or `false` for a boolean.
    pub value: TextArea<'static>,
}

impl JsonRow {
    fn new(depth: usize, key: &str, kind: JsonKind, value: &str) -> Self {
        Self {
            depth,
            key: single_line(key),
            kind,
            value: single_line(value),
        }
    }

    pub fn key_text(&self) -> String {
        self.key.lines().join("")
    }

    pub fn value_text(&self) -> String {
        self.value.lines().join("")
    }

    /// An untouched row: nothing is sent for it.
    fn is_blank(&self) -> bool {
        self.kind == JsonKind::String
            && self.key_text().trim().is_empty()
            && self.value_text().is_empty()
    }

    /// Whether a number row holds something that is not a number.
    pub fn is_invalid_number(&self) -> bool {
        self.kind == JsonKind::Number && parse_number(&self.value_text()).is_none()
    }
}

fn parse_number(text: &str) -> Option<serde_json::Number> {
    text.trim().parse().ok()
}

/// The editable JSON tree and which input has the cursor.
pub struct JsonEditor {
    pub rows: Vec<JsonRow>,
    pub row: usize,
    pub col: JsonCol,
    /// Whether the document is an array instead of an object.
    pub root_array: bool,
}

impl Default for JsonEditor {
    fn default() -> Self {
        Self {
            rows: vec![JsonRow::new(0, "", JsonKind::String, "")],
            row: 0,
            col: JsonCol::Key,
            root_array: false,
        }
    }
}

impl JsonEditor {
    /// The editor for `value`; `None` when it is not an object or an array.
    pub fn from_value(value: &JsonValue) -> Option<Self> {
        let (root_array, mut rows) = match value {
            JsonValue::Object(members) => (false, rows_of_object(members, 0)),
            JsonValue::Array(items) => (true, rows_of_array(items, 0)),
            _ => return None,
        };
        rows.push(JsonRow::new(0, "", JsonKind::String, ""));
        Some(Self {
            row: rows.len() - 1,
            rows,
            col: JsonCol::Key,
            root_array,
        })
    }

    /// Whether there is nothing to send.
    pub fn is_empty(&self) -> bool {
        self.rows.iter().all(|r| r.is_blank())
    }

    /// The document. With `strict`, a number that is not one or an object member without a
    /// key is an error naming the row; otherwise numbers become `null` and such members
    /// are skipped.
    pub fn to_value(&self, strict: bool) -> Result<JsonValue, String> {
        let kind = if self.root_array {
            JsonKind::Array
        } else {
            JsonKind::Object
        };
        self.container(kind, 0, self.rows.len(), strict)
    }

    fn container(
        &self,
        kind: JsonKind,
        start: usize,
        end: usize,
        strict: bool,
    ) -> Result<JsonValue, String> {
        let array = kind == JsonKind::Array;
        let (mut members, mut items) = (Vec::new(), Vec::new());
        let mut i = start;
        while i < end {
            let row = &self.rows[i];
            let next = self.subtree_end(i).min(end);
            if row.is_blank() && next == i + 1 {
                i = next;
                continue;
            }
            let value = match row.kind {
                JsonKind::String => JsonValue::String(row.value_text()),
                JsonKind::Number => match parse_number(&row.value_text()) {
                    Some(n) => JsonValue::Number(n),
                    None if strict => {
                        return Err(format!(
                            "json row {}: `{}` is not a number",
                            i + 1,
                            row.value_text()
                        ));
                    }
                    None => JsonValue::Null,
                },
                JsonKind::Bool => JsonValue::Bool(row.value_text() == "true"),
                JsonKind::Null => JsonValue::Null,
                JsonKind::Object | JsonKind::Array => {
                    self.container(row.kind, i + 1, next, strict)?
                }
            };
            if array {
                items.push(value);
            } else if row.key_text().is_empty() {
                if strict {
                    return Err(format!("json row {}: the key is empty", i + 1));
                }
            } else {
                members.push((row.key_text(), value));
            }
            i = next;
        }
        Ok(if array {
            JsonValue::Array(items)
        } else {
            JsonValue::Object(members)
        })
    }

    /// The index after the last row below `i`.
    fn subtree_end(&self, i: usize) -> usize {
        let depth = self.rows[i].depth;
        (i + 1..self.rows.len())
            .find(|j| self.rows[*j].depth <= depth)
            .unwrap_or(self.rows.len())
    }

    /// The position of row `i` among the items of its array.
    pub fn array_index(&self, i: usize) -> usize {
        let depth = self.rows[i].depth;
        (0..i)
            .rev()
            .take_while(|j| self.rows[*j].depth >= depth)
            .filter(|j| self.rows[*j].depth == depth)
            .count()
    }

    /// How many fields or items the container on row `i` has.
    pub fn children(&self, i: usize) -> usize {
        let depth = self.rows[i].depth + 1;
        (i + 1..self.subtree_end(i))
            .filter(|j| self.rows[*j].depth == depth)
            .count()
    }

    /// Whether row `i` is an item of an array, so it has no key.
    pub fn in_array(&self, i: usize) -> bool {
        let depth = self.rows[i].depth;
        if depth == 0 {
            return self.root_array;
        }
        (0..i)
            .rev()
            .find(|j| self.rows[*j].depth == depth - 1)
            .is_some_and(|j| self.rows[j].kind == JsonKind::Array)
    }

    /// The input under the cursor, when it takes text.
    pub fn cell(&self) -> Option<&TextArea<'static>> {
        let row = &self.rows[self.row];
        match self.col {
            JsonCol::Key if !self.in_array(self.row) => Some(&row.key),
            JsonCol::Value if row.kind.is_typed() => Some(&row.value),
            _ => None,
        }
    }

    fn cell_mut(&mut self) -> Option<&mut TextArea<'static>> {
        let (row, col) = (self.row, self.col);
        let keyed = !self.in_array(row);
        let r = &mut self.rows[row];
        match col {
            JsonCol::Key if keyed => Some(&mut r.key),
            JsonCol::Value if r.kind.is_typed() => Some(&mut r.value),
            _ => None,
        }
    }

    /// Puts the cursor on `row`, in the key when it has one and `col` asks for it.
    pub fn select(&mut self, row: usize, col: JsonCol) {
        self.row = row.min(self.rows.len() - 1);
        self.col = if col == JsonCol::Key && self.in_array(self.row) {
            JsonCol::Value
        } else {
            col
        };
    }

    fn goto(&mut self, row: usize) {
        self.select(row, self.col);
        if let Some(cell) = self.cell_mut() {
            cell.move_cursor(ratatui_textarea::CursorMove::End);
        }
    }

    fn insert(&mut self, at: usize, depth: usize) {
        self.rows
            .insert(at, JsonRow::new(depth, "", JsonKind::String, ""));
        self.select(at, JsonCol::Key);
    }

    /// Adds a first child to the container on `row`.
    fn add_child(&mut self, row: usize) {
        self.insert(row + 1, self.rows[row].depth + 1);
    }

    pub fn toggle_root(&mut self) {
        self.root_array = !self.root_array;
        self.select(self.row, self.col);
    }

    fn remove(&mut self) {
        let end = self.subtree_end(self.row);
        self.rows.drain(self.row..end);
        if self.rows.is_empty() {
            self.rows.push(JsonRow::new(0, "", JsonKind::String, ""));
        }
        self.select(self.row.min(self.rows.len() - 1), JsonCol::Key);
    }

    fn cycle_kind(&mut self) {
        let i = self.row;
        let kind = self.rows[i].kind.next();
        if !kind.is_container() {
            let end = self.subtree_end(i);
            self.rows.drain(i + 1..end);
        }
        let row = &mut self.rows[i];
        row.kind = kind;
        row.value = match kind {
            JsonKind::Bool => single_line("false"),
            JsonKind::Object | JsonKind::Array | JsonKind::Null => single_line(""),
            _ if matches!(row.value_text().as_str(), "true" | "false") => single_line(""),
            _ => single_line(&row.value_text()),
        };
        self.select(i, self.col);
    }

    /// Makes the row (and what is below it) the last child of the container above it.
    fn indent(&mut self) {
        let i = self.row;
        let depth = self.rows[i].depth;
        let sibling = (0..i)
            .rev()
            .take_while(|j| self.rows[*j].depth >= depth)
            .find(|j| self.rows[*j].depth == depth);
        if sibling.is_some_and(|j| self.rows[j].kind.is_container()) {
            for j in i..self.subtree_end(i) {
                self.rows[j].depth += 1;
            }
            self.select(i, JsonCol::Key);
        }
    }

    /// Moves the row (and what is below it) out of its container, when it is the last child.
    fn outdent(&mut self) {
        let i = self.row;
        let depth = self.rows[i].depth;
        let end = self.subtree_end(i);
        let last = self.rows.get(end).is_none_or(|r| r.depth < depth);
        if depth > 0 && last {
            for j in i..end {
                self.rows[j].depth -= 1;
            }
            self.select(i, JsonCol::Key);
        }
    }

    fn enter(&mut self) {
        let i = self.row;
        if self.col == JsonCol::Key && self.rows[i].kind.has_value() {
            self.col = JsonCol::Value;
            if let Some(cell) = self.cell_mut() {
                cell.move_cursor(ratatui_textarea::CursorMove::End);
            }
        } else if self.rows[i].kind.is_container() && self.subtree_end(i) == i + 1 {
            self.add_child(i);
        } else if i + 1 < self.rows.len() {
            self.col = JsonCol::Key;
            self.goto(i + 1);
        } else if !self.rows[i].is_blank() {
            self.insert(i + 1, self.rows[i].depth);
        }
    }

    pub fn key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let i = self.row;
        match (key.code, ctrl, alt) {
            (KeyCode::Char('d'), true, _) => self.remove(),
            (KeyCode::Char('r'), true, _) => self.cycle_kind(),
            (KeyCode::Char('t'), _, true) => self.toggle_root(),
            (KeyCode::Left, _, true) | (KeyCode::Char('h'), _, true) => self.outdent(),
            (KeyCode::Right, _, true) | (KeyCode::Char('l'), _, true) => self.indent(),
            (KeyCode::Enter, ..) => self.enter(),
            (KeyCode::Up, ..) if i > 0 => self.goto(i - 1),
            (KeyCode::Down, ..) if i + 1 < self.rows.len() => self.goto(i + 1),
            (KeyCode::Up | KeyCode::Down, ..) => {}
            (KeyCode::Char(' '), false, false)
                if self.col == JsonCol::Value && self.rows[i].kind == JsonKind::Bool =>
            {
                let on = self.rows[i].value_text() == "true";
                self.rows[i].value = single_line(if on { "false" } else { "true" });
            }
            (KeyCode::Right, false, false)
                if self.col == JsonCol::Key
                    && self.rows[i].kind.has_value()
                    && self.cell().is_some_and(|c| cursor_col(c) >= len(c)) =>
            {
                self.col = JsonCol::Value;
                if let Some(cell) = self.cell_mut() {
                    cell.move_cursor(ratatui_textarea::CursorMove::Head);
                }
            }
            (KeyCode::Left, false, false)
                if self.col == JsonCol::Value
                    && !self.in_array(i)
                    && self.cell().is_none_or(|c| cursor_col(c) == 0) =>
            {
                self.col = JsonCol::Key;
                if let Some(cell) = self.cell_mut() {
                    cell.move_cursor(ratatui_textarea::CursorMove::End);
                }
            }
            _ => {
                if let Some(cell) = self.cell_mut() {
                    crate::tui::headers::edit(cell, key);
                }
            }
        }
    }
}

fn rows_of_object(members: &[(String, JsonValue)], depth: usize) -> Vec<JsonRow> {
    members
        .iter()
        .flat_map(|(key, value)| rows_of(key, value, depth))
        .collect()
}

fn rows_of_array(items: &[JsonValue], depth: usize) -> Vec<JsonRow> {
    items
        .iter()
        .flat_map(|value| rows_of("", value, depth))
        .collect()
}

fn rows_of(key: &str, value: &JsonValue, depth: usize) -> Vec<JsonRow> {
    match value {
        JsonValue::String(s) => vec![JsonRow::new(depth, key, JsonKind::String, s)],
        JsonValue::Number(n) => vec![JsonRow::new(depth, key, JsonKind::Number, &n.to_string())],
        JsonValue::Bool(b) => vec![JsonRow::new(depth, key, JsonKind::Bool, &b.to_string())],
        JsonValue::Null => vec![JsonRow::new(depth, key, JsonKind::Null, "")],
        JsonValue::Object(members) => {
            let mut rows = vec![JsonRow::new(depth, key, JsonKind::Object, "")];
            rows.extend(rows_of_object(members, depth + 1));
            rows
        }
        JsonValue::Array(items) => {
            let mut rows = vec![JsonRow::new(depth, key, JsonKind::Array, "")];
            rows.extend(rows_of_array(items, depth + 1));
            rows
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(editor: &mut JsonEditor, code: KeyCode, modifiers: KeyModifiers) {
        editor.key(KeyEvent::new(code, modifiers));
    }

    fn type_text(editor: &mut JsonEditor, text: &str) {
        for c in text.chars() {
            press(editor, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    fn json(editor: &JsonEditor) -> String {
        editor.to_value(true).unwrap().to_json()
    }

    #[test]
    fn typing_fields_of_each_type_builds_the_document() {
        let mut e = JsonEditor::default();
        type_text(&mut e, "name");
        press(&mut e, KeyCode::Enter, KeyModifiers::NONE);
        type_text(&mut e, "ada");
        press(&mut e, KeyCode::Enter, KeyModifiers::NONE);

        type_text(&mut e, "age");
        press(&mut e, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut e, KeyCode::Char('r'), KeyModifiers::CONTROL);
        type_text(&mut e, "36");
        press(&mut e, KeyCode::Enter, KeyModifiers::NONE);

        type_text(&mut e, "admin");
        for _ in 0..2 {
            press(&mut e, KeyCode::Char('r'), KeyModifiers::CONTROL);
        }
        press(&mut e, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(e.rows[2].kind, JsonKind::Bool);
        press(&mut e, KeyCode::Char(' '), KeyModifiers::NONE);

        assert_eq!(json(&e), r#"{"name":"ada","age":36,"admin":true}"#);
    }

    #[test]
    fn objects_and_arrays_nest() {
        let mut e = JsonEditor::default();
        type_text(&mut e, "tags");
        for _ in 0..4 {
            press(&mut e, KeyCode::Char('r'), KeyModifiers::CONTROL);
        }
        assert_eq!(e.rows[0].kind, JsonKind::Object);
        press(&mut e, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert_eq!(e.rows[0].kind, JsonKind::Array);

        // Enter on an empty container adds its first child, an array item without a key
        press(&mut e, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!((e.row, e.col, e.rows[1].depth), (1, JsonCol::Value, 1));
        type_text(&mut e, "a");
        press(&mut e, KeyCode::Enter, KeyModifiers::NONE);
        type_text(&mut e, "b");
        assert_eq!(json(&e), r#"{"tags":["a","b"]}"#);

        // the last item steps back out to be a field of the object
        press(&mut e, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut e, KeyCode::Left, KeyModifiers::ALT);
        type_text(&mut e, "after");
        press(&mut e, KeyCode::Enter, KeyModifiers::NONE);
        type_text(&mut e, "x");
        assert_eq!(json(&e), r#"{"tags":["a","b"],"after":"x"}"#);
    }

    #[test]
    fn indent_nests_a_row_into_the_container_above() {
        let mut e =
            JsonEditor::from_value(&JsonValue::parse(r#"{"o":{},"k":1}"#).unwrap()).unwrap();
        e.select(1, JsonCol::Key);
        press(&mut e, KeyCode::Right, KeyModifiers::ALT);
        assert_eq!(json(&e), r#"{"o":{"k":1}}"#);
        press(&mut e, KeyCode::Left, KeyModifiers::ALT);
        assert_eq!(json(&e), r#"{"o":{},"k":1}"#);
    }

    #[test]
    fn changing_a_container_to_a_scalar_drops_its_children() {
        let mut e =
            JsonEditor::from_value(&JsonValue::parse(r#"{"o":{"a":1,"b":2},"z":0}"#).unwrap())
                .unwrap();
        e.select(0, JsonCol::Key);
        press(&mut e, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert_eq!(e.rows[0].kind, JsonKind::Array);
        press(&mut e, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert_eq!(e.rows[0].kind, JsonKind::String);
        assert_eq!(json(&e), r#"{"o":"","z":0}"#);
    }

    #[test]
    fn remove_takes_the_children_with_it() {
        let mut e =
            JsonEditor::from_value(&JsonValue::parse(r#"{"o":{"a":1},"z":0}"#).unwrap()).unwrap();
        e.select(0, JsonCol::Key);
        press(&mut e, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(json(&e), r#"{"z":0}"#);
    }

    #[test]
    fn a_root_array_has_keyless_rows() {
        let mut e = JsonEditor::default();
        e.toggle_root();
        type_text(&mut e, "x");
        assert_eq!(e.col, JsonCol::Value);
        assert_eq!(json(&e), r#"["x"]"#);
    }

    #[test]
    fn bad_numbers_and_empty_keys_are_reported_by_row() {
        let mut e = JsonEditor::default();
        type_text(&mut e, "n");
        press(&mut e, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut e, KeyCode::Char('r'), KeyModifiers::CONTROL);
        type_text(&mut e, "12x");
        assert!(e.rows[0].is_invalid_number());
        assert!(e.to_value(true).unwrap_err().contains("row 1"));
        assert_eq!(e.to_value(false).unwrap().to_json(), r#"{"n":null}"#);

        let mut e = JsonEditor {
            col: JsonCol::Value,
            ..JsonEditor::default()
        };
        type_text(&mut e, "v");
        assert!(e.to_value(true).unwrap_err().contains("key is empty"));
        assert_eq!(e.to_value(false).unwrap().to_json(), "{}");
    }

    #[test]
    fn a_document_loads_back_into_the_editor() {
        let text = r#"{"a":"s","b":1.5,"c":false,"d":null,"e":{"f":[1,{"g":"h"}]}}"#;
        let value = JsonValue::parse(text).unwrap();
        let e = JsonEditor::from_value(&value).unwrap();
        assert_eq!(e.to_value(true).unwrap(), value);
        assert!(JsonEditor::from_value(&JsonValue::Null).is_none());
    }

    #[test]
    fn a_fresh_editor_is_empty() {
        assert!(JsonEditor::default().is_empty());
    }
}
