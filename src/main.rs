mod app;
mod document;
mod settings;
mod state;
mod tools;
mod updater;

fn main() -> gtk::glib::ExitCode {
    app::run()
}
