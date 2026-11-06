//! Remembered preferences, kept as one small JSON file in the app's config dir.

use crate::doc::write_atomic;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

const MAX_RECENT: usize = 15;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FileState {
    pub position: Option<(i32, i32)>,
    pub size: Option<(u32, u32)>,
    pub pinned: bool,
    pub opacity: f64,
}

impl Default for FileState {
    fn default() -> Self {
        Self { position: None, size: None, pinned: true, opacity: 1.0 }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// `None` follows the system theme.
    pub theme: Option<String>,
    pub files: HashMap<String, FileState>,
    /// Most recent first.
    pub recent: Vec<String>,
    /// Notes not yet closed by the user; reopened at sign-in.
    pub open: Vec<String>,
}

fn same_path(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

impl Store {
    pub fn note_opened(&mut self, path: &str) {
        self.recent.retain(|p| !same_path(p, path));
        self.recent.insert(0, path.to_owned());
        self.recent.truncate(MAX_RECENT);
        if !self.open.iter().any(|p| same_path(p, path)) {
            self.open.push(path.to_owned());
        }
    }

    pub fn note_closed(&mut self, path: &str) {
        self.open.retain(|p| !same_path(p, path));
    }

    pub fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        let written = serde_json::to_vec_pretty(self)
            .map_err(std::io::Error::from)
            .and_then(|bytes| write_atomic(path, &bytes));
        if let Err(e) = written {
            eprintln!("could not save state: {e}");
        }
    }
}
