mod app;
mod editor;
mod file_document;
mod host;
mod ipc;
mod library_preview;
mod paint;
mod recovery;
mod store;
mod vim;

pub use host::AppResult;
fn main() -> AppResult {
    let dirs = directories::BaseDirs::new().ok_or("Could not find the user's home directory")?;
    let data = dirs.data_dir().join("knave-notepad");
    let config = dirs.config_dir().join("knave-notepad");
    let socket = ipc::socket_path(&data);
    let mut arguments = std::env::args_os().skip(1).peekable();
    if arguments.peek().is_some_and(|argument| argument == "--") {
        arguments.next();
    }
    let paths = arguments
        .map(std::path::absolute)
        .collect::<std::io::Result<Vec<_>>>()?;
    let storage = match store::Storage::open(data, config) {
        Ok(storage) => storage,
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            return Ok(ipc::forward(&socket, &paths)?);
        }
        Err(error) => return Err(error.into()),
    };
    let server = ipc::Server::bind(socket)?;
    let mut notepad = app::Notes::new(storage)?;
    for path in paths {
        notepad.open_cli_file(&path)?;
    }
    let state = dirs
        .state_dir()
        .unwrap_or(dirs.data_dir())
        .join("knave-notepad");
    notepad.set_recovery(recovery::Recovery::new(&state)?)?;
    host::run("Notes", [1000.0, 720.0], notepad, server)
}
