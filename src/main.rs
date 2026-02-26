use std::path::Path;
use std::{fs, process};

use rbtags::indexer::{self, DefinitionKind};
use rbtags::location::LineIndex;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("Usage: rbtags <file-or-directory>");
            process::exit(1);
        }
    };

    let rb_files = match rbtags::collect_rb_files(Path::new(&path)) {
        Ok(files) => files,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };

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
