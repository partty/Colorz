//! Shared application state: config, current color, history, palettes and
//! the "copy as" target, with change notifications. GTK-free and main-thread
//! only (`Rc`/`RefCell`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use colorz_core::color::{Color, RgbSpace};
use colorz_core::format::{Form, Format, FormatOptions, Formatted, format_color};
use colorz_core::icc::IccTarget;
use colorz_core::palette::Palette;
use colorz_core::palette_ops::{self, EditSession};
use colorz_core::storage::{self, Config, History, Paths, TargetChoice};
use colorz_core::target::{BuiltinTarget, TargetSpace};

/// Color shown before anything has been picked.
const INITIAL_COLOR: [u8; 3] = [0x34, 0x78, 0xF6];

/// What set the current color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSource {
    Pick,
    Paste,
    Wheel,
    Palette,
    History,
    Hex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateEvent {
    Color(ColorSource),
    History,
    Palettes,
    Config,
    /// Saving or deleting a file failed. The message is ready to show,
    /// e.g. "Could not save palette Brand: <error>".
    SaveFailed(String),
    /// An editing session started or ended (for any reason).
    Editing,
    /// An editing session ended because its palette or color changed
    /// underneath it; emitted before the matching [`StateEvent::Editing`].
    EditingLost,
}

/// Where the next pick should go.
#[derive(Clone, Debug, PartialEq)]
pub enum PickPurpose {
    AddTo {
        palette_id: String,
    },
    Replace {
        palette_id: String,
        index: usize,
        original: Color,
    },
}

/// What a finished pick does, by priority: a pending purpose, then an open
/// editing session, then "Add every pick" with an active palette.
#[derive(Debug, PartialEq)]
pub enum PickRoute {
    Purpose(PickPurpose),
    Edit,
    AutoAdd(String),
    Normal,
}

pub fn pick_route(
    purpose: Option<PickPurpose>,
    editing: bool,
    auto_add: bool,
    active: Option<&str>,
) -> PickRoute {
    if let Some(p) = purpose {
        return PickRoute::Purpose(p);
    }
    if editing {
        return PickRoute::Edit;
    }
    match active {
        Some(id) if auto_add => PickRoute::AutoAdd(id.to_owned()),
        _ => PickRoute::Normal,
    }
}

type Listener = Rc<dyn Fn(&StateEvent)>;

struct Inner {
    paths: Paths,
    config_writable: bool,
    readonly_logged: Cell<bool>,
    config: RefCell<Config>,
    current: Cell<Color>,
    history: RefCell<History>,
    palettes: RefCell<Vec<Palette>>,
    editing: RefCell<Option<EditSession>>,
    pick_purpose: RefCell<Option<PickPurpose>>,
    target: RefCell<Box<dyn TargetSpace>>,
    target_error: RefCell<Option<String>>,
    listeners: RefCell<Vec<(usize, Listener)>>,
    next_listener: Cell<usize>,
}

/// Cloneable handle to the shared state.
#[derive(Clone)]
pub struct AppState(Rc<Inner>);

fn build_target(choice: &TargetChoice) -> (Box<dyn TargetSpace>, Option<String>) {
    let builtin = |s| Box::new(BuiltinTarget(s)) as Box<dyn TargetSpace>;
    match choice {
        TargetChoice::Srgb => (builtin(RgbSpace::Srgb), None),
        TargetChoice::DisplayP3 => (builtin(RgbSpace::DisplayP3), None),
        TargetChoice::AdobeRgb => (builtin(RgbSpace::A98Rgb), None),
        TargetChoice::Icc { path } => match IccTarget::load(path) {
            Ok(t) => (Box::new(t), None),
            Err(e) => {
                let msg = format!("{}: {e}", path.display());
                eprintln!("colorz: cannot load ICC profile {msg}; using sRGB");
                (builtin(RgbSpace::Srgb), Some(msg))
            }
        },
    }
}

fn sort_palettes(pals: &mut [Palette]) {
    // Same order as `storage::load_palettes`.
    pals.sort_by_cached_key(|p| (p.name.to_lowercase(), p.id.clone()));
}

impl AppState {
    /// Loads config, history and palettes from `paths`.
    pub fn new(paths: Paths) -> AppState {
        let loaded = storage::load_config(&paths);
        let history = storage::load_history(&paths);
        let palettes = storage::load_palettes(&paths);
        let (target, target_error) = build_target(&loaded.config.target);
        let current = history
            .entries
            .first()
            .copied()
            .unwrap_or_else(|| Color::from_srgb8(INITIAL_COLOR));
        AppState(Rc::new(Inner {
            paths,
            config_writable: loaded.writable,
            readonly_logged: Cell::new(false),
            config: RefCell::new(loaded.config),
            current: Cell::new(current),
            history: RefCell::new(history),
            palettes: RefCell::new(palettes),
            editing: RefCell::new(None),
            pick_purpose: RefCell::new(None),
            target: RefCell::new(target),
            target_error: RefCell::new(target_error),
            listeners: RefCell::new(Vec::new()),
            next_listener: Cell::new(0),
        }))
    }

    // ------------------------------------------------------------ events

    /// Registers a listener; returns an id for [`AppState::disconnect`].
    pub fn connect(&self, f: impl Fn(&StateEvent) + 'static) -> usize {
        let id = self.0.next_listener.get();
        self.0.next_listener.set(id + 1);
        self.0.listeners.borrow_mut().push((id, Rc::new(f)));
        id
    }

    pub fn disconnect(&self, id: usize) {
        self.0.listeners.borrow_mut().retain(|(i, _)| *i != id);
    }

    /// Logs a failed save or delete and emits [`StateEvent::SaveFailed`].
    fn save_failed(&self, msg: String) {
        eprintln!("colorz: {msg}");
        self.emit(StateEvent::SaveFailed(msg));
    }

    fn emit(&self, e: StateEvent) {
        // Clone first so listeners may connect, disconnect or emit again.
        let listeners: Vec<Listener> = self
            .0
            .listeners
            .borrow()
            .iter()
            .map(|(_, l)| l.clone())
            .collect();
        for l in listeners {
            l(&e);
        }
    }

    // ------------------------------------------------------------ config

    pub fn config(&self) -> Config {
        self.0.config.borrow().clone()
    }

    /// Applies `f`, saves the config (unless it was unreadable at startup),
    /// rebuilds the target if it changed and emits [`StateEvent::Config`].
    /// `f` must not call back into the state.
    pub fn update_config(&self, f: impl FnOnce(&mut Config)) {
        let (old_target, new) = {
            let mut cfg = self.0.config.borrow_mut();
            let old_target = cfg.target.clone();
            f(&mut cfg);
            (old_target, cfg.clone())
        };
        let mut saved = Ok(());
        if self.0.config_writable {
            saved = storage::save_config(&self.0.paths, &new)
                .map_err(|e| format!("Could not save settings: {e}"));
        } else if !self.0.readonly_logged.replace(true) {
            eprintln!(
                "colorz: not saving settings; {} could not be read",
                self.0.paths.config_file.display()
            );
        }
        if new.target != old_target {
            let (target, error) = build_target(&new.target);
            *self.0.target.borrow_mut() = target;
            *self.0.target_error.borrow_mut() = error;
        }
        let trimmed = {
            let mut h = self.0.history.borrow_mut();
            let over = h.entries.len() > new.history_size;
            h.entries.truncate(new.history_size);
            over
        };
        if trimmed {
            self.save_history();
        }
        self.emit(StateEvent::Config);
        if trimmed {
            self.emit(StateEvent::History);
        }
        if let Err(msg) = saved {
            self.save_failed(msg);
        }
    }

    // ------------------------------------------------------ current color

    pub fn current(&self) -> Color {
        self.0.current.get()
    }

    pub fn set_current(&self, c: Color, source: ColorSource) {
        self.0.current.set(c);
        self.emit(StateEvent::Color(source));
    }

    // ----------------------------------------------------------- history

    pub fn history(&self) -> History {
        self.0.history.borrow().clone()
    }

    pub fn push_history(&self, c: Color) {
        let cap = self.0.config.borrow().history_size;
        self.0.history.borrow_mut().push(c, cap);
        self.save_history();
        self.emit(StateEvent::History);
    }

    fn save_history(&self) {
        let saved = storage::save_history(&self.0.paths, &self.0.history.borrow());
        if let Err(e) = saved {
            self.save_failed(format!("Could not save recent picks: {e}"));
        }
    }

    // ---------------------------------------------------------- palettes

    pub fn palettes(&self) -> Vec<Palette> {
        self.0.palettes.borrow().clone()
    }

    pub fn palette(&self, id: &str) -> Option<Palette> {
        self.0
            .palettes
            .borrow()
            .iter()
            .find(|p| p.id == id)
            .cloned()
    }

    /// Inserts or replaces (by id) and saves the palette.
    pub fn upsert_palette(&self, pal: Palette) {
        let saved = storage::save_palette(&self.0.paths, &pal)
            .map_err(|e| format!("Could not save palette {}: {e}", pal.name));
        {
            let mut pals = self.0.palettes.borrow_mut();
            match pals.iter_mut().find(|p| p.id == pal.id) {
                Some(slot) => *slot = pal,
                None => pals.push(pal),
            }
            sort_palettes(&mut pals);
        }
        self.emit(StateEvent::Palettes);
        self.end_invalid_session();
        if let Err(msg) = saved {
            self.save_failed(msg);
        }
    }

    /// Removes the palette and its file; returns it (for undo).
    pub fn delete_palette(&self, id: &str) -> Option<Palette> {
        let removed = {
            let mut pals = self.0.palettes.borrow_mut();
            let idx = pals.iter().position(|p| p.id == id)?;
            pals.remove(idx)
        };
        let deleted = storage::delete_palette(&self.0.paths, id);
        self.emit(StateEvent::Palettes);
        self.end_invalid_session();
        if let Err(e) = deleted {
            self.save_failed(format!("Could not delete palette {}: {e}", removed.name));
        }
        Some(removed)
    }
}

impl AppState {
    // ---------------------------------------------- active palette / edits

    /// The palette named by `last_palette`, if it exists.
    pub fn active_palette(&self) -> Option<Palette> {
        let id = self.0.config.borrow().last_palette.clone()?;
        self.palette(&id)
    }

    pub fn set_active_palette(&self, id: &str) {
        if self.0.config.borrow().last_palette.as_deref() != Some(id) {
            self.update_config(|c| c.last_palette = Some(id.to_string()));
        }
    }

    /// Appends `colors` to the palette, saves it and makes it active.
    pub fn add_to_palette(&self, id: &str, colors: &[Color]) -> Option<Palette> {
        let mut pal = self.palette(id)?;
        palette_ops::append_colors(&mut pal, colors);
        self.upsert_palette(pal.clone());
        self.set_active_palette(id);
        Some(pal)
    }

    pub fn undo_add(&self, id: &str, colors: &[Color]) -> bool {
        let Some(mut pal) = self.palette(id) else {
            return false;
        };
        let ok = palette_ops::undo_append(&mut pal, colors);
        if ok {
            self.upsert_palette(pal);
        }
        ok
    }

    /// Replaces the color at `index`; returns the old color.
    pub fn replace_in_palette(&self, id: &str, index: usize, c: Color) -> Option<Color> {
        let mut pal = self.palette(id)?;
        let old = palette_ops::replace_color(&mut pal, index, c)?;
        self.upsert_palette(pal);
        Some(old)
    }

    pub fn undo_replace(&self, id: &str, index: usize, old: Color, new: Color) -> bool {
        let Some(mut pal) = self.palette(id) else {
            return false;
        };
        let ok = palette_ops::undo_replace(&mut pal, index, old, new);
        if ok {
            self.upsert_palette(pal);
        }
        ok
    }

    pub fn editing(&self) -> Option<EditSession> {
        self.0.editing.borrow().clone()
    }

    /// Starts editing a palette color on the wheel. `Err(())` (nothing
    /// changed) if it does not exist. Otherwise returns the session it
    /// replaced if that one had unsaved changes.
    #[allow(clippy::result_unit_err)]
    pub fn start_editing(&self, id: &str, index: usize) -> Result<Option<EditSession>, ()> {
        let color = self
            .palette(id)
            .and_then(|p| p.colors.get(index).map(|pc| pc.color))
            .ok_or(())?;
        if self
            .0
            .editing
            .borrow()
            .as_ref()
            .is_some_and(|s| s.palette_id == id && s.index == index)
        {
            return Ok(None);
        }
        let previous = self.0.editing.replace(Some(EditSession {
            palette_id: id.to_string(),
            index,
            original: color,
        }));
        let discarded = previous.filter(|s| s.original != self.current());
        self.set_active_palette(id);
        self.set_current(color, ColorSource::Palette);
        self.emit(StateEvent::Editing);
        Ok(discarded)
    }

    /// Writes the current color into the edited palette slot. `Err(())` if
    /// the session was no longer valid (it is ended either way).
    #[allow(clippy::result_unit_err)]
    pub fn save_editing(&self) -> Result<(), ()> {
        let Some(session) = self.0.editing.take() else {
            return Ok(());
        };
        if session.is_valid_in(&self.0.palettes.borrow()) {
            self.replace_in_palette(&session.palette_id, session.index, self.current());
            self.emit(StateEvent::Editing);
            Ok(())
        } else {
            self.emit(StateEvent::EditingLost);
            self.emit(StateEvent::Editing);
            Err(())
        }
    }

    pub fn cancel_editing(&self) {
        if let Some(session) = self.0.editing.take() {
            self.set_current(session.original, ColorSource::Palette);
            self.emit(StateEvent::Editing);
        }
    }

    /// Ends the editing session if its palette or color changed.
    fn end_invalid_session(&self) {
        let lost = match &*self.0.editing.borrow() {
            Some(s) => !s.is_valid_in(&self.0.palettes.borrow()),
            None => false,
        };
        if lost {
            *self.0.editing.borrow_mut() = None;
            self.emit(StateEvent::EditingLost);
            self.emit(StateEvent::Editing);
        }
    }

    // ------------------------------------------------------ pick purpose

    pub fn set_pick_purpose(&self, p: Option<PickPurpose>) {
        *self.0.pick_purpose.borrow_mut() = p;
    }

    pub fn take_pick_purpose(&self) -> Option<PickPurpose> {
        self.0.pick_purpose.borrow_mut().take()
    }
}

impl AppState {
    // ---------------------------------------------------- target / format

    pub fn with_target<R>(&self, f: impl FnOnce(&dyn TargetSpace) -> R) -> R {
        f(self.0.target.borrow().as_ref())
    }

    /// Why the configured ICC target could not be used (sRGB is used instead).
    pub fn target_error(&self) -> Option<String> {
        self.0.target_error.borrow().clone()
    }

    /// The configured hex case, CSS syntax and decimal places.
    pub fn format_options(&self) -> FormatOptions {
        let cfg = self.0.config.borrow();
        FormatOptions {
            hex_case: cfg.hex_case,
            css_syntax: cfg.css_syntax,
            decimals: cfg.decimals,
        }
    }

    pub fn format(&self, c: &Color, f: Format, form: Form) -> Option<Formatted> {
        let opts = self.format_options();
        self.with_target(|t| format_color(c, f, form, &opts, t))
    }

    /// The configured default format, or HEX when the target cannot show
    /// it (`color()` with a custom ICC profile), so default-format copies
    /// never fail.
    pub fn default_format(&self) -> Format {
        let f = self.0.config.borrow().default_format;
        self.with_target(|t| f.or_hex_for(t))
    }

    /// `c` in the default format (see [`AppState::default_format`]) and the
    /// configured form.
    pub fn default_text(&self, c: &Color) -> Option<Formatted> {
        let form = self.0.config.borrow().default_form;
        self.format(c, self.default_format(), form)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::rc::Rc;

    use colorz_core::color::Color;
    use colorz_core::storage::{Config, Paths, TargetChoice, save_config};

    fn add_to_purpose() -> PickPurpose {
        PickPurpose::AddTo {
            palette_id: "p".into(),
        }
    }

    #[test]
    fn pick_route_purpose_wins() {
        assert_eq!(
            pick_route(Some(add_to_purpose()), true, true, Some("a")),
            PickRoute::Purpose(add_to_purpose())
        );
    }

    #[test]
    fn pick_route_editing_beats_auto_add() {
        assert_eq!(pick_route(None, true, true, Some("a")), PickRoute::Edit);
    }

    #[test]
    fn pick_route_auto_add_needs_active() {
        assert_eq!(
            pick_route(None, false, true, Some("a")),
            PickRoute::AutoAdd("a".into())
        );
        assert_eq!(pick_route(None, false, true, None), PickRoute::Normal);
        assert_eq!(pick_route(None, false, false, Some("a")), PickRoute::Normal);
    }

    #[test]
    fn pick_route_normal() {
        assert_eq!(pick_route(None, false, false, None), PickRoute::Normal);
    }

    fn blue() -> Color {
        Color::from_srgb8([0x34, 0x78, 0xF6])
    }

    fn state_with_palette(n: u8) -> (tempfile::TempDir, AppState, Palette) {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(Paths::in_dir(dir.path()));
        let mut pal = Palette::new("Brand");
        for i in 0..n {
            pal.colors.push(colorz_core::palette::PaletteColor {
                color: Color::from_srgb8([i * 40, 10, 20]),
                label: format!("c{i}"),
                var_name: String::new(),
            });
        }
        state.upsert_palette(pal.clone());
        (dir, state, pal)
    }

    fn collect(state: &AppState) -> Rc<RefCell<Vec<StateEvent>>> {
        let events = Rc::new(RefCell::new(Vec::new()));
        let seen = events.clone();
        state.connect(move |e| seen.borrow_mut().push(e.clone()));
        events
    }

    #[test]
    fn active_palette_resolves_and_clears() {
        let (_d, state, pal) = state_with_palette(0);
        assert!(state.active_palette().is_none());
        let added = state.add_to_palette(&pal.id, &[blue()]).unwrap();
        assert_eq!(added.colors.len(), 1);
        assert_eq!(state.active_palette().unwrap().id, pal.id);
        assert!(state.add_to_palette("nope", &[blue()]).is_none());
        state.delete_palette(&pal.id);
        assert!(state.active_palette().is_none());
    }

    #[test]
    fn add_and_undo_add() {
        let (_d, state, pal) = state_with_palette(1);
        let before = state.palette(&pal.id).unwrap();
        let cs = [
            Color::from_srgb8([1, 2, 3]),
            Color::from_srgb8([4, 5, 6]),
            Color::from_srgb8([7, 8, 9]),
        ];
        state.add_to_palette(&pal.id, &cs).unwrap();
        assert_eq!(state.palette(&pal.id).unwrap().colors.len(), 4);
        assert!(state.undo_add(&pal.id, &cs));
        assert_eq!(state.palette(&pal.id).unwrap(), before);
        assert!(!state.undo_add(&pal.id, &cs));
    }

    #[test]
    fn start_save_writes_current() {
        let (_d, state, pal) = state_with_palette(3);
        let want = pal.colors[1].color;
        assert_eq!(state.start_editing(&pal.id, 1), Ok(None));
        assert_eq!(state.current(), want);
        let x = Color::from_srgb8([200, 100, 50]);
        state.set_current(x, ColorSource::Wheel);
        assert_eq!(state.save_editing(), Ok(()));
        let after = state.palette(&pal.id).unwrap();
        assert_eq!(after.colors[1].color, x);
        assert_eq!(after.colors[1].label, "c1");
        assert!(state.editing().is_none());
    }

    #[test]
    fn cancel_restores_original() {
        let (_d, state, pal) = state_with_palette(2);
        let orig = pal.colors[0].color;
        state.start_editing(&pal.id, 0).unwrap();
        state.set_current(blue(), ColorSource::Wheel);
        state.cancel_editing();
        assert_eq!(state.current(), orig);
        assert!(state.editing().is_none());
        assert_eq!(state.palette(&pal.id).unwrap(), pal);
    }

    #[test]
    fn start_reports_discarded_changes() {
        let (_d, state, pal) = state_with_palette(3);
        state.start_editing(&pal.id, 0).unwrap();
        let session_a = state.editing().unwrap();
        state.set_current(blue(), ColorSource::Wheel);
        assert_eq!(state.start_editing(&pal.id, 1), Ok(Some(session_a)));
        assert_eq!(state.start_editing(&pal.id, 1), Ok(None));
    }

    #[test]
    fn start_same_color_keeps_edit() {
        let (_d, state, pal) = state_with_palette(3);
        state.start_editing(&pal.id, 1).unwrap();
        state.set_current(blue(), ColorSource::Wheel);
        let session = state.editing();
        let events = collect(&state);
        assert_eq!(state.start_editing(&pal.id, 1), Ok(None));
        assert_eq!(state.current(), blue());
        assert_eq!(state.editing(), session);
        assert!(events.borrow().is_empty(), "{:?}", events.borrow());
    }

    #[test]
    fn start_rejects_bad_index() {
        let (_d, state, pal) = state_with_palette(2);
        assert_eq!(state.start_editing(&pal.id, 5), Err(()));
        assert_eq!(state.start_editing("nope", 0), Err(()));
        assert!(state.editing().is_none());
    }

    #[test]
    fn session_ends_when_palette_changes() {
        let (_d, state, pal) = state_with_palette(3);
        state.start_editing(&pal.id, 0).unwrap();
        let events = collect(&state);
        let mut moved = state.palette(&pal.id).unwrap();
        moved.colors.swap(0, 1);
        state.upsert_palette(moved.clone());
        assert!(state.editing().is_none());
        let ev = events.borrow();
        let lost = ev.iter().position(|e| *e == StateEvent::EditingLost);
        let editing = ev.iter().position(|e| *e == StateEvent::Editing);
        assert!(lost.is_some() && editing > lost, "{ev:?}");
        assert_eq!(state.palette(&pal.id).unwrap(), moved);
    }

    #[test]
    fn save_after_external_change_is_refused() {
        let (_d, state, pal) = state_with_palette(2);
        state.start_editing(&pal.id, 0).unwrap();
        let ext = Color::from_srgb8([9, 9, 9]);
        state.replace_in_palette(&pal.id, 0, ext).unwrap();
        assert!(state.editing().is_none());
        assert_eq!(state.save_editing(), Ok(()));
        assert_eq!(state.palette(&pal.id).unwrap().colors[0].color, ext);
    }

    #[test]
    fn pick_purpose_is_taken_once() {
        let (_d, state, pal) = state_with_palette(0);
        assert_eq!(state.take_pick_purpose(), None);
        let p = PickPurpose::AddTo { palette_id: pal.id };
        state.set_pick_purpose(Some(p.clone()));
        assert_eq!(state.take_pick_purpose(), Some(p));
        assert_eq!(state.take_pick_purpose(), None);
    }

    #[test]
    fn set_current_emits_once() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(Paths::in_dir(dir.path()));
        let events = Rc::new(RefCell::new(Vec::new()));
        let seen = events.clone();
        state.connect(move |e| seen.borrow_mut().push(e.clone()));
        state.set_current(blue(), ColorSource::Pick);
        assert_eq!(*events.borrow(), vec![StateEvent::Color(ColorSource::Pick)]);
        assert_eq!(state.current(), blue());
    }

    #[test]
    fn listener_can_reenter() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(Paths::in_dir(dir.path()));
        let calls = Rc::new(Cell::new(0));
        let (s, n) = (state.clone(), calls.clone());
        state.connect(move |e| {
            n.set(n.get() + 1);
            if *e == StateEvent::Color(ColorSource::Pick) {
                s.set_current(Color::from_srgb8([1, 2, 3]), ColorSource::History);
                s.connect(|_| {});
            }
        });
        state.set_current(blue(), ColorSource::Pick);
        assert_eq!(calls.get(), 2);
        assert_eq!(state.current().to_srgb8(), [1, 2, 3]);
    }

    #[test]
    fn target_falls_back_on_bad_icc() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        let config = Config {
            target: TargetChoice::Icc {
                path: PathBuf::from("/nonexistent.icc"),
            },
            ..Config::default()
        };
        save_config(&paths, &config).unwrap();
        let state = AppState::new(paths);
        assert!(state.with_target(|t| t.is_srgb()));
        assert!(state.target_error().is_some());
    }

    #[test]
    #[cfg(unix)]
    fn unreadable_config_is_never_saved() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        std::fs::write(&paths.config_file, "history_size = 7\n").unwrap();
        let mode = |m| std::fs::Permissions::from_mode(m);
        std::fs::set_permissions(&paths.config_file, mode(0o000)).unwrap();
        if std::fs::read(&paths.config_file).is_ok() {
            return; // running as root: the file is readable anyway
        }
        let state = AppState::new(paths.clone());
        let configs = Rc::new(Cell::new(0));
        let n = configs.clone();
        state.connect(move |e| {
            if *e == StateEvent::Config {
                n.set(n.get() + 1);
            }
        });
        state.update_config(|c| c.chrome_correction = true);
        state.update_config(|c| c.target = TargetChoice::AdobeRgb);
        assert!(state.config().chrome_correction);
        assert_eq!(state.with_target(|t| t.name()), "Adobe RGB");
        assert_eq!(configs.get(), 2);
        std::fs::set_permissions(&paths.config_file, mode(0o600)).unwrap();
        assert_eq!(
            std::fs::read_to_string(&paths.config_file).unwrap(),
            "history_size = 7\n"
        );
    }

    #[test]
    fn palettes_upsert_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        let state = AppState::new(paths.clone());
        let mut b = Palette::new("b");
        state.upsert_palette(b.clone());
        state.upsert_palette(Palette::new("a"));
        b.name = "c".into();
        state.upsert_palette(b.clone());
        let names: Vec<_> = state.palettes().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["a", "c"]);
        assert_eq!(storage::load_palettes(&paths), state.palettes());
        assert_eq!(state.delete_palette(&b.id), Some(b.clone()));
        assert_eq!(state.delete_palette(&b.id), None);
        assert!(state.palette(&b.id).is_none());
        assert_eq!(storage::load_palettes(&paths), state.palettes());
    }

    #[test]
    fn failed_saves_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        // Files where directories are expected, and vice versa.
        std::fs::write(&paths.palettes_dir, "").unwrap();
        std::fs::create_dir(&paths.history_file).unwrap();
        let state = AppState::new(paths);
        let failures = Rc::new(RefCell::new(Vec::new()));
        let seen = failures.clone();
        state.connect(move |e| {
            if let StateEvent::SaveFailed(msg) = e {
                seen.borrow_mut().push(msg.clone());
            }
        });
        let pal = Palette::new("Brand");
        state.upsert_palette(pal.clone());
        state.push_history(blue());
        assert_eq!(failures.borrow().len(), 2, "{:?}", failures.borrow());
        assert!(failures.borrow()[0].contains("Brand"));
        // The in-memory state is still updated.
        assert_eq!(state.palettes(), [pal]);
        assert_eq!(state.history().entries, [blue()]);
    }

    #[test]
    fn failed_delete_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::in_dir(dir.path());
        let state = AppState::new(paths.clone());
        let pal = Palette::new("Brand");
        state.upsert_palette(pal.clone());
        // A directory where the palette file was: removing it fails.
        let file = paths.palettes_dir.join(format!("{}.json", pal.id));
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        let failures = Rc::new(RefCell::new(Vec::new()));
        let seen = failures.clone();
        state.connect(move |e| {
            if let StateEvent::SaveFailed(msg) = e {
                seen.borrow_mut().push(msg.clone());
            }
        });
        assert_eq!(state.delete_palette(&pal.id), Some(pal));
        let failures = failures.borrow();
        assert_eq!(failures.len(), 1);
        assert!(
            failures[0].starts_with("Could not delete palette Brand: "),
            "{}",
            failures[0]
        );
        assert!(state.palettes().is_empty());
    }
}
