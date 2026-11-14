mod app;
mod editor;
mod host;
mod paint;
mod store;
mod vim;

pub use host::AppResult;
fn main() -> AppResult {
    let dirs = directories::BaseDirs::new().ok_or("Could not find the user's home directory")?;
    let storage = store::Storage::open(
        dirs.data_dir().join("knave-notepad"),
        dirs.config_dir().join("knave-notepad"),
    )?;
    host::run("Notes", [1000.0, 720.0], app::Notes::new(storage)?)
}
