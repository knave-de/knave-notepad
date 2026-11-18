use super::*;

impl Notes {
    pub fn open_cli_file(&mut self, path: &Path) -> AppResult {
        if let Err(error) = self.open_file(path) {
            let message = format!("Could not open {}: {error}", path.display());
            eprintln!("{message}");
            self.view.message = Some(message);
            self.refresh_status()?;
        }
        Ok(())
    }

    pub fn open_file(&mut self, path: &Path) -> AppResult {
        let file = FileDocument::open(path)?;
        if let Some(id) = self
            .files
            .iter()
            .find_map(|(id, existing)| existing.same_target(&file).then(|| id.clone()))
        {
            return self.open(id);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let text = file.text().to_owned();
        self.attach_file(id, file, &text)
    }

    pub(super) fn attach_file(&mut self, id: String, file: FileDocument, text: &str) -> AppResult {
        let doc = Document::new(text);
        let vim = self
            .settings
            .vim
            .then(|| VimEditor::with_store(&doc.text, 0, self.modal_store.clone()));
        self.tabs.insert(id.clone(), Tab { doc, vim });
        self.files.insert(id.clone(), file);
        self.update_shared_views(&id);
        self.open(id)
    }

    pub(super) fn bookmark_index(&self, file: &FileDocument) -> Option<usize> {
        self.book.files.iter().position(|entry| {
            entry.path == file.path
                || entry
                    .canonical
                    .as_deref()
                    .zip(file.target().to_str())
                    .is_some_and(|(left, right)| left == right)
        })
    }

    pub(super) fn bookmark_file(&mut self, id: &str) -> AppResult {
        let Some(file) = self.files.get(id) else {
            return Ok(());
        };
        if let Some(index) = self.bookmark_index(file) {
            self.book.files.remove(index);
        } else {
            self.book.files.push(LibraryFile {
                id: uuid::Uuid::new_v4().to_string(),
                path: file.path.clone(),
                canonical: file.target().to_str().map(str::to_owned),
                added: now(),
            });
        }
        self.previews.start(&self.book.files);
        self.save();
        self.rebuild()?;
        if self.view.page == Page::Editor {
            self.focus_editor()?;
        }
        Ok(())
    }

    pub(super) fn locate_library_entry(&mut self, id: &str) -> AppResult {
        let Some(path) = rfd::FileDialog::new().set_title("Locate File").pick_file() else {
            return Ok(());
        };
        if let Some(entry) = self.book.files.iter_mut().find(|entry| entry.id == id) {
            entry.path = std::path::absolute(path)?;
            entry.canonical = entry
                .path
                .canonicalize()
                .ok()
                .and_then(|path| path.to_str().map(str::to_owned));
        }
        self.previews.start(&self.book.files);
        self.save();
        self.rebuild()
    }

    pub(super) fn open_library_entry(&mut self, id: String) -> AppResult {
        if let Some(entry) = self.book.files.iter().find(|entry| entry.id == id) {
            let path = entry.path.clone();
            let result = match path.try_exists() {
                Ok(true) => self.open_file(&path),
                Ok(false) => {
                    Err("File is missing. Use Locate File to update its Library entry.".into())
                }
                Err(error) => Err(error.into()),
            };
            if let Err(error) = result {
                self.view.message = Some(format!("Could not open {}: {error}", path.display()));
                self.previews.start(&self.book.files);
                self.refresh_status()?;
            }
            Ok(())
        } else {
            self.open(id)
        }
    }

    pub(super) fn pick_file(&mut self) -> AppResult {
        if let Some(path) = rfd::FileDialog::new().set_title("Open File").pick_file()
            && let Err(error) = self.open_file(&path)
        {
            self.view.message = Some(format!("Could not open {}: {error}", path.display()));
            self.refresh_status()?;
        }
        Ok(())
    }

    pub(super) fn save_current(&mut self) -> AppResult<bool> {
        if self.view.page == Page::Editor
            && let Some(id) = self.view.active.clone()
            && self.files.contains_key(&id)
        {
            self.save_file(&id)
        } else {
            self.save();
            self.refresh_status()?;
            Ok(self.save_error.is_none())
        }
    }

    pub(super) fn save_file(&mut self, id: &str) -> AppResult<bool> {
        self.write_file(id, false)
    }

    pub(super) fn write_file(&mut self, id: &str, overwrite: bool) -> AppResult<bool> {
        let text = &self.tabs[id].doc.text;
        let file = self.files.get_mut(id).expect("open external file");
        match file.save(text, overwrite) {
            Ok(()) => {
                self.view.message = None;
                let cleared = self.clear_recovery(id)?;
                self.generation = self.generation.wrapping_add(1);
                self.refresh_status()?;
                Ok(cleared)
            }
            Err(SaveError::Io(error)) => {
                self.view.message =
                    Some(format!("Save failed for {}: {error}", file.path.display()));
                self.refresh_status()?;
                Ok(false)
            }
            Err(SaveError::Conflict) => {
                self.view.file_prompt = Some(FilePrompt::Conflict(id.into()));
                self.rebuild()?;
                Ok(false)
            }
        }
    }

    pub(super) fn reload_file(&mut self, id: &str) -> AppResult<bool> {
        match FileDocument::open(&self.files[id].path) {
            Ok(file) => {
                let text = file.text().to_owned();
                if !self.clear_recovery(id)? {
                    return Ok(false);
                }
                self.attach_file(id.to_owned(), file, &text)?;
                Ok(true)
            }
            Err(error) => {
                self.view.message = Some(format!("Reload failed: {error}"));
                self.refresh_status()?;
                Ok(false)
            }
        }
    }

    pub(super) fn save_file_as(&mut self, id: &str) -> AppResult<bool> {
        let file = &self.files[id];
        let Some(path) = rfd::FileDialog::new().set_title("Save As").save_file() else {
            return Ok(false);
        };
        let saved = match file.save_as(&path, &self.tabs[id].doc.text) {
            Ok(file) => {
                self.files.insert(id.into(), file);
                self.generation = self.generation.wrapping_add(1);
                self.view.message = None;
                self.clear_recovery(id)?
            }
            Err(error) => {
                self.view.message = Some(format!("Save As failed: {error}"));
                false
            }
        };
        self.refresh_status()?;
        Ok(saved)
    }

    pub fn set_recovery(&mut self, recovery: Recovery) -> AppResult {
        self.recovery_after_crash = recovery.start_session()?;
        self.recoverable = recovery.pending()?.into();
        self.recovery = Some(recovery);
        self.rebuild()
    }

    pub(super) fn clear_recovery(&mut self, id: &str) -> AppResult<bool> {
        if let Some(recovery) = &self.recovery
            && let Err(error) = recovery.clear(id)
        {
            self.view.message = Some(format!("Could not clear the recovery checkpoint: {error}"));
            self.refresh_status()?;
            return Ok(false);
        }
        self.checkpoints.remove(id);
        Ok(true)
    }

    pub(super) fn recover_file(&mut self, snapshot_id: String, snapshot: Snapshot) -> AppResult {
        let existing = self
            .files
            .iter()
            .find_map(|(id, file)| file.same_target(&snapshot.file).then(|| id.clone()));
        let id = if let Some(id) = existing {
            if let Some(recovery) = &self.recovery
                && let Err(error) = recovery.checkpoint(&id, &snapshot.file, &snapshot.text)
            {
                self.view.message =
                    Some(format!("Could not attach the recovery checkpoint: {error}"));
                return self.refresh_status();
            }
            if id != snapshot_id {
                self.clear_recovery(&snapshot_id)?;
            }
            id
        } else {
            snapshot_id
        };
        self.attach_file(id, snapshot.file, &snapshot.text)
    }

    pub(super) fn close_progress(&mut self) -> AppResult<bool> {
        let dirty: Vec<_> = self
            .book
            .tabs
            .iter()
            .filter(|id| {
                !self.elsewhere(id)
                    && self
                        .files
                        .get(*id)
                        .is_some_and(|file| file.dirty(&self.tabs[*id].doc.text))
            })
            .cloned()
            .collect();
        if dirty.is_empty() {
            self.view.closing = false;
            self.view.save_all = false;
            return Ok(true);
        }
        if self.view.save_all {
            for id in dirty {
                if !self.write_file(&id, false)? {
                    if self.view.file_prompt.is_some() {
                        self.view.close_after_save = Some(id);
                    } else {
                        self.view.closing = false;
                        self.view.save_all = false;
                    }
                    return Ok(false);
                }
            }
            self.view.closing = false;
            self.view.save_all = false;
            return Ok(true);
        }
        self.view.file_prompt = if dirty.len() == 1 {
            Some(FilePrompt::Close(dirty.into_iter().next().unwrap()))
        } else {
            Some(FilePrompt::CloseWindow(dirty))
        };
        self.rebuild()?;
        Ok(false)
    }

    pub(super) fn cancel_file_prompt(&mut self) -> AppResult {
        self.view.file_prompt = None;
        self.view.close_after_save = None;
        self.view.closing = false;
        self.view.save_all = false;
        self.rebuild()?;
        if self.view.page == Page::Editor {
            self.focus_editor()?;
        }
        Ok(())
    }

    pub(super) fn resolve_file_prompt(&mut self, action: u64) -> AppResult {
        let Some(prompt) = self.view.file_prompt.take() else {
            return Ok(());
        };
        self.rebuild()?;
        let (id, resolved) = match prompt {
            FilePrompt::Close(id) => {
                self.view.close_after_save = Some(id.clone());
                let resolved = match action {
                    102 => self.write_file(&id, false)?,
                    103 => true,
                    _ => false,
                };
                (id, resolved)
            }
            FilePrompt::CloseWindow(ids) => {
                match action {
                    102 => {
                        self.view.save_all = true;
                        if self.close_progress()? {
                            self.view.exit_requested = true;
                        }
                    }
                    103 => {
                        for id in ids {
                            if !self.elsewhere(&id) && !self.clear_recovery(&id)? {
                                self.view.closing = false;
                                return Ok(());
                            }
                        }
                        self.view.exit_requested = true;
                    }
                    _ => {}
                }
                return Ok(());
            }
            FilePrompt::Conflict(id) => {
                let resolved = match action {
                    102 => self.reload_file(&id)?,
                    103 => self.write_file(&id, true)?,
                    104 => self.save_file_as(&id)?,
                    _ => false,
                };
                (id, resolved)
            }
        };
        if resolved && self.view.close_after_save.as_deref() == Some(&id) {
            self.view.close_after_save = None;
            self.remove_tab(&id)?;
            if self.view.closing && self.close_progress()? {
                self.view.exit_requested = true;
            }
        } else if self.view.file_prompt.is_none() {
            self.view.close_after_save = None;
            self.view.closing = false;
            self.view.save_all = false;
        }
        if self.view.page == Page::Editor && self.view.file_prompt.is_none() {
            self.focus_editor()?;
        }
        Ok(())
    }
}
