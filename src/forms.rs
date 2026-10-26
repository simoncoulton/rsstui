//! Form state for the tag checklist and the folder chooser. Pure key handling,
//! so it is tested without a terminal; `ui.rs` draws it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{input::Input, tags};

pub struct TagRow {
    pub name: String,
    pub checked: bool,
    /// How many feeds carry the tag (0 for one only an action mentions).
    pub count: usize,
}

/// Every known tag as a checkbox, plus a field to add a new one. Unchecking a
/// tag removes it.
pub struct TagChecklist {
    /// Sorted by name.
    pub rows: Vec<TagRow>,
    /// 0 is the new-tag field; `n` is `rows[n - 1]`.
    pub cursor: usize,
    pub input: Input,
    pub error: Option<String>,
    /// A tag waiting for y/n before it is deleted everywhere.
    pub confirm_delete: Option<String>,
    /// A tag the user confirmed deleting; the owner takes it with
    /// [`TagChecklist::take_deleted`] and decides whether it may go.
    deleted: Option<String>,
}

impl TagChecklist {
    /// `known` are the tags to offer with their use counts; `checked` are
    /// ticked (and offered even when not in `known`).
    pub fn new(known: Vec<(String, usize)>, checked: &[String]) -> Self {
        let mut rows: Vec<TagRow> = Vec::new();
        for (name, count) in known {
            rows.push(TagRow {
                name,
                checked: false,
                count,
            });
        }
        for name in checked {
            match rows.iter_mut().find(|r| &r.name == name) {
                Some(r) => r.checked = true,
                None => rows.push(TagRow {
                    name: name.clone(),
                    checked: true,
                    count: 0,
                }),
            }
        }
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            // Start on the first tag when there is one, else on the field.
            cursor: usize::from(!rows.is_empty()),
            rows,
            input: Input::default(),
            error: None,
            confirm_delete: None,
            deleted: None,
        }
    }

    pub fn take_deleted(&mut self) -> Option<String> {
        self.deleted.take()
    }

    /// Drop a tag's row (after it was deleted everywhere).
    pub fn remove(&mut self, name: &str) {
        self.rows.retain(|r| r.name != name);
        self.cursor = self.cursor.min(self.rows.len());
    }

    pub fn checked(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|r| r.checked)
            .map(|r| r.name.clone())
            .collect()
    }

    pub fn on_input(&self) -> bool {
        self.cursor == 0
    }

    /// Add what is typed as new, checked tags (existing ones get checked).
    /// Returns whether anything was typed and accepted.
    fn add_typed(&mut self) -> bool {
        match tags::parse_tags(&self.input.text()) {
            Ok(list) => {
                for name in list {
                    match self.rows.binary_search_by(|r| r.name.cmp(&name)) {
                        Ok(i) => self.rows[i].checked = true,
                        Err(i) => self.rows.insert(
                            i,
                            TagRow {
                                name,
                                checked: true,
                                count: 0,
                            },
                        ),
                    }
                }
                self.input.clear();
                self.error = None;
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }

    /// Returns true when the user asked to save: Enter on a tag, or on the
    /// field with nothing typed. Enter with text typed adds the tag instead.
    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let typed = !self.input.text().trim().is_empty();
        if self.confirm_delete.is_some() {
            // Only `y` confirms; any other key backs out.
            if key.code == KeyCode::Char('y') {
                self.deleted = self.confirm_delete.take();
            } else {
                self.confirm_delete = None;
            }
            return false;
        }
        match key.code {
            KeyCode::Delete if !self.on_input() => {
                self.confirm_delete = Some(self.rows[self.cursor - 1].name.clone());
            }
            KeyCode::Char('d') if ctrl && !self.on_input() => {
                self.confirm_delete = Some(self.rows[self.cursor - 1].name.clone());
            }
            KeyCode::Down => self.cursor = (self.cursor + 1).min(self.rows.len()),
            KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Enter if self.on_input() && typed => {
                self.add_typed();
            }
            KeyCode::Enter => {
                // Text typed but not added yet counts too.
                return !typed || self.add_typed();
            }
            KeyCode::Char('j') if !self.on_input() && !ctrl => {
                self.cursor = (self.cursor + 1).min(self.rows.len());
            }
            KeyCode::Char('k') if !self.on_input() && !ctrl => {
                self.cursor = self.cursor.saturating_sub(1);
            }
            KeyCode::Char(' ') if !self.on_input() => {
                let row = &mut self.rows[self.cursor - 1];
                row.checked = !row.checked;
                self.error = None;
            }
            code => {
                // Typing on a tag jumps to the field so a new tag can be
                // started without moving first.
                if !self.on_input() {
                    if matches!(code, KeyCode::Char(_)) && !ctrl {
                        self.cursor = 0;
                    } else {
                        return false;
                    }
                }
                if self.input.handle_key(key) {
                    self.error = None;
                }
            }
        }
        false
    }
}

/// Choose (or create) the folder a feed lives in.
pub struct FolderEditor {
    pub feed: usize,
    pub input: Input,
    /// Existing folders, in the order the Feeds pane shows them.
    pub folders: Vec<String>,
    /// 0 is the new-folder field, 1 is "no folder", `n >= 2` is `folders[n - 2]`.
    pub cursor: usize,
    pub error: Option<String>,
}

/// What the user chose.
#[derive(Debug, PartialEq)]
pub enum FolderChoice {
    None,
    Folder(String),
}

impl FolderEditor {
    pub fn new(feed: usize, folders: Vec<String>, current: Option<&str>) -> Self {
        let cursor = match current {
            None => 1,
            Some(c) => folders.iter().position(|f| f == c).map_or(1, |i| i + 2),
        };
        Self {
            feed,
            input: Input::default(),
            folders,
            cursor,
            error: None,
        }
    }

    pub fn on_input(&self) -> bool {
        self.cursor == 0
    }

    /// `Some` when the user picked something; `None` to keep the dialog open.
    pub fn handle_key(&mut self, key: &KeyEvent) -> Option<FolderChoice> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let last = self.folders.len() + 1;
        match key.code {
            KeyCode::Down => self.cursor = (self.cursor + 1).min(last),
            KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Char('j') if !self.on_input() && !ctrl => {
                self.cursor = (self.cursor + 1).min(last)
            }
            KeyCode::Char('k') if !self.on_input() && !ctrl => {
                self.cursor = self.cursor.saturating_sub(1)
            }
            KeyCode::Enter => {
                if self.on_input() {
                    let text = self.input.text();
                    if text.trim().is_empty() {
                        return None;
                    }
                    return match tags::parse_folder(&text) {
                        Ok(name) => {
                            // Reuse an existing folder's spelling.
                            let name = self
                                .folders
                                .iter()
                                .find(|f| f.to_lowercase() == name.to_lowercase())
                                .cloned()
                                .unwrap_or(name);
                            Some(FolderChoice::Folder(name))
                        }
                        Err(e) => {
                            self.error = Some(e);
                            None
                        }
                    };
                }
                return Some(match self.cursor {
                    1 => FolderChoice::None,
                    n => FolderChoice::Folder(self.folders[n - 2].clone()),
                });
            }
            code => {
                // Typing on a list entry jumps to the field, as in the tag list.
                if !self.on_input() {
                    if matches!(code, KeyCode::Char(_)) && !ctrl {
                        self.cursor = 0;
                    } else {
                        return None;
                    }
                }
                if self.input.handle_key(key) {
                    self.error = None;
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn type_text(list: &mut TagChecklist, s: &str) {
        for c in s.chars() {
            list.handle_key(&key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn tag_field_takes_command_letters() {
        let mut list = TagChecklist::new(vec![("news".into(), 2)], &[]);
        list.handle_key(&key(KeyCode::Up)); // onto the field
        type_text(&mut list, "jkq d");
        assert_eq!(list.input.text(), "jkq d");
        assert!(list.on_input());
    }

    #[test]
    fn typing_on_a_row_jumps_to_the_field_and_enter_adds_then_saves() {
        let mut list = TagChecklist::new(vec![("news".into(), 2)], &[]);
        assert_eq!(list.cursor, 1);
        type_text(&mut list, "rust");
        assert!(list.on_input());
        assert!(!list.handle_key(&key(KeyCode::Enter)), "first Enter adds");
        assert_eq!(list.checked(), ["rust"]);
        assert!(list.handle_key(&key(KeyCode::Enter)), "second Enter saves");
    }

    #[test]
    fn space_toggles_and_delete_asks_first() {
        let mut list = TagChecklist::new(vec![("news".into(), 2)], &[]);
        list.handle_key(&key(KeyCode::Char(' ')));
        assert_eq!(list.checked(), ["news"]);
        list.handle_key(&key(KeyCode::Delete));
        assert_eq!(list.confirm_delete.as_deref(), Some("news"));
        list.handle_key(&key(KeyCode::Esc));
        assert!(list.confirm_delete.is_none() && list.take_deleted().is_none());
        list.handle_key(&key(KeyCode::Delete));
        list.handle_key(&key(KeyCode::Char('y')));
        assert_eq!(list.take_deleted().as_deref(), Some("news"));
    }

    #[test]
    fn bad_tags_report_an_error_and_keep_the_text() {
        let mut list = TagChecklist::new(vec![], &[]);
        type_text(&mut list, "a:b");
        list.handle_key(&key(KeyCode::Enter));
        assert!(list.error.is_some());
        assert_eq!(list.input.text(), "a:b");
    }

    #[test]
    fn folder_editor_picks_entries_and_new_names() {
        let folders = vec!["News".to_string(), "Writers".to_string()];
        let mut ed = FolderEditor::new(0, folders.clone(), Some("Writers"));
        assert_eq!(ed.cursor, 3);
        assert_eq!(
            ed.handle_key(&key(KeyCode::Enter)),
            Some(FolderChoice::Folder("Writers".into()))
        );

        let mut ed = FolderEditor::new(0, folders.clone(), None);
        assert_eq!(ed.cursor, 1);
        assert_eq!(
            ed.handle_key(&key(KeyCode::Enter)),
            Some(FolderChoice::None)
        );

        // Typing jumps to the field; letters (even j/k/q) are text there.
        let mut ed = FolderEditor::new(0, folders, None);
        for c in "news".chars() {
            assert!(ed.handle_key(&key(KeyCode::Char(c))).is_none());
        }
        assert!(ed.on_input());
        assert_eq!(
            ed.handle_key(&key(KeyCode::Enter)),
            Some(FolderChoice::Folder("News".into())),
            "existing spelling reused"
        );
    }

    #[test]
    fn empty_folder_field_does_nothing() {
        let mut ed = FolderEditor::new(0, vec![], None);
        ed.cursor = 0;
        assert!(ed.handle_key(&key(KeyCode::Enter)).is_none());
    }
}
