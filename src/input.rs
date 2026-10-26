//! A single-line text field with a cursor. Pure, so it is tested without a terminal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Default, Clone)]
pub struct Input {
    chars: Vec<char>,
    cursor: usize,
}

impl Input {
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars().filter(|c| !c.is_control()) {
            self.chars.insert(self.cursor, c);
            self.cursor += 1;
        }
    }

    #[cfg(test)]
    pub fn set(&mut self, s: &str) {
        self.clear();
        self.insert_str(s);
    }

    /// Apply a text-editing key. Returns false if the key isn't an editing key.
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u') if ctrl => self.clear(),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.chars.len(),
            KeyCode::Char(c) if !ctrl => {
                let mut buf = [0u8; 4];
                self.insert_str(c.encode_utf8(&mut buf));
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.chars.remove(self.cursor);
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.chars.len() {
                    self.chars.remove(self.cursor);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.chars.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.chars.len(),
            _ => return false,
        }
        true
    }

    /// The slice of the text that fits in `width` cells with the cursor kept in
    /// view, and the cursor's column within it.
    pub fn view(&self, width: u16) -> (String, u16) {
        let width = (width as usize).max(1);
        let start = (self.cursor + 1).saturating_sub(width);
        let shown: String = self.chars.iter().skip(start).take(width).collect();
        (shown, (self.cursor - start) as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn edits_at_the_cursor() {
        let mut i = Input::default();
        i.insert_str("abd");
        i.handle_key(&key(KeyCode::Left));
        i.handle_key(&key(KeyCode::Char('c')));
        assert_eq!(i.text(), "abcd");
        i.handle_key(&key(KeyCode::Backspace));
        i.handle_key(&key(KeyCode::Delete));
        assert_eq!(i.text(), "ab");
    }

    #[test]
    fn view_keeps_the_cursor_visible() {
        let mut i = Input::default();
        i.insert_str("0123456789");
        let (shown, col) = i.view(5);
        assert_eq!(shown, "6789");
        assert_eq!(col, 4);
        i.handle_key(&key(KeyCode::Home));
        assert_eq!(i.view(5), ("01234".to_string(), 0));
    }
}
