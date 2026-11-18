use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
enum Newlines {
    Lf,
    CrLf,
    Cr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Baseline {
    hash: [u8; 32],
    device: u64,
    inode: u64,
    uid: u32,
    gid: u32,
    mode: u32,
    links: u64,
}

impl Baseline {
    fn new(bytes: &[u8], metadata: &Metadata) -> Self {
        Self {
            hash: *blake3::hash(bytes).as_bytes(),
            device: metadata.dev(),
            inode: metadata.ino(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            mode: metadata.mode(),
            links: metadata.nlink(),
        }
    }
}

#[derive(Debug)]
pub enum SaveError {
    Conflict,
    Io(io::Error),
}

impl From<io::Error> for SaveError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Serialize, Deserialize)]
pub struct FileDocument {
    #[serde(with = "crate::store::unix_path")]
    pub path: PathBuf,
    #[serde(with = "crate::store::unix_path")]
    target: PathBuf,
    baseline: Option<Baseline>,
    saved_text: String,
    newlines: Newlines,
    bom: bool,
}

impl FileDocument {
    pub fn open(path: &Path) -> io::Result<Self> {
        let path = std::path::absolute(path)?;
        let target = match path.canonicalize() {
            Ok(target) => target,
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && fs::symlink_metadata(&path)
                        .is_err_and(|error| error.kind() == io::ErrorKind::NotFound) =>
            {
                let parent = path
                    .parent()
                    .ok_or_else(|| io::Error::other("Missing parent directory"))?;
                let name = path
                    .file_name()
                    .ok_or_else(|| io::Error::other("Missing filename"))?;
                return Ok(Self {
                    target: parent.canonicalize()?.join(name),
                    path,
                    baseline: None,
                    saved_text: String::new(),
                    newlines: Newlines::Lf,
                    bom: false,
                });
            }
            Err(error) => return Err(error),
        };
        let (_, metadata, bytes) = read_regular(&target, false)?;
        let baseline = Some(Baseline::new(&bytes, &metadata));
        let bom = bytes.starts_with(b"\xef\xbb\xbf");
        let text = std::str::from_utf8(&bytes[if bom { 3 } else { 0 }..]).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "Expected a UTF-8 text file")
        })?;
        if text.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "File contains NUL bytes",
            ));
        }
        let pairs = text.matches("\r\n").count();
        let lf = text.matches('\n').count() - pairs;
        let cr = text.matches('\r').count() - pairs;
        if [pairs, lf, cr]
            .into_iter()
            .filter(|&count| count > 0)
            .count()
            > 1
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "File has mixed line endings",
            ));
        }
        let newlines = if pairs > 0 {
            Newlines::CrLf
        } else if cr > 0 {
            Newlines::Cr
        } else {
            Newlines::Lf
        };
        Ok(Self {
            path,
            target,
            baseline,
            saved_text: crate::editor::normalize(text),
            newlines,
            bom,
        })
    }

    pub fn target(&self) -> &Path {
        &self.target
    }

    pub fn text(&self) -> &str {
        &self.saved_text
    }

    pub fn dirty(&self, text: &str) -> bool {
        self.baseline.is_none() || self.saved_text != text
    }

    pub fn same_target(&self, other: &Self) -> bool {
        self.target == other.target
            || self
                .baseline
                .zip(other.baseline)
                .is_some_and(|(left, right)| {
                    (left.device, left.inode) == (right.device, right.inode)
                })
    }

    pub fn line_endings(&self) -> &'static str {
        match self.newlines {
            Newlines::Lf => "LF",
            Newlines::CrLf => "CRLF",
            Newlines::Cr => "CR",
        }
    }

    fn encode(&self, text: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        if self.bom {
            bytes.extend_from_slice(b"\xef\xbb\xbf");
        }
        match self.newlines {
            Newlines::Lf => bytes.extend_from_slice(text.as_bytes()),
            Newlines::CrLf => bytes.extend_from_slice(text.replace('\n', "\r\n").as_bytes()),
            Newlines::Cr => bytes.extend_from_slice(text.replace('\n', "\r").as_bytes()),
        }
        bytes
    }

    pub fn save(&mut self, text: &str, overwrite: bool) -> Result<(), SaveError> {
        if !self.dirty(text) {
            return Ok(());
        }
        self.write(text, overwrite)
    }

    pub fn export(path: &Path, text: &str) -> io::Result<()> {
        let mut file = Self::open(path)?;
        let parent = file
            .path
            .parent()
            .ok_or_else(|| io::Error::other("Missing parent directory"))?;
        let name = file
            .path
            .file_name()
            .ok_or_else(|| io::Error::other("Missing filename"))?;
        if parent.canonicalize()?.join(name) != file.target {
            return Err(io::Error::other(
                "Choose an export path that is not a symbolic link",
            ));
        }
        if !file.bom && file.newlines == Newlines::Lf && !file.dirty(text) {
            return Ok(());
        }
        file.bom = false;
        file.newlines = Newlines::Lf;
        file.write(text, false).map_err(|error| match error {
            SaveError::Io(error) => error,
            SaveError::Conflict => {
                io::Error::other("The export destination changed. Choose a destination again.")
            }
        })
    }

    fn write(&mut self, text: &str, overwrite: bool) -> Result<(), SaveError> {
        if text.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "The buffer contains NUL bytes",
            )
            .into());
        }
        let missing = fs::symlink_metadata(&self.path)
            .is_err_and(|error| error.kind() == io::ErrorKind::NotFound);
        if (self.baseline.is_none() && !overwrite) || (overwrite && missing) {
            if overwrite && self.path.file_name() != self.target.file_name() {
                return Err(io::Error::other(
                    "The original path no longer resolves to the same file. Choose Save As.",
                )
                .into());
            }
            if self
                .path
                .parent()
                .map(Path::canonicalize)
                .transpose()?
                .as_deref()
                != self.target.parent()
            {
                return Err(SaveError::Conflict);
            }
            return match self.save_as(&self.path, text) {
                Ok(saved) => {
                    *self = saved;
                    Ok(())
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    Err(SaveError::Conflict)
                }
                Err(error) => Err(error.into()),
            };
        }
        match self.path.canonicalize() {
            Ok(target) if target == self.target => {}
            Ok(_) if overwrite => return Err(io::Error::other(
                "The path now points to a different target. Choose Save As or Reload from Disk.",
            )
            .into()),
            Ok(_) => return Err(SaveError::Conflict),
            Err(error) if error.kind() == io::ErrorKind::NotFound && !overwrite => {
                return Err(SaveError::Conflict);
            }
            Err(error) => return Err(error.into()),
        }
        let (original, metadata, previous) = read_regular(&self.target, true)?;
        let observed = Baseline::new(&previous, &metadata);
        if Some(observed) != self.baseline && !overwrite {
            return Err(SaveError::Conflict);
        }
        if metadata.nlink() != 1 || has_attributes(&original)? {
            return Err(io::Error::other(
                "Saving would risk losing hard links, ACLs, or extended attributes. Save to a new path instead.",
            ).into());
        }
        let bytes = self.encode(text);
        let parent = self.target.parent().expect("canonical file has parent");
        let temporary = parent.join(format!(".knave-notepad-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            let output_metadata = output.metadata()?;
            if (output_metadata.uid(), output_metadata.gid()) != (metadata.uid(), metadata.gid())
                || has_attributes(&output)?
            {
                return Err(io::Error::other(
                    "Could not preserve ownership or other file metadata",
                )
                .into());
            }
            output.write_all(&bytes)?;
            output.set_permissions(metadata.permissions())?;
            output.sync_all()?;
            let (latest, latest_metadata, latest_bytes) = read_regular(&self.target, true)?;
            if self.path.canonicalize()? != self.target
                || Baseline::new(&latest_bytes, &latest_metadata) != observed
                || has_attributes(&latest)?
            {
                return Err(SaveError::Conflict);
            }
            fs::rename(&temporary, &self.target)?;
            File::open(parent)?.sync_all()?;
            self.baseline = Some(Baseline::new(&bytes, &output.metadata()?));
            self.saved_text = text.into();
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn save_as(&self, path: &Path, text: &str) -> io::Result<Self> {
        if text.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "The buffer contains NUL bytes",
            ));
        }
        let path = std::path::absolute(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("Missing parent directory"))?;
        let temporary = parent.join(format!(".knave-notepad-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(&self.encode(text))?;
            file.sync_all()?;
            fs::hard_link(&temporary, &path)
        })();
        fs::remove_file(&temporary)?;
        result?;
        File::open(parent)?.sync_all()?;
        Self::open(&path)
    }
}

fn read_regular(path: &Path, writable: bool) -> io::Result<(File, Metadata, Vec<u8>)> {
    // Check before opening to avoid interacting with unsupported device files.
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::other("Expected a regular file"));
    }
    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
    let mut file = OpenOptions::new()
        .read(true)
        .write(writable)
        .custom_flags(flags.bits() as i32)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("Expected a regular file"));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok((file, metadata, bytes))
}

#[cfg(target_os = "linux")]
fn has_attributes(file: &File) -> io::Result<bool> {
    match rustix::fs::flistxattr_query(file) {
        Ok(size) => Ok(size != 0),
        Err(rustix::io::Errno::NOTSUP) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(target_os = "linux"))]
fn has_attributes(_: &File) -> io::Result<bool> {
    Err(io::Error::other(
        "File metadata checks are unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_preserves_symlinks_utf8_bom_line_endings_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let alias = dir.path().join("alias");
        fs::write(&target, b"\xef\xbb\xbffirst\r\nsecond").unwrap();
        fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(0o751)).unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let mut file = FileDocument::open(&alias).unwrap();
        assert_eq!(file.text(), "first\nsecond");
        file.save("first\nchanged", false).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"\xef\xbb\xbffirst\r\nchanged");
        assert_eq!(fs::read_link(alias).unwrap(), target);
        assert_eq!(fs::metadata(target).unwrap().mode() & 0o777, 0o751);
    }

    #[test]
    fn saving_hard_links_keeps_the_original_inode_and_bytes_intact() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let alias = dir.path().join("alias");
        fs::write(&target, "original").unwrap();
        fs::hard_link(&target, &alias).unwrap();
        let mut file = FileDocument::open(&target).unwrap();
        assert!(matches!(file.save("changed", false), Err(SaveError::Io(_))));
        assert!(file.dirty("changed"));
        assert_eq!(
            fs::metadata(&target).unwrap().ino(),
            fs::metadata(&alias).unwrap().ino()
        );
        assert_eq!(fs::read_to_string(alias).unwrap(), "original");
    }
}
