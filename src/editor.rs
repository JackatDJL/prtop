//! Minimal multiline text editor used by the create wizard, metadata editing, and comments.
//! Cursor-aware, but deliberately simple: no word wrapping, no syntax, no undo.

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TextArea {
    lines: Vec<String>,
    pub row: usize,
    pub col: usize,
}
impl TextArea {
    pub fn from_str(text: &str) -> Self {
        Self {
            lines: text.split('\n').map(str::to_owned).collect(),
            row: 0,
            col: 0,
        }
    }
    pub fn empty() -> Self {
        Self::from_str("")
    }
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }
    pub fn line(&self, row: usize) -> &str {
        self.lines.get(row).map(String::as_str).unwrap_or("")
    }
    pub fn lines(&self) -> usize {
        self.lines.len()
    }
    pub fn insert_char(&mut self, c: char) {
        let line = &mut self.lines[self.row];
        let byte = self.col.clamp(0, line.len());
        line.insert(byte, c);
        self.col = byte + c.len_utf8();
    }
    pub fn newline(&mut self) {
        let line = &mut self.lines[self.row];
        let byte = self.col.clamp(0, line.len());
        let rest = line[byte..].to_owned();
        line.truncate(byte);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
    }
    pub fn backspace(&mut self) {
        if self.col > 0 {
            let line = &mut self.lines[self.row];
            let byte = self.col.clamp(0, line.len());
            if let Some(previous) = line[..byte].chars().next_back() {
                line.remove(byte - previous.len_utf8());
                self.col = byte - previous.len_utf8();
            }
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].push_str(&line);
        }
    }
    pub fn left(&mut self) {
        if self.col > 0 {
            self.col = self
                .line(self.row)[..self.col]
                .chars()
                .next_back()
                .map(|c| self.col - c.len_utf8())
                .unwrap_or(0);
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
    }
    pub fn right(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.col = self.line(self.row)[self.col..]
                .chars()
                .next()
                .map(|c| self.col + c.len_utf8())
                .unwrap_or(self.lines[self.row].len());
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }
    pub fn up(&mut self) {
        self.row = self.row.saturating_sub(1);
        self.col = self.col.min(self.lines[self.row].len());
    }
    pub fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].len());
        }
    }
    pub fn home(&mut self) {
        self.col = 0;
    }
    pub fn end(&mut self) {
        self.col = self.lines[self.row].len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_multiline_text_with_a_cursor() {
        let mut area = TextArea::from_str("hello\nworld");
        area.end();
        area.insert_char('!');
        assert_eq!(area.text(), "hello\nworld!");
        area.newline();
        area.insert_char('x');
        assert_eq!(area.text(), "hello\nworld!\nx");
    }

    #[test]
    fn backspace_joins_lines_at_column_zero() {
        let mut area = TextArea::from_str("ab\ncd");
        area.down();
        area.home();
        area.backspace();
        assert_eq!(area.text(), "abcd");
        assert_eq!((area.row, area.col), (0, 2));
    }

    #[test]
    fn navigation_clamps_to_line_bounds() {
        let mut area = TextArea::from_str("ab\nc");
        area.down();
        area.end();
        area.right();
        assert_eq!(area.col, 1);
        area.up();
        assert_eq!((area.row, area.col), (0, 2));
    }

    #[test]
    fn multibyte_characters_move_by_character_not_byte() {
        let mut area = TextArea::from_str("éé");
        area.right();
        area.right();
        assert_eq!(area.col, 4);
        area.backspace();
        assert_eq!(area.text(), "é");
    }
}
