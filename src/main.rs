mod app;
mod document;
mod state;
mod tools;

fn main() -> gtk::glib::ExitCode {
    app::run()
}
