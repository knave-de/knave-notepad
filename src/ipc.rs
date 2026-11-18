use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{DirBuilderExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

const MAX_REQUEST: u64 = 256 * 1024;

#[derive(Serialize, Deserialize)]
struct OpenFiles(Vec<Vec<u8>>);

pub fn socket_path(data: &Path) -> PathBuf {
    data.join("ipc").join("notepad.sock")
}

pub fn forward(socket: &Path, paths: &[PathBuf]) -> io::Result<()> {
    let request = OpenFiles(
        paths
            .iter()
            .map(|path| path.as_os_str().as_bytes().to_vec())
            .collect(),
    );
    let request = serde_json::to_vec(&request)?;
    if request.len() as u64 >= MAX_REQUEST {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Too many file arguments",
        ));
    }
    let until = Instant::now() + Duration::from_secs(3);
    let mut stream = loop {
        match UnixStream::connect(socket) {
            Ok(stream) => break stream,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) && Instant::now() < until =>
            {
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(error) => return Err(error),
        }
    };
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(&request)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut response = [0];
    stream.read_exact(&mut response)?;
    if response == [1] {
        Ok(())
    } else {
        Err(io::Error::other("Notepad did not accept the open request"))
    }
}

pub struct Server {
    listener: UnixListener,
    path: PathBuf,
}

impl Server {
    /// Called only by the process holding the data-store lock.
    pub fn bind(path: PathBuf) -> io::Result<Self> {
        let directory = path.parent().expect("IPC has private parent");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(&path)?;
        Ok(Self { listener, path })
    }

    pub fn receive(
        &self,
        wake: impl Fn() -> bool + Send + 'static,
    ) -> io::Result<Receiver<Vec<PathBuf>>> {
        let listener = self.listener.try_clone()?;
        let (send, receive) = mpsc::sync_channel(32);
        std::thread::Builder::new()
            .name("notepad-open-files".into())
            .spawn(move || {
                for connection in listener.incoming() {
                    let Ok(mut stream) = connection else { continue };
                    if stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .is_err()
                        || stream
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .is_err()
                    {
                        continue;
                    }
                    let mut bytes = Vec::new();
                    let result = (&mut stream).take(MAX_REQUEST).read_to_end(&mut bytes);
                    if result.is_err() || bytes.len() as u64 >= MAX_REQUEST {
                        continue;
                    }
                    let Ok(OpenFiles(paths)) = serde_json::from_slice(&bytes) else {
                        continue;
                    };
                    let paths: Vec<_> = paths
                        .into_iter()
                        .map(|path| PathBuf::from(std::ffi::OsString::from_vec(path)))
                        .collect();
                    let accepted =
                        paths.iter().all(|path| path.is_absolute()) && send.try_send(paths).is_ok();
                    let _ = stream.write_all(&[u8::from(accepted)]);
                    if accepted && !wake() {
                        break;
                    }
                }
            })?;
        Ok(receive)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
