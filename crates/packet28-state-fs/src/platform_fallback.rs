use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use super::FileAccess;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Identity {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

#[derive(Debug)]
pub(super) struct RetainedDir {
    root: PathBuf,
    path: PathBuf,
}

impl RetainedDir {
    pub(super) fn open(root: &Path, components: &[&str], create: bool) -> io::Result<Self> {
        let root = fs::canonicalize(root)?;
        let mut path = root.clone();
        for component in components {
            validate_component(component)?;
            path.push(component);
            if create {
                match fs::create_dir(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
            }
            require_directory(&path)?;
        }
        Ok(Self { root, path })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn validate(&self) -> io::Result<()> {
        require_directory(&self.root)?;
        require_directory(&self.path)
    }

    pub(super) fn open_existing(
        &self,
        name: &str,
        access: FileAccess,
    ) -> io::Result<Option<(File, Identity)>> {
        validate_component(name)?;
        let path = self.path.join(name);
        let mut options = options(access);
        let file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let identity = require_regular(&path, &file)?;
        Ok(Some((file, identity)))
    }

    pub(super) fn open_or_create(
        &self,
        name: &str,
        access: FileAccess,
    ) -> io::Result<(File, Identity, bool)> {
        match self.create_new(name, access) {
            Ok((file, identity)) => Ok((file, identity, true)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let (file, identity) = self.open_existing(name, access)?.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "state leaf vanished")
                })?;
                Ok((file, identity, false))
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn create_new(
        &self,
        name: &str,
        access: FileAccess,
    ) -> io::Result<(File, Identity)> {
        validate_component(name)?;
        let path = self.path.join(name);
        let mut options = options(access);
        let file = options.create_new(true).open(&path)?;
        let identity = require_regular(&path, &file)?;
        Ok((file, identity))
    }

    pub(super) fn validate_replace_target(&self, name: &str) -> io::Result<()> {
        validate_component(name)?;
        let path = self.path.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "state replacement target is not a regular file",
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub(super) fn validate_entry(
        &self,
        name: &std::ffi::OsStr,
        expected: Identity,
    ) -> io::Result<()> {
        let path = self.path.join(name);
        let file = File::open(&path)?;
        let actual = require_regular(&path, &file)?;
        if actual == expected {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "retained state file was replaced",
            ))
        }
    }

    pub(super) fn rename(&self, source: &str, destination: &str) -> io::Result<()> {
        fs::rename(self.path.join(source), self.path.join(destination))
    }

    pub(super) fn names(&self, max_entries: usize) -> io::Result<Vec<OsString>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.path)? {
            if names.len() >= max_entries {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "state directory entry limit exceeded",
                ));
            }
            names.push(entry?.file_name());
        }
        names.sort();
        Ok(names)
    }

    pub(super) fn remove_file_if_exists(&self, name: &str) -> io::Result<()> {
        match fs::remove_file(self.path.join(name)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub(super) fn remove_if_identity(&self, name: &str, expected: Identity) -> io::Result<()> {
        self.validate_entry(std::ffi::OsStr::new(name), expected)?;
        self.remove_file_if_exists(name)
    }

    pub(super) fn remove_tree_if_exists(&self, name: &str) -> io::Result<()> {
        match fs::remove_dir_all(self.path.join(name)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        File::open(&self.path)?.sync_all()
    }
}

fn options(access: FileAccess) -> OpenOptions {
    let mut options = OpenOptions::new();
    match access {
        FileAccess::ReadOnly => {
            options.read(true);
        }
        FileAccess::ReadWrite => {
            options.read(true).write(true);
        }
        FileAccess::Append => {
            options.read(true).append(true);
        }
    }
    options
}

fn validate_component(name: &str) -> io::Result<()> {
    let path = Path::new(name);
    if name.is_empty() || path.components().count() != 1 || name == "." || name == ".." {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "state path must be one non-special component",
        ))
    } else {
        Ok(())
    }
}

fn require_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "state ancestor is not a real directory",
        ))
    }
}

fn require_regular(path: &Path, file: &File) -> io::Result<Identity> {
    let attached = fs::symlink_metadata(path)?;
    let metadata = file.metadata()?;
    if attached.file_type().is_symlink() || !attached.is_file() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "state leaf is not a regular file",
        ));
    }
    Ok(Identity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}
