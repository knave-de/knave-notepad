use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub text: String,
    pub updated: u64,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_empty: bool,
    #[serde(skip)]
    pub indexed_title: Option<String>,
}
impl Note {
    pub fn new() -> Self {
        let timestamp = now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            text: String::new(),
            updated: timestamp,
            created: timestamp,
            name: None,
            keep_empty: false,
            indexed_title: None,
        }
    }
    pub fn retained(&self) -> bool {
        self.keep_empty || self.indexed_title.is_some() || !self.text.is_empty()
    }

    pub fn title(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        if let Some(title) = &self.indexed_title {
            return title.clone();
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
    #[serde(skip)]
    pub files: Vec<LibraryFile>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
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
    pub word_wrap: bool,
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
            word_wrap: false,
        }
    }
}

#[derive(PartialEq, Serialize, Deserialize)]
struct LibraryIndex {
    notes: Vec<NoteSummary>,
    tabs: Vec<String>,
    #[serde(default)]
    files: Vec<LibraryFile>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
struct NoteSummary {
    id: String,
    title: String,
    created: u64,
    updated: u64,
    preview: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    keep_empty: bool,
    #[serde(default)]
    file: Option<NoteFile>,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
struct NoteFile {
    inode: u64,
    bytes: u64,
    modified: (i64, i64),
}

impl NoteFile {
    fn read(path: &Path) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() {
            return Err(io::Error::other("Expected a regular note file"));
        }
        Ok(Self {
            inode: metadata.ino(),
            bytes: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
        })
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryFile {
    pub id: String,
    #[serde(with = "unix_path")]
    pub path: PathBuf,
    #[serde(default)]
    pub canonical: Option<String>,
    pub added: u64,
}

impl LibraryFile {
    pub fn title(&self) -> String {
        self.path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy()
            .into_owned()
    }
}

pub struct Storage {
    data: PathBuf,
    config: PathBuf,
    _lock: File,
    persisted: RefCell<HashMap<String, NoteSummary>>,
    loaded: RefCell<HashMap<String, Note>>,
    last_index: RefCell<Option<LibraryIndex>>,
    last_settings: RefCell<Option<Settings>>,
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
        lock.try_lock_exclusive()?;
        fs::create_dir_all(data.join("notes"))?;
        Ok(Self {
            data,
            config,
            _lock: lock,
            persisted: RefCell::new(HashMap::new()),
            loaded: RefCell::new(HashMap::new()),
            last_index: RefCell::new(None),
            last_settings: RefCell::new(None),
        })
    }
    pub fn load_library(&self) -> io::Result<(Notebook, Settings)> {
        if self.data.join("notebook.json").try_exists()? {
            return self.load();
        }
        let Some(index) = read_or_default::<Option<LibraryIndex>>(&self.data.join("library.json"))
            .unwrap_or(None)
        else {
            return self.load();
        };
        let mut summaries: HashMap<_, _> = index
            .notes
            .iter()
            .map(|summary| (summary.id.clone(), summary.clone()))
            .collect();
        if summaries.len() != index.notes.len() {
            return self.load();
        }
        let notes_dir = self.data.join("notes");
        let paths = fs::read_dir(&notes_dir)?;
        let mut found = HashSet::new();
        for entry in paths {
            let path = entry?.path();
            let Some(id) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            if path.extension().and_then(|extension| extension.to_str()) != Some("json")
                || uuid::Uuid::parse_str(id).is_err()
            {
                continue;
            }
            let indexed = summaries.get(id).and_then(|summary| summary.file);
            if indexed.is_none() || indexed != NoteFile::read(&path).ok() {
                return self.load();
            }
            found.insert(id.to_owned());
        }
        if found.len() != summaries.len() {
            return self.load();
        }
        let notes = index
            .notes
            .iter()
            .map(|summary| Note {
                id: summary.id.clone(),
                text: summary.preview.clone(),
                updated: summary.updated,
                created: summary.created,
                name: summary.name.clone(),
                keep_empty: summary.keep_empty,
                indexed_title: Some(summary.title.clone()),
            })
            .collect();
        let mut tabs = HashSet::new();
        let tab_order = index
            .tabs
            .iter()
            .filter(|id| summaries.contains_key(*id) && tabs.insert((*id).clone()))
            .cloned()
            .collect();
        let book = Notebook {
            notes,
            tabs: tab_order,
            files: index.files.clone(),
        };
        *self.persisted.borrow_mut() = std::mem::take(&mut summaries);
        self.loaded.borrow_mut().clear();
        *self.last_index.borrow_mut() = Some(index);
        Ok((book, self.load_settings()?))
    }

    pub fn materialize(&self, note: &mut Note) -> io::Result<()> {
        if note.indexed_title.is_some() {
            let path = note_path(&self.data.join("notes"), &note.id)?;
            let bytes = fs::read(path)?;
            let loaded: Note = serde_json::from_slice(&bytes)?;
            if loaded.id != note.id {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Note ID does not match its filename",
                ));
            }
            self.loaded
                .borrow_mut()
                .insert(note.id.clone(), loaded.clone());
            *note = loaded;
        }
        Ok(())
    }

    fn load_settings(&self) -> io::Result<Settings> {
        let loaded: Settings = read_or_default(&self.config.join("settings.json"))?;
        *self.last_settings.borrow_mut() = Some(loaded.clone());
        let mut settings = loaded;
        settings.font_size = settings.font_size.clamp(10, 40);
        settings.tab_width = settings.tab_width.clamp(1, 16);
        Ok(settings)
    }

    pub fn load(&self) -> io::Result<(Notebook, Settings)> {
        let legacy_path = self.data.join("notebook.json");
        let mut book: Notebook = read_or_default(&legacy_path)?;
        let mut indices = HashMap::new();
        for (index, note) in book.notes.iter().enumerate() {
            if indices.insert(note.id.clone(), index).is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Duplicate note IDs in notebook",
                ));
            }
        }
        let mut persisted = self.persisted.borrow_mut();
        let mut loaded = self.loaded.borrow_mut();
        persisted.clear();
        loaded.clear();
        *self.last_index.borrow_mut() = None;
        for entry in fs::read_dir(self.data.join("notes"))? {
            let path = entry?.path();
            let Some(id) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            if path.extension().and_then(|extension| extension.to_str()) != Some("json")
                || uuid::Uuid::parse_str(id).is_err()
            {
                continue;
            }
            let note = fs::read(&path)
                .and_then(|bytes| serde_json::from_slice::<Note>(&bytes).map_err(io::Error::from));
            let note = match note {
                Ok(note) if note.id == id => note,
                Ok(_) => {
                    eprintln!(
                        "Skipping {} because its note ID does not match its filename",
                        path.display()
                    );
                    continue;
                }
                Err(error) => {
                    eprintln!("Skipping {}: {error}", path.display());
                    continue;
                }
            };
            persisted.insert(
                note.id.clone(),
                note_summary(&note, NoteFile::read(&path).ok()),
            );
            loaded.insert(note.id.clone(), note.clone());
            if let Some(&index) = indices.get(id) {
                book.notes[index] = note;
            } else {
                book.notes.push(note);
            }
        }
        if !legacy_path.try_exists()? {
            book.notes
                .sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        }
        if let Ok(Some(index)) =
            read_or_default::<Option<LibraryIndex>>(&self.data.join("library.json"))
        {
            book.tabs = index.tabs;
            book.files = index.files;
        }
        let settings = self.load_settings()?;
        book.notes.retain(Note::retained);
        for note in &mut book.notes {
            if note.name.as_deref().is_some_and(|name| {
                name.strip_prefix("Untitled Document ")
                    .is_some_and(|number| number.parse::<usize>().is_ok())
            }) {
                note.name = None;
            }
        }
        let ids: std::collections::HashSet<_> =
            book.notes.iter().map(|note| note.id.clone()).collect();
        let mut tabs = std::collections::HashSet::new();
        book.tabs
            .retain(|id| ids.contains(id) && tabs.insert(id.clone()));
        Ok((book, settings))
    }
    pub fn save(&self, book: &Notebook, settings: &Settings) -> io::Result<()> {
        let mut persisted = self.persisted.borrow_mut();
        let mut loaded = self.loaded.borrow_mut();
        let mut live = HashSet::new();
        let mut summaries = Vec::new();
        let notes_dir = self.data.join("notes");
        for note in book.notes.iter().filter(|note| note.retained()) {
            let path = note_path(&notes_dir, &note.id)?;
            if note.indexed_title.is_none() && loaded.get(&note.id) != Some(note) {
                atomic_json(&path, note)?;
                loaded.insert(note.id.clone(), note.clone());
                persisted.insert(
                    note.id.clone(),
                    note_summary(note, Some(NoteFile::read(&path)?)),
                );
            }
            let summary = persisted
                .get(&note.id)
                .ok_or_else(|| io::Error::other("Missing note index entry"))?;
            live.insert(note.id.as_str());
            summaries.push(summary.clone());
        }
        let obsolete: Vec<_> = persisted
            .keys()
            .filter(|id| !live.contains(id.as_str()))
            .cloned()
            .collect();
        for id in obsolete {
            let path = note_path(&notes_dir, &id)?;
            match fs::remove_file(path) {
                Ok(()) => File::open(&notes_dir)?.sync_all()?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            persisted.remove(&id);
            loaded.remove(&id);
        }
        let tabs = book
            .tabs
            .iter()
            .filter(|id| live.contains(id.as_str()))
            .cloned()
            .collect();
        let index = LibraryIndex {
            notes: summaries,
            tabs,
            files: book.files.clone(),
        };
        let mut last_index = self.last_index.borrow_mut();
        if last_index.as_ref() != Some(&index) {
            atomic_json(&self.data.join("library.json"), &index)?;
            *last_index = Some(index);
        }
        let mut last_settings = self.last_settings.borrow_mut();
        let settings_path = self.config.join("settings.json");
        if last_settings.as_ref() != Some(settings) || !settings_path.try_exists()? {
            atomic_json(&settings_path, settings)?;
            *last_settings = Some(settings.clone());
        }
        let legacy = self.data.join("notebook.json");
        if legacy.try_exists()? {
            fs::rename(legacy, self.data.join("notebook.json.migrated"))?;
            File::open(&self.data)?.sync_all()?;
        }
        Ok(())
    }
}
fn note_summary(note: &Note, file: Option<NoteFile>) -> NoteSummary {
    NoteSummary {
        id: note.id.clone(),
        title: note.title(),
        created: note.created,
        updated: note.updated,
        preview: note.text.chars().take(512).collect(),
        name: note.name.clone(),
        keep_empty: note.keep_empty,
        file,
    }
}

fn note_path(directory: &Path, id: &str) -> io::Result<PathBuf> {
    uuid::Uuid::parse_str(id)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid note ID"))?;
    Ok(directory.join(format!("{id}.json")))
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
pub(crate) fn atomic_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let temporary = path.with_extension("json.tmp");
    let result = (|| {
        match fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
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

pub(crate) mod unix_path {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::{
        ffi::OsString,
        os::unix::ffi::{OsStrExt, OsStringExt},
        path::{Path, PathBuf},
    };

    pub fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(text) = path.to_str() {
            text.serialize(serializer)
        } else {
            path.as_os_str().as_bytes().serialize(serializer)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StoredPath {
            Text(String),
            Bytes(Vec<u8>),
        }
        Ok(match StoredPath::deserialize(deserializer)? {
            StoredPath::Text(text) => text.into(),
            StoredPath::Bytes(bytes) => OsString::from_vec(bytes).into(),
        })
    }
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
            files: vec![],
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
    fn notes_are_independently_written_and_a_stale_index_loads_authoritative_records() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let config = dir.path().join("config");
        let store = Storage::open(data.clone(), config).unwrap();
        let mut first = Note::new();
        first.text = "full note ".repeat(200);
        let mut second = Note::new();
        second.text = "intact".into();
        let first_path = note_path(&data.join("notes"), &first.id).unwrap();
        let second_path = note_path(&data.join("notes"), &second.id).unwrap();
        let mut book = Notebook {
            notes: vec![first, second],
            ..Default::default()
        };
        store.save(&book, &Settings::default()).unwrap();
        let second_inode = fs::metadata(&second_path).unwrap().ino();
        let (mut indexed, _) = store.load_library().unwrap();
        assert_eq!(indexed.notes[0].text.len(), 512);
        store.materialize(&mut indexed.notes[0]).unwrap();
        assert_eq!(indexed.notes[0].text, book.notes[0].text);

        indexed.notes[0].text.push_str(" updated");
        store.save(&indexed, &Settings::default()).unwrap();
        assert_eq!(fs::metadata(&second_path).unwrap().ino(), second_inode);

        book.notes[0].text = "note record newer than index".into();
        atomic_json(&first_path, &book.notes[0]).unwrap();
        let (rebuilt, _) = store.load_library().unwrap();
        assert!(
            rebuilt
                .notes
                .iter()
                .any(|note| note.text == "note record newer than index")
        );
        assert!(rebuilt.notes.iter().any(|note| note.text == "intact"));
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

#[cfg(test)]
mod empty_note_tests {
    use super::*;
    #[test]
    fn loading_removes_legacy_empty_notes_and_generated_names() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let storage = Storage::open(data.clone(), dir.path().join("config")).unwrap();
        let empty = Note::new();
        let mut spaces = Note::new();
        spaces.text = "  ".into();
        spaces.name = Some("Untitled Document 4".into());
        let mut named = Note::new();
        named.text = "content".into();
        named.name = Some("My name".into());
        atomic_json(
            &data.join("notebook.json"),
            &Notebook {
                tabs: vec![empty.id.clone(), spaces.id.clone()],
                notes: vec![empty, spaces, named],
                files: vec![],
            },
        )
        .unwrap();
        let (book, _) = storage.load().unwrap();
        assert_eq!(book.notes.len(), 2);
        assert_eq!(book.tabs.len(), 1);
        assert_eq!(book.notes[0].text, "  ");
        assert_eq!(book.notes[0].title(), "Untitled note");
        assert_eq!(book.notes[1].title(), "My name");
    }
}
