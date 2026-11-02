//! Undo and redo, by keeping earlier copies of the project.

const DEFAULT_LIMIT: usize = 200;

#[derive(Clone, Debug)]
pub struct History<T> {
    past: Vec<T>,
    future: Vec<T>,
    limit: usize,
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self {
            past: Vec::new(),
            future: Vec::new(),
            limit: DEFAULT_LIMIT,
        }
    }
}

impl<T> History<T> {
    /// Remembers the state from just before a change. A new change ends the chance
    /// to redo anything undone earlier.
    pub fn record(&mut self, before: T) {
        self.past.push(before);
        if self.past.len() > self.limit {
            self.past.remove(0);
        }
        self.future.clear();
    }

    /// Goes back one step, keeping `current` so it can be redone.
    pub fn undo(&mut self, current: T) -> Result<T, T> {
        match self.past.pop() {
            Some(previous) => {
                self.future.push(current);
                Ok(previous)
            }
            None => Err(current),
        }
    }

    /// Goes forward one step, keeping `current` so it can be undone.
    pub fn redo(&mut self, current: T) -> Result<T, T> {
        match self.future.pop() {
            Some(next) => {
                self.past.push(current);
                Ok(next)
            }
            None => Err(current),
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_walk_through_the_changes() {
        let mut history = History::default();
        history.record(1);
        history.record(2);

        assert_eq!(history.undo(3), Ok(2));
        assert_eq!(history.undo(2), Ok(1));
        assert_eq!(history.redo(1), Ok(2));
        assert_eq!(history.redo(2), Ok(3));
    }

    #[test]
    fn with_nothing_to_do_the_state_comes_back_unchanged() {
        let mut history: History<i32> = History::default();

        assert_eq!(history.undo(5), Err(5));
        assert_eq!(history.redo(5), Err(5));
        assert!(!history.can_undo());
        assert!(!history.can_redo());
    }

    #[test]
    fn a_new_change_ends_any_redo() {
        let mut history = History::default();
        history.record(1);
        assert_eq!(history.undo(2), Ok(1));
        assert!(history.can_redo());

        history.record(1);

        assert!(!history.can_redo());
    }

    #[test]
    fn only_the_newest_steps_are_kept() {
        let mut history = History {
            limit: 2,
            ..Default::default()
        };
        history.record(1);
        history.record(2);
        history.record(3);

        assert_eq!(history.undo(4), Ok(3));
        assert_eq!(history.undo(3), Ok(2));
        assert_eq!(history.undo(2), Err(2));
    }
}
