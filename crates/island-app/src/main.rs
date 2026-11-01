#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod platform;
mod render;
mod sources;

fn main() -> windows::core::Result<()> {
    app::run()
}
