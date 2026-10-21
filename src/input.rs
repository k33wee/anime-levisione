use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

#[derive(Default, Debug, Clone)]
pub struct Input {
    pub value: String,
    cursor: usize,
}
impl Input {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.len();
        Self { value, cursor }
    }
    pub fn column(&self) -> usize {
        self.value[..self.cursor].width()
    }
    pub fn insert(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).take(200).collect();
        if self.value.chars().count() + text.chars().count() > 200 {
            return;
        }
        self.value.insert_str(self.cursor, &text);
        self.cursor += text.len();
    }
    pub fn key(&mut self, key: KeyEvent) -> bool {
        let before = self.value.clone();
        let previous = || {
            self.value[..self.cursor]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i)
        };
        let next = || {
            self.value[self.cursor..]
                .chars()
                .next()
                .map_or(self.cursor, |c| self.cursor + c.len_utf8())
        };
        match key.code {
            KeyCode::Left => self.cursor = previous(),
            KeyCode::Right => self.cursor = next(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.value.len(),
            KeyCode::Backspace if self.cursor > 0 => {
                let p = previous();
                self.value.drain(p..self.cursor);
                self.cursor = p;
            }
            KeyCode::Delete if self.cursor < self.value.len() => {
                let n = next();
                self.value.drain(self.cursor..n);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.value.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let prefix = &self.value[..self.cursor];
                let start = prefix
                    .trim_end()
                    .rfind(char::is_whitespace)
                    .map_or(0, |i| i + 1);
                self.value.drain(start..self.cursor);
                self.cursor = start;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert(&c.to_string())
            }
            _ => {}
        }
        before != self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edit_unicode_without_invalid_boundaries() {
        let mut input = Input::new("進撃é");
        input.key(KeyCode::Left.into());
        input.key(KeyCode::Backspace.into());
        input.insert("猫");
        assert_eq!(input.value, "進猫é");
        input.key(KeyCode::Home.into());
        input.key(KeyCode::Delete.into());
        assert_eq!(input.value, "猫é");
    }
    #[test]
    fn paste_is_bounded_and_cannot_inject_controls() {
        let mut input = Input::default();
        input.insert("hi\n\x1b\tthere");
        assert_eq!(input.value, "hithere");
        input.insert(&"x".repeat(300));
        assert!(input.value.len() <= 200);
    }
}
