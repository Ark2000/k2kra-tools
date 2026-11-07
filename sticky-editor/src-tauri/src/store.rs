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
    pub color: String,
    /// Blurs what's behind the note while it is translucent.
    pub glass: bool,
    /// `None` follows the app theme.
    pub theme: Option<String>,
    /// The profile this note follows; changing the note by hand lets go of it.
    pub profile: Option<String>,
}

impl Default for FileState {
    fn default() -> Self {
        Self {
            position: None,
            size: None,
            pinned: true,
            opacity: 1.0,
            color: "yellow".to_owned(),
            glass: true,
            theme: None,
            profile: None,
        }
    }
}

/// A named look that notes can follow.
#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub theme: Option<String>,
    pub color: String,
    pub opacity: f64,
    pub glass: bool,
}

impl Profile {
    fn new(id: &str, name: &str, theme: Option<&str>, color: &str, opacity: f64) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            theme: theme.map(str::to_owned),
            color: color.to_owned(),
            opacity,
            glass: true,
        }
    }

    pub fn apply_to(&self, file: &mut FileState) {
        file.profile = Some(self.id.clone());
        file.theme = self.theme.clone();
        file.color = self.color.clone();
        file.opacity = self.opacity.clamp(0.3, 1.0);
        file.glass = self.glass;
    }
}

fn builtin_profiles() -> Vec<Profile> {
    vec![
        Profile::new("paper", "Paper", None, "yellow", 1.0),
        Profile::new("aero-light", "Aero Light", Some("light"), "blue", 0.5),
        Profile::new("aero-dark", "Aero Dark", Some("dark"), "blue", 0.5),
    ]
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
    pub profiles: Vec<Profile>,
    /// The profile new notes start with.
    pub default_profile: String,
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
        let mut store: Self = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        store.ensure_profiles();
        store
    }

    fn ensure_profiles(&mut self) {
        if self.profiles.is_empty() {
            self.profiles = builtin_profiles();
        }
        if !self.profiles.iter().any(|p| p.id == self.default_profile) {
            self.default_profile = self.profiles[0].id.clone();
        }
    }

    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// How a file that has never been opened starts out.
    pub fn new_file_state(&self) -> FileState {
        let mut file = FileState::default();
        if let Some(profile) = self.profile(&self.default_profile) {
            profile.apply_to(&mut file);
        }
        file
    }

    /// Adds or updates a profile, and carries the change to the notes following it.
    pub fn save_profile(&mut self, profile: Profile) {
        for file in self.files.values_mut() {
            if file.profile.as_deref() == Some(&profile.id) {
                profile.apply_to(file);
            }
        }
        match self.profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => *existing = profile,
            None => self.profiles.push(profile),
        }
    }

    /// Notes following the profile keep their look. The last profile stays.
    pub fn delete_profile(&mut self, id: &str) -> bool {
        if self.profiles.len() < 2 || self.profile(id).is_none() {
            return false;
        }
        self.profiles.retain(|p| p.id != id);
        for file in self.files.values_mut() {
            if file.profile.as_deref() == Some(id) {
                file.profile = None;
            }
        }
        self.ensure_profiles();
        true
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

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_note(profile: Option<&str>) -> Store {
        let mut store = Store::default();
        store.ensure_profiles();
        let file = FileState { profile: profile.map(str::to_owned), ..FileState::default() };
        store.files.insert("note".to_owned(), file);
        store
    }

    #[test]
    fn a_new_store_starts_with_paper_as_default() {
        let store = store_with_note(None);
        assert_eq!(store.default_profile, "paper");
        assert_eq!(store.new_file_state().profile.as_deref(), Some("paper"));
    }

    #[test]
    fn saving_a_profile_updates_notes_following_it() {
        let mut store = store_with_note(Some("aero-dark"));
        let mut profile = store.profile("aero-dark").unwrap().clone();
        profile.opacity = 0.7;
        profile.color = "green".to_owned();
        store.save_profile(profile);
        let file = &store.files["note"];
        assert_eq!((file.opacity, file.color.as_str()), (0.7, "green"));
        assert_eq!(file.theme.as_deref(), Some("dark"));
    }

    #[test]
    fn saving_a_profile_leaves_other_notes_alone() {
        let mut store = store_with_note(None);
        let mut profile = store.profile("aero-dark").unwrap().clone();
        profile.color = "green".to_owned();
        store.save_profile(profile);
        assert_eq!(store.files["note"].color, "yellow");
    }

    #[test]
    fn deleting_a_profile_keeps_the_note_look_and_moves_the_default() {
        let mut store = store_with_note(Some("paper"));
        assert!(store.delete_profile("paper"));
        assert_eq!(store.files["note"].profile, None);
        assert_eq!(store.files["note"].color, "yellow");
        assert_eq!(store.default_profile, "aero-light");
    }

    #[test]
    fn the_last_profile_cannot_be_deleted() {
        let mut store = store_with_note(None);
        assert!(store.delete_profile("aero-light"));
        assert!(store.delete_profile("aero-dark"));
        assert!(!store.delete_profile("paper"));
    }
}
