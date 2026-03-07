use std::path::Path;
use std::{env, fs, process};

use rbtags::indexer::{self, DefinitionKind};
use rbtags::location::LineIndex;

fn main() {
    let args: Vec<String> = env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("dump") => {
            let path = args.get(2).unwrap_or_else(|| {
                eprintln!("Usage: rbtags dump <file-or-directory>");
                process::exit(1);
            });
            run_dump(Path::new(path));
        }
        Some("lsp") => {
            if let Err(e) = rbtags::server::run() {
                eprintln!("error: {e}");
                process::exit(1);
            }
        }
        _ => {
            eprintln!("Usage: rbtags <command>");
            eprintln!();
            eprintln!("Commands:");
            eprintln!("  dump <path>   Print definition index for Ruby files");
            eprintln!("  lsp           Start LSP server");
            process::exit(1);
        }
    }
}

fn run_dump(path: &Path) {
    let rb_files = match rbtags::collect_rb_files(path) {
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
                DefinitionKind::Constant => "constant",
                DefinitionKind::InstanceVariable => "ivar",
            };
            println!(
                "{}\t{}\t{}:{}",
                def.fqn,
                kind,
                file_path.display(),
                line + 1
            );
        }
    }
}
