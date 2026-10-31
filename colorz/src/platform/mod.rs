//! Desktop integration (portals, tray, clipboard, notifications, autostart).
//! Kept separate from `ui` so a port only has to replace this module.

pub mod autostart;
pub mod clipboard;
pub mod notify;
pub mod pick;
pub mod shortcuts;
pub mod tray;

use std::cell::OnceCell;
use std::future::Future;
use std::sync::OnceLock;

use futures_util::FutureExt;
use futures_util::future::{LocalBoxFuture, Shared};

use crate::APP_ID;

/// Prints `msg` to stderr when the environment has `COLORZ_DEBUG=1`.
pub fn debug(msg: &str) {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("COLORZ_DEBUG").is_some_and(|v| v == "1")) {
        eprintln!("colorz: {msg}");
    }
}

thread_local! {
    static HOST_REGISTRATION: OnceCell<Shared<LocalBoxFuture<'static, ()>>> =
        const { OnceCell::new() };
}

/// Resolves once Colorz has told the portals its app ID. Every portal call
/// must await this first: the portal ties a connection to the app ID of
/// its first call, so registering later has no effect. Registration
/// failing (e.g. no installed `.desktop` file) is logged and does not
/// block portal use.
pub fn portals_ready() -> impl Future<Output = ()> {
    HOST_REGISTRATION.with(|r| {
        r.get_or_init(|| register_host_app().boxed_local().shared())
            .clone()
    })
}

async fn register_host_app() {
    let registered = match ashpd::AppID::try_from(APP_ID) {
        Ok(app_id) => ashpd::register_host_app(app_id).await,
        Err(e) => Err(e),
    };
    match registered {
        Ok(()) => debug("registered with the portals"),
        Err(e) => debug(&format!("portal host registration failed: {e}")),
    }
}
