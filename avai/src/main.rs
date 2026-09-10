mod app;

fn main() {
    base::daemon::run::<app::App, _>();
}
