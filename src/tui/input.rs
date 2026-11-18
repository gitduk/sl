//! Single-line text input with a cursor, shared by every panel field.
//!
//! The cursor is a byte offset kept on a char boundary, so movement and
//! deletion stay safe on UTF-8 input.

#[derive(Debug, Clone, Default)]
pub struct TextInput {
    value: String,
    cursor: usize,
}

impl TextInput {
    pub fn new(value: &str) -> Self {
        TextInput {
            value: value.to_string(),
            cursor: value.len(),
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// The text either side of the cursor, for rendering the caret in place.
    pub fn split(&self) -> (&str, &str) {
        self.value.split_at(self.cursor)
    }

    pub fn insert(&mut self, c: char) {
        self.value.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn insert_str(&mut self, text: &str) {
        self.value.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    pub fn backspace(&mut self) {
        let Some(prev) = self.value[..self.cursor].chars().next_back() else {
            return;
        };
        self.cursor -= prev.len_utf8();
        self.value.remove(self.cursor);
    }

    pub fn delete(&mut self) {
        if self.cursor < self.value.len() {
            self.value.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        if let Some(prev) = self.value[..self.cursor].chars().next_back() {
            self.cursor -= prev.len_utf8();
        }
    }

    pub fn right(&mut self) {
        if let Some(next) = self.value[self.cursor..].chars().next() {
            self.cursor += next.len_utf8();
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.value.len();
    }

    /// Deletes the separators before the cursor plus the word before them.
    /// `,` breaks a word too: in a pattern it separates one port from the next.
    pub fn delete_word_back(&mut self) {
        let mut start = self.cursor;
        while let Some(prev) = self.value[..start].chars().next_back() {
            if is_word_char(prev) {
                break;
            }
            start -= prev.len_utf8();
        }
        while let Some(prev) = self.value[..start].chars().next_back() {
            if !is_word_char(prev) {
                break;
            }
            start -= prev.len_utf8();
        }
        if start == self.cursor {
            return;
        }
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;
    }
}

/// A single-line field never holds whitespace here, so a paste drops it —
/// a copied list arrives with newlines the row syntax cannot use.
pub fn sanitize_paste(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn is_word_char(c: char) -> bool {
    !c.is_whitespace() && c != ','
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_the_cursor_at_the_end() {
        let input = TextInput::new("wk:8000");
        assert_eq!(input.split(), ("wk:8000", ""));
    }

    #[test]
    fn inserts_where_the_cursor_sits() {
        let mut input = TextInput::new("wk8000");
        input.home();
        input.right();
        input.right();
        input.insert(':');
        assert_eq!(input.value(), "wk:8000");
    }

    #[test]
    fn backspace_and_delete_remove_on_their_own_sides() {
        let mut input = TextInput::new("abc");
        input.left();
        input.backspace();
        assert_eq!(input.value(), "ac");
        input.home();
        input.delete();
        assert_eq!(input.value(), "c");
        input.delete();
        assert_eq!(input.value(), "");
        input.delete();
        assert_eq!(input.value(), "");
    }

    #[test]
    fn cursor_walks_char_boundaries_of_multibyte_text() {
        let mut input = TextInput::new("端口");
        input.left();
        input.insert('9');
        assert_eq!(input.value(), "端9口");
        input.backspace();
        assert_eq!(input.value(), "端口");
        input.home();
        input.delete();
        assert_eq!(input.value(), "口");
    }

    #[test]
    fn delete_word_back_stops_at_the_previous_port() {
        let mut input = TextInput::new("wk:8000,9200");
        input.delete_word_back();
        assert_eq!(input.value(), "wk:8000,");
        input.delete_word_back();
        assert_eq!(input.value(), "");
    }

    #[test]
    fn delete_word_back_crosses_separators() {
        let mut input = TextInput::new("wk:8000, 9200");
        input.delete_word_back();
        assert_eq!(input.value(), "wk:8000, ");
    }

    #[test]
    fn insert_str_lands_at_the_cursor() {
        let mut input = TextInput::new("w8000");
        input.home();
        input.right();
        input.insert_str("k:");
        assert_eq!(input.value(), "wk:8000");
        assert_eq!(input.split(), ("wk:", "8000"));
    }
}
