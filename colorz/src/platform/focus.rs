//! Waiting for a window to become the active (focused) one.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

/// Runs `on_active` once `win` is the active window, or `on_timeout` if it
/// is not within `timeout`. Exactly one of the two runs.
pub fn when_active(
    win: &gtk::Window,
    timeout: Duration,
    on_active: impl FnOnce() + 'static,
    on_timeout: impl FnOnce() + 'static,
) {
    if win.is_active() {
        on_active();
        return;
    }
    type Pending = Option<(Box<dyn FnOnce()>, Box<dyn FnOnce()>)>;
    let pending: Rc<RefCell<Pending>> = Rc::new(RefCell::new(Some((
        Box::new(on_active),
        Box::new(on_timeout),
    ))));
    let handler = Rc::new(RefCell::new(None::<glib::SignalHandlerId>));
    let timer = Rc::new(RefCell::new(None::<glib::SourceId>));

    let id = win.connect_is_active_notify({
        let (pending, handler, timer) = (pending.clone(), handler.clone(), timer.clone());
        move |win| {
            if !win.is_active() {
                return;
            }
            let Some((on_active, _)) = pending.borrow_mut().take() else {
                return;
            };
            if let Some(id) = handler.borrow_mut().take() {
                win.disconnect(id);
            }
            if let Some(source) = timer.borrow_mut().take() {
                source.remove();
            }
            on_active();
        }
    });
    handler.replace(Some(id));

    let source = glib::timeout_add_local_once(timeout, {
        let (win, timer) = (win.clone(), timer.clone());
        move || {
            // This source is finishing; it must not be removed later.
            timer.borrow_mut().take();
            let Some((_, on_timeout)) = pending.borrow_mut().take() else {
                return;
            };
            if let Some(id) = handler.borrow_mut().take() {
                win.disconnect(id);
            }
            on_timeout();
        }
    });
    timer.replace(Some(source));
}
