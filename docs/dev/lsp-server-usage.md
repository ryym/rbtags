# Building an LSP Server in Rust with lsp-server

## Crate choice

| Crate        | Version | Role                                              |
| ------------ | ------- | ------------------------------------------------- |
| `lsp-server` | 0.7.8   | Protocol scaffold (stdio transport, message loop) |
| `lsp-types`  | 0.97.0  | Rust type definitions for LSP messages            |

`lsp-server` is maintained by the rust-analyzer team. It provides a synchronous, crossbeam-channel based API.
The alternative `tower-lsp` offers an async/trait-based API, but `lsp-server` is simpler and sufficient for our needs.

For the full API docs, run:

```sh
cargo doc --open
```

## Architecture

```
Client (Vim)  <--stdio-->  lsp-server  <--channels-->  our handler code
```

`lsp-server` manages:

- stdio I/O threads
- LSP protocol framing (Content-Length headers)
- Initialize/shutdown handshake

We manage:

- The message dispatch loop
- Request handling logic

## Core types

### Connection

```rust
pub struct Connection {
    pub sender: Sender<Message>,
    pub receiver: Receiver<Message>,
}
```

Created via `Connection::stdio()`. Returns `(Connection, IoThreads)`.

### Message

```rust
pub enum Message {
    Request(Request),
    Response(Response),
    Notification(Notification),
}
```

### Request / Response

```rust
pub struct Request {
    pub id: RequestId,
    pub method: String,
    pub params: serde_json::Value,
}

pub struct Response {
    pub id: RequestId,
    pub result: Option<serde_json::Value>,
    pub error: Option<ResponseError>,
}
```

`Request` has an `extract(method)` method to deserialize params into a typed struct.

### ExtractError

```rust
pub enum ExtractError<R> {
    MethodMismatch(R),       // Different method, returns the original request
    JsonError { method: String, error: serde_json::Error },
}
```

## Minimal server pattern

Based on the official `goto_def.rs` example from the lsp-server repository.

```rust
use std::error::Error;
use lsp_server::{Connection, ExtractError, Message, Request, RequestId, Response};
use lsp_types::{
    request::GotoDefinition, GotoDefinitionResponse,
    InitializeParams, ServerCapabilities, OneOf,
};

fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    eprintln!("starting LSP server");

    // 1. Create stdio transport
    let (connection, io_threads) = Connection::stdio();

    // 2. Initialize handshake
    let server_capabilities = serde_json::to_value(&ServerCapabilities {
        definition_provider: Some(OneOf::Left(true)),
        ..Default::default()
    }).unwrap();
    let initialization_params = connection.initialize(server_capabilities)?;

    // 3. Main loop
    main_loop(connection, initialization_params)?;

    // 4. Clean up
    io_threads.join()?;
    eprintln!("shutting down server");
    Ok(())
}

fn main_loop(
    connection: Connection,
    params: serde_json::Value,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let _params: InitializeParams = serde_json::from_value(params).unwrap();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                // Dispatch requests
                match cast::<GotoDefinition>(req) {
                    Ok((id, params)) => {
                        let result = Some(GotoDefinitionResponse::Array(Vec::new()));
                        let result = serde_json::to_value(&result).unwrap();
                        let resp = Response { id, result: Some(result), error: None };
                        connection.sender.send(Message::Response(resp))?;
                    }
                    Err(ExtractError::MethodMismatch(_req)) => {
                        // Unknown method — ignore or log
                    }
                    Err(err @ ExtractError::JsonError { .. }) => {
                        panic!("{err:?}");
                    }
                };
            }
            Message::Response(_resp) => {}
            Message::Notification(_not) => {}
        }
    }
    Ok(())
}

fn cast<R>(req: Request) -> Result<(RequestId, R::Params), ExtractError<Request>>
where
    R: lsp_types::request::Request,
    R::Params: serde::de::DeserializeOwned,
{
    req.extract(R::METHOD)
}
```

## Key LSP types (from lsp-types)

### ServerCapabilities

Set `definition_provider: Some(OneOf::Left(true))` to advertise `textDocument/definition` support.

### GotoDefinition request

- Method: `textDocument/definition`
- Params: `GotoDefinitionParams` (contains `TextDocumentPositionParams`)
  - `text_document.uri`: file URI
  - `position.line`: 0-based line
  - `position.character`: 0-based character (UTF-16 code units)
- Response: `GotoDefinitionResponse`
  - `GotoDefinitionResponse::Scalar(Location)` for a single result
  - `GotoDefinitionResponse::Array(Vec<Location>)` for multiple results

### Location

```rust
pub struct Location {
    pub uri: Url,
    pub range: Range,
}

pub struct Range {
    pub start: Position,
    pub end: Position,
}

pub struct Position {
    pub line: u32,      // 0-based
    pub character: u32, // 0-based, UTF-16 code units
}
```

## Debugging

Set environment variable for verbose logging:

```sh
RUST_LOG=lsp_server=debug cargo run
```

## References

- [lsp-server repository](https://github.com/rust-analyzer/lsp-server)
- [lsp-server docs.rs](https://docs.rs/lsp-server/latest/lsp_server/)
- [lsp-types docs.rs](https://docs.rs/lsp-types/latest/lsp_types/)
- [Vendored in rust-analyzer](https://github.com/rust-lang/rust-analyzer/tree/master/lib/lsp-server)
