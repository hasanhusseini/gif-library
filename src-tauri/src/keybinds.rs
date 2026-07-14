use crate::database::Database;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_global_shortcut::{
    Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutEvent, ShortcutState,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeybindAction {
    ShowFocus,
    ToggleVisibility,
}

#[derive(Default)]
struct RuntimeKeybinds {
    show_focus: Option<Shortcut>,
    toggle_visibility: Option<Shortcut>,
    show_active: bool,
    toggle_active: bool,
    warnings: Vec<String>,
}

#[derive(Default)]
pub struct KeybindManager(Mutex<RuntimeKeybinds>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeybindSettingsView {
    show_focus_keybind: Option<String>,
    toggle_visibility_keybind: Option<String>,
    show_focus_active: bool,
    toggle_visibility_active: bool,
    warnings: Vec<String>,
}

pub fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(handle_shortcut)
        .build()
}

pub fn initialize(app: &AppHandle) {
    let settings = match load_settings(&app.state::<Database>()) {
        Ok(settings) => settings,
        Err(_) => {
            let manager = app.state::<KeybindManager>();
            let mut runtime = manager.0.lock().unwrap();
            runtime
                .warnings
                .push("Keybind settings could not be loaded.".into());
            return;
        }
    };

    for (action, value) in [
        (KeybindAction::ShowFocus, settings.0),
        (KeybindAction::ToggleVisibility, settings.1),
    ] {
        let Some(value) = value else { continue };
        let shortcut = match validate_shortcut(&value) {
            Ok((shortcut, _)) => shortcut,
            Err(_) => {
                push_startup_warning(app, action, "The saved shortcut is invalid.");
                continue;
            }
        };
        if app.global_shortcut().register(shortcut).is_err() {
            push_startup_warning(app, action, "The saved shortcut could not be registered.");
            continue;
        }
        let manager = app.state::<KeybindManager>();
        let mut runtime = manager.0.lock().unwrap();
        set_runtime(&mut runtime, action, Some(shortcut), true);
    }
}

pub fn clear_runtime(manager: &KeybindManager) -> Result<(), String> {
    let mut runtime = manager.0.lock().map_err(|_| "keybind state unavailable")?;
    *runtime = RuntimeKeybinds::default();
    Ok(())
}

fn push_startup_warning(app: &AppHandle, action: KeybindAction, message: &str) {
    let label = if action == KeybindAction::ShowFocus {
        "Show/focus"
    } else {
        "Toggle"
    };
    app.state::<KeybindManager>()
        .0
        .lock()
        .unwrap()
        .warnings
        .push(format!("{label} keybind: {message}"));
}

fn handle_shortcut(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state != ShortcutState::Pressed {
        return;
    }
    let action = {
        let manager = app.state::<KeybindManager>();
        let runtime = manager.0.lock().unwrap();
        if runtime.show_active && runtime.show_focus.as_ref() == Some(shortcut) {
            Some(KeybindAction::ShowFocus)
        } else if runtime.toggle_active && runtime.toggle_visibility.as_ref() == Some(shortcut) {
            Some(KeybindAction::ToggleVisibility)
        } else {
            None
        }
    };
    match action {
        Some(KeybindAction::ShowFocus) => show_and_focus(app),
        Some(KeybindAction::ToggleVisibility) => toggle_window(app),
        None => {}
    }
}

fn show_and_focus(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        eprintln!("Global shortcut could not find the main window.");
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    if window.set_focus().is_err() {
        eprintln!("Global shortcut restored the window but focus was unavailable.");
    }
}

fn toggle_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        eprintln!("Global shortcut could not find the main window.");
        return;
    };
    let minimized = window.is_minimized().unwrap_or(false);
    let visible = window.is_visible().unwrap_or(true);
    if visible && !minimized {
        if window.minimize().is_err() {
            eprintln!("Global shortcut could not minimize the main window.");
        }
    } else {
        show_and_focus(app);
    }
}

#[tauri::command]
pub fn get_keybind_settings(
    database: State<'_, Database>,
    manager: State<'_, KeybindManager>,
) -> Result<KeybindSettingsView, String> {
    let settings = load_settings(&database)?;
    let runtime = manager.0.lock().map_err(|_| "keybind state unavailable")?;
    Ok(KeybindSettingsView {
        show_focus_keybind: settings.0,
        toggle_visibility_keybind: settings.1,
        show_focus_active: runtime.show_active,
        toggle_visibility_active: runtime.toggle_active,
        warnings: runtime.warnings.clone(),
    })
}

#[tauri::command]
pub fn set_show_focus_keybind(
    app: AppHandle,
    database: State<'_, Database>,
    manager: State<'_, KeybindManager>,
    shortcut: Option<String>,
) -> Result<KeybindSettingsView, String> {
    set_keybind(
        &app,
        &database,
        &manager,
        KeybindAction::ShowFocus,
        shortcut,
    )?;
    get_keybind_settings(database, manager)
}

#[tauri::command]
pub fn set_toggle_visibility_keybind(
    app: AppHandle,
    database: State<'_, Database>,
    manager: State<'_, KeybindManager>,
    shortcut: Option<String>,
) -> Result<KeybindSettingsView, String> {
    set_keybind(
        &app,
        &database,
        &manager,
        KeybindAction::ToggleVisibility,
        shortcut,
    )?;
    get_keybind_settings(database, manager)
}

fn set_keybind(
    app: &AppHandle,
    database: &Database,
    manager: &KeybindManager,
    action: KeybindAction,
    value: Option<String>,
) -> Result<(), String> {
    set_keybind_with_registrar(&AppRegistrar(app), database, manager, action, value)
}

trait ShortcutRegistrar {
    fn register(&self, shortcut: Shortcut) -> Result<(), ()>;
    fn unregister(&self, shortcut: Shortcut) -> Result<(), ()>;
}

struct AppRegistrar<'a>(&'a AppHandle);

impl ShortcutRegistrar for AppRegistrar<'_> {
    fn register(&self, shortcut: Shortcut) -> Result<(), ()> {
        self.0.global_shortcut().register(shortcut).map_err(|_| ())
    }

    fn unregister(&self, shortcut: Shortcut) -> Result<(), ()> {
        self.0
            .global_shortcut()
            .unregister(shortcut)
            .map_err(|_| ())
    }
}

fn set_keybind_with_registrar(
    registrar: &impl ShortcutRegistrar,
    database: &Database,
    manager: &KeybindManager,
    action: KeybindAction,
    value: Option<String>,
) -> Result<(), String> {
    let normalized = value.map(|value| validate_shortcut(&value)).transpose()?;
    let settings = load_settings(database)?;
    let old_display = if action == KeybindAction::ShowFocus {
        settings.0.clone()
    } else {
        settings.1.clone()
    };
    let other_display = if action == KeybindAction::ShowFocus {
        settings.1.clone()
    } else {
        settings.0.clone()
    };
    if normalized
        .as_ref()
        .is_some_and(|value| Some(&value.1) == other_display.as_ref())
    {
        return Err("That shortcut is already assigned to the other keybind.".into());
    }
    if normalized.as_ref().map(|value| &value.1) == old_display.as_ref() {
        return Ok(());
    }

    let mut runtime = manager.0.lock().map_err(|_| "keybind state unavailable")?;
    let (old_shortcut, old_active) = runtime_for(&runtime, action);
    if old_active {
        if let Some(old) = old_shortcut {
            registrar
                .unregister(old)
                .map_err(|_| "The previous shortcut could not be unregistered.".to_string())?;
        }
    }

    if let Some((new_shortcut, _)) = normalized.as_ref() {
        if registrar.register(*new_shortcut).is_err() {
            restore_old(registrar, &mut runtime, action, old_shortcut, old_active);
            return Err(
                "The shortcut is invalid, unavailable, or already used by another application."
                    .into(),
            );
        }
    }

    let new_display = normalized.as_ref().map(|value| value.1.clone());
    if persist_setting(database, action, new_display.as_deref()).is_err() {
        if let Some((new_shortcut, _)) = normalized.as_ref() {
            let _ = registrar.unregister(*new_shortcut);
        }
        restore_old(registrar, &mut runtime, action, old_shortcut, old_active);
        return Err(
            "The shortcut could not be saved; the previous shortcut was restored where possible."
                .into(),
        );
    }

    set_runtime(
        &mut runtime,
        action,
        normalized.map(|value| value.0),
        new_display.is_some(),
    );
    let prefix = if action == KeybindAction::ShowFocus {
        "Show/focus keybind:"
    } else {
        "Toggle keybind:"
    };
    runtime
        .warnings
        .retain(|warning| !warning.starts_with(prefix));
    Ok(())
}

fn restore_old(
    registrar: &impl ShortcutRegistrar,
    runtime: &mut RuntimeKeybinds,
    action: KeybindAction,
    old: Option<Shortcut>,
    was_active: bool,
) {
    let restored = was_active
        && old
            .map(|shortcut| registrar.register(shortcut).is_ok())
            .unwrap_or(false);
    set_runtime(runtime, action, old, restored);
    if was_active && !restored {
        runtime
            .warnings
            .push("A previous shortcut could not be restored and is currently inactive.".into());
    }
}

fn runtime_for(runtime: &RuntimeKeybinds, action: KeybindAction) -> (Option<Shortcut>, bool) {
    if action == KeybindAction::ShowFocus {
        (runtime.show_focus, runtime.show_active)
    } else {
        (runtime.toggle_visibility, runtime.toggle_active)
    }
}

fn set_runtime(
    runtime: &mut RuntimeKeybinds,
    action: KeybindAction,
    shortcut: Option<Shortcut>,
    active: bool,
) {
    if action == KeybindAction::ShowFocus {
        runtime.show_focus = shortcut;
        runtime.show_active = active;
    } else {
        runtime.toggle_visibility = shortcut;
        runtime.toggle_active = active;
    }
}

fn load_settings(database: &Database) -> Result<(Option<String>, Option<String>), String> {
    database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?
        .query_row(
            "SELECT show_focus_keybind, toggle_visibility_keybind FROM app_settings WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| format!("failed to read keybind settings: {error}"))
}

fn persist_setting(
    database: &Database,
    action: KeybindAction,
    value: Option<&str>,
) -> Result<(), String> {
    let sql = if action == KeybindAction::ShowFocus {
        "UPDATE app_settings SET show_focus_keybind = ?1 WHERE id = 1"
    } else {
        "UPDATE app_settings SET toggle_visibility_keybind = ?1 WHERE id = 1"
    };
    database
        .connection
        .lock()
        .map_err(|_| "database lock poisoned")?
        .execute(sql, [value])
        .map(|_| ())
        .map_err(|error| format!("failed to save keybind settings: {error}"))
}

fn validate_shortcut(value: &str) -> Result<(Shortcut, String), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Enter a shortcut or use Clear.".into());
    }
    let shortcut: Shortcut = value
        .parse()
        .map_err(|_| "Use a shortcut such as Ctrl+Shift+J.".to_string())?;
    if shortcut.mods.contains(Modifiers::SUPER) {
        return Err("Win-key shortcuts are not supported.".into());
    }
    if !shortcut
        .mods
        .intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT)
    {
        return Err("Use at least one modifier: Ctrl, Alt, or Shift.".into());
    }
    if matches!(
        shortcut.key,
        Code::Escape | Code::Enter | Code::Tab | Code::Space
    ) {
        return Err("That key is reserved and cannot be used as a global shortcut.".into());
    }
    if shortcut.mods == Modifiers::ALT && shortcut.key == Code::F4
        || shortcut.mods == Modifiers::CONTROL
            && matches!(
                shortcut.key,
                Code::KeyC | Code::KeyV | Code::KeyX | Code::KeyS | Code::KeyP
            )
        || shortcut.mods == (Modifiers::CONTROL | Modifiers::ALT) && shortcut.key == Code::Delete
    {
        return Err("That common system or editing shortcut is blocked.".into());
    }
    Ok((shortcut, display_shortcut(shortcut)))
}

fn display_shortcut(shortcut: Shortcut) -> String {
    let mut parts = Vec::new();
    if shortcut.mods.contains(Modifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if shortcut.mods.contains(Modifiers::ALT) {
        parts.push("Alt".to_string());
    }
    if shortcut.mods.contains(Modifiers::SHIFT) {
        parts.push("Shift".to_string());
    }
    let raw = shortcut.key.to_string();
    let key = raw
        .strip_prefix("Key")
        .or_else(|| raw.strip_prefix("Digit"))
        .unwrap_or(&raw);
    parts.push(key.to_string());
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::{
        load_settings, runtime_for, set_keybind_with_registrar, validate_shortcut, KeybindAction,
        KeybindManager, Shortcut, ShortcutRegistrar,
    };
    use crate::database::Database;
    use std::{
        cell::RefCell,
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[derive(Default)]
    struct FakeRegistrar {
        registered: RefCell<Vec<Shortcut>>,
        fail_register_id: RefCell<Option<u32>>,
    }

    impl ShortcutRegistrar for FakeRegistrar {
        fn register(&self, shortcut: Shortcut) -> Result<(), ()> {
            if *self.fail_register_id.borrow() == Some(shortcut.id()) {
                return Err(());
            }
            if !self.registered.borrow().contains(&shortcut) {
                self.registered.borrow_mut().push(shortcut);
            }
            Ok(())
        }

        fn unregister(&self, shortcut: Shortcut) -> Result<(), ()> {
            self.registered
                .borrow_mut()
                .retain(|value| *value != shortcut);
            Ok(())
        }
    }

    fn test_database(label: &str) -> (Database, std::path::PathBuf) {
        let token = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("gif-library-keybind-{label}-{token}"));
        (Database::initialize(&root).unwrap(), root)
    }

    #[test]
    fn validates_and_normalizes_shortcuts() {
        let (_, display) = validate_shortcut("ctrl + shift + KeyJ").unwrap();
        assert_eq!(display, "Ctrl+Shift+J");
        let (_, display) = validate_shortcut("Alt+9").unwrap();
        assert_eq!(display, "Alt+9");
        assert!(validate_shortcut("Alt+ArrowUp").is_ok());
        assert!(validate_shortcut("Ctrl+F12").is_ok());
    }

    #[test]
    fn rejects_unmodified_reserved_and_system_shortcuts() {
        for value in [
            "J",
            "Escape",
            "Ctrl+C",
            "Ctrl+V",
            "Ctrl+X",
            "Ctrl+S",
            "Ctrl+P",
            "Alt+F4",
            "Ctrl+Alt+Delete",
            "Super+Shift+J",
        ] {
            assert!(
                validate_shortcut(value).is_err(),
                "{value} should be rejected"
            );
        }
    }

    #[test]
    fn registration_failure_preserves_previous_setting_and_runtime() {
        let (database, root) = test_database("restore");
        let manager = KeybindManager::default();
        let registrar = FakeRegistrar::default();
        set_keybind_with_registrar(
            &registrar,
            &database,
            &manager,
            KeybindAction::ShowFocus,
            Some("Ctrl+Shift+J".into()),
        )
        .unwrap();
        let replacement = validate_shortcut("Ctrl+Shift+K").unwrap().0;
        *registrar.fail_register_id.borrow_mut() = Some(replacement.id());
        assert!(set_keybind_with_registrar(
            &registrar,
            &database,
            &manager,
            KeybindAction::ShowFocus,
            Some("Ctrl+Shift+K".into()),
        )
        .is_err());
        assert_eq!(
            load_settings(&database).unwrap().0.as_deref(),
            Some("Ctrl+Shift+J")
        );
        let runtime = manager.0.lock().unwrap();
        let (shortcut, active) = runtime_for(&runtime, KeybindAction::ShowFocus);
        assert!(active);
        assert_eq!(shortcut, Some(validate_shortcut("Ctrl+Shift+J").unwrap().0));
        drop(runtime);
        drop(database);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clear_unregisters_and_persists_null() {
        let (database, root) = test_database("clear");
        let manager = KeybindManager::default();
        let registrar = FakeRegistrar::default();
        set_keybind_with_registrar(
            &registrar,
            &database,
            &manager,
            KeybindAction::ToggleVisibility,
            Some("Alt+Shift+T".into()),
        )
        .unwrap();
        set_keybind_with_registrar(
            &registrar,
            &database,
            &manager,
            KeybindAction::ToggleVisibility,
            None,
        )
        .unwrap();
        assert_eq!(load_settings(&database).unwrap().1, None);
        assert!(registrar.registered.borrow().is_empty());
        drop(database);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn database_failure_restores_previous_runtime_shortcut() {
        let (database, root) = test_database("database-rollback");
        let manager = KeybindManager::default();
        let registrar = FakeRegistrar::default();
        set_keybind_with_registrar(
            &registrar,
            &database,
            &manager,
            KeybindAction::ShowFocus,
            Some("Ctrl+Shift+J".into()),
        )
        .unwrap();
        database.connection.lock().unwrap().execute_batch(
            "CREATE TRIGGER reject_keybind_update BEFORE UPDATE ON app_settings BEGIN SELECT RAISE(FAIL, 'test failure'); END;",
        ).unwrap();
        assert!(set_keybind_with_registrar(
            &registrar,
            &database,
            &manager,
            KeybindAction::ShowFocus,
            Some("Ctrl+Shift+K".into()),
        )
        .is_err());
        let old = validate_shortcut("Ctrl+Shift+J").unwrap().0;
        assert_eq!(registrar.registered.borrow().as_slice(), &[old]);
        let runtime = manager.0.lock().unwrap();
        assert_eq!(
            runtime_for(&runtime, KeybindAction::ShowFocus),
            (Some(old), true)
        );
        drop(runtime);
        drop(database);
        fs::remove_dir_all(root).unwrap();
    }
}
