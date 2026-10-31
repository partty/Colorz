//! Palette actions shared by the Pick and Wheel tabs.

use colorz_core::color::Color;
use colorz_core::palette::Palette;
use colorz_core::palette_ops::palette_display_name;

use super::NEW_PALETTE_NAME;
use super::window::ColorzWindow;
use crate::state::AppState;

/// The menu item that toggles "Add every pick": it names what a click does.
pub fn auto_add_label(on: bool) -> &'static str {
    if on {
        "Stop adding every pick"
    } else {
        "Add every pick"
    }
}

/// A palette name as a menu label (menus read `_` as a mnemonic).
pub fn menu_label(name: &str) -> String {
    if name.trim().is_empty() {
        "Untitled palette".to_owned()
    } else {
        name.replace('_', "__")
    }
}

/// `pals` with the last used one first.
pub fn palettes_last_used_first(mut pals: Vec<Palette>, last: Option<&str>) -> Vec<Palette> {
    if let Some(i) = last.and_then(|id| pals.iter().position(|p| p.id == id)) {
        let p = pals.remove(i);
        pals.insert(0, p);
    }
    pals
}

pub fn added_message(added: usize, name: &str) -> String {
    if added == 1 {
        format!("Added to {name}")
    } else {
        format!("Added {added} colors to {name}")
    }
}

/// Appends `colors` to the active palette with an "Added" toast and Undo.
/// With no active palette, asks for a name and creates one first.
pub fn add_colors(state: &AppState, window: &ColorzWindow, colors: Vec<Color>) {
    if colors.is_empty() {
        return;
    }
    if let Some(pal) = state.active_palette() {
        add_to(state, window, &pal.id, colors);
        return;
    }
    let (state, window) = (state.clone(), window.clone());
    super::prompt_name(
        &window.clone(),
        "New Palette",
        NEW_PALETTE_NAME,
        "_Create",
        move |name| {
            let pal = Palette::new(&name);
            let id = pal.id.clone();
            state.upsert_palette(pal);
            add_to(&state, &window, &id, colors.clone());
        },
    );
}

fn add_to(state: &AppState, window: &ColorzWindow, id: &str, colors: Vec<Color>) {
    let Some(pal) = state.add_to_palette(id, &colors) else {
        window.show_toast("That palette no longer exists");
        return;
    };
    let msg = added_message(colors.len(), &palette_display_name(&pal));
    let (state, id) = (state.clone(), id.to_owned());
    let undo_window = window.clone();
    window.toast_with_undo(&msg, move || {
        if !state.undo_add(&id, &colors) {
            undo_window.show_toast("Can't undo: the palette has changed");
        }
    });
}

/// Starts editing color `index` of palette `id` and shows the Wheel tab. An
/// edit already open on that color is kept; an edit of another color is
/// dropped without saving.
pub fn edit_on_wheel(state: &AppState, window: &ColorzWindow, id: &str, index: usize) {
    match state.start_editing(id, index) {
        Ok(_) => window.set_page("wheel"),
        Err(()) => window.show_toast("That color no longer exists"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_toast_counts_colors() {
        assert_eq!(added_message(1, "Brand"), "Added to Brand");
        assert_eq!(added_message(3, "Brand"), "Added 3 colors to Brand");
    }

    #[test]
    fn last_used_palette_first() {
        let pals: Vec<Palette> = ["a", "b", "c"].map(Palette::new).into();
        let last = pals[2].id.clone();
        let names = |v: Vec<Palette>| v.into_iter().map(|p| p.name).collect::<Vec<_>>();
        assert_eq!(
            names(palettes_last_used_first(pals.clone(), Some(&last))),
            ["c", "a", "b"]
        );
        assert_eq!(
            names(palettes_last_used_first(pals.clone(), Some("gone"))),
            ["a", "b", "c"]
        );
        assert_eq!(names(palettes_last_used_first(pals, None)), ["a", "b", "c"]);
        assert_eq!(menu_label("my_brand"), "my__brand");
    }

    #[test]
    fn auto_add_label_names_the_click() {
        assert_eq!(auto_add_label(false), "Add every pick");
        assert_eq!(auto_add_label(true), "Stop adding every pick");
        assert_eq!(menu_label(" "), "Untitled palette");
    }
}
