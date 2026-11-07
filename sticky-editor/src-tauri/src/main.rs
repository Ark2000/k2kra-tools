#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod doc;
mod store;
#[cfg(windows)]
mod topmost;

use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use store::{FileState, Profile, Store};
use tauri_plugin_autostart::MacosLauncher;
use tauri::{
    utils::config::WindowEffectsConfig,
    window::{Color, Effect, EffectsBuilder},
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, Window, WindowEvent,
};

const WELCOME: &str = "welcome";
const SETTINGS: &str = "settings";
/// Passed by the sign-in entry: reopen the last notes, or quietly do nothing.
const RESTORE: &str = "--restore";
const STORE_SAVE_INTERVAL: Duration = Duration::from_secs(2);

/// The file behind one note window. Commands find it by window label, so the
/// webview can only ever touch the file its window was opened on.
struct Session {
    path: PathBuf,
    key: String,
    doc: Option<doc::Doc>,
}

struct AppState {
    sessions: Mutex<HashMap<String, Session>>,
    store: Mutex<Store>,
    store_path: PathBuf,
    store_dirty: AtomicBool,
}

impl AppState {
    fn save_store(&self) {
        self.store.lock().unwrap().save(&self.store_path);
    }

    /// Updates the store and leaves the write to `save_periodically`.
    fn update_store(&self, update: impl FnOnce(&mut Store)) {
        update(&mut self.store.lock().unwrap());
        self.store_dirty.store(true, Ordering::Relaxed);
    }
}

/// A system shutdown kills the process without closing its windows, so state
/// can't wait for a window to close before reaching the disk.
fn save_periodically(app: AppHandle) {
    loop {
        std::thread::sleep(STORE_SAVE_INTERVAL);
        let state = app.state::<AppState>();
        if state.store_dirty.swap(false, Ordering::Relaxed) {
            state.save_store();
        }
    }
}

/// Stable identity for a path; Windows paths are case-insensitive.
fn file_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn label_for(key: &str) -> String {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    format!("note-{:016x}", hasher.finish())
}

fn resolve(path: &str, cwd: &Path) -> PathBuf {
    let joined = cwd.join(path);
    dunce::canonicalize(&joined).unwrap_or(joined)
}

fn focus(window: &WebviewWindow) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

fn open_welcome(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(WELCOME) {
        focus(&window);
        return Ok(());
    }
    note_window(app, WELCOME, "Sticky Editor", &FileState::default())?;
    Ok(())
}

fn open_file(app: &AppHandle, path: PathBuf) -> tauri::Result<()> {
    let key = file_key(&path);
    let label = label_for(&key);
    if let Some(window) = app.get_webview_window(&label) {
        focus(&window);
        return Ok(());
    }
    let state = app.state::<AppState>();
    let saved = {
        let mut store = state.store.lock().unwrap();
        let fresh = store.new_file_state();
        store.files.entry(key.clone()).or_insert(fresh).clone()
    };
    let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    state.sessions.lock().unwrap().insert(label.clone(), Session { path, key, doc: None });
    note_window(app, &label, &title, &saved)?;
    Ok(())
}

/// Only a translucent note has anything to blur; an opaque one skips the cost.
fn glass_effects(saved: &FileState) -> Option<WindowEffectsConfig> {
    (saved.glass && saved.opacity < 1.0)
        .then(|| EffectsBuilder::new().effect(Effect::Acrylic).color(Color(255, 255, 255, 1)).build())
}

/// Built hidden; the page shows it once it has content, to avoid a blank flash.
fn note_window(
    app: &AppHandle,
    label: &str,
    title: &str,
    saved: &FileState,
) -> tauri::Result<WebviewWindow> {
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(title)
        .decorations(false)
        .transparent(true)
        .always_on_top(saved.pinned)
        .inner_size(380.0, 420.0)
        .min_inner_size(220.0, 140.0)
        .visible(false)
        .build()?;

    window.set_effects(glass_effects(saved))?;
    if let Some((w, h)) = saved.size {
        window.set_size(PhysicalSize::new(w, h))?;
    }
    if let Some((x, y)) = saved.position.filter(|p| on_screen(&window, *p)) {
        window.set_position(PhysicalPosition::new(x, y))?;
    }
    Ok(window)
}

/// A saved position may belong to a monitor that is no longer attached.
fn on_screen(window: &WebviewWindow, (x, y): (i32, i32)) -> bool {
    let (x, y) = (x + 40, y + 12);
    window.available_monitors().unwrap_or_default().iter().any(|m| {
        let (pos, size) = (m.position(), m.size());
        x >= pos.x
            && y >= pos.y
            && x < pos.x + size.width as i32
            && y < pos.y + size.height as i32
    })
}

/// The most recently opened file that still exists.
fn last_file(app: &AppHandle) -> Option<PathBuf> {
    let state = app.state::<AppState>();
    let store = state.store.lock().unwrap();
    store.recent.iter().find(|p| Path::new(p).exists()).map(PathBuf::from)
}

fn open_all(app: &AppHandle, paths: Vec<PathBuf>) {
    let result = if paths.is_empty() {
        match last_file(app) {
            Some(path) => open_file(app, path),
            None => open_welcome(app),
        }
    } else {
        paths.into_iter().try_for_each(|path| open_file(app, path))
    };
    if let Err(e) = result {
        eprintln!("could not open window: {e}");
    }
}

fn paths_from_args(args: impl Iterator<Item = String>, cwd: &Path) -> Vec<PathBuf> {
    args.map(|arg| resolve(&arg, cwd)).collect()
}

fn restore(app: &AppHandle) {
    let paths: Vec<PathBuf> = {
        let state = app.state::<AppState>();
        let mut store = state.store.lock().unwrap();
        store.open.retain(|p| Path::new(p).exists());
        store.open.iter().map(PathBuf::from).collect()
    };
    if paths.is_empty() {
        std::process::exit(0);
    }
    open_all(app, paths);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocView {
    path: String,
    name: String,
    text: String,
    encoding: String,
    eol: &'static str,
    readonly: bool,
    notice: Option<String>,
}

#[derive(Serialize)]
struct Prefs {
    theme: Option<String>,
    pinned: bool,
    opacity: f64,
    color: String,
    glass: bool,
    profile: Option<String>,
}

#[tauri::command]
fn prefs(window: Window, state: tauri::State<AppState>) -> Prefs {
    let key = state.sessions.lock().unwrap().get(window.label()).map(|s| s.key.clone());
    let store = state.store.lock().unwrap();
    let saved = key
        .and_then(|k| store.files.get(&k).cloned())
        .unwrap_or_else(|| store.new_file_state());
    Prefs {
        theme: saved.theme.or_else(|| store.theme.clone()),
        pinned: saved.pinned,
        opacity: saved.opacity,
        color: saved.color,
        glass: saved.glass,
        profile: saved.profile,
    }
}

/// Recently opened files that still exist.
#[tauri::command]
fn recent(state: tauri::State<AppState>) -> Vec<String> {
    let store = state.store.lock().unwrap();
    store.recent.iter().filter(|p| Path::new(p).exists()).cloned().collect()
}

#[tauri::command]
fn clear_recent(state: tauri::State<AppState>) {
    state.update_store(|store| store.recent.clear());
}

#[tauri::command]
fn set_opacity(window: Window, state: tauri::State<AppState>, opacity: f64) {
    remember(&window, &state, |file| {
        file.opacity = opacity.clamp(0.3, 1.0);
        file.profile = None;
    });
    refresh_glass(window.app_handle(), window.label());
}

#[tauri::command]
fn set_glass(window: Window, state: tauri::State<AppState>, glass: bool) {
    remember(&window, &state, |file| {
        file.glass = glass;
        file.profile = None;
    });
    refresh_glass(window.app_handle(), window.label());
}

fn refresh_glass(app: &AppHandle, label: &str) {
    let state = app.state::<AppState>();
    let key = state.sessions.lock().unwrap().get(label).map(|s| s.key.clone());
    let saved = key.and_then(|k| state.store.lock().unwrap().files.get(&k).cloned());
    if let (Some(saved), Some(window)) = (saved, app.get_webview_window(label)) {
        if let Err(e) = window.set_effects(glass_effects(&saved)) {
            eprintln!("could not change glass: {e}");
        }
    }
}

/// Tells every open note to take on its look again.
fn visuals_changed(app: &AppHandle) {
    let labels: Vec<String> =
        app.state::<AppState>().sessions.lock().unwrap().keys().cloned().collect();
    for label in labels {
        refresh_glass(app, &label);
    }
    let _ = app.emit("visual-changed", ());
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileList {
    profiles: Vec<Profile>,
    default_profile: String,
}

#[tauri::command]
fn profiles(state: tauri::State<AppState>) -> ProfileList {
    let store = state.store.lock().unwrap();
    ProfileList { profiles: store.profiles.clone(), default_profile: store.default_profile.clone() }
}

#[tauri::command]
fn save_profile(app: AppHandle, state: tauri::State<AppState>, profile: Profile) {
    state.update_store(|store| store.save_profile(profile));
    visuals_changed(&app);
}

#[tauri::command]
fn delete_profile(app: AppHandle, state: tauri::State<AppState>, id: String) {
    state.update_store(|store| {
        store.delete_profile(&id);
    });
    visuals_changed(&app);
}

#[tauri::command]
fn set_default_profile(state: tauri::State<AppState>, id: String) {
    state.update_store(|store| store.default_profile = id);
}

/// Makes this window's note follow a profile.
#[tauri::command]
fn use_profile(window: Window, state: tauri::State<AppState>, id: String) {
    let profile = state.store.lock().unwrap().profile(&id).cloned();
    if let Some(profile) = profile {
        remember(&window, &state, |file| profile.apply_to(file));
        visuals_changed(window.app_handle());
    }
}

/// Async: creating a webview from a sync command deadlocks on Windows.
#[tauri::command]
async fn open_settings(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(SETTINGS) {
        focus(&window);
        return Ok(());
    }
    WebviewWindowBuilder::new(&app, SETTINGS, WebviewUrl::App("settings.html".into()))
        .title("Sticky Editor Settings")
        .always_on_top(true)
        .inner_size(520.0, 380.0)
        .min_inner_size(420.0, 320.0)
        .center()
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn set_color(window: Window, state: tauri::State<AppState>, color: String) {
    remember(&window, &state, |file| {
        file.color = color;
        file.profile = None;
    });
}

/// Returns `None` for the welcome window, which has no file.
#[tauri::command(async)]
fn doc_load(window: Window, state: tauri::State<AppState>) -> Result<Option<DocView>, String> {
    let Some(path) = state.sessions.lock().unwrap().get(window.label()).map(|s| s.path.clone())
    else {
        return Ok(None);
    };
    let loaded = doc::load(&path)?;
    let view = DocView {
        path: path.to_string_lossy().into_owned(),
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        text: loaded.text,
        encoding: loaded.doc.encoding.label(),
        eol: loaded.doc.eol.label(),
        readonly: loaded.doc.readonly,
        notice: loaded.notice,
    };
    if let Some(session) = state.sessions.lock().unwrap().get_mut(window.label()) {
        session.doc = Some(loaded.doc);
    }
    state.update_store(|store| store.note_opened(&view.path));
    Ok(Some(view))
}

#[tauri::command(async)]
fn doc_save(
    window: Window,
    state: tauri::State<AppState>,
    text: String,
    force: bool,
) -> Result<(), doc::SaveError> {
    let mut sessions = state.sessions.lock().unwrap();
    let session = sessions.get_mut(window.label());
    match session.and_then(|s| Some((s.path.clone(), s.doc.as_mut()?))) {
        Some((path, doc)) => doc::save(&path, doc, &text, force),
        None => Err(doc::SaveError { kind: "io", message: "No document is open.".into() }),
    }
}

/// A Markdown image, resolved against the note's own folder.
#[tauri::command(async)]
fn doc_image(window: Window, state: tauri::State<AppState>, src: String) -> Result<String, String> {
    let path = state.sessions.lock().unwrap().get(window.label()).map(|s| s.path.clone());
    let dir = path.as_deref().and_then(Path::parent).ok_or("No document is open.")?;
    doc::image_url(dir, &src)
}

/// `same`, `changed` or `deleted`, compared with what this window last saw.
#[tauri::command(async)]
fn doc_check(window: Window, state: tauri::State<AppState>) -> Result<&'static str, String> {
    let mut sessions = state.sessions.lock().unwrap();
    let Some(session) = sessions.get_mut(window.label()) else { return Ok("same") };
    let Some(doc) = &mut session.doc else { return Ok("same") };
    Ok(match doc::refresh(&session.path, doc).map_err(|e| e.to_string())? {
        doc::Disk::Same => "same",
        doc::Disk::Changed => "changed",
        doc::Disk::Deleted => "deleted",
    })
}

#[tauri::command]
fn set_pinned(window: Window, state: tauri::State<AppState>, pinned: bool) -> Result<(), String> {
    window.set_always_on_top(pinned).map_err(|e| e.to_string())?;
    remember(&window, &state, |file| file.pinned = pinned);
    Ok(())
}

#[tauri::command]
fn set_theme(window: Window, state: tauri::State<AppState>, theme: String) {
    state.update_store(|store| store.theme = Some(theme));
    remember(&window, &state, |file| {
        if file.theme.take().is_some() {
            file.profile = None;
        }
    });
}

/// Async: creating a webview from a sync command deadlocks on Windows.
#[tauri::command]
async fn open_files(app: AppHandle, window: Window, paths: Vec<String>) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    open_all(&app, paths_from_args(paths.into_iter(), &cwd));
    if window.label() == WELCOME {
        window.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn remember(window: &Window, state: &AppState, update: impl FnOnce(&mut FileState)) {
    let key = state.sessions.lock().unwrap().get(window.label()).map(|s| s.key.clone());
    if let Some(key) = key {
        state.update_store(|store| update(store.files.entry(key).or_default()));
    }
}

fn on_window_event(window: &Window, event: &WindowEvent) {
    let state = window.state::<AppState>();
    match event {
        // Minimised windows report sentinel values; don't remember those.
        WindowEvent::Moved(p) if p.x > -30000 && p.y > -30000 => {
            remember(window, &state, |file| file.position = Some((p.x, p.y)));
        }
        WindowEvent::Resized(s) if s.width > 0 && s.height > 0 => {
            remember(window, &state, |file| file.size = Some((s.width, s.height)));
        }
        // Without this the glass goes solid until the next check.
        #[cfg(windows)]
        WindowEvent::Focused(false) if window.label() != SETTINGS => {
            if let Ok(hwnd) = window.hwnd() {
                topmost::keep_backdrop(hwnd.0 as _);
            }
        }
        WindowEvent::Destroyed => {
            let session = state.sessions.lock().unwrap().remove(window.label());
            if let Some(session) = session {
                let path = session.path.to_string_lossy();
                state.update_store(|store| store.note_closed(&path));
            }
            state.save_store();
        }
        _ => {}
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            if argv.iter().any(|arg| arg == RESTORE) {
                return;
            }
            let paths = paths_from_args(argv.into_iter().skip(1), Path::new(&cwd));
            let app = app.clone();
            tauri::async_runtime::spawn(async move { open_all(&app, paths) });
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![RESTORE])))
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            fs::create_dir_all(&dir)?;
            let store_path = dir.join("state.json");
            app.manage(AppState {
                sessions: Mutex::default(),
                store: Mutex::new(Store::load(&store_path)),
                store_path,
                store_dirty: AtomicBool::new(false),
            });
            let handle = app.handle().clone();
            std::thread::spawn(move || save_periodically(handle));
            #[cfg(windows)]
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || topmost::keep_pinned_on_top(handle));
            }

            if std::env::args().any(|arg| arg == RESTORE) {
                restore(app.handle());
            } else {
                let cwd = std::env::current_dir()?;
                open_all(app.handle(), paths_from_args(std::env::args().skip(1), &cwd));
            }
            Ok(())
        })
        .on_window_event(on_window_event)
        .invoke_handler(tauri::generate_handler![
            prefs,
            recent,
            clear_recent,
            doc_load,
            doc_save,
            doc_check,
            doc_image,
            set_pinned,
            set_opacity,
            set_color,
            set_glass,
            profiles,
            save_profile,
            delete_profile,
            set_default_profile,
            use_profile,
            open_settings,
            set_theme,
            open_files
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Sticky Editor");
}
