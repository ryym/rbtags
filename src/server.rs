use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

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
use crate::workspace::{LocationInfo, WorkspaceIndex};

const LOG_PATH: &str = "/tmp/rbtags.log";

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

fn location_to_lsp(loc: &LocationInfo) -> Option<Location> {
    let uri = path_to_uri(&loc.path)?;
    let pos = Position::new(loc.line, loc.col);
    Some(Location::new(uri, Range::new(pos, pos)))
}

fn def_kind_to_symbol_kind(kind: &indexer::DefinitionKind) -> SymbolKind {
    match kind {
        indexer::DefinitionKind::Module => SymbolKind::MODULE,
        indexer::DefinitionKind::Class => SymbolKind::CLASS,
        indexer::DefinitionKind::Method => SymbolKind::METHOD,
        indexer::DefinitionKind::Constant => SymbolKind::CONSTANT,
        indexer::DefinitionKind::InstanceVariable => SymbolKind::FIELD,
    }
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

    log(format_args!(
        "indexed {} definitions across {} FQNs",
        index.definition_count(),
        index.fqn_count()
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
                let req =
                    match req.extract::<lsp_types::GotoDefinitionParams>("rbtags/bestDefinition") {
                        Ok((id, params)) => {
                            let result = handle_best_definition(&index, &params);
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

    let results = index.search(query);
    log(format_args!("  found {} symbol(s)", results.len()));

    if results.is_empty() {
        return None;
    }

    let symbols: Vec<_> = results
        .into_iter()
        .filter_map(|(fqn, loc)| {
            Some(SymbolInformation {
                name: fqn.to_string(),
                kind: def_kind_to_symbol_kind(&loc.kind),
                tags: None,
                #[allow(deprecated)] // The field is deprecated in favor of tags
                deprecated: None,
                location: location_to_lsp(loc)?,
                container_name: None,
            })
        })
        .collect();

    if symbols.is_empty() {
        None
    } else {
        Some(WorkspaceSymbolResponse::Flat(symbols))
    }
}

fn resolve_definition_locations(
    index: &WorkspaceIndex,
    params: &lsp_types::GotoDefinitionParams,
) -> Vec<Location> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    log(format_args!(
        "definition: uri={} line={} char={}",
        uri.as_str(),
        position.line,
        position.character
    ));

    let file_path = uri_to_path(uri);
    log(format_args!("  file_path: {file_path:?}"));
    let Some(file_path) = file_path else {
        return Vec::new();
    };

    let source = match fs::read(&file_path) {
        Ok(s) => s,
        Err(e) => {
            log(format_args!("  failed to read file: {e}"));
            return Vec::new();
        }
    };

    let line_index = LineIndex::new(&source);
    let offset = line_index.offset(position.line as usize, position.character as usize);
    log(format_args!("  byte offset: {offset}"));

    let reference = resolver::resolve_reference(&source, offset);
    log(format_args!("  resolved reference: {reference:?}"));
    let Some(reference) = reference else {
        return Vec::new();
    };

    // Local variables are resolved directly (no workspace index lookup).
    if let Reference::LocalVariable {
        definition_offset, ..
    } = &reference
    {
        let (line, col) = line_index.line_col(*definition_offset);
        let uri = path_to_uri(&file_path);
        let locations: Vec<_> = uri
            .map(|uri| {
                let pos = Position::new(line as u32, col as u32);
                Location::new(uri, Range::new(pos, pos))
            })
            .into_iter()
            .collect();
        log(format_args!("  found {} location(s)", locations.len()));
        return locations;
    }

    let raw_locations = match &reference {
        Reference::Constant { .. } => index.lookup_constant(&reference, &file_path),
        Reference::Method { .. } => index.lookup_method(&reference, &file_path),
        Reference::InstanceVariable { .. } => {
            index.lookup_instance_variable(&reference, &file_path)
        }
        Reference::LocalVariable { .. } => unreachable!(),
    };

    let locations: Vec<_> = raw_locations
        .iter()
        .filter_map(|loc| location_to_lsp(loc))
        .collect();

    log(format_args!("  found {} location(s)", locations.len()));
    for loc in &locations {
        log(format_args!(
            "    -> {} {}:{}",
            loc.uri.as_str(),
            loc.range.start.line,
            loc.range.start.character
        ));
    }

    locations
}

fn handle_goto_definition(
    index: &WorkspaceIndex,
    params: &lsp_types::GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
    let locations = resolve_definition_locations(index, params);
    if locations.is_empty() {
        None
    } else {
        Some(GotoDefinitionResponse::Array(locations))
    }
}

fn handle_best_definition(
    index: &WorkspaceIndex,
    params: &lsp_types::GotoDefinitionParams,
) -> Option<Location> {
    resolve_definition_locations(index, params)
        .into_iter()
        .next()
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
