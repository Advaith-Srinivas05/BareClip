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

    /// Changes every remembered state the same way, as if it had always been so. A step that is
    /// left changing nothing goes, so undo and redo never seem to do nothing.
    pub fn rewrite(&mut self, current: &T, mut change: impl FnMut(&mut T))
    where
        T: PartialEq,
    {
        self.past.iter_mut().for_each(&mut change);
        self.future.iter_mut().for_each(&mut change);
        self.past = without_repeats(std::mem::take(&mut self.past), current);
        self.future = without_repeats(std::mem::take(&mut self.future), current);
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }
}

/// Drops states the same as the one nearer `current`. Both lists end nearest it.
fn without_repeats<T: PartialEq>(states: Vec<T>, current: &T) -> Vec<T> {
    let mut kept: Vec<T> = Vec::with_capacity(states.len());
    for state in states.into_iter().rev() {
        let repeat = kept
            .last()
            .map_or(state == *current, |nearer| state == *nearer);
        if !repeat {
            kept.push(state);
        }
    }
    kept.reverse();
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewriting_drops_the_steps_that_no_longer_change_anything() {
        let mut history = History::default();
        // Each state is a list of things; 9 is about to go for good.
        for state in [vec![1], vec![1, 9], vec![1, 9, 2], vec![1, 2, 9]] {
            history.record(state);
        }
        let current = vec![1, 2, 9, 3];
        assert_eq!(history.undo(current), Ok(vec![1, 2, 9]));
        assert_eq!(history.undo(vec![1, 2, 9]), Ok(vec![1, 9, 2]));
        let current = vec![1, 2];

        history.rewrite(&current, |state| state.retain(|thing| *thing != 9));

        assert_eq!(history.undo(current.clone()), Ok(vec![1]));
        assert_eq!(history.redo(vec![1]), Ok(vec![1, 2]));
        assert_eq!(history.redo(vec![1, 2]), Ok(vec![1, 2, 3]));
        assert!(!history.can_redo());
    }

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
