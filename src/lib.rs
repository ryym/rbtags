pub mod indexer;
pub mod location;
pub mod log;
pub mod resolver;
pub mod server;

use std::path::{Path, PathBuf};
use std::{fs, io};

/// Recursively collect all `.rb` files under the given path.
pub fn collect_rb_files(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_rb_files_inner(path, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_rb_files_inner(path: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    if path.is_file() {
        if path.extension().is_some_and(|ext| ext == "rb") {
            files.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        for entry in fs::read_dir(path)? {
            collect_rb_files_inner(&entry?.path(), files)?;
        }
    }
    Ok(())
}
