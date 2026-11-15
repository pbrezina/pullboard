//! File-descriptor-relative operations keep symlinks out of every traversed component.
//! EXCHANGE validates the displaced inode before accepting a replacement; NOREPLACE
//! protects absent destinations. Existing bytes are backed up privately, never logged.

use super::{InstallError, MAX_FILE, Paths, check_cancel};
use rustix::fs::{self, AtFlags, Mode, OFlags, RenameFlags};
use std::{
    cell::Cell,
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path},
    sync::atomic::{AtomicU64, Ordering},
};
use tokio_util::sync::CancellationToken;

const DIRS: [&str; 5] = [
    "",
    ".config",
    ".config/pullboard",
    ".config/containers",
    ".config/containers/systemd",
];
const TARGETS: [(usize, &str); 4] = [
    (2, "config.yaml"),
    (4, "pullboard.container"),
    (2, "config.yaml.bak"),
    (4, "pullboard.container.bak"),
];
const INVALID: InstallError = InstallError(
    "installation path is unsafe, not owned by this user, linked, or writable by another user",
);
const IO: InstallError = InstallError(
    "protected installation file operation failed; inspect file permissions and available disk space",
);
const CONFLICT: InstallError = InstallError(
    "installation files or directories changed since inspection; inspect again before confirming",
);

#[derive(Clone, Eq, PartialEq)]
struct Identity {
    dev: u64,
    ino: u64,
    uid: u32,
    mode: u32,
}
impl Identity {
    fn of(file: &File) -> Result<Self, InstallError> {
        let m = file.metadata().map_err(|_| IO)?;
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
            uid: m.uid(),
            mode: m.mode(),
        })
    }
}
#[derive(Clone, Eq, PartialEq)]
struct Fingerprint {
    identity: Identity,
    gid: u32,
    links: u64,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
impl Fingerprint {
    fn of(file: &File) -> Result<Self, InstallError> {
        let m = file.metadata().map_err(|_| IO)?;
        Ok(Self {
            identity: Identity::of(file)?,
            gid: m.gid(),
            links: m.nlink(),
            size: m.len(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
        })
    }
}
#[derive(Clone, Eq, PartialEq)]
struct SavedFile {
    fingerprint: Fingerprint,
    contents: Vec<u8>,
}

/// All secret-bearing snapshot types intentionally omit Debug.
#[derive(Clone)]
pub(super) struct Snapshot {
    dirs: Vec<Option<Identity>>,
    files: Vec<Option<SavedFile>>,
}

impl Snapshot {
    pub(super) fn inspect(paths: &Paths) -> Result<Self, InstallError> {
        let home = open_home(paths.home())?;
        let mut dirs = Vec::new();
        let mut handles = Vec::new();
        for relative in DIRS {
            let handle = open_dir(&home, relative)?;
            dirs.push(handle.as_ref().map(Identity::of).transpose()?);
            handles.push(handle);
        }
        if let Some(dir) = &handles[4] {
            reject_custom(dir, &["pullboard.container.d", "container.d"])?;
        }
        if let Some(dir) = open_dir(&home, ".config/systemd/user")? {
            reject_custom(
                &dir,
                &["pullboard.service", "pullboard.service.d", "service.d"],
            )?;
        }
        let files = TARGETS
            .into_iter()
            .map(|(dir, name)| match &handles[dir] {
                Some(h) => read_file(h, name),
                None => Ok(None),
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { dirs, files })
    }
    pub(super) fn config_text(&self) -> Result<Option<String>, InstallError> {
        self.text(0)
    }
    pub(super) fn quadlet_text(&self) -> Result<Option<String>, InstallError> {
        self.text(1)
    }
    fn text(&self, index: usize) -> Result<Option<String>, InstallError> {
        self.files[index]
            .as_ref()
            .map(|s| {
                String::from_utf8(s.contents.clone()).map_err(|_| {
                    InstallError("existing configuration or Quadlet is not valid UTF-8")
                })
            })
            .transpose()
    }
    pub(super) fn config_is_readable_by_others(&self) -> bool {
        self.files[0]
            .as_ref()
            .is_some_and(|s| s.fingerprint.identity.mode & 0o044 != 0)
    }
    pub(super) fn will_replace_backups(&self) -> bool {
        (self.files[0].is_some() && self.files[2].is_some())
            || (self.files[1].is_some() && self.files[3].is_some())
    }
    fn matches(&self, other: &Self) -> bool {
        self.dirs == other.dirs && self.files == other.files
    }
}

fn reject_custom(directory: &File, names: &[&str]) -> Result<(), InstallError> {
    for name in names {
        match fs::statat(directory, *name, AtFlags::SYMLINK_NOFOLLOW) {
            Err(rustix::io::Errno::NOENT) => (),
            Ok(_) => {
                return Err(InstallError(
                    "custom Quadlet/service overrides exist; preserve them and resolve manually before setup",
                ));
            }
            Err(_) => return Err(INVALID),
        }
    }
    Ok(())
}

fn flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

fn open_home(path: &Path) -> Result<File, InstallError> {
    let uid = rustix::process::geteuid();
    if uid.is_root() {
        return Err(InstallError("setup must run as a regular user, never root"));
    }
    let mut directory = File::from(fs::open("/", flags(), Mode::empty()).map_err(|_| INVALID)?);
    for component in path.components() {
        if let Component::Normal(name) = component {
            directory = File::from(
                fs::openat(&directory, name, flags(), Mode::empty()).map_err(|_| INVALID)?,
            );
        }
    }
    validate_directory(&directory)?;
    Ok(directory)
}

fn validate_directory(directory: &File) -> Result<(), InstallError> {
    let metadata = directory.metadata().map_err(|_| IO)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o022 != 0
    {
        return Err(INVALID);
    }
    Ok(())
}

fn open_dir(home: &File, relative: &str) -> Result<Option<File>, InstallError> {
    let mut dir = home.try_clone().map_err(|_| IO)?;
    for name in Path::new(relative).components() {
        let Component::Normal(name) = name else {
            return Err(INVALID);
        };
        dir = match fs::openat(&dir, name, flags(), Mode::empty()) {
            Ok(fd) => File::from(fd),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(INVALID),
        };
        validate_directory(&dir)?;
    }
    Ok(Some(dir))
}

fn create_dir(home: &File, relative: &str, changed: &mut bool) -> Result<File, InstallError> {
    let mut dir = home.try_clone().map_err(|_| IO)?;
    for name in Path::new(relative).components() {
        let Component::Normal(name) = name else {
            return Err(INVALID);
        };
        match fs::mkdirat(&dir, name, Mode::from_raw_mode(0o700)) {
            Ok(()) => {
                *changed = true;
                dir.sync_all().map_err(|_| IO)?;
            }
            Err(rustix::io::Errno::EXIST) => (),
            Err(_) => return Err(IO),
        }
        dir = File::from(fs::openat(&dir, name, flags(), Mode::empty()).map_err(|_| INVALID)?);
        validate_directory(&dir)?;
    }
    Ok(dir)
}

fn read_file(dir: &File, name: &str) -> Result<Option<SavedFile>, InstallError> {
    let fd = match fs::openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(_) => return Err(INVALID),
    };
    let file = File::from(fd);
    let metadata = file.metadata().map_err(|_| IO)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o022 != 0
        || metadata.len() > MAX_FILE as u64
    {
        return Err(INVALID);
    }
    let fingerprint = Fingerprint::of(&file)?;
    let mut contents = Vec::new();
    (&file)
        .take((MAX_FILE + 1) as u64)
        .read_to_end(&mut contents)
        .map_err(|_| IO)?;
    if contents.len() > MAX_FILE || Fingerprint::of(&file)? != fingerprint {
        return Err(CONFLICT);
    }
    Ok(Some(SavedFile {
        fingerprint,
        contents,
    }))
}

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Stage {
    directory: File,
    name: String,
    identity: Identity,
    cleanup: Cell<bool>,
}
impl Stage {
    fn new(directory: &File, contents: &[u8]) -> Result<Self, InstallError> {
        for _ in 0..64 {
            let name = format!(
                ".pullboard-setup-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            );
            let fd = match fs::openat(
                directory,
                name.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            ) {
                Ok(fd) => fd,
                Err(rustix::io::Errno::EXIST) => continue,
                Err(_) => return Err(IO),
            };
            let mut file = File::from(fd);
            // fchmod makes the desired mode independent of the calling process's umask.
            fs::fchmod(&file, Mode::from_raw_mode(0o600)).map_err(|_| IO)?;
            let stage = Self {
                directory: directory.try_clone().map_err(|_| IO)?,
                name,
                identity: Identity::of(&file)?,
                cleanup: Cell::new(true),
            };
            file.write_all(contents)
                .and_then(|()| file.sync_all())
                .map_err(|_| IO)?;
            return Ok(stage);
        }
        Err(InstallError(
            "could not allocate a private staging file; inspect abandoned staging files manually",
        ))
    }

    fn replace(&self, target: &str, expected: Option<&SavedFile>) -> Result<(), InstallError> {
        if read_file(&self.directory, target)?.as_ref() != expected {
            return Err(CONFLICT);
        }
        if expected.is_none() {
            fs::renameat_with(
                &self.directory,
                self.name.as_str(),
                &self.directory,
                target,
                RenameFlags::NOREPLACE,
            )
            .map_err(|_| CONFLICT)?;
        } else {
            fs::renameat_with(
                &self.directory,
                self.name.as_str(),
                &self.directory,
                target,
                RenameFlags::EXCHANGE,
            )
            .map_err(|_| CONFLICT)?;
            self.cleanup.set(false); // Preserve displaced user bytes until verified or restored.
            // An exchange changes ctime. Compare displaced identity, contents, and
            // security-relevant attributes, not timestamps altered by this rename.
            let displaced = read_file(&self.directory, &self.name);
            let matched = match (&displaced, expected) {
                (Ok(Some(actual)), Some(old)) => {
                    actual.contents == old.contents
                        && actual.fingerprint.identity == old.fingerprint.identity
                        && actual.fingerprint.gid == old.fingerprint.gid
                        && actual.fingerprint.links == old.fingerprint.links
                        && actual.fingerprint.size == old.fingerprint.size
                        && actual.fingerprint.mtime == old.fingerprint.mtime
                }
                _ => false,
            };
            if !matched {
                if read_file(&self.directory, target)?
                    .is_some_and(|s| s.fingerprint.identity == self.identity)
                {
                    fs::renameat_with(
                        &self.directory,
                        self.name.as_str(),
                        &self.directory,
                        target,
                        RenameFlags::EXCHANGE,
                    )
                    .map_err(|_| {
                        InstallError(
                            "concurrent replacement detected; manual file recovery is required",
                        )
                    })?;
                    self.cleanup.set(true);
                }
                return Err(CONFLICT);
            }
            self.cleanup.set(true);
        }
        self.directory.sync_all().map_err(|_| IO)
    }
    fn installed(&self, target: &str) -> bool {
        read_file(&self.directory, target)
            .ok()
            .flatten()
            .is_some_and(|s| s.fingerprint.identity == self.identity)
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        if self.cleanup.get() {
            let _ = fs::unlinkat(&self.directory, self.name.as_str(), AtFlags::empty());
        }
    }
}

pub(super) struct SaveResult {
    pub config: bool,
    pub quadlet: bool,
    pub changed: bool,
    pub result: Result<(), InstallError>,
}
pub(super) fn save(
    paths: &Paths,
    snapshot: &Snapshot,
    yaml: &str,
    quadlet: &str,
    cancel: &CancellationToken,
) -> SaveResult {
    let mut result = SaveResult {
        config: false,
        quadlet: false,
        changed: false,
        result: Ok(()),
    };
    result.result = save_inner(paths, snapshot, yaml, quadlet, cancel, &mut result);
    result
}

fn save_inner(
    paths: &Paths,
    snapshot: &Snapshot,
    yaml: &str,
    quadlet: &str,
    cancel: &CancellationToken,
    result: &mut SaveResult,
) -> Result<(), InstallError> {
    check_cancel(cancel)?;
    let home = open_home(paths.home())?;
    // No lockfile (and thus no pre-confirmation persistent changes). All installer
    // processes cooperate on the owned home-directory inode.
    fs::flock(&home, fs::FlockOperation::NonBlockingLockExclusive).map_err(|_| {
        InstallError("another installer holds the home-directory installation lock")
    })?;
    let current = Snapshot::inspect(paths)?;
    if !snapshot.matches(&current) {
        return Err(CONFLICT);
    }
    check_cancel(cancel)?;
    let config_dir = create_dir(&home, DIRS[2], &mut result.changed)?;
    let quadlet_dir = create_dir(&home, DIRS[4], &mut result.changed)?;
    let prepared = Snapshot::inspect(paths)?;
    // Existing directories must retain their original inodes; only absent ones may appear.
    if snapshot
        .dirs
        .iter()
        .zip(&prepared.dirs)
        .any(|(old, new)| old.is_some() && old != new)
        || snapshot.files != prepared.files
    {
        return Err(CONFLICT);
    }
    let config_stage = Stage::new(&config_dir, yaml.as_bytes())?;
    let quadlet_stage = Stage::new(&quadlet_dir, quadlet.as_bytes())?;
    let config_backup = snapshot.files[0]
        .as_ref()
        .map(|f| Stage::new(&config_dir, &f.contents))
        .transpose()?;
    let quadlet_backup = snapshot.files[1]
        .as_ref()
        .map(|f| Stage::new(&quadlet_dir, &f.contents))
        .transpose()?;
    check_cancel(cancel)?;
    if !prepared.matches(&Snapshot::inspect(paths)?) {
        return Err(CONFLICT);
    }
    if let Some(backup) = config_backup {
        backup.replace(TARGETS[2].1, snapshot.files[2].as_ref())?;
        result.changed = true;
    }
    if let Some(backup) = quadlet_backup {
        backup.replace(TARGETS[3].1, snapshot.files[3].as_ref())?;
        result.changed = true;
    }
    check_cancel(cancel)?;
    verify_dirs(paths, &prepared)?;
    let commit = config_stage.replace(TARGETS[0].1, snapshot.files[0].as_ref());
    result.config = config_stage.installed(TARGETS[0].1);
    commit?;
    check_cancel(cancel)?;
    verify_dirs(paths, &prepared)?;
    let commit = quadlet_stage.replace(TARGETS[1].1, snapshot.files[1].as_ref());
    result.quadlet = quadlet_stage.installed(TARGETS[1].1);
    commit?;
    verify_dirs(paths, &prepared)?;
    check_cancel(cancel)
}

fn verify_dirs(paths: &Paths, expected: &Snapshot) -> Result<(), InstallError> {
    if Snapshot::inspect(paths)?.dirs != expected.dirs {
        return Err(CONFLICT);
    }
    Ok(())
}
