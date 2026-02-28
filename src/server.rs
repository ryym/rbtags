use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{fs, io};

use lsp_server::{Connection, ExtractError, Message, Request, RequestId, Response};
use lsp_types::{
    request::GotoDefinition, GotoDefinitionResponse, InitializeParams, Location, OneOf, Position,
    Range, ServerCapabilities, Uri,
};

use crate::indexer;
use crate::location::LineIndex;
use crate::log::write as log;
use crate::resolver;

const LOG_PATH: &str = "/tmp/rbtags.log";

struct LocationInfo {
    path: PathBuf,
    line: u32,
    col: u32,
}

struct WorkspaceIndex {
    definitions: HashMap<String, Vec<LocationInfo>>,
}

impl WorkspaceIndex {
    fn build(root: &Path) -> io::Result<Self> {
        let rb_files = crate::collect_rb_files(root)?;
        log(format_args!("found {} .rb files", rb_files.len()));

        let mut definitions: HashMap<String, Vec<LocationInfo>> = HashMap::new();

        for file_path in &rb_files {
            let source = match fs::read(file_path) {
                Ok(s) => s,
                Err(e) => {
                    log(format_args!("warning: failed to read {}: {e}", file_path.display()));
                    continue;
                }
            };

            let defs = indexer::index_source(&source);
            if defs.is_empty() {
                continue;
            }

            let line_index = LineIndex::new(&source);
            for def in &defs {
                let (line, col) = line_index.line_col(def.offset);
                definitions
                    .entry(def.fqn.clone())
                    .or_default()
                    .push(LocationInfo {
                        path: file_path.clone(),
                        line: line as u32,
                        col: col as u32,
                    });
            }
        }

        Ok(Self { definitions })
    }

    fn lookup(&self, fqn: &str) -> Vec<Location> {
        let Some(locations) = self.definitions.get(fqn) else {
            return Vec::new();
        };
        locations
            .iter()
            .filter_map(|loc| {
                let uri = path_to_uri(&loc.path)?;
                let pos = Position::new(loc.line, loc.col);
                Some(Location::new(uri, Range::new(pos, pos)))
            })
            .collect()
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
        .or_else(|| {
            #[allow(deprecated)]
            params.root_uri.as_ref().and_then(uri_to_path)
        })
        .or_else(|| {
            #[allow(deprecated)]
            params.root_path.as_ref().map(PathBuf::from)
        });

    log(format_args!("root_path: {root_path:?}"));

    let index = match root_path {
        Some(root) => {
            log(format_args!("indexing {}", root.display()));
            WorkspaceIndex::build(&root)?
        }
        None => {
            log(format_args!("no workspace root, index will be empty"));
            WorkspaceIndex {
                definitions: HashMap::new(),
            }
        }
    };

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
                match cast::<GotoDefinition>(req) {
                    Ok((id, params)) => {
                        let result = handle_goto_definition(&index, &params);
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
            }
        }
    }
    Ok(())
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

    let fqn = resolver::resolve_reference(&source, offset);
    log(format_args!("  resolved FQN: {fqn:?}"));
    let fqn = fqn?;

    let locations = index.lookup(&fqn);
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
