use crate::{file_document::FileDocument, store};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub file: FileDocument,
    pub text: String,
    pub updated: u64,
}

#[derive(Serialize, Deserialize)]
struct Session {
    clean: bool,
}

pub struct Recovery {
    state: PathBuf,
    directory: PathBuf,
}

impl Recovery {
    pub fn new(state: &Path) -> io::Result<Self> {
        let directory = state.join("recovery");
        fs::create_dir_all(&directory)?;
        File::open(state)?.sync_all()?;
        Ok(Self {
            state: state.to_owned(),
            directory,
        })
    }

    pub fn start_session(&self) -> io::Result<bool> {
        let path = self.state.join("session.json");
        let was_unclean = match fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice::<Session>(&bytes).is_ok_and(|session| !session.clean)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        store::atomic_json(&path, &Session { clean: false })?;
        Ok(was_unclean)
    }

    pub fn end_session(&self) -> io::Result<()> {
        store::atomic_json(&self.state.join("session.json"), &Session { clean: true })
    }

    fn path(&self, id: &str) -> PathBuf {
        debug_assert!(uuid::Uuid::parse_str(id).is_ok());
        self.directory.join(format!("{id}.json"))
    }

    pub fn pending(&self) -> io::Result<Vec<(String, Snapshot)>> {
        let mut snapshots = Vec::new();
        for entry in fs::read_dir(&self.directory)? {
            let path = entry?.path();
            let Some(id) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            if path.extension().and_then(|value| value.to_str()) != Some("json")
                || uuid::Uuid::parse_str(id).is_err()
            {
                continue;
            }
            match fs::read(&path).and_then(|bytes| {
                serde_json::from_slice::<Snapshot>(&bytes).map_err(io::Error::from)
            }) {
                Ok(snapshot) => snapshots.push((id.to_owned(), snapshot)),
                Err(error) => eprintln!(
                    "Could not read recovery snapshot {}: {error}",
                    path.display()
                ),
            }
        }
        snapshots.sort_by_key(|(_, snapshot)| std::cmp::Reverse(snapshot.updated));
        Ok(snapshots)
    }

    pub fn checkpoint(&self, id: &str, file: &FileDocument, text: &str) -> io::Result<()> {
        #[derive(Serialize)]
        struct BorrowedSnapshot<'a> {
            file: &'a FileDocument,
            text: &'a str,
            updated: u64,
        }
        store::atomic_json(
            &self.path(id),
            &BorrowedSnapshot {
                file,
                text,
                updated: store::now(),
            },
        )
    }

    pub fn clear(&self, id: &str) -> io::Result<()> {
        match fs::remove_file(self.path(id)) {
            Ok(()) => File::open(&self.directory)?.sync_all()?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }
}
