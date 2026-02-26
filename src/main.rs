use std::path::Path;
use std::{env, fs, process};

use rbtags::indexer::{self, DefinitionKind};
use rbtags::location::LineIndex;

fn main() {
    let path = match env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("Usage: rbtags <file-or-directory>");
            process::exit(1);
        }
    };

    let path = Path::new(&path);
    let mut rb_files = Vec::new();
    collect_rb_files(path, &mut rb_files);
    rb_files.sort();

    for file_path in &rb_files {
        let source = match fs::read(file_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("warning: failed to read {}: {e}", file_path.display());
                continue;
            }
        };

        let defs = indexer::index_source(&source);
        if defs.is_empty() {
            continue;
        }

        let line_index = LineIndex::new(&source);
        for def in &defs {
            let (line, _col) = line_index.line_col(def.offset);
            let kind = match def.kind {
                DefinitionKind::Module => "module",
                DefinitionKind::Class => "class",
                DefinitionKind::Method => "method",
            };
            println!("{}\t{}\t{}:{}", def.fqn, kind, file_path.display(), line + 1);
        }
    }
}

fn collect_rb_files(path: &Path, files: &mut Vec<std::path::PathBuf>) {
    if path.is_file() {
        if path.extension().is_some_and(|ext| ext == "rb") {
            files.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        let entries = match fs::read_dir(path) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("warning: failed to read directory {}: {e}", path.display());
                return;
            }
        };
        for entry in entries.flatten() {
            collect_rb_files(&entry.path(), files);
        }
    }
}
