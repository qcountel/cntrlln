//! The Controllin web installer: a small window that downloads the latest release, installs it
//! for the current user and adds the chosen shortcuts.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod install;
#[cfg(windows)]
mod shortcuts;
#[cfg(windows)]
mod window;

#[cfg(windows)]
fn main() {
    window::run();
}

#[cfg(not(windows))]
fn main() {
    std::process::exit(1);
}
