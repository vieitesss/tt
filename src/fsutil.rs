//! Filesystem helpers shared by the config and vault writers.
//!
//! Both the registry config and the task vault are written atomically: content
//! goes to a temporary file in the target folder and is renamed over the
//! target. Keeping one implementation means durability fixes (directory fsync,
//! Windows rename retries) land once.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use rand::Rng;

/// Write `contents` to `path` via a temporary file in the same folder.
///
/// The temporary file is named `.{name}.{pid}.{suffix}.tmp`, synced, and
/// renamed over the target; it is removed again when any step fails. When
/// `create_parents` is true the target's parent directories are created first
/// (the first save of the registry config); vault writes assume the folder
/// already exists.
pub(crate) fn write_atomic(path: &Path, contents: &str, create_parents: bool) -> io::Result<()> {
    let folder = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or("file");
    let temp_path = folder.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        random_suffix()
    ));

    let outcome = (|| -> io::Result<()> {
        if create_parents {
            fs::create_dir_all(folder)?;
        }
        let mut file = File::create(&temp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp_path, path)
    })();

    if let Err(source) = outcome {
        let _ = fs::remove_file(&temp_path);
        return Err(source);
    }
    Ok(())
}

/// Atomically create `path` without replacing it. Returns `false` if another
/// writer already created the target.
pub(crate) fn write_atomic_new(
    path: &Path,
    contents: &str,
    create_parents: bool,
) -> io::Result<bool> {
    let folder = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or("file");
    let temp_path = folder.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        random_suffix()
    ));

    let outcome = (|| -> io::Result<bool> {
        if create_parents {
            fs::create_dir_all(folder)?;
        }
        let mut file = File::create(&temp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        match fs::hard_link(&temp_path, path) {
            Ok(()) => Ok(true),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(source) => Err(source),
        }
    })();

    let _ = fs::remove_file(&temp_path);
    outcome
}

fn random_suffix() -> String {
    let mut rng = rand::thread_rng();
    (0..6)
        .map(|_| char::from(b'a' + rng.gen_range(0..26u8)))
        .collect()
}
