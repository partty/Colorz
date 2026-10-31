//! The global "pick" shortcut: the GlobalShortcuts portal, plus a GNOME
//! custom keybinding running `colorz --pick` as a fallback.

use std::pin::pin;

use adw::prelude::*;
use ashpd::desktop::global_shortcuts::{Activated, GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;
use gtk::{gio, glib};

use super::clipboard;

/// Portal id of the pick shortcut.
const PICK_ID: &str = "pick";
const PICK_DESCRIPTION: &str = "Pick a color from the screen";
/// Suggested to GNOME, which asks the user to confirm or change it.
const PREFERRED_TRIGGER: &str = "SUPER+SHIFT+c";

const MEDIA_KEYS_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const CUSTOM_KEYBINDING_SCHEMA: &str =
    "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const KEYBINDINGS_KEY: &str = "custom-keybindings";
const FALLBACK_PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/colorz-pick/";
const FALLBACK_NAME: &str = "Colorz pick";
const FALLBACK_COMMAND: &str = "colorz --pick";

/// Registers the pick shortcut with the GlobalShortcuts portal and starts
/// activating `app.pick` whenever it fires. Returns how the shortcut is
/// triggered, as described by the portal (may be empty).
pub async fn register(app: &adw::Application) -> Result<String, String> {
    super::portals_ready().await;
    let portal = GlobalShortcuts::new().await.map_err(|e| e.to_string())?;
    let session = portal
        .create_session(Default::default())
        .await
        .map_err(|e| e.to_string())?;
    // Listen before binding, so no activation can be missed.
    let activated = portal
        .receive_activated()
        .await
        .map_err(|e| e.to_string())?;
    let shortcut = NewShortcut::new(PICK_ID, PICK_DESCRIPTION).preferred_trigger(PREFERRED_TRIGGER);
    let bound = portal
        .bind_shortcuts(&session, &[shortcut], None, Default::default())
        .await
        .and_then(|request| request.response())
        .map_err(|e| e.to_string())?;
    let trigger = bound
        .shortcuts()
        .iter()
        .find(|s| s.id() == PICK_ID)
        .map(|s| s.trigger_description().to_owned())
        .unwrap_or_default();

    let weak = app.downgrade();
    glib::spawn_future_local(async move {
        // The shortcut lives as long as its session.
        let _session = session;
        let mut activated = pin!(activated);
        while let Some(a) = activated.next().await {
            if a.shortcut_id() != PICK_ID {
                continue;
            }
            let Some(app) = weak.upgrade() else { break };
            clipboard::set_activation_token(activation_token(&a));
            app.activate_action("pick", None);
        }
    });
    Ok(trigger)
}

/// The XDG activation token GNOME passes with the activation, if any.
fn activation_token(a: &Activated) -> Option<String> {
    let value = a.options().get("activation_token")?;
    value.downcast_ref::<String>().ok()
}

/// Adds a GNOME custom keybinding `binding` (e.g. `<Super><Shift>c`) that
/// runs `colorz --pick`. Only for when the user asks for it in Settings.
pub fn install_gsettings_fallback(binding: &str) -> Result<(), String> {
    // Look the schemas up first: gio aborts the process on a missing one.
    let source =
        gio::SettingsSchemaSource::default().ok_or("no GSettings schemas are installed")?;
    let lookup = |id: &str, keys: &[&str]| {
        let schema = source
            .lookup(id, true)
            .ok_or_else(|| format!("GSettings schema {id} is not installed"))?;
        match keys.iter().find(|k| !schema.has_key(k)) {
            Some(k) => Err(format!("GSettings schema {id} has no key {k}")),
            None => Ok(schema),
        }
    };
    let media_keys = lookup(MEDIA_KEYS_SCHEMA, &[KEYBINDINGS_KEY])?;
    let custom = lookup(CUSTOM_KEYBINDING_SCHEMA, &["name", "command", "binding"])?;
    // gio also aborts on a path given to a fixed-path schema, or none to a
    // relocatable one.
    if media_keys.path().is_none() || custom.path().is_some() {
        return Err("the GNOME keybinding schemas are not laid out as expected".into());
    }

    let none = None::<&gio::SettingsBackend>;
    let entry = gio::Settings::new_full(&custom, none, Some(FALLBACK_PATH));
    let failed = |e: glib::BoolError| format!("cannot write the keybinding: {e}");
    entry.set_string("name", FALLBACK_NAME).map_err(failed)?;
    entry
        .set_string("command", FALLBACK_COMMAND)
        .map_err(failed)?;
    entry.set_string("binding", binding).map_err(failed)?;

    let list = gio::Settings::new_full(&media_keys, none, None);
    let existing: Vec<String> = list
        .strv(KEYBINDINGS_KEY)
        .iter()
        .map(|s| s.to_string())
        .collect();
    let merged = merged_keybinding_list(&existing, FALLBACK_PATH);
    if merged != existing {
        list.set_strv(KEYBINDINGS_KEY, merged).map_err(failed)?;
    }
    gio::Settings::sync();
    Ok(())
}

/// `existing` with `ours` appended, unless it is already listed.
fn merged_keybinding_list(existing: &[String], ours: &str) -> Vec<String> {
    let mut list = existing.to_vec();
    if !list.iter().any(|p| p == ours) {
        list.push(ours.to_owned());
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_keybinding_list_appends_once() {
        let ours = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/colorz-pick/";
        let other =
            "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/custom0/".to_owned();
        let merged = merged_keybinding_list(std::slice::from_ref(&other), ours);
        assert_eq!(merged, [other.clone(), ours.to_owned()]);
        assert_eq!(merged_keybinding_list(&merged, ours), merged);
        assert_eq!(merged_keybinding_list(&[], ours), [ours.to_owned()]);
    }
}
