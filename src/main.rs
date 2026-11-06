mod account;
mod api;
mod audio;
mod download;
mod graphics;
mod mpris;
mod storage;
mod tray;
mod ui;

use gtk::{gio, glib, prelude::*};
fn main() -> glib::ExitCode {
    graphics::configure_startup();
    glib::set_application_name("Buisc");
    if let Err(e) = gstreamer::init() {
        eprintln!("无法初始化音频组件：{e}");
        return glib::ExitCode::FAILURE;
    }
    let app = gtk::Application::builder()
        .application_id("io.github.bilimusic.Linux")
        .flags(gio::ApplicationFlags::empty())
        .build();
    app.connect_activate(|app| {
        if present_existing_window(app) {
            return;
        }
        match ui::Ui::new(app) {
            Ok(ui) => ui.start(),
            Err(error) => {
                eprintln!("{error}");
                app.quit();
            }
        }
    });
    app.run()
}

fn present_existing_window(app: &gtk::Application) -> bool {
    if let Some(window) = app
        .windows()
        .into_iter()
        .find(|window| window.is::<gtk::ApplicationWindow>())
    {
        window.present();
        true
    } else {
        false
    }
}
