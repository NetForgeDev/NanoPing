#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod ping;
mod state;
mod ui;
mod updater;

fn main() {
    ui::win_util::init_process();
    ui::main_win::run();
}