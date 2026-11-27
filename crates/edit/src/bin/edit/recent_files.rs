use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::settings::config_dir;

const LIMIT: usize = 10;

pub struct RecentFiles {
    storage: Option<PathBuf>,
    paths: Vec<PathBuf>,
}

impl Default for RecentFiles {
    fn default() -> Self {
        let storage = config_dir().map(|dir| dir.join("recent-files"));
        let paths = storage.as_deref().and_then(|path| read_paths(path).ok()).unwrap_or_default();
        Self { storage, paths }
    }
}

impl RecentFiles {
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub fn remember(&mut self, path: &Path) {
        if !path.is_absolute() {
            return;
        }

        // Hold the lock across reading and replacing the history so simultaneous
        // editor instances do not overwrite each other's changes.
        let locked_storage = self
            .storage
            .as_deref()
            .and_then(|storage| lock_storage(storage).ok().map(|lock| (storage, lock)));
        if let Some((storage, _)) = &locked_storage
            && let Ok(mut paths) = read_paths(storage)
        {
            for entry in self.paths.drain(..) {
                if !paths.contains(&entry) {
                    paths.push(entry);
                }
            }
            self.paths = paths;
        }

        self.paths.retain(|entry| entry != path);
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(LIMIT);

        // History is optional. A settings directory that cannot be written should
        // still allow the file to open and appear in this session's history.
        if let Some((storage, _lock)) = locked_storage {
            let _ = write_paths(storage, &self.paths);
        }
    }
}

// NUL separates paths without reserving any characters that filenames can contain.
#[cfg(unix)]
fn decode_paths(bytes: &[u8]) -> impl Iterator<Item = OsString> + '_ {
    use std::os::unix::ffi::OsStringExt;
    bytes.split(|&byte| byte == 0).map(|part| OsString::from_vec(part.to_vec()))
}

#[cfg(windows)]
fn decode_paths(bytes: &[u8]) -> impl Iterator<Item = OsString> {
    use std::os::windows::ffi::OsStringExt;
    let units: Vec<_> =
        bytes.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
    let paths: Vec<_> =
        units.split(|&unit| unit == 0).take(LIMIT).map(OsString::from_wide).collect();
    paths.into_iter()
}

fn read_paths(storage: &Path) -> io::Result<Vec<PathBuf>> {
    let bytes = fs::read(storage)?;
    Ok(decode_paths(&bytes)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .take(LIMIT)
        .collect())
}

fn lock_storage(storage: &Path) -> io::Result<File> {
    fs::create_dir_all(storage.parent().unwrap())?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    let lock = options.open(storage.with_extension("lock"))?;
    lock.lock()?;
    Ok(lock)
}

fn write_paths(storage: &Path, paths: &[PathBuf]) -> io::Result<()> {
    let temporary = storage.with_extension("tmp");
    // Only the lock holder uses this file, including cleanup after an interrupted write.
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = paths.iter().try_for_each(|path| write_path(&mut file, path));
    drop(file);
    let result = result.and_then(|()| fs::rename(&temporary, storage));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn write_path(file: &mut fs::File, path: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    file.write_all(path.as_os_str().as_bytes())?;
    file.write_all(&[0])
}

#[cfg(windows)]
fn write_path(file: &mut fs::File, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let bytes: Vec<_> =
        path.as_os_str().encode_wide().chain([0]).flat_map(u16::to_le_bytes).collect();
    file.write_all(&bytes)
}
