//! Remembered preferences, kept as one small JSON file in the app's config dir.

use crate::doc::write_atomic;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FileState {
    pub position: Option<(i32, i32)>,
    pub size: Option<(u32, u32)>,
    pub pinned: bool,
}

impl Default for FileState {
    fn default() -> Self {
        Self { position: None, size: None, pinned: true }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    /// `None` follows the system theme.
    pub theme: Option<String>,
    pub files: HashMap<String, FileState>,
}

impl Store {
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
