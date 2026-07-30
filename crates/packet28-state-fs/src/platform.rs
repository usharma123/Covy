use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use rustix::fs::{self as rfs, AtFlags, Dir, FileType, Mode, OFlags};

use super::FileAccess;

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const MAX_REMOVE_DEPTH: usize = 64;
const MAX_REMOVE_ENTRIES: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
}

#[derive(Debug)]
struct Binding {
    name: OsString,
    file: OwnedFd,
    identity: Identity,
}

#[derive(Debug)]
pub(super) struct RetainedDir {
    root_path: PathBuf,
    root: OwnedFd,
    root_identity: Identity,
    bindings: Vec<Binding>,
    path: PathBuf,
}

impl RetainedDir {
    pub(super) fn open(root: &Path, components: &[&str], create: bool) -> io::Result<Self> {
        let root_path = std::fs::canonicalize(root)?;
        let root_fd =
            rfs::open(&root_path, DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?;
        let root_stat = rfs::fstat(&root_fd).map_err(io::Error::from)?;
        require_directory(&root_stat, root.as_os_str())?;
        let root_identity = identity(&root_stat);
        let mut current = rustix::io::dup(&root_fd).map_err(io::Error::from)?;
        let mut path = root_path.clone();
        let mut bindings = Vec::with_capacity(components.len());
        for component in components {
            validate_component(OsStr::new(component))?;
            let name = OsString::from(component);
            if create {
                match rfs::mkdirat(&current, &name, Mode::RWXU) {
                    Ok(()) => {
                        rfs::fsync(&current).map_err(io::Error::from)?;
                    }
                    Err(rustix::io::Errno::EXIST) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            let child = rfs::openat(&current, &name, DIRECTORY_FLAGS, Mode::empty())
                .map_err(io::Error::from)?;
            let stat = rfs::fstat(&child).map_err(io::Error::from)?;
            require_directory(&stat, &name)?;
            let child_identity = identity(&stat);
            path.push(&name);
            bindings.push(Binding {
                name,
                file: child,
                identity: child_identity,
            });
            current = rustix::io::dup(&bindings.last().unwrap().file).map_err(io::Error::from)?;
        }
        let retained = Self {
            root_path,
            root: root_fd,
            root_identity,
            bindings,
            path,
        };
        retained.validate()?;
        Ok(retained)
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    fn directory(&self) -> &OwnedFd {
        self.bindings
            .last()
            .map_or(&self.root, |binding| &binding.file)
    }

    pub(super) fn validate(&self) -> io::Result<()> {
        let current_root =
            rfs::open(&self.root_path, DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?;
        let root_stat = rfs::fstat(&current_root).map_err(io::Error::from)?;
        if identity(&root_stat) != self.root_identity {
            return Err(replaced(&self.root_path));
        }
        let mut current = current_root;
        for binding in &self.bindings {
            let child = rfs::openat(&current, &binding.name, DIRECTORY_FLAGS, Mode::empty())
                .map_err(io::Error::from)?;
            let stat = rfs::fstat(&child).map_err(io::Error::from)?;
            require_directory(&stat, &binding.name)?;
            if identity(&stat) != binding.identity {
                return Err(replaced(&self.path));
            }
            current = child;
        }
        Ok(())
    }

    pub(super) fn open_existing(
        &self,
        name: &str,
        access: FileAccess,
    ) -> io::Result<Option<(File, Identity)>> {
        let name = OsStr::new(name);
        validate_component(name)?;
        let flags = access_flags(access) | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let fd = match rfs::openat(self.directory(), name, flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
        require_regular(&stat, name)?;
        Ok(Some((File::from(fd), identity(&stat))))
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
        let name = OsStr::new(name);
        validate_component(name)?;
        let flags = access_flags(access)
            | OFlags::CREATE
            | OFlags::EXCL
            | OFlags::NOFOLLOW
            | OFlags::CLOEXEC;
        let fd = rfs::openat(self.directory(), name, flags, Mode::RUSR | Mode::WUSR)
            .map_err(io::Error::from)?;
        let stat = rfs::fstat(&fd).map_err(io::Error::from)?;
        require_regular(&stat, name)?;
        Ok((File::from(fd), identity(&stat)))
    }

    pub(super) fn validate_replace_target(&self, name: &str) -> io::Result<()> {
        let name = OsStr::new(name);
        validate_component(name)?;
        match rfs::statat(self.directory(), name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => require_regular(&stat, name),
            Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn validate_entry(&self, name: &OsStr, expected: Identity) -> io::Result<()> {
        validate_component(name)?;
        let stat = rfs::statat(self.directory(), name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(io::Error::from)?;
        require_regular(&stat, name)?;
        if identity(&stat) == expected {
            Ok(())
        } else {
            Err(replaced(&self.path.join(name)))
        }
    }

    pub(super) fn rename(&self, source: &str, destination: &str) -> io::Result<()> {
        let source = OsStr::new(source);
        let destination = OsStr::new(destination);
        validate_component(source)?;
        validate_component(destination)?;
        rfs::renameat(self.directory(), source, self.directory(), destination)
            .map_err(io::Error::from)
    }

    pub(super) fn names(&self, max_entries: usize) -> io::Result<Vec<OsString>> {
        let mut directory = Dir::read_from(self.directory()).map_err(io::Error::from)?;
        let mut names = Vec::new();
        while let Some(entry) = directory.read() {
            let entry = entry.map_err(io::Error::from)?;
            let bytes = entry.file_name().to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if names.len() >= max_entries {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("state directory exceeds {max_entries} entries"),
                ));
            }
            names.push(OsString::from_vec(bytes.to_vec()));
        }
        names.sort();
        Ok(names)
    }

    pub(super) fn remove_file_if_exists(&self, name: &str) -> io::Result<()> {
        let name = OsStr::new(name);
        validate_component(name)?;
        match rfs::unlinkat(self.directory(), name, AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn remove_if_identity(&self, name: &str, expected: Identity) -> io::Result<()> {
        let name = OsStr::new(name);
        validate_component(name)?;
        let stat = match rfs::statat(self.directory(), name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        if identity(&stat) != expected {
            return Err(replaced(&self.path.join(name)));
        }
        rfs::unlinkat(self.directory(), name, AtFlags::empty()).map_err(io::Error::from)
    }

    pub(super) fn remove_tree_if_exists(&self, name: &str) -> io::Result<()> {
        validate_component(OsStr::new(name))?;
        let mut budget = MAX_REMOVE_ENTRIES;
        remove_entry(self.directory(), OsStr::new(name), 0, &mut budget)
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        rfs::fsync(self.directory()).map_err(io::Error::from)
    }
}

fn remove_entry(
    parent: &OwnedFd,
    name: &OsStr,
    depth: usize,
    budget: &mut usize,
) -> io::Result<()> {
    if depth > MAX_REMOVE_DEPTH || *budget == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "state removal traversal budget exhausted",
        ));
    }
    *budget -= 1;
    let stat = match rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return rfs::unlinkat(parent, name, AtFlags::empty()).map_err(io::Error::from);
    }
    let expected = identity(&stat);
    let child =
        rfs::openat(parent, name, DIRECTORY_FLAGS, Mode::empty()).map_err(io::Error::from)?;
    let child_stat = rfs::fstat(&child).map_err(io::Error::from)?;
    if identity(&child_stat) != expected {
        return Err(replaced(Path::new(name)));
    }
    let mut directory = Dir::read_from(&child).map_err(io::Error::from)?;
    let mut entries = Vec::new();
    while let Some(entry) = directory.read() {
        let entry = entry.map_err(io::Error::from)?;
        let bytes = entry.file_name().to_bytes();
        if bytes != b"." && bytes != b".." {
            entries.push(OsString::from_vec(bytes.to_vec()));
        }
    }
    drop(directory);
    for entry in entries {
        remove_entry(&child, &entry, depth + 1, budget)?;
    }
    let current = rfs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    if identity(&current) != expected {
        return Err(replaced(Path::new(name)));
    }
    rfs::unlinkat(parent, name, AtFlags::REMOVEDIR).map_err(io::Error::from)
}

fn access_flags(access: FileAccess) -> OFlags {
    match access {
        FileAccess::ReadOnly => OFlags::RDONLY,
        FileAccess::ReadWrite => OFlags::RDWR,
        FileAccess::Append => OFlags::RDWR | OFlags::APPEND,
    }
}

fn validate_component(name: &OsStr) -> io::Result<()> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "state path must be one non-special component",
        ))
    } else {
        Ok(())
    }
}

fn require_directory(stat: &rustix::fs::Stat, name: &OsStr) -> io::Result<()> {
    if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!(
                "state ancestor '{}' is not a real directory",
                Path::new(name).display()
            ),
        ))
    }
}

fn require_regular(stat: &rustix::fs::Stat, name: &OsStr) -> io::Result<()> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "state leaf '{}' is not a regular file",
                Path::new(name).display()
            ),
        ));
    }
    if stat.st_nlink != 1 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "state leaf '{}' has multiple hard links",
                Path::new(name).display()
            ),
        ));
    }
    Ok(())
}

fn identity(stat: &rustix::fs::Stat) -> Identity {
    Identity {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
    }
}

fn replaced(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("retained state path was replaced: {}", path.display()),
    )
}
