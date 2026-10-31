//! The Palettes tab: a searchable list of palettes that opens the palette
//! view.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use colorz_core::palette::Palette;
use gtk::{gio, glib};

use super::palette_view::PaletteView;
use super::pick_page::swatch_area;
use super::window::ColorzWindow;
use super::{NEW_PALETTE_NAME, prompt_name};
use crate::state::{AppState, StateEvent};

/// A card's color strip never shows more cells than this.
const STRIP_CELLS: usize = 16;
const CELL_SIZE: (i32, i32) = (22, 16);
const CELL_RADIUS: f64 = 3.0;

/// The palettes whose name contains `q`, ignoring case and surrounding
/// spaces; all of them when `q` is blank.
pub fn filter<'a>(pals: &'a [Palette], q: &str) -> Vec<&'a Palette> {
    let q = q.trim().to_lowercase();
    pals.iter()
        .filter(|p| p.name.to_lowercase().contains(&q))
        .collect()
}

fn count_text(n: usize) -> String {
    match n {
        1 => "1 color".to_owned(),
        n => format!("{n} colors"),
    }
}

struct Inner {
    state: AppState,
    window: ColorzWindow,
    search: gtk::SearchEntry,
    list: gtk::ListBox,
    stack: gtk::Stack,
    /// Ids of the cards, in order.
    ids: RefCell<Vec<String>>,
    view: PaletteView,
}

/// The page widget plus the state that drives it. Keep the `PalettesPage`
/// alive as long as the page is shown: the state listener holds it weakly.
pub struct PalettesPage {
    widget: adw::NavigationView,
    _inner: Rc<Inner>,
}

impl PalettesPage {
    pub fn widget(&self) -> &adw::NavigationView {
        &self.widget
    }

    pub fn new(state: &AppState, window: &ColorzWindow) -> PalettesPage {
        let nav = adw::NavigationView::new();
        let view = PaletteView::new(state, window, &nav);

        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search palettes")
            .hexpand(true)
            .build();
        search.update_property(&[gtk::accessible::Property::Label("Search palettes")]);
        let new_menu = gio::Menu::new();
        new_menu.append(Some("Empty Palette"), Some("palettes.new-empty"));
        new_menu.append(Some("From Image\u{2026}"), Some("app.palette-from-image"));
        let new = gtk::MenuButton::builder()
            .label("New")
            .always_show_arrow(true)
            .menu_model(&new_menu)
            .tooltip_text("New Palette")
            .build();
        let import = gtk::Button::builder()
            .label("Import\u{2026}")
            .action_name("app.import-palette")
            .tooltip_text("Import a Palette From a CSS or GIMP File")
            .build();
        let bar = gtk::Box::builder().spacing(6).build();
        bar.append(&search);
        bar.append(&new);
        bar.append(&import);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .valign(gtk::Align::Start)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        let empty = adw::StatusPage::builder()
            .icon_name("view-grid-symbolic")
            .title("No palettes yet")
            .description("Create a palette, or import one from a CSS or GIMP file.")
            .build();
        let no_match = adw::StatusPage::builder()
            .icon_name("edit-find-symbolic")
            .title("No matching palettes")
            .build();
        let stack = gtk::Stack::builder().vexpand(true).build();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&no_match, Some("no-match"));
        stack.add_named(&scrolled, Some("list"));

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&bar);
        content.append(&stack);
        content.set_vexpand(true);
        let list_page = adw::NavigationPage::builder()
            .tag("list")
            .title("Palettes")
            .child(&content)
            .build();
        nav.add(&list_page);

        let inner = Rc::new(Inner {
            state: state.clone(),
            window: window.clone(),
            search: search.clone(),
            list: list.clone(),
            stack,
            ids: RefCell::default(),
            view,
        });

        let weak = Rc::downgrade(&inner);
        let actions = gio::SimpleActionGroup::new();
        let new_empty = gio::SimpleAction::new("new-empty", None);
        new_empty.connect_activate({
            let weak = weak.clone();
            move |_, _| {
                if let Some(i) = weak.upgrade() {
                    i.new_empty();
                }
            }
        });
        actions.add_action(&new_empty);
        list_page.insert_action_group("palettes", Some(&actions));
        // Lets the import dialogs show a palette they created.
        let open = gio::SimpleAction::new("open-palette", Some(glib::VariantTy::STRING));
        open.connect_activate({
            let weak = weak.clone();
            move |_, param| {
                let id = param.and_then(|p| p.get::<String>());
                if let (Some(i), Some(id)) = (weak.upgrade(), id) {
                    i.view.open(&id);
                }
            }
        });
        window.window().add_action(&open);

        search.connect_search_changed({
            let weak = weak.clone();
            move |_| {
                if let Some(i) = weak.upgrade() {
                    i.refresh();
                }
            }
        });
        list.connect_row_activated({
            let weak = weak.clone();
            move |_, row| {
                let Some(i) = weak.upgrade() else { return };
                let id = usize::try_from(row.index())
                    .ok()
                    .and_then(|n| i.ids.borrow().get(n).cloned());
                if let Some(id) = id {
                    i.view.open(&id);
                }
            }
        });
        inner.refresh();
        state.connect(move |e| {
            if let (StateEvent::Palettes, Some(i)) = (e, weak.upgrade()) {
                i.refresh();
            }
        });
        PalettesPage {
            widget: nav,
            _inner: inner,
        }
    }
}

impl Inner {
    fn refresh(&self) {
        let all = self.state.palettes();
        let shown = filter(&all, &self.search.text());
        self.list.remove_all();
        *self.ids.borrow_mut() = shown.iter().map(|p| p.id.clone()).collect();
        for p in &shown {
            self.list.append(&card(p));
        }
        let page = if all.is_empty() {
            "empty"
        } else if shown.is_empty() {
            "no-match"
        } else {
            "list"
        };
        self.stack.set_visible_child_name(page);
    }

    fn new_empty(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        prompt_name(
            &self.window,
            "New Palette",
            NEW_PALETTE_NAME,
            "_Create",
            move |name| {
                if let Some(this) = weak.upgrade() {
                    let pal = Palette::new(&name);
                    let id = pal.id.clone();
                    this.state.upsert_palette(pal);
                    this.view.open(&id);
                }
            },
        );
    }
}

/// A list card: name, color count and a strip of the colors.
fn card(p: &Palette) -> gtk::ListBoxRow {
    let name = gtk::Label::builder()
        .label(if p.name.trim().is_empty() {
            "Untitled palette"
        } else {
            &p.name
        })
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["heading"])
        .build();
    let count = gtk::Label::builder()
        .label(count_text(p.colors.len()))
        .css_classes(["dim-label", "caption"])
        .build();
    let top = gtk::Box::builder().spacing(8).build();
    top.append(&name);
    top.append(&count);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&top);
    if !p.colors.is_empty() {
        let strip = gtk::Box::builder().spacing(2).build();
        for c in p.colors.iter().take(STRIP_CELLS) {
            let cell = swatch_area(
                Some(CELL_SIZE),
                CELL_RADIUS,
                Rc::new(Cell::new(c.color.to_srgb8())),
            );
            strip.append(&cell);
        }
        content.append(&strip);
    }
    gtk::ListBoxRow::builder()
        .child(&content)
        .activatable(true)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_palettes() {
        let pals: Vec<Palette> = ["Brand", "Dark theme", "brandy"].map(Palette::new).into();
        let names = |q: &str| -> Vec<&str> {
            filter(&pals, q)
                .into_iter()
                .map(|p| p.name.as_str())
                .collect()
        };
        assert_eq!(names(""), ["Brand", "Dark theme", "brandy"]);
        assert_eq!(names("  "), ["Brand", "Dark theme", "brandy"]);
        assert_eq!(names("BRAND"), ["Brand", "brandy"]);
        assert_eq!(names(" theme "), ["Dark theme"]);
        assert!(names("zzz").is_empty());
        assert_eq!(count_text(1), "1 color");
        assert_eq!(count_text(0), "0 colors");
    }
}
