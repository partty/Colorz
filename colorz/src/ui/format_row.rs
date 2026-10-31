//! One row of the Pick tab: a format's value and its copy controls.

use adw::prelude::*;
use colorz_core::color::Color;
use colorz_core::format::Format;

use super::copy_pair::CopyPair;
use crate::state::AppState;

#[derive(Clone)]
pub struct FormatRow {
    row: adw::ActionRow,
    format: Format,
    default_pill: gtk::Label,
    copy: CopyPair,
}

impl FormatRow {
    pub fn new(format: Format) -> FormatRow {
        let row = adw::ActionRow::builder()
            .title(format.label())
            .use_markup(false)
            .subtitle_selectable(true)
            .css_classes(["mono-subtitle"])
            .build();
        let default_pill = gtk::Label::builder()
            .label("Default")
            .css_classes(["default-pill"])
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        let copy = CopyPair::new();
        row.add_suffix(&default_pill);
        for w in copy.widgets() {
            row.add_suffix(&w);
        }
        FormatRow {
            row,
            format,
            default_pill,
            copy,
        }
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn widget(&self) -> &adw::ActionRow {
        &self.row
    }

    /// Called with the text to put on the clipboard when a copy button is
    /// clicked.
    pub fn connect_copy(&self, f: impl Fn(&str) + 'static) {
        self.copy.connect_copy(f);
    }

    /// Shows `color`. Returns whether the format can show it at all; the
    /// caller hides the row when not.
    pub fn update(&self, state: &AppState, color: &Color) -> bool {
        let Some(full) = self.copy.update(state, color, self.format) else {
            return false;
        };
        self.row.set_subtitle(&full);
        let is_default = state.config().default_format == self.format;
        self.default_pill.set_visible(is_default);
        if is_default {
            self.row.add_css_class("default-format");
        } else {
            self.row.remove_css_class("default-format");
        }
        true
    }
}
