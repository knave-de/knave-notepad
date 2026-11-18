use crate::store::LibraryFile;
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub enum Preview {
    Available {
        text: String,
        created: u64,
        modified: u64,
    },
    Missing,
    Unavailable,
}

#[derive(Default)]
pub struct Previews {
    entries: HashMap<String, (PathBuf, Preview)>,
    batch: Option<Receiver<(String, PathBuf, Preview)>>,
    deadline: Option<Instant>,
    rescan: bool,
}

impl Previews {
    pub fn start(&mut self, files: &[LibraryFile]) {
        if self.batch.is_some() {
            self.rescan = true;
            return;
        }
        self.entries.retain(|id, (path, _)| {
            files
                .iter()
                .any(|entry| &entry.id == id && &entry.path == path)
        });
        if files.is_empty() {
            return;
        }
        let files = files.to_vec();
        let (send, receive) = mpsc::channel();
        let result = std::thread::Builder::new()
            .name("notepad-library-previews".into())
            .spawn(move || {
                for file in files {
                    let preview = read_preview(&file.path, file.added);
                    if send.send((file.id, file.path, preview)).is_err() {
                        break;
                    }
                }
            });
        if let Err(error) = result {
            eprintln!("Could not start Library preview reader: {error}");
            return;
        }
        self.batch = Some(receive);
        self.deadline = Some(Instant::now() + Duration::from_millis(60));
    }

    pub fn get(&self, entry: &LibraryFile) -> Option<&Preview> {
        self.entries
            .get(&entry.id)
            .filter(|(path, _)| path == &entry.path)
            .map(|(_, preview)| preview)
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub fn poll(&mut self, now: Instant, files: &[LibraryFile]) -> bool {
        let Some(batch) = &self.batch else {
            return false;
        };
        let mut updated = false;
        loop {
            match batch.try_recv() {
                Ok((id, path, preview)) => {
                    if files
                        .iter()
                        .any(|entry| entry.id == id && entry.path == path)
                    {
                        self.entries.insert(id, (path, preview));
                        updated = true;
                    }
                }
                Err(TryRecvError::Empty) => {
                    self.deadline = Some(now + Duration::from_millis(150));
                    return updated;
                }
                Err(TryRecvError::Disconnected) => break,
            }
        }
        self.batch = None;
        self.deadline = None;
        if std::mem::take(&mut self.rescan) {
            self.start(files);
        }
        updated
    }
}

fn read_preview(path: &Path, added: u64) -> Preview {
    let result = (|| {
        if !std::fs::metadata(path)?.is_file() {
            return Err(io::Error::other("Expected a regular file"));
        }
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::other("Expected a regular file"));
        }
        let timestamp = |value: io::Result<SystemTime>| {
            value
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(added, |duration| duration.as_millis() as u64)
        };
        const LIMIT: usize = 64 * 1024;
        let mut bytes = Vec::new();
        (&mut file).take(LIMIT as u64).read_to_end(&mut bytes)?;
        let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
        let text = match std::str::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) if bytes.len() >= LIMIT - 3 && error.error_len().is_none() => {
                std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap()
            }
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Expected UTF-8 text",
                ));
            }
        };
        if text.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "File contains NUL bytes",
            ));
        }
        Ok(Preview::Available {
            text: text.into(),
            created: timestamp(metadata.created()),
            modified: timestamp(metadata.modified()),
        })
    })();
    match result {
        Ok(preview) => preview,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Preview::Missing,
        Err(_) => Preview::Unavailable,
    }
}
