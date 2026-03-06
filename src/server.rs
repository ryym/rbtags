use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{fs, io};

use rayon::prelude::*;

use lsp_server::{Connection, ExtractError, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    GotoDefinitionResponse, InitializeParams, Location, OneOf, Position, Range, ServerCapabilities,
    SymbolInformation, SymbolKind, TextDocumentSyncCapability, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, Uri, WorkspaceSymbolResponse, notification::DidSaveTextDocument,
    request::GotoDefinition, request::WorkspaceSymbolRequest,
};

use crate::indexer;
use crate::location::LineIndex;
use crate::log::write as log;
use crate::resolver::{self, Reference};

const LOG_PATH: &str = "/tmp/rbtags.log";

struct LocationInfo {
    path: PathBuf,
    kind: indexer::DefinitionKind,
    line: u32,
    col: u32,
}

struct WorkspaceIndex {
    definitions: HashMap<String, Vec<LocationInfo>>,
}

impl WorkspaceIndex {
    fn new() -> Self {
        Self {
            definitions: HashMap::new(),
        }
    }

    fn build(root: &Path) -> io::Result<Self> {
        let t_start = std::time::Instant::now();
        let rb_files = crate::collect_rb_files(root)?;
        log(format_args!("found {} .rb files", rb_files.len()));

        let file_results: Vec<_> = rb_files
            .par_iter()
            .filter_map(|file_path| {
                let source = match fs::read(file_path) {
                    Ok(s) => s,
                    Err(e) => {
                        log(format_args!(
                            "warning: failed to read {}: {e}",
                            file_path.display()
                        ));
                        return None;
                    }
                };
                let defs = indexer::index_source(&source);
                if defs.is_empty() {
                    return None;
                }
                let line_index = crate::location::LineIndex::new(&source);
                let locations: Vec<_> = defs
                    .into_iter()
                    .map(|def| {
                        let (line, col) = line_index.line_col(def.offset);
                        (
                            def.fqn,
                            LocationInfo {
                                path: file_path.to_owned(),
                                kind: def.kind,
                                line: line as u32,
                                col: col as u32,
                            },
                        )
                    })
                    .collect();
                Some(locations)
            })
            .collect();

        let mut index = Self::new();
        for file_locs in file_results {
            for (fqn, loc) in file_locs {
                index.definitions.entry(fqn).or_default().push(loc);
            }
        }

        log(format_args!("build completed in {:?}", t_start.elapsed()));

        Ok(index)
    }

    fn index_file(&mut self, path: &Path, source: &[u8]) {
        let defs = indexer::index_source(source);
        if defs.is_empty() {
            return;
        }

        let line_index = LineIndex::new(source);
        for def in defs {
            let (line, col) = line_index.line_col(def.offset);
            self.definitions
                .entry(def.fqn)
                .or_default()
                .push(LocationInfo {
                    path: path.to_owned(),
                    kind: def.kind,
                    line: line as u32,
                    col: col as u32,
                });
        }
    }

    fn remove_file(&mut self, path: &Path) {
        self.definitions.retain(|_fqn, locations| {
            locations.retain(|loc| loc.path != *path);
            !locations.is_empty()
        });
    }

    fn update_file(&mut self, path: &Path) {
        self.remove_file(path);
        match fs::read(path) {
            Ok(source) => {
                self.index_file(path, &source);
                log(format_args!("re-indexed {}", path.display()));
            }
            Err(e) => {
                log(format_args!(
                    "warning: failed to read {}: {e}",
                    path.display()
                ));
            }
        }
    }

    fn lookup_constant(&self, reference: &Reference, cursor_file: &Path) -> Vec<Location> {
        let Reference::Constant { name, namespace } = reference else {
            return Vec::new();
        };

        let mut candidates: Vec<(&LocationInfo, &str)> = Vec::new();

        // Tier 1: Nesting-aware resolution (Ruby's constant lookup order).
        // For name="Bar" with namespace=["A","B"], try "A::B::Bar", "A::Bar", "Bar".
        let nesting_fqns = build_nesting_candidates(name, namespace);
        for candidate_fqn in &nesting_fqns {
            if let Some(locations) = self.definitions.get(candidate_fqn.as_str()) {
                for loc in locations {
                    candidates.push((loc, candidate_fqn));
                }
            }
        }

        if !candidates.is_empty() {
            let fqn_order: HashMap<&str, usize> = nesting_fqns
                .iter()
                .enumerate()
                .map(|(i, fqn)| (fqn.as_str(), i))
                .collect();

            candidates.sort_by_key(|(loc, fqn)| {
                let nesting_rank = fqn_order.get(fqn).copied().unwrap_or(usize::MAX);
                (nesting_rank, file_distance(&loc.path, cursor_file))
            });

            return candidates
                .iter()
                .filter_map(|(loc, _)| {
                    let uri = path_to_uri(&loc.path)?;
                    let pos = Position::new(loc.line, loc.col);
                    Some(Location::new(uri, Range::new(pos, pos)))
                })
                .collect();
        }

        // Tier 2: Suffix match fallback — any FQN ending with ::{name} or equal to {name}.
        let suffix = format!("::{name}");
        for (fqn, locations) in &self.definitions {
            if fqn.ends_with(&suffix) || fqn == name {
                for loc in locations {
                    candidates.push((loc, fqn));
                }
            }
        }

        candidates.sort_by_key(|(loc, _fqn)| file_distance(&loc.path, cursor_file));

        candidates
            .iter()
            .filter_map(|(loc, _)| {
                let uri = path_to_uri(&loc.path)?;
                let pos = Position::new(loc.line, loc.col);
                Some(Location::new(uri, Range::new(pos, pos)))
            })
            .collect()
    }

    fn lookup_method(&self, reference: &Reference, cursor_file: &Path) -> Vec<Location> {
        let Reference::Method {
            name,
            receiver,
            namespace,
        } = reference
        else {
            return Vec::new();
        };

        let instance_suffix = format!("#{name}");
        let class_suffix = format!(".{name}");

        // Collect all method definitions matching the name
        let mut candidates: Vec<(&LocationInfo, &str)> = Vec::new();
        for (fqn, locations) in &self.definitions {
            if fqn.ends_with(&instance_suffix) || fqn.ends_with(&class_suffix) {
                for loc in locations {
                    if loc.kind == indexer::DefinitionKind::Method {
                        candidates.push((loc, fqn));
                    }
                }
            }
        }

        if candidates.is_empty() {
            return Vec::new();
        }

        // Score each candidate for priority sorting
        let cursor_ns = namespace.join("::");
        let guessed_class = match receiver {
            resolver::MethodReceiver::Variable(var) => Some(snake_to_camel(var)),
            _ => None,
        };

        candidates.sort_by_key(|(loc, fqn)| {
            score_method_candidate(
                fqn,
                loc,
                receiver,
                &cursor_ns,
                guessed_class.as_deref(),
                cursor_file,
            )
        });

        candidates
            .iter()
            .filter_map(|(loc, _fqn)| {
                let uri = path_to_uri(&loc.path)?;
                let pos = Position::new(loc.line, loc.col);
                Some(Location::new(uri, Range::new(pos, pos)))
            })
            .collect()
    }

    fn search(&self, query: &str) -> Vec<SymbolInformation> {
        let mut results = Vec::new();
        for (fqn, locations) in &self.definitions {
            if !fqn.contains(query) {
                continue;
            }
            for loc in locations {
                let Some(uri) = path_to_uri(&loc.path) else {
                    continue;
                };
                let pos = Position::new(loc.line, loc.col);
                #[allow(deprecated)] // `deprecated` field is deprecated in favor of tags
                results.push(SymbolInformation {
                    name: fqn.clone(),
                    kind: def_kind_to_symbol_kind(&loc.kind),
                    tags: None,
                    deprecated: None,
                    location: Location::new(uri, Range::new(pos, pos)),
                    container_name: None,
                });
            }
        }
        results
    }
}

/// Build candidate FQNs by walking outward from the current namespace.
/// For name="Bar" with namespace=["A","B"], returns ["A::B::Bar", "A::Bar", "Bar"].
fn build_nesting_candidates(name: &str, namespace: &[String]) -> Vec<String> {
    let mut candidates = Vec::new();
    for i in (0..=namespace.len()).rev() {
        let prefix = &namespace[..i];
        let candidate = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}::{}", prefix.join("::"), name)
        };
        candidates.push(candidate);
    }
    candidates
}

/// Lower score = higher priority.
fn score_method_candidate(
    fqn: &str,
    loc: &LocationInfo,
    receiver: &resolver::MethodReceiver,
    cursor_ns: &str,
    guessed_class: Option<&str>,
    cursor_file: &Path,
) -> (u8, u32) {
    // Priority tier (lower = better)
    let tier = match receiver {
        // Constant receiver: exact FQN match (e.g., User.find → User.find)
        resolver::MethodReceiver::Constant(constant) => {
            if fqn.starts_with(constant) {
                0
            } else {
                4
            }
        }
        // self.bar or bare bar: prioritize current namespace
        resolver::MethodReceiver::SelfRef | resolver::MethodReceiver::None => {
            if !cursor_ns.is_empty() && fqn.starts_with(cursor_ns) {
                1
            } else {
                4
            }
        }
        // Variable receiver: guess class from variable name
        resolver::MethodReceiver::Variable(_) => {
            if let Some(class) = guessed_class {
                if fqn.starts_with(class) { 2 } else { 4 }
            } else {
                4
            }
        }
    };

    // File distance as tiebreaker
    let distance = file_distance(&loc.path, cursor_file);

    (tier, distance)
}

fn file_distance(a: &Path, b: &Path) -> u32 {
    if a == b {
        return 0;
    }
    if a.parent() == b.parent() {
        return 1;
    }
    // Count common prefix components
    let a_components: Vec<_> = a.components().collect();
    let b_components: Vec<_> = b.components().collect();
    let common = a_components
        .iter()
        .zip(b_components.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let max_len = a_components.len().max(b_components.len());
    (max_len - common) as u32
}

fn snake_to_camel(s: &str) -> String {
    s.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    upper + chars.as_str()
                }
                None => String::new(),
            }
        })
        .collect()
}

fn def_kind_to_symbol_kind(kind: &indexer::DefinitionKind) -> SymbolKind {
    match kind {
        indexer::DefinitionKind::Module => SymbolKind::MODULE,
        indexer::DefinitionKind::Class => SymbolKind::CLASS,
        indexer::DefinitionKind::Method => SymbolKind::METHOD,
        indexer::DefinitionKind::Constant => SymbolKind::CONSTANT,
    }
}

fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let s = uri.as_str();
    let path = s.strip_prefix("file://")?;
    Some(PathBuf::from(path))
}

fn path_to_uri(path: &Path) -> Option<Uri> {
    let abs = fs::canonicalize(path).ok()?;
    let uri_str = format!("file://{}", abs.display());
    Uri::from_str(&uri_str).ok()
}

pub fn run() -> Result<(), Box<dyn Error + Sync + Send>> {
    crate::log::init(LOG_PATH);
    log(format_args!("starting LSP server"));

    let (connection, io_threads) = Connection::stdio();

    let server_capabilities = serde_json::to_value(ServerCapabilities {
        definition_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..Default::default()
            },
        )),
        ..Default::default()
    })?;
    log(format_args!("server capabilities: {server_capabilities}"));
    let init_params = connection.initialize(server_capabilities)?;
    log(format_args!("initialize params: {init_params}"));

    main_loop(connection, init_params)?;
    io_threads.join()?;

    log(format_args!("server shut down"));
    Ok(())
}

fn main_loop(
    connection: Connection,
    params: serde_json::Value,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let params: InitializeParams = serde_json::from_value(params)?;

    let root_path = params
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        .and_then(|folder| uri_to_path(&folder.uri))
        .or_else(|| std::env::current_dir().ok());

    log(format_args!("root_path: {root_path:?}"));

    let root = root_path.expect("failed to determine workspace root");
    log(format_args!("indexing {}", root.display()));
    let mut index = WorkspaceIndex::build(&root)?;

    let def_count: usize = index.definitions.values().map(|v| v.len()).sum();
    log(format_args!(
        "indexed {def_count} definitions across {} FQNs",
        index.definitions.len()
    ));

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                log(format_args!("request: method={} id={}", req.method, req.id));
                if connection.handle_shutdown(&req)? {
                    log(format_args!("shutdown requested"));
                    return Ok(());
                }
                let req = match cast::<GotoDefinition>(req) {
                    Ok((id, params)) => {
                        let result = handle_goto_definition(&index, &params);
                        let result = serde_json::to_value(&result)?;
                        let resp = Response {
                            id,
                            result: Some(result),
                            error: None,
                        };
                        connection.sender.send(Message::Response(resp))?;
                        continue;
                    }
                    Err(ExtractError::MethodMismatch(req)) => req,
                    Err(err @ ExtractError::JsonError { .. }) => {
                        log(format_args!("error extracting request: {err:?}"));
                        continue;
                    }
                };
                match cast::<WorkspaceSymbolRequest>(req) {
                    Ok((id, params)) => {
                        let result = handle_workspace_symbol(&index, &params);
                        let result = serde_json::to_value(&result)?;
                        let resp = Response {
                            id,
                            result: Some(result),
                            error: None,
                        };
                        connection.sender.send(Message::Response(resp))?;
                    }
                    Err(ExtractError::MethodMismatch(_)) => {}
                    Err(err @ ExtractError::JsonError { .. }) => {
                        log(format_args!("error extracting request: {err:?}"));
                    }
                }
            }
            Message::Response(resp) => {
                log(format_args!("response: id={}", resp.id));
            }
            Message::Notification(not) => {
                log(format_args!("notification: method={}", not.method));
                if let Ok(params) = cast_notification::<DidSaveTextDocument>(not)
                    && let Some(path) = uri_to_path(&params.text_document.uri)
                {
                    index.update_file(&path);
                }
            }
        }
    }
    Ok(())
}

fn handle_workspace_symbol(
    index: &WorkspaceIndex,
    params: &lsp_types::WorkspaceSymbolParams,
) -> Option<WorkspaceSymbolResponse> {
    let query = &params.query;
    log(format_args!("workspace/symbol: query={query:?}"));

    let symbols = index.search(query);
    log(format_args!("  found {} symbol(s)", symbols.len()));

    if symbols.is_empty() {
        None
    } else {
        Some(WorkspaceSymbolResponse::Flat(symbols))
    }
}

fn handle_goto_definition(
    index: &WorkspaceIndex,
    params: &lsp_types::GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    log(format_args!(
        "gotoDefinition: uri={} line={} char={}",
        uri.as_str(),
        position.line,
        position.character
    ));

    let file_path = uri_to_path(uri);
    log(format_args!("  file_path: {file_path:?}"));
    let file_path = file_path?;

    let source = match fs::read(&file_path) {
        Ok(s) => s,
        Err(e) => {
            log(format_args!("  failed to read file: {e}"));
            return None;
        }
    };

    let line_index = LineIndex::new(&source);
    let offset = line_index.offset(position.line as usize, position.character as usize);
    log(format_args!("  byte offset: {offset}"));

    let reference = resolver::resolve_reference(&source, offset);
    log(format_args!("  resolved reference: {reference:?}"));
    let reference = reference?;

    let locations = match &reference {
        Reference::Constant { .. } => index.lookup_constant(&reference, &file_path),
        Reference::Method { .. } => index.lookup_method(&reference, &file_path),
    };

    log(format_args!("  found {} location(s)", locations.len()));
    for loc in &locations {
        log(format_args!(
            "    -> {} {}:{}",
            loc.uri.as_str(),
            loc.range.start.line,
            loc.range.start.character
        ));
    }

    if locations.is_empty() {
        None
    } else {
        Some(GotoDefinitionResponse::Array(locations))
    }
}

fn cast<R>(req: Request) -> Result<(RequestId, R::Params), ExtractError<Request>>
where
    R: lsp_types::request::Request,
    R::Params: serde::de::DeserializeOwned,
{
    req.extract(R::METHOD)
}

fn cast_notification<N>(not: Notification) -> Result<N::Params, ExtractError<Notification>>
where
    N: lsp_types::notification::Notification,
    N::Params: serde::de::DeserializeOwned,
{
    not.extract(N::METHOD)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup_fqns(index: &WorkspaceIndex, fqn: &str) -> Vec<(PathBuf, u32, u32)> {
        let Some(locations) = index.definitions.get(fqn) else {
            return Vec::new();
        };
        locations
            .iter()
            .map(|loc| (loc.path.clone(), loc.line, loc.col))
            .collect()
    }

    #[test]
    fn index_file_and_lookup() {
        let mut index = WorkspaceIndex::new();
        let source = b"module Foo\n  class Bar\n  end\nend\n";
        index.index_file(Path::new("a.rb"), source);

        let locs = lookup_fqns(&index, "Foo");
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].0, Path::new("a.rb"));

        let locs = lookup_fqns(&index, "Foo::Bar");
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].0, Path::new("a.rb"));
    }

    #[test]
    fn update_file_replaces_definitions() {
        let mut index = WorkspaceIndex::new();
        let path = Path::new("a.rb");

        index.index_file(path, b"class Foo\nend\n");
        assert_eq!(lookup_fqns(&index, "Foo").len(), 1);

        // Simulate update: remove + re-index with new content
        index.remove_file(path);
        index.index_file(path, b"class Foo\n  def hello\n  end\nend\n");

        let locs = lookup_fqns(&index, "Foo");
        assert_eq!(locs.len(), 1);
        let locs = lookup_fqns(&index, "Foo#hello");
        assert_eq!(locs.len(), 1);
    }

    #[test]
    fn update_file_removes_stale_fqns() {
        let mut index = WorkspaceIndex::new();
        let path = Path::new("a.rb");

        index.index_file(path, b"class Foo\nend\n");
        assert_eq!(lookup_fqns(&index, "Foo").len(), 1);

        index.remove_file(path);
        index.index_file(path, b"class Bar\nend\n");

        assert_eq!(lookup_fqns(&index, "Foo").len(), 0);
        assert_eq!(lookup_fqns(&index, "Bar").len(), 1);
    }

    #[test]
    fn remove_file() {
        let mut index = WorkspaceIndex::new();
        index.index_file(Path::new("a.rb"), b"class Foo\nend\n");
        assert_eq!(lookup_fqns(&index, "Foo").len(), 1);

        index.remove_file(Path::new("a.rb"));
        assert_eq!(lookup_fqns(&index, "Foo").len(), 0);
        assert!(index.definitions.is_empty());
    }

    #[test]
    fn multiple_files_same_fqn() {
        let mut index = WorkspaceIndex::new();
        index.index_file(Path::new("a.rb"), b"class Foo\nend\n");
        index.index_file(Path::new("b.rb"), b"class Foo\nend\n");
        assert_eq!(lookup_fqns(&index, "Foo").len(), 2);

        index.remove_file(Path::new("a.rb"));
        let locs = lookup_fqns(&index, "Foo");
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].0, Path::new("b.rb"));
    }

    // --- Method lookup tests ---

    fn lookup_method_fqns(
        index: &WorkspaceIndex,
        reference: &Reference,
        cursor_file: &Path,
    ) -> Vec<(PathBuf, String)> {
        let Reference::Method {
            name,
            receiver,
            namespace,
        } = reference
        else {
            panic!("expected Method reference");
        };

        let instance_suffix = format!("#{name}");
        let class_suffix = format!(".{name}");

        let cursor_ns = namespace.join("::");
        let guessed_class = match receiver {
            resolver::MethodReceiver::Variable(var) => Some(snake_to_camel(var)),
            _ => None,
        };

        let mut candidates: Vec<(&LocationInfo, &str)> = Vec::new();
        for (fqn, locations) in &index.definitions {
            if fqn.ends_with(&instance_suffix) || fqn.ends_with(&class_suffix) {
                for loc in locations {
                    if loc.kind == indexer::DefinitionKind::Method {
                        candidates.push((loc, fqn));
                    }
                }
            }
        }

        candidates.sort_by_key(|(loc, fqn)| {
            score_method_candidate(
                fqn,
                loc,
                receiver,
                &cursor_ns,
                guessed_class.as_deref(),
                cursor_file,
            )
        });

        candidates
            .iter()
            .map(|(loc, fqn)| (loc.path.clone(), fqn.to_string()))
            .collect()
    }

    #[test]
    fn method_lookup_constant_receiver() {
        let mut index = WorkspaceIndex::new();
        index.index_file(
            Path::new("app/models/user.rb"),
            b"class User\n  def self.find\n  end\nend\n",
        );
        index.index_file(
            Path::new("app/models/post.rb"),
            b"class Post\n  def self.find\n  end\nend\n",
        );

        let reference = Reference::Method {
            name: "find".to_string(),
            receiver: resolver::MethodReceiver::Constant("User".to_string()),
            namespace: vec![],
        };

        let results = lookup_method_fqns(&index, &reference, Path::new("app/controllers/test.rb"));
        assert_eq!(results[0].1, "User.find");
    }

    #[test]
    fn method_lookup_same_namespace() {
        let mut index = WorkspaceIndex::new();
        index.index_file(Path::new("a.rb"), b"class Foo\n  def bar\n  end\nend\n");
        index.index_file(Path::new("b.rb"), b"class Baz\n  def bar\n  end\nend\n");

        let reference = Reference::Method {
            name: "bar".to_string(),
            receiver: resolver::MethodReceiver::None,
            namespace: vec!["Foo".to_string()],
        };

        let results = lookup_method_fqns(&index, &reference, Path::new("c.rb"));
        assert_eq!(results[0].1, "Foo#bar");
    }

    #[test]
    fn method_lookup_variable_guess() {
        let mut index = WorkspaceIndex::new();
        index.index_file(Path::new("a.rb"), b"class User\n  def save\n  end\nend\n");
        index.index_file(Path::new("b.rb"), b"class Post\n  def save\n  end\nend\n");

        let reference = Reference::Method {
            name: "save".to_string(),
            receiver: resolver::MethodReceiver::Variable("user".to_string()),
            namespace: vec![],
        };

        let results = lookup_method_fqns(&index, &reference, Path::new("c.rb"));
        assert_eq!(results[0].1, "User#save");
    }

    #[test]
    fn method_lookup_variable_snake_case() {
        let mut index = WorkspaceIndex::new();
        index.index_file(
            Path::new("a.rb"),
            b"class OrderItem\n  def total\n  end\nend\n",
        );
        index.index_file(
            Path::new("b.rb"),
            b"class Invoice\n  def total\n  end\nend\n",
        );

        let reference = Reference::Method {
            name: "total".to_string(),
            receiver: resolver::MethodReceiver::Variable("order_item".to_string()),
            namespace: vec![],
        };

        let results = lookup_method_fqns(&index, &reference, Path::new("c.rb"));
        assert_eq!(results[0].1, "OrderItem#total");
    }

    #[test]
    fn method_lookup_fallback_returns_all() {
        let mut index = WorkspaceIndex::new();
        index.index_file(Path::new("a.rb"), b"class Foo\n  def bar\n  end\nend\n");
        index.index_file(Path::new("b.rb"), b"class Baz\n  def bar\n  end\nend\n");

        // Unknown receiver - should return all candidates
        let reference = Reference::Method {
            name: "bar".to_string(),
            receiver: resolver::MethodReceiver::None,
            namespace: vec![],
        };

        let results = lookup_method_fqns(&index, &reference, Path::new("c.rb"));
        assert_eq!(results.len(), 2);
    }

    // --- Constant lookup tests ---

    fn lookup_constant_fqns(
        index: &WorkspaceIndex,
        name: &str,
        namespace: &[&str],
        cursor_file: &Path,
    ) -> Vec<(PathBuf, String)> {
        let reference = Reference::Constant {
            name: name.to_string(),
            namespace: namespace.iter().map(|s| s.to_string()).collect(),
        };

        let Reference::Constant {
            name: ref_name,
            namespace: ref_ns,
        } = &reference
        else {
            unreachable!();
        };

        let mut candidates: Vec<(&LocationInfo, &str)> = Vec::new();

        let nesting_fqns = build_nesting_candidates(ref_name, ref_ns);
        for candidate_fqn in &nesting_fqns {
            if let Some(locations) = index.definitions.get(candidate_fqn.as_str()) {
                for loc in locations {
                    candidates.push((loc, candidate_fqn));
                }
            }
        }

        if !candidates.is_empty() {
            let fqn_order: HashMap<&str, usize> = nesting_fqns
                .iter()
                .enumerate()
                .map(|(i, fqn)| (fqn.as_str(), i))
                .collect();

            candidates.sort_by_key(|(loc, fqn)| {
                let nesting_rank = fqn_order.get(fqn).copied().unwrap_or(usize::MAX);
                (nesting_rank, file_distance(&loc.path, cursor_file))
            });

            return candidates
                .iter()
                .map(|(loc, fqn)| (loc.path.clone(), fqn.to_string()))
                .collect();
        }

        let suffix = format!("::{ref_name}");
        for (fqn, locations) in &index.definitions {
            if fqn.ends_with(&suffix) || fqn == ref_name {
                for loc in locations {
                    candidates.push((loc, fqn));
                }
            }
        }

        candidates.sort_by_key(|(loc, _)| file_distance(&loc.path, cursor_file));

        candidates
            .iter()
            .map(|(loc, fqn)| (loc.path.clone(), fqn.to_string()))
            .collect()
    }

    #[test]
    fn constant_lookup_exact_match() {
        let mut index = WorkspaceIndex::new();
        index.index_file(Path::new("a.rb"), b"class Foo\n  class Bar\n  end\nend\n");

        // "Foo::Bar" with no namespace → exact nesting match
        let results = lookup_constant_fqns(&index, "Foo::Bar", &[], Path::new("b.rb"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "Foo::Bar");
    }

    #[test]
    fn constant_lookup_nesting_resolution() {
        let mut index = WorkspaceIndex::new();
        index.index_file(Path::new("a.rb"), b"class Foo\n  class Bar\n  end\nend\n");
        index.index_file(Path::new("b.rb"), b"class Baz\n  class Bar\n  end\nend\n");

        // "Bar" inside namespace ["Foo"] → prefers "Foo::Bar"
        let results = lookup_constant_fqns(&index, "Bar", &["Foo"], Path::new("c.rb"));
        assert_eq!(results[0].1, "Foo::Bar");
    }

    #[test]
    fn constant_lookup_nested_outward() {
        let mut index = WorkspaceIndex::new();
        index.index_file(
            Path::new("a.rb"),
            b"module A\n  class Bar\n  end\n  module B\n    class Bar\n    end\n  end\nend\n",
        );

        // "Bar" inside ["A", "B"] → prefers "A::B::Bar" over "A::Bar"
        let results = lookup_constant_fqns(&index, "Bar", &["A", "B"], Path::new("c.rb"));
        assert_eq!(results[0].1, "A::B::Bar");
        assert_eq!(results[1].1, "A::Bar");
    }

    #[test]
    fn constant_lookup_suffix_fallback() {
        let mut index = WorkspaceIndex::new();
        index.index_file(Path::new("a.rb"), b"class Foo\n  class Bar\n  end\nend\n");

        // "Bar" inside namespace ["X"] (no X::Bar) → falls back to suffix match
        let results = lookup_constant_fqns(&index, "Bar", &["X"], Path::new("c.rb"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, "Foo::Bar");
    }

    #[test]
    fn constant_lookup_already_qualified() {
        let mut index = WorkspaceIndex::new();
        index.index_file(
            Path::new("a.rb"),
            b"module X\n  class Foo\n    class Bar\n    end\n  end\nend\n",
        );
        index.index_file(Path::new("b.rb"), b"class Foo\n  class Bar\n  end\nend\n");

        // "Foo::Bar" inside namespace ["X"] → tries "X::Foo::Bar" first, then "Foo::Bar"
        let results = lookup_constant_fqns(&index, "Foo::Bar", &["X"], Path::new("c.rb"));
        assert_eq!(results[0].1, "X::Foo::Bar");
        assert_eq!(results[1].1, "Foo::Bar");
    }

    #[test]
    fn test_build_nesting_candidates() {
        let ns = vec!["A".to_string(), "B".to_string()];
        let candidates = build_nesting_candidates("Bar", &ns);
        assert_eq!(candidates, vec!["A::B::Bar", "A::Bar", "Bar"]);

        let candidates = build_nesting_candidates("Bar", &[]);
        assert_eq!(candidates, vec!["Bar"]);

        let candidates = build_nesting_candidates("Foo::Bar", &ns);
        assert_eq!(
            candidates,
            vec!["A::B::Foo::Bar", "A::Foo::Bar", "Foo::Bar"]
        );
    }

    // --- Utility tests ---

    #[test]
    fn test_snake_to_camel() {
        assert_eq!(snake_to_camel("user"), "User");
        assert_eq!(snake_to_camel("order_item"), "OrderItem");
        assert_eq!(snake_to_camel("foo_bar_baz"), "FooBarBaz");
    }

    #[test]
    fn test_file_distance_same_file() {
        assert_eq!(file_distance(Path::new("a.rb"), Path::new("a.rb")), 0);
    }

    #[test]
    fn test_file_distance_same_dir() {
        assert_eq!(
            file_distance(Path::new("app/a.rb"), Path::new("app/b.rb")),
            1
        );
    }

    #[test]
    fn test_file_distance_different_dirs() {
        let d1 = file_distance(
            Path::new("app/models/user.rb"),
            Path::new("app/controllers/users.rb"),
        );
        let d2 = file_distance(
            Path::new("app/models/user.rb"),
            Path::new("lib/tasks/seed.rb"),
        );
        assert!(d1 < d2);
    }
}
