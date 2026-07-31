//! Bounded, descriptor-anchored access to persisted regex index state.

use std::fs::File;
use std::io;
use std::path::Path;

use packet28_state_fs::{FileAccess, StateDir, StateFile};

use crate::error::{Result, SearchError};
use crate::model::REGEX_DIR_NAME;

pub(crate) const MAX_REGEX_METADATA_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_REGEX_DOCS_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const MAX_REGEX_MMAP_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub(crate) fn regex_state_dir(root: &Path, create: bool) -> Result<StateDir> {
    StateDir::open(root, &[".packet28", "index", REGEX_DIR_NAME], create).map_err(SearchError::from)
}

pub(crate) fn index_parent_state_dir(root: &Path, create: bool) -> Result<StateDir> {
    StateDir::open(root, &[".packet28", "index"], create).map_err(SearchError::from)
}

fn state_path_spec(path: &Path) -> Result<(&Path, &'static [&'static str], String)> {
    let parent = path.parent().ok_or_else(|| {
        SearchError::corrupt(format!("index artifact '{}' has no parent", path.display()))
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            SearchError::corrupt(format!(
                "index artifact '{}' has an invalid leaf",
                path.display()
            ))
        })?
        .to_string();
    let (root, components): (&Path, &'static [&'static str]) = if parent
        .file_name()
        .is_some_and(|name| name == REGEX_DIR_NAME)
    {
        let index = parent
            .parent()
            .filter(|path| path.file_name().is_some_and(|name| name == "index"));
        let packet28 = index
            .and_then(Path::parent)
            .filter(|path| path.file_name().is_some_and(|name| name == ".packet28"));
        let root = packet28.and_then(Path::parent).ok_or_else(|| {
            SearchError::corrupt(format!(
                "index artifact '{}' is outside managed regex state",
                path.display()
            ))
        })?;
        (root, &[".packet28", "index", REGEX_DIR_NAME])
    } else if parent.file_name().is_some_and(|name| name == "index") {
        let packet28 = parent
            .parent()
            .filter(|path| path.file_name().is_some_and(|name| name == ".packet28"));
        let root = packet28.and_then(Path::parent).ok_or_else(|| {
            SearchError::corrupt(format!(
                "index artifact '{}' is outside managed regex state",
                path.display()
            ))
        })?;
        (root, &[".packet28", "index"])
    } else {
        return Err(SearchError::corrupt(format!(
            "index artifact '{}' is outside managed regex state",
            path.display()
        )));
    };
    Ok((root, components, name))
}

pub(crate) fn read_state_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let (root, components, name) = state_path_spec(path)?;
    let directory = StateDir::open(root, components, false)?;
    directory
        .read_bounded(&name, max_bytes)?
        .ok_or_else(|| SearchError::from(io::Error::from(io::ErrorKind::NotFound)))
}

pub(crate) fn read_optional_state_file(path: &Path, max_bytes: u64) -> Result<Option<Vec<u8>>> {
    let (root, components, name) = state_path_spec(path)?;
    let directory = match StateDir::open(root, components, false) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    directory.read_bounded(&name, max_bytes).map_err(Into::into)
}

pub(crate) fn open_optional_state_file(path: &Path, max_bytes: u64) -> Result<Option<StateFile>> {
    let (root, components, name) = state_path_spec(path)?;
    let directory = match StateDir::open(root, components, false) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    directory
        .open_bounded(&name, FileAccess::ReadOnly, max_bytes)
        .map_err(Into::into)
}

pub(crate) fn write_state_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let (root, components, name) = state_path_spec(path)?;
    StateDir::open(root, components, true)?
        .write_atomic(&name, bytes)
        .map_err(Into::into)
}

pub(crate) fn write_state_atomic_stream(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<()> {
    let (root, components, name) = state_path_spec(path)?;
    StateDir::open(root, components, true)?
        .write_atomic_stream(&name, write)
        .map_err(Into::into)
}

pub(crate) fn write_state_immutable(path: &Path, bytes: &[u8]) -> Result<()> {
    let (root, components, name) = state_path_spec(path)?;
    StateDir::open(root, components, true)?
        .write_immutable(&name, bytes)
        .map_err(Into::into)
}

pub(crate) fn remove_state_file_if_exists(path: &Path) -> Result<()> {
    let (root, components, name) = state_path_spec(path)?;
    let directory = match StateDir::open(root, components, false) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    directory.remove_file_if_exists(&name).map_err(Into::into)
}
