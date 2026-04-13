mod app;
mod document;
mod palette;
mod selection;
mod settings;
mod state;
mod tool_cursors;
mod tools;
mod updater;

fn main() -> gtk::glib::ExitCode {
    app::run()
}
