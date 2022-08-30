use anyhow::Context;
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path};
use std::process::Command;

#[cfg(windows)]
pub fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> anyhow::Result<()> {
    let (from, to) = (from.as_ref(), to.as_ref());

    let ctx = format!("renaming file {:?} to {:?}", from, to);

    if fs::metadata(from)?.is_file() {
        return Ok(fs::rename(from, to).with_context(|| ctx.clone())?);
    }

    robocopy(from, to, &[&"/move"]).with_context(|| ctx.clone())
}

#[cfg(unix)]
pub fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> anyhow::Result<()> {
    let (from, to) = (from.as_ref(), to.as_ref());
    if fs::rename(from, to).is_err() {
        // This is necessary if from and to are on different
        // mount points (e.g., if /tmp is in tmpfs instead of on
        // the same disk). We don't want to implement a full recursive solution
        // to copying directories, so just shell out to `mv`.
        let ctx = format!("mv {:?} {:?}", from, to);
        let status = Command::new("mv")
            .arg(from)
            .arg(to)
            .status()
            .with_context(|| ctx.clone())?;
        if !status.success() {
            anyhow::bail!("mv {:?} {:?}: {:?}", from, to, status);
        }
    }

    Ok(())
}

/// Touch a file, resetting its modification time.
pub fn touch(path: &Path) -> anyhow::Result<()> {
    log::trace!("touching file {:?}", path);

    filetime::set_file_mtime(path, filetime::FileTime::now())
        .with_context(|| format!("touching file {:?}", path))?;

    Ok(())
}

/// Reset the modification time of all files in the given path.
pub fn touch_all(path: &Path) -> anyhow::Result<()> {
    fn is_valid(path: &Path) -> bool {
        let target_dir = Component::Normal(OsStr::new("target"));

        // Don't touch files in `target/`, since they're likely generated by build scripts and might be from a dependency.
        if path.components().any(|component| component == target_dir) {
            return false;
        }

        if let Some(extn) = path.extension() {
            if extn.to_str() == Some("rs") {
                // Don't touch build scripts, which confuses the wrapped rustc.
                return path.file_name() != Some(OsStr::new("build.rs"));
            }
        }

        false
    }

    for entry in walkdir::WalkDir::new(path) {
        let entry = entry?;
        let path = entry.path();

        // We also delete the cmake caches to avoid errors when moving directories around.
        // This might be a bit slower but at least things build
        if path.file_name() == Some(OsStr::new("CMakeCache.txt")) {
            fs::remove_file(path)
                .with_context(|| format!("deleting cmake caches in {:?}", path))?;
        }

        if is_valid(path) {
            touch(path)?;
        }
    }

    Ok(())
}

/// Counts the number of files and the total size of all files within the given `path`.
/// File size is counted as the actual size in bytes, i.e. the size returned by
/// [std::path::Path::metadata].
///
/// Returns (file_count, size).
pub fn get_file_count_and_size(path: &Path) -> std::io::Result<(u64, u64)> {
    let (count, size) = if path.is_dir() {
        let mut file_count = 0;
        let mut total_size = 0;
        for entry in fs::read_dir(&path)? {
            let path = entry?.path();
            let (count, size) = get_file_count_and_size(&path)?;
            file_count += count;
            total_size += size;
        }
        (file_count, total_size)
    } else if path.is_file() {
        (1, path.metadata()?.len())
    } else {
        (0, 0)
    };
    Ok((count, size))
}

#[cfg(windows)]
pub fn robocopy(
    from: &std::path::Path,
    to: &std::path::Path,
    extra_args: &[&dyn AsRef<std::ffi::OsStr>],
) -> anyhow::Result<()> {
    use crate::run_command_with_output;

    let mut cmd = Command::new("robocopy");
    cmd.arg(from).arg(to).arg("/s").arg("/e");

    for arg in extra_args {
        cmd.arg(arg.as_ref());
    }

    let output = run_command_with_output(&mut cmd)?;

    if output.status.code() >= Some(8) {
        // robocopy returns 0-7 on success
        return Err(anyhow::anyhow!(
            "expected success, got {}\n\nstderr={}\n\n stdout={}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::get_file_count_and_size;
    use std::path::PathBuf;

    #[test]
    fn test_get_file_count_and_size() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();

        let write = |path: PathBuf, size: usize| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, vec![0u8; size].as_slice()).unwrap();
        };

        write(root.join("a/b/c.rs"), 1024);
        write(root.join("a/b/d.rs"), 16);
        write(root.join("a/x.rs"), 32);
        write(root.join("b/x.rs"), 64);
        write(root.join("b/x2.rs"), 64);
        write(root.join("x.rs"), 128);

        let (files, size) = get_file_count_and_size(root).unwrap();
        assert_eq!(files, 6);
        assert_eq!(size, 1024 + 16 + 32 + 64 + 64 + 128);
    }
}
