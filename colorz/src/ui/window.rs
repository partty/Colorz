//! The main window: header bar, view switcher, toast overlay and the
//! Pick / Wheel / Palettes pages.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib::prelude::ToValue;

use crate::platform::CAN_PICK;

/// Window width from which the view switcher shows labels, not just icons.
const LABELS_MIN_WIDTH_SP: f64 = 520.0;
const DEFAULT_WIDTH: i32 = 420;
const DEFAULT_HEIGHT: i32 = 620;

/// (name, title, icon) of the view stack pages.
const PAGES: [(&str, &str, &str); 3] = [
    ("pick", "Pick", "color-select-symbolic"),
    ("wheel", "Wheel", "applications-graphics-symbolic"),
    ("palettes", "Palettes", "view-grid-symbolic"),
];

#[derive(Clone)]
pub struct ColorzWindow {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    stack: adw::ViewStack,
    /// Holds the Pick page once [`ColorzWindow::set_pick_page`] is called.
    pick_slot: adw::Bin,
    /// Holds the Wheel page once [`ColorzWindow::set_wheel_page`] is called.
    wheel_slot: adw::Bin,
    /// Holds the Palettes page once [`ColorzWindow::set_palettes_page`] is called.
    palettes_slot: adw::Bin,
    /// Set by [`ColorzWindow::present`]; cleared when the window is shown
    /// only briefly, for a clipboard copy.
    shown_on_purpose: Rc<Cell<bool>>,
}

fn primary_menu() -> gio::Menu {
    let main = gio::Menu::new();
    main.append(Some("_Settings"), Some("app.preferences"));
    main.append(Some("_Keyboard Shortcuts"), Some("app.shortcuts"));
    main.append(Some("_About Colorz"), Some("app.about"));
    let quit = gio::Menu::new();
    quit.append(Some("_Quit"), Some("app.quit"));
    let menu = gio::Menu::new();
    menu.append_section(None, &main);
    menu.append_section(None, &quit);
    menu
}

impl ColorzWindow {
    pub fn new(app: &adw::Application) -> ColorzWindow {
        let stack = adw::ViewStack::new();
        let pick_slot = adw::Bin::new();
        let wheel_slot = adw::Bin::new();
        let palettes_slot = adw::Bin::new();
        // The pages are built after the window (they take it), so the stack
        // holds a slot for each.
        let slots = [&pick_slot, &wheel_slot, &palettes_slot];
        for ((name, title, icon), slot) in PAGES.into_iter().zip(slots) {
            stack.add_titled_with_icon(slot, Some(name), title, icon);
        }

        let pick = gtk::Button::builder()
            .label("Pick")
            .tooltip_text("Pick a Color From the Screen")
            .action_name("app.pick")
            .css_classes(["suggested-action"])
            .visible(CAN_PICK)
            .build();
        let paste = gtk::Button::builder()
            .icon_name("edit-paste-symbolic")
            .tooltip_text("Paste Color")
            .action_name("app.paste")
            .build();
        let menu = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Main Menu")
            .menu_model(&primary_menu())
            .primary(true)
            .build();
        // The inline switcher fits next to the buttons at the default width,
        // where a wide `adw::ViewSwitcher` would not.
        // Icons only until there is room for labels (see the breakpoint).
        let switcher = adw::InlineViewSwitcher::builder()
            .stack(&stack)
            .display_mode(adw::InlineViewSwitcherDisplayMode::Icons)
            .build();

        let header = adw::HeaderBar::builder().title_widget(&switcher).build();
        header.pack_start(&pick);
        header.pack_start(&paste);
        header.pack_end(&menu);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&toasts));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Colorz")
            .default_width(DEFAULT_WIDTH)
            .default_height(DEFAULT_HEIGHT)
            .hide_on_close(true)
            .content(&toolbar)
            .build();

        let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MinWidth,
            LABELS_MIN_WIDTH_SP,
            adw::LengthUnit::Sp,
        ));
        breakpoint.add_setter(
            &switcher,
            "display-mode",
            Some(&adw::InlineViewSwitcherDisplayMode::Labels.to_value()),
        );
        window.add_breakpoint(breakpoint);

        ColorzWindow {
            window,
            toasts,
            stack,
            pick_slot,
            wheel_slot,
            palettes_slot,
            shown_on_purpose: Rc::new(Cell::new(false)),
        }
    }

    pub fn set_pick_page(&self, page: &impl IsA<gtk::Widget>) {
        self.pick_slot.set_child(Some(page));
    }

    pub fn set_wheel_page(&self, page: &impl IsA<gtk::Widget>) {
        self.wheel_slot.set_child(Some(page));
    }

    pub fn set_palettes_page(&self, page: &impl IsA<gtk::Widget>) {
        self.palettes_slot.set_child(Some(page));
    }

    /// Names of the view stack pages, in order.
    pub fn page_names(&self) -> impl Iterator<Item = &'static str> {
        PAGES.into_iter().map(|(name, ..)| name)
    }

    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    /// Shows the window, or raises it when it is already shown.
    pub fn present(&self) {
        self.shown_on_purpose.set(true);
        self.window.present();
    }

    /// Shows the hidden window only to get focus for a clipboard copy; it
    /// is hidden again afterwards unless [`ColorzWindow::present`] is called
    /// meanwhile.
    #[cfg_attr(
        windows,
        expect(dead_code, reason = "only the Linux clipboard fallback uses it")
    )]
    pub fn present_briefly(&self) {
        self.shown_on_purpose.set(false);
        self.window.present();
    }

    /// Whether the window was last shown with [`ColorzWindow::present`].
    #[cfg_attr(
        windows,
        expect(dead_code, reason = "only the Linux clipboard fallback uses it")
    )]
    pub fn shown_on_purpose(&self) -> bool {
        self.shown_on_purpose.get()
    }

    pub fn show_toast(&self, msg: &str) {
        self.toasts.add_toast(Self::toast(msg));
    }

    /// A toast with an Undo button that runs `undo` when clicked.
    pub fn toast_with_undo(&self, msg: &str, undo: impl Fn() + 'static) {
        let toast = Self::toast(msg);
        toast.set_button_label(Some("_Undo"));
        toast.set_priority(adw::ToastPriority::High);
        toast.connect_button_clicked(move |_| undo());
        self.toasts.add_toast(toast);
    }

    fn toast(msg: &str) -> adw::Toast {
        // Messages may contain palette names, so no markup.
        adw::Toast::builder().title(msg).use_markup(false).build()
    }

    pub fn set_page(&self, name: &str) {
        self.stack.set_visible_child_name(name);
    }

    pub fn stack(&self) -> adw::ViewStack {
        self.stack.clone()
    }
}
