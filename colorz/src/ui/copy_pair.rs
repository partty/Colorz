//! The copy controls shown wherever a color is copied in a format: the
//! "Copy as" target badge, the out-of-gamut warning, and the full and
//! numbers-only copy buttons.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use colorz_core::color::Color;
use colorz_core::format::{Form, Format};

use crate::state::AppState;

/// Formats whose text depends on the "copy as" target.
fn uses_target(format: Format) -> bool {
    matches!(
        format,
        Format::Hex | Format::Rgb | Format::Hsl | Format::Hsv
    )
}

/// Whether the target badge shows: the target is not sRGB and the value
/// (or, with `labels_hex`, a HEX value shown beside it) is in the target.
fn badge_visible(format: Format, labels_hex: bool, target_is_srgb: bool) -> bool {
    !target_is_srgb && (labels_hex || uses_target(format))
}

type CopyHandler = Rc<dyn Fn(&str)>;

#[derive(Clone)]
pub struct CopyPair {
    badge: gtk::Label,
    warning: gtk::Image,
    full: gtk::Button,
    numbers: gtk::Button,
    /// (full, numbers only) as last shown.
    texts: Rc<RefCell<(String, String)>>,
    on_copy: Rc<RefCell<Option<CopyHandler>>>,
    labels_hex: Rc<Cell<bool>>,
}

impl CopyPair {
    pub fn new() -> CopyPair {
        let badge = gtk::Label::builder()
            .css_classes(["badge"])
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        let warning = gtk::Image::builder()
            .icon_name("dialog-warning-symbolic")
            .css_classes(["warning"])
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        let full = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let numbers = gtk::Button::builder()
            .label("123")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "numbers-button"])
            .build();
        let this = CopyPair {
            badge,
            warning,
            full,
            numbers,
            texts: Rc::default(),
            on_copy: Rc::default(),
            labels_hex: Rc::default(),
        };
        this.set_texts(None, None);
        for (button, full) in [(&this.full, true), (&this.numbers, false)] {
            let (texts, on_copy) = (this.texts.clone(), this.on_copy.clone());
            button.connect_clicked(move |_| {
                let text = {
                    let t = texts.borrow();
                    if full { t.0.clone() } else { t.1.clone() }
                };
                let handler = on_copy.borrow().clone();
                if let (Some(f), false) = (handler, text.is_empty()) {
                    f(&text);
                }
            });
        }
        this
    }

    /// The badge also covers a HEX value shown beside the buttons, which is
    /// always in the target (the Wheel's hex entry).
    pub fn labels_hex(self) -> CopyPair {
        self.labels_hex.set(true);
        self
    }

    /// Badge, warning, copy full, copy numbers only, in display order.
    pub fn widgets(&self) -> [gtk::Widget; 4] {
        [
            self.badge.clone().upcast(),
            self.warning.clone().upcast(),
            self.full.clone().upcast(),
            self.numbers.clone().upcast(),
        ]
    }

    /// Called with the text to put on the clipboard when a button is
    /// clicked.
    pub fn connect_copy(&self, f: impl Fn(&str) + 'static) {
        *self.on_copy.borrow_mut() = Some(Rc::new(f));
    }

    fn set_texts(&self, full: Option<&str>, numbers: Option<&str>) {
        let label = |what: &str, text: Option<&str>| match text {
            Some(t) => format!("{what} {t}"),
            None => what.to_owned(),
        };
        let full_label = label("Copy", full);
        let numbers_label = label("Copy numbers only", numbers);
        for (button, text, label) in [
            (&self.full, full, &full_label),
            (&self.numbers, numbers, &numbers_label),
        ] {
            button.set_sensitive(text.is_some());
            button.set_tooltip_text(Some(label));
            button.update_property(&[gtk::accessible::Property::Label(label)]);
        }
        *self.texts.borrow_mut() = (
            full.unwrap_or_default().to_owned(),
            numbers.unwrap_or_default().to_owned(),
        );
    }

    /// Shows `color` in `format`. Returns the full text, or `None` (with
    /// the buttons disabled) when the format cannot show it.
    pub fn update(&self, state: &AppState, color: &Color, format: Format) -> Option<String> {
        let full = state.format(color, format, Form::Full);
        let numbers = state.format(color, format, Form::Numbers);
        let (Some(full), Some(numbers)) = (full, numbers) else {
            self.set_texts(None, None);
            self.badge.set_visible(false);
            self.warning.set_visible(false);
            return None;
        };
        self.set_texts(Some(&full.text), Some(&numbers.text));

        let labels_hex = self.labels_hex.get();
        let (name, is_srgb) = state.with_target(|t| (t.name(), t.is_srgb()));
        self.badge.set_label(&name);
        let badge = format!("Copy as {name}: values are not sRGB");
        self.badge.set_tooltip_text(Some(&badge));
        self.badge
            .update_property(&[gtk::accessible::Property::Label(&badge)]);
        self.badge
            .set_visible(badge_visible(format, labels_hex, is_srgb));

        // The color() value falls back to Display P3 for an sRGB target.
        let hex_out = labels_hex
            && state
                .format(color, Format::Hex, Form::Full)
                .is_some_and(|h| h.out_of_gamut);
        let outside = if full.out_of_gamut && format == Format::CssColor && is_srgb {
            Some("Display P3".to_owned())
        } else if full.out_of_gamut || hex_out {
            Some(name)
        } else {
            None
        };
        self.warning.set_visible(outside.is_some());
        if let Some(space) = outside {
            let warning = format!("Outside {space} gamut \u{2014} value was mapped");
            self.warning.set_tooltip_text(Some(&warning));
            self.warning
                .update_property(&[gtk::accessible::Property::Label(&warning)]);
        }
        Some(full.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_only_for_target_values() {
        for f in [Format::Hex, Format::Rgb, Format::Hsl, Format::Hsv] {
            assert!(badge_visible(f, false, false), "{f:?}");
            assert!(!badge_visible(f, false, true), "{f:?}");
        }
        for f in [Format::Oklch, Format::Oklab, Format::CssColor] {
            assert!(!badge_visible(f, false, false), "{f:?}");
            // Beside the Wheel's hex entry it labels the hex value.
            assert!(badge_visible(f, true, false), "{f:?}");
            assert!(!badge_visible(f, true, true), "{f:?}");
        }
    }
}
