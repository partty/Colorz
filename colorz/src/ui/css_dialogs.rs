//! Palette file dialogs: CSS variables export/import and GIMP `.gpl`
//! export/import.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use colorz_core::css_vars::{
    CssBlock, MergeSummary, export_css, merge_into, merge_preview, palette_from_block,
    parse_css_blocks,
};
use colorz_core::format::Format;
use colorz_core::gpl::{export_gpl, import_gpl};
use gtk::{gio, glib};

use super::copy_text;
use super::window::ColorzWindow;
use crate::state::AppState;

const DEFAULT_SELECTOR: &str = ":root";
const IMPORTED_NAME: &str = "Imported";

// ------------------------------------------------------------------ helpers

/// The summary line of an import: merge counts, or the size of a new palette.
pub fn summary_text(s: &MergeSummary, new_palette: bool) -> String {
    if new_palette {
        let n = s.added;
        let colors = if n == 1 { "color" } else { "colors" };
        format!("{n} {colors} \u{b7} {} skipped", s.skipped)
    } else {
        format!(
            "{} updated \u{b7} {} added \u{b7} {} skipped",
            s.updated, s.added, s.skipped
        )
    }
}

/// A selector that is safe to write in front of `{`; `:root` when blank.
/// Line breaks (CSS also counts form feed) and other control characters
/// never survive, not even inside quoted strings. Outside
/// strings `{`, `}` and `;` become spaces and runs of whitespace collapse;
/// closed strings are otherwise kept as they are. If a string is left open,
/// this fails closed: quotes are dropped too and every `{ } ;` and control
/// character is stripped, so the text can never close or open a rule.
fn clean_selector(raw: &str) -> String {
    let out = clean_quoted(raw).unwrap_or_else(|| strip_structure(raw));
    if out.is_empty() {
        DEFAULT_SELECTOR.to_owned()
    } else {
        out
    }
}

fn is_separator(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, '{' | '}' | ';')
}

/// [`clean_selector`] for well-formed input; `None` when a string is left
/// open.
fn clean_quoted(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut space = false;
    for c in raw.chars() {
        if c.is_control() {
            if quote.is_none() {
                space = true;
            }
            continue;
        }
        if let Some(q) = quote {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        if escaped {
            escaped = false;
            // An escaped structural character is still dropped; the
            // backslash then escapes the space in its place.
            if is_separator(c) {
                space = true;
            } else {
                out.push(c);
            }
            continue;
        }
        if is_separator(c) {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(c);
        if c == '\\' {
            escaped = true;
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        }
    }
    quote.is_none().then_some(out)
}

/// The fail-closed fallback: no quotes, braces, semicolons or control
/// characters.
fn strip_structure(raw: &str) -> String {
    raw.chars()
        .filter(|c| *c != '"' && *c != '\'')
        .map(|c| if is_separator(c) { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A file name that cannot leave its folder.
fn safe_file_name(name: &str, ext: &str) -> String {
    let name: String = name
        .trim()
        .chars()
        .map(|c| if c == '/' || c.is_control() { '_' } else { c })
        .collect();
    let name = if name.is_empty() || name == "." || name == ".." {
        "palette".to_owned()
    } else {
        name
    };
    format!("{name}.{ext}")
}

fn file_stem(file: &gio::File) -> Option<String> {
    let stem = file.basename()?.file_stem()?.to_string_lossy().into_owned();
    (!stem.trim().is_empty()).then_some(stem)
}

fn has_extension(file: &gio::File, ext: &str) -> bool {
    file.basename()
        .and_then(|p| p.extension().map(|e| e.eq_ignore_ascii_case(ext)))
        .unwrap_or(false)
}

fn filter(name: &str, pattern: &str, mime: Option<&str>) -> gtk::FileFilter {
    let f = gtk::FileFilter::new();
    f.set_name(Some(name));
    f.add_pattern(pattern);
    if let Some(m) = mime {
        f.add_mime_type(m);
    }
    f
}

fn css_filter() -> gtk::FileFilter {
    filter("CSS", "*.css", Some("text/css"))
}

fn gpl_filter() -> gtk::FileFilter {
    filter("GIMP palette", "*.gpl", None)
}

fn all_supported_filter() -> gtk::FileFilter {
    let f = css_filter();
    f.set_name(Some("All supported"));
    f.add_pattern("*.gpl");
    f
}

fn dismissed(e: &glib::Error) -> bool {
    e.matches(gtk::DialogError::Dismissed) || e.matches(gio::IOErrorEnum::Cancelled)
}

fn filters(list: &[gtk::FileFilter]) -> gio::ListStore {
    let store = gio::ListStore::new::<gtk::FileFilter>();
    for f in list {
        store.append(f);
    }
    store
}

/// Asks for a file to open; `None` when cancelled (an error is toasted).
async fn pick_open(window: &ColorzWindow, list: &[gtk::FileFilter]) -> Option<gio::File> {
    let dialog = gtk::FileDialog::builder()
        .title("Open")
        .modal(true)
        .filters(&filters(list))
        .build();
    if let Some(first) = list.first() {
        dialog.set_default_filter(Some(first));
    }
    match dialog.open_future(Some(window.window())).await {
        Ok(f) => Some(f),
        Err(e) => {
            if !dismissed(&e) {
                window.show_toast("Could not open the file chooser");
            }
            None
        }
    }
}

/// Asks where to save; `None` when cancelled (an error is toasted).
async fn pick_save(
    window: &ColorzWindow,
    title: &str,
    initial_name: &str,
    filter: &gtk::FileFilter,
) -> Option<gio::File> {
    let dialog = gtk::FileDialog::builder()
        .title(title)
        .modal(true)
        .initial_name(initial_name)
        .filters(&filters(std::slice::from_ref(filter)))
        .default_filter(filter)
        .build();
    match dialog.save_future(Some(window.window())).await {
        Ok(f) => Some(f),
        Err(e) => {
            if !dismissed(&e) {
                window.show_toast("Could not open the file chooser");
            }
            None
        }
    }
}

async fn read_text(file: &gio::File) -> Result<String, &'static str> {
    let (bytes, _) = file
        .load_contents_future()
        .await
        .map_err(|_| "Could not read the file")?;
    let text = String::from_utf8(bytes.to_vec()).map_err(|_| "The file is not valid UTF-8 text")?;
    Ok(text.trim_start_matches('\u{feff}').to_owned())
}

/// Writes `text` atomically; true on success, otherwise toasts.
async fn write_text(window: &ColorzWindow, file: &gio::File, text: String) -> bool {
    let res = file
        .replace_contents_future(
            text.into_bytes(),
            None,
            false,
            gio::FileCreateFlags::REPLACE_DESTINATION,
        )
        .await;
    match res {
        Ok(_) => true,
        Err(_) => {
            window.show_toast("Could not save the file");
            false
        }
    }
}

fn saved_toast(window: &ColorzWindow, file: &gio::File) {
    let name = file
        .basename()
        .map_or_else(String::new, |p| p.to_string_lossy().into_owned());
    window.show_toast(&format!("Saved {name}"));
}

/// Shows palette `id` in the Palettes tab.
pub(super) fn open_palette(window: &ColorzWindow, id: &str) {
    window.set_page("palettes");
    let _ = WidgetExt::activate_action(window.window(), "win.open-palette", Some(&id.to_variant()));
}

// -------------------------------------------------------------- CSS export

/// The formats offered for CSS export: those that write valid CSS.
fn css_formats() -> Vec<Format> {
    Format::ALL.into_iter().filter(|f| f.is_css()).collect()
}

/// Export the palette as CSS variables.
pub fn export_css_dialog(window: &ColorzWindow, state: &AppState, palette_id: &str) {
    let Some(pal) = state.palette(palette_id) else {
        return;
    };
    let opts = state.format_options();
    let formats = css_formats();
    // The default format when it is valid CSS, otherwise HEX (the first).
    let default_format = state.config().default_format;
    let default_index = formats
        .iter()
        .position(|f| *f == default_format)
        .unwrap_or(0);

    let format_list = gtk::StringList::new(&[]);
    for f in &formats {
        format_list.append(f.label());
    }
    let format_row = adw::ComboRow::builder()
        .title("Format")
        .model(&format_list)
        .selected(default_index as u32)
        .build();
    let selector_row = adw::EntryRow::builder()
        .title("Selector")
        .text(&pal.selector)
        .build();
    let group = adw::PreferencesGroup::new();
    group.add(&format_row);
    group.add(&selector_row);

    let preview = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::None)
        .left_margin(8)
        .right_margin(8)
        .top_margin(8)
        .bottom_margin(8)
        .build();
    preview.update_property(&[gtk::accessible::Property::Label("CSS preview")]);
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&preview)
        .min_content_height(220)
        .vexpand(true)
        .css_classes(["card"])
        .build();

    let copy = gtk::Button::builder()
        .label("_Copy")
        .use_underline(true)
        .build();
    let save = gtk::Button::builder()
        .label("_Save\u{2026}")
        .use_underline(true)
        .css_classes(["suggested-action"])
        .build();
    let buttons = gtk::Box::builder()
        .spacing(6)
        .halign(gtk::Align::End)
        .build();
    buttons.append(&copy);
    buttons.append(&save);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&group);
    content.append(&scrolled);
    content.append(&buttons);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&content));
    let dialog = adw::Dialog::builder()
        .title("Export as CSS Variables")
        .content_width(480)
        .child(&toolbar)
        .build();

    // The rows own these closures, so they are held weakly.
    let id = pal.id.clone();
    let current = {
        let (state, format_row, selector_row) = (
            state.clone(),
            format_row.downgrade(),
            selector_row.downgrade(),
        );
        move || -> String {
            let (Some(p), Some(format_row), Some(selector_row)) = (
                state.palette(&id),
                format_row.upgrade(),
                selector_row.upgrade(),
            ) else {
                return String::new();
            };
            let format = formats
                .get(format_row.selected() as usize)
                .copied()
                .unwrap_or(Format::Hex);
            export_css(&p, format, &clean_selector(&selector_row.text()), &opts)
        }
    };
    let current = Rc::new(current);
    let refresh = {
        let (preview, current) = (preview.downgrade(), current.clone());
        move || {
            if let Some(preview) = preview.upgrade() {
                preview.buffer().set_text(&current());
            }
        }
    };
    refresh();
    format_row.connect_selected_notify({
        let refresh = refresh.clone();
        move |_| refresh()
    });
    selector_row.connect_changed(move |_| refresh());

    // Remembers the selector on the palette.
    let remember = {
        let (state, id, selector_row) = (state.clone(), pal.id.clone(), selector_row.downgrade());
        move || {
            let Some(selector_row) = selector_row.upgrade() else {
                return;
            };
            let selector = clean_selector(&selector_row.text());
            if let Some(mut p) = state.palette(&id)
                && p.selector != selector
            {
                p.selector = selector;
                state.upsert_palette(p);
            }
        }
    };
    let remember = Rc::new(remember);

    copy.connect_clicked({
        let (window, dialog, current, remember) = (
            window.clone(),
            dialog.downgrade(),
            current.clone(),
            remember.clone(),
        );
        move |_| {
            if copy_text(&window, &current()) {
                remember();
                window.show_toast("Copied CSS");
            }
            if let Some(d) = dialog.upgrade() {
                d.close();
            }
        }
    });
    save.connect_clicked({
        let (window, dialog, state, id) = (
            window.clone(),
            dialog.downgrade(),
            state.clone(),
            pal.id.clone(),
        );
        move |_| {
            let (window, dialog, text) = (window.clone(), dialog.clone(), current());
            let (remember, name) = (
                remember.clone(),
                state.palette(&id).map_or_else(String::new, |p| p.name),
            );
            glib::spawn_future_local(async move {
                let initial = safe_file_name(&name, "css");
                let Some(file) = pick_save(&window, "Export CSS", &initial, &css_filter()).await
                else {
                    return;
                };
                if write_text(&window, &file, text).await {
                    remember();
                    saved_toast(&window, &file);
                    if let Some(d) = dialog.upgrade() {
                        d.close();
                    }
                }
            });
        }
    });
    dialog.set_focus(Some(&selector_row));
    dialog.present(Some(window.window()));
}

// -------------------------------------------------------------- CSS import

/// How a block is named in the chooser.
fn block_title(b: &CssBlock) -> String {
    if b.selector.is_empty() {
        "(no selector)".to_owned()
    } else {
        b.selector.clone()
    }
}

struct Import {
    window: ColorzWindow,
    state: AppState,
    into: Option<String>,
    dialog: glib::WeakRef<adw::Dialog>,
    blocks: RefCell<Vec<CssBlock>>,
    loaded: Cell<bool>,
    /// The name `load` filled in last.
    auto_name: RefCell<String>,
    block_row: adw::ComboRow,
    name_row: adw::EntryRow,
    summary: gtk::Label,
    apply: gtk::Button,
}

impl Import {
    /// Takes CSS text (from a file or the clipboard) as the source.
    fn load(&self, text: &str, stem: Option<&str>) {
        let blocks = parse_css_blocks(text);
        let titles = gtk::StringList::new(&[]);
        for b in &blocks {
            titles.append(&block_title(b));
        }
        self.block_row.set_model(Some(&titles));
        self.block_row.set_selected(0);
        self.block_row.set_visible(blocks.len() > 1);
        *self.blocks.borrow_mut() = blocks;
        self.loaded.set(true);
        // Fill the name only while the user has not typed their own.
        let typed = self.name_row.text();
        if self.into.is_none() && (typed.is_empty() || *typed == *self.auto_name.borrow()) {
            let name = stem.unwrap_or(IMPORTED_NAME);
            self.auto_name.replace(name.to_owned());
            self.name_row.set_text(name);
        }
        self.update();
    }

    fn selected_block(&self) -> Option<CssBlock> {
        let blocks = self.blocks.borrow();
        blocks.get(self.block_row.selected() as usize).cloned()
    }

    /// The summary of what Apply would do; `None` without colors.
    fn preview(&self) -> Option<MergeSummary> {
        let block = self.selected_block().filter(|b| !b.vars.is_empty())?;
        match &self.into {
            None => Some(MergeSummary {
                updated: 0,
                added: block.vars.len(),
                skipped: block.skipped,
            }),
            Some(id) => Some(merge_preview(&self.state.palette(id)?, &block)),
        }
    }

    fn update(&self) {
        let (text, ok) = match self.preview() {
            Some(s) => {
                let named = self.into.is_some() || !self.name_row.text().trim().is_empty();
                (summary_text(&s, self.into.is_none()), named)
            }
            None if self.loaded.get() => ("No color variables found".to_owned(), false),
            None => (
                "Open a CSS file or paste CSS from the clipboard.".to_owned(),
                false,
            ),
        };
        self.summary.set_label(&text);
        self.apply.set_sensitive(ok);
    }

    fn close(&self) {
        if let Some(d) = self.dialog.upgrade() {
            d.close();
        }
    }

    fn apply(&self) {
        let Some(block) = self.selected_block().filter(|b| !b.vars.is_empty()) else {
            return;
        };
        match &self.into {
            None => {
                let name = self.name_row.text();
                let name = name.trim();
                if name.is_empty() {
                    return;
                }
                let pal = palette_from_block(name, &block);
                let id = pal.id.clone();
                self.state.upsert_palette(pal);
                self.close();
                open_palette(&self.window, &id);
                self.window.show_toast(&format!("Imported {name}"));
            }
            Some(id) => {
                let Some(mut pal) = self.state.palette(id) else {
                    self.window.show_toast("The palette no longer exists");
                    self.close();
                    return;
                };
                let s = merge_into(&mut pal, &block);
                let name = pal.name.clone();
                self.state.upsert_palette(pal);
                self.close();
                self.window.show_toast(&format!(
                    "Imported into {name}: {}",
                    summary_text(&s, false)
                ));
            }
        }
    }
}

fn import_css_with(
    window: &ColorzWindow,
    state: &AppState,
    into: Option<String>,
    source: Option<(String, Option<String>)>,
) {
    if let Some(id) = &into
        && state.palette(id).is_none()
    {
        return;
    }
    let open_btn = gtk::Button::builder()
        .label("_Open File\u{2026}")
        .use_underline(true)
        .hexpand(true)
        .build();
    let paste_btn = gtk::Button::builder()
        .label("_Paste From Clipboard")
        .use_underline(true)
        .hexpand(true)
        .build();
    let sources = gtk::Box::builder().spacing(6).homogeneous(true).build();
    sources.append(&open_btn);
    sources.append(&paste_btn);

    let block_row = adw::ComboRow::builder()
        .title("Rule")
        .visible(false)
        .build();
    let name_row = adw::EntryRow::builder()
        .title("Palette name")
        .visible(into.is_none())
        .build();
    let group = adw::PreferencesGroup::new();
    group.add(&block_row);
    group.add(&name_row);

    let summary = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label"])
        .build();
    let cancel = gtk::Button::builder()
        .label("_Cancel")
        .use_underline(true)
        .build();
    let apply = gtk::Button::builder()
        .label("_Apply")
        .use_underline(true)
        .sensitive(false)
        .css_classes(["suggested-action"])
        .build();
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    header.pack_start(&cancel);
    header.pack_end(&apply);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&sources);
    content.append(&group);
    content.append(&summary);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    let title = if into.is_some() {
        "Import CSS Variables Into Palette"
    } else {
        "Import CSS Variables"
    };
    let dialog = adw::Dialog::builder()
        .title(title)
        .content_width(480)
        .child(&toolbar)
        .build();

    let imp = Rc::new(Import {
        window: window.clone(),
        state: state.clone(),
        into,
        dialog: dialog.downgrade(),
        blocks: RefCell::default(),
        loaded: Cell::new(false),
        auto_name: RefCell::default(),
        block_row: block_row.clone(),
        name_row: name_row.clone(),
        summary,
        apply: apply.clone(),
    });
    imp.update();
    // The dialog's handlers hold `imp` weakly except for this one, which
    // keeps it alive until the dialog is gone.
    dialog.connect_closed({
        let keep = imp.clone();
        move |_| {
            let _ = &keep;
        }
    });
    let weak = Rc::downgrade(&imp);

    cancel.connect_clicked({
        let dialog = dialog.downgrade();
        move |_| {
            if let Some(d) = dialog.upgrade() {
                d.close();
            }
        }
    });
    apply.connect_clicked({
        let weak = weak.clone();
        move |_| {
            if let Some(i) = weak.upgrade() {
                i.apply();
            }
        }
    });
    block_row.connect_selected_notify({
        let weak = weak.clone();
        move |_| {
            if let Some(i) = weak.upgrade() {
                i.update();
            }
        }
    });
    name_row.connect_changed({
        let weak = weak.clone();
        move |_| {
            if let Some(i) = weak.upgrade() {
                i.update();
            }
        }
    });
    open_btn.connect_clicked({
        let (weak, window) = (weak.clone(), window.clone());
        move |_| {
            let (weak, window) = (weak.clone(), window.clone());
            glib::spawn_future_local(async move {
                let Some(file) = pick_open(&window, &[css_filter()]).await else {
                    return;
                };
                match read_text(&file).await {
                    Ok(text) => {
                        if let Some(i) = weak.upgrade() {
                            i.load(&text, file_stem(&file).as_deref());
                        }
                    }
                    Err(msg) => window.show_toast(msg),
                }
            });
        }
    });
    paste_btn.connect_clicked({
        let (weak, window) = (weak.clone(), window.clone());
        move |btn| {
            let clipboard = btn.clipboard();
            let (weak, window) = (weak.clone(), window.clone());
            glib::spawn_future_local(async move {
                match clipboard.read_text_future().await {
                    Ok(Some(text)) => {
                        if let Some(i) = weak.upgrade() {
                            i.load(&text, None);
                        }
                    }
                    Ok(None) => window.show_toast("The clipboard has no text"),
                    Err(_) => window.show_toast("Could not read the clipboard"),
                }
            });
        }
    });

    if let Some((text, stem)) = source {
        imp.load(&text, stem.as_deref());
    }
    dialog.present(Some(window.window()));
}

/// Import CSS variables as a new palette, or into palette `into`.
pub fn import_css_dialog(window: &ColorzWindow, state: &AppState, into: Option<String>) {
    import_css_with(window, state, into, None);
}

// --------------------------------------------------------------------- GPL

/// Export the palette as a GIMP `.gpl` file.
pub fn export_gpl_dialog(window: &ColorzWindow, state: &AppState, palette_id: &str) {
    let Some(pal) = state.palette(palette_id) else {
        return;
    };
    let window = window.clone();
    glib::spawn_future_local(async move {
        let initial = safe_file_name(&pal.name, "gpl");
        let Some(file) = pick_save(&window, "Export GIMP Palette", &initial, &gpl_filter()).await
        else {
            return;
        };
        if write_text(&window, &file, export_gpl(&pal)).await {
            saved_toast(&window, &file);
        }
    });
}

fn import_gpl_text(window: &ColorzWindow, state: &AppState, text: &str, stem: Option<&str>) {
    match import_gpl(text, stem.unwrap_or(IMPORTED_NAME)) {
        Ok(pal) => {
            let name = pal.name.clone();
            state.upsert_palette(pal);
            window.show_toast(&format!("Imported {name}"));
        }
        Err(e) => window.show_toast(&format!("Not a valid GIMP palette: {e}")),
    }
}

/// Import a palette from a `.css` or `.gpl` file.
pub fn import_palette_file(window: &ColorzWindow, state: &AppState) {
    let (window, state) = (window.clone(), state.clone());
    glib::spawn_future_local(async move {
        let list = [all_supported_filter(), css_filter(), gpl_filter()];
        let Some(file) = pick_open(&window, &list).await else {
            return;
        };
        let is_gpl = has_extension(&file, "gpl");
        if !is_gpl && !has_extension(&file, "css") {
            window.show_toast("Unsupported file");
            return;
        }
        let text = match read_text(&file).await {
            Ok(t) => t,
            Err(msg) => {
                window.show_toast(msg);
                return;
            }
        };
        let stem = file_stem(&file);
        if is_gpl {
            import_gpl_text(&window, &state, &text, stem.as_deref());
        } else {
            import_css_with(&window, &state, None, Some((text, stem)));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_text_formats() {
        let s = MergeSummary {
            updated: 3,
            added: 2,
            skipped: 5,
        };
        assert_eq!(
            summary_text(&s, false),
            "3 updated \u{b7} 2 added \u{b7} 5 skipped"
        );
        assert_eq!(summary_text(&s, true), "2 colors \u{b7} 5 skipped");
        let one = MergeSummary {
            updated: 0,
            added: 1,
            skipped: 0,
        };
        assert_eq!(summary_text(&one, true), "1 color \u{b7} 0 skipped");
    }

    #[test]
    fn selectors_and_file_names_are_cleaned() {
        assert_eq!(clean_selector("  "), ":root");
        assert_eq!(clean_selector("a {\n b }"), "a b");
        assert_eq!(
            clean_selector("a[title=\"x  {y}\"]  b"),
            "a[title=\"x  {y}\"] b"
        );
        assert_eq!(clean_selector("a[t='it\\'s  {']"), "a[t='it\\'s  {']");
        assert_eq!(
            clean_selector("[data-theme=\"dark\"]"),
            "[data-theme=\"dark\"]"
        );
        assert_eq!(clean_selector("a;\r\nb"), "a b");
        assert_eq!(clean_selector("a[t=\"{;}\"]"), "a[t=\"{;}\"]");
        assert_eq!(safe_file_name("a/b", "css"), "a_b.css");
        assert_eq!(safe_file_name("..", "gpl"), "palette.gpl");
    }

    /// Scans `sel {` the way a CSS tokenizer does; true when the only
    /// structural character outside strings is the final `{`, every string
    /// is closed and there are no line breaks or other control characters.
    fn is_one_prelude(sel: &str) -> bool {
        let text = format!("{sel} {{");
        if text.chars().any(char::is_control) {
            return false;
        }
        let mut chars = text.chars();
        let mut structural = Vec::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' | '\'' => loop {
                    match chars.next() {
                        None => return false,
                        Some('\\') => {
                            chars.next();
                        }
                        Some(q) if q == c => break,
                        Some(_) => {}
                    }
                },
                '{' | '}' | ';' => structural.push(c),
                _ => {}
            }
        }
        structural == ['{']
    }

    #[test]
    fn unterminated_quotes_fail_closed() {
        assert_eq!(
            clean_selector("a[\"x {} body {color:red}"),
            "a[x body color:red"
        );
        assert_eq!(clean_selector("a['x;\n}"), "a[x");
        // Line breaks are dropped even inside a closed string.
        assert_eq!(clean_selector("a[t=\"x\ny\"]"), "a[t=\"xy\"]");
        // So are form feeds (a CSS newline) and other control characters.
        assert_eq!(
            clean_selector("a[t=\"x\u{c}y\0z\u{7f}\"]\u{c}b\0c"),
            "a[t=\"xyz\"] b c"
        );
        assert_eq!(clean_selector("a[\"x\u{c}{}\0b"), "a[x b");
        // An escaped quote does not open a string.
        assert_eq!(clean_selector("a\\\" {} b"), "a\\\" b");
        for raw in [
            "a[\"x {} body {color:red}",
            "a\\\" {} body {color:red} \"",
            "a[t=\"x\\\"] {} body {color:red}",
            "a[t=\"x\\\n\"] } body {",
            "a[t='x\n'] { b }",
            "a[t=\"x\u{c}\"] } body {",
            "a[t=\"x\\\u{c}\"] } body {",
            "a[t='\0'] {",
            "\"",
            "a\\",
            "a\\{}",
            "a[t=\"x\"] ;} b {",
            "@media x { a",
        ] {
            let clean = clean_selector(raw);
            assert!(is_one_prelude(&clean), "{raw:?} -> {clean:?}");
        }
        // Every string of up to five of these characters.
        let alphabet = ['a', '"', '\'', '\\', '{', '}', ';', '\n', '\u{c}', '\0'];
        let mut raw = Vec::new();
        for len in 0..=5u32 {
            for mut n in 0..alphabet.len().pow(len) {
                raw.clear();
                for _ in 0..len {
                    raw.push(alphabet[n % alphabet.len()]);
                    n /= alphabet.len();
                }
                let raw: String = raw.iter().collect();
                let clean = clean_selector(&raw);
                assert!(is_one_prelude(&clean), "{raw:?} -> {clean:?}");
            }
        }
    }
}
