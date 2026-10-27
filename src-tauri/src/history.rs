//! Snapshot-based undo/redo. Projects are small (audio data lives in the asset
//! store, not the model), so a full clone per step is cheap and always correct.

use crate::model::Project;
use std::time::{Duration, Instant};

const MAX_STEPS: usize = 300;
const MERGE_WINDOW: Duration = Duration::from_millis(1500);

pub struct History {
    undo: Vec<Project>,
    redo: Vec<Project>,
    last_key: Option<String>,
    last_time: Instant,
}

impl Default for History {
    fn default() -> Self {
        History { undo: vec![], redo: vec![], last_key: None, last_time: Instant::now() }
    }
}

impl History {
    /// Record the state from before an edit. Consecutive edits with the same
    /// merge key inside the merge window collapse into one undo step.
    pub fn record(&mut self, before: Project, key: Option<String>) {
        let merge = key.is_some() && key == self.last_key && self.last_time.elapsed() < MERGE_WINDOW;
        if !merge {
            self.undo.push(before);
            if self.undo.len() > MAX_STEPS {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_key = key;
        self.last_time = Instant::now();
    }

    pub fn undo(&mut self, current: &mut Project) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(current, prev));
                self.last_key = None;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, current: &mut Project) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(current, next));
                self.last_key = None;
                true
            }
            None => false,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.last_key = None;
    }
}
