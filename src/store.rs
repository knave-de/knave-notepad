use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub text: String,
    pub updated: u64,
    #[serde(default)]
    pub name: Option<String>,
}
impl Note {
    pub fn new() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            text: String::new(),
            updated: now(),
            name: None,
        }
    }
    pub fn title(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        self.text
            .lines()
            .find(|s| !s.trim().is_empty())
            .unwrap_or("Untitled note")
            .trim()
            .chars()
            .take(60)
            .collect()
    }
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
#[derive(Default, Serialize, Deserialize)]
pub struct Notebook {
    pub notes: Vec<Note>,
    pub tabs: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub vim: bool,
    pub font_size: u16,
    pub line_numbers: bool,
    pub relative_numbers: bool,
    pub spaces: bool,
    pub tab_width: u16,
    pub auto_indent: bool,
    pub highlight_line: bool,
    pub show_whitespace: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            vim: false,
            font_size: 16,
            line_numbers: true,
            relative_numbers: false,
            spaces: true,
            tab_width: 4,
            auto_indent: true,
            highlight_line: true,
            show_whitespace: false,
        }
    }
}

pub struct Storage {
    data: PathBuf,
    config: PathBuf,
    _lock: File,
}
impl Storage {
    pub fn open(data: PathBuf, config: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&data)?;
        fs::create_dir_all(&config)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(data.join("notes.lock"))?;
        lock.try_lock_exclusive()
            .map_err(|_| io::Error::other("Notes is already running for this data directory"))?;
        Ok(Self {
            data,
            config,
            _lock: lock,
        })
    }
    pub fn load(&self) -> io::Result<(Notebook, Settings)> {
        let mut book: Notebook = read_or_default(&self.data.join("notebook.json"))?;
        let mut settings: Settings = read_or_default(&self.config.join("settings.json"))?;
        settings.font_size = settings.font_size.clamp(10, 40);
        settings.tab_width = settings.tab_width.clamp(1, 16);
        let mut ids = std::collections::HashSet::new();
        if book.notes.iter().any(|n| !ids.insert(n.id.clone())) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Duplicate note IDs in notebook",
            ));
        }
        let mut tabs = std::collections::HashSet::new();
        book.tabs
            .retain(|id| ids.contains(id) && tabs.insert(id.clone()));
        Ok((book, settings))
    }
    pub fn save(&self, book: &Notebook, settings: &Settings) -> io::Result<()> {
        atomic_json(&self.data.join("notebook.json"), book)?;
        atomic_json(&self.config.join("settings.json"), settings)
    }
}
fn read_or_default<T: serde::de::DeserializeOwned + Default>(path: &Path) -> io::Result<T> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {e}", path.display()),
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e),
    }
}
pub fn export_text(path: &Path, text: &str) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let temporary = parent.join(format!(".notes-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        if let Ok(metadata) = fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
fn atomic_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let temporary = path.with_extension("json.tmp");
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(path.parent().expect("storage has parent"))?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saves_notes_settings_and_tabs_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let config = dir.path().join("config");
        let store = Storage::open(data.clone(), config.clone()).unwrap();
        let mut note = Note::new();
        note.text = "नोट\nRemember this".into();
        let book = Notebook {
            tabs: vec![note.id.clone()],
            notes: vec![note],
        };
        store
            .save(
                &book,
                &Settings {
                    vim: true,
                    font_size: 22,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(Storage::open(data.clone(), config.clone()).is_err());
        drop(store);
        let (loaded, settings) = Storage::open(data, config).unwrap().load().unwrap();
        assert_eq!(loaded.notes[0].text, book.notes[0].text);
        assert_eq!(loaded.tabs, book.tabs);
        assert!(settings.vim);
        assert_eq!(settings.font_size, 22);
    }
    #[test]
    fn corrupt_data_is_reported_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path().join("data"), dir.path().join("config")).unwrap();
        let path = storage.data.join("notebook.json");
        fs::write(&path, b"broken").unwrap();
        assert!(storage.load().is_err());
        assert_eq!(fs::read(path).unwrap(), b"broken");
    }
}
