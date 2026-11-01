#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod platform;
mod state;
mod ui;

pub const APP_ID: &str = "io.github.partty.Colorz";

fn main() -> gtk::glib::ExitCode {
    app::run()
}
