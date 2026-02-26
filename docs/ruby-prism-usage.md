# ruby-prism Rust API Usage Guide

This document covers the `ruby-prism` crate API as used in this project.
For the full generated API documentation, run:

```sh
cargo doc --open
```

The docs will be available at `target/doc/ruby_prism/index.html`.

## Crate version

1.9.0 (see `Cargo.toml`)

## Parsing

```rust
use ruby_prism::parse;

let source = b"module Foo; end";
let result = parse(source);
let root = result.node(); // Node::ProgramNode
```

- Input is `&[u8]` (byte slice).
- Returns `ParseResult<'pr>` which owns the AST. All node references are borrowed from it.
- `ParseResult` is `!Send` and `!Sync` (cannot share across threads), but each file can be parsed independently.

### ParseResult methods

| Method                | Return Type   | Description                     |
| --------------------- | ------------- | ------------------------------- |
| `node()`              | `Node<'_>`    | Root AST node (ProgramNode)     |
| `source()`            | `&[u8]`       | Original source bytes           |
| `errors()`            | `Diagnostics` | Parse errors (iterable)         |
| `as_slice(&Location)` | `&[u8]`       | Extract source text by location |

## Node enum

`Node<'pr>` is a large enum with a variant for every AST node type. Pattern match to dispatch:

```rust
match node {
    Node::ModuleNode(n) => { /* ... */ }
    Node::ClassNode(n) => { /* ... */ }
    Node::DefNode(n) => { /* ... */ }
    _ => {}
}
```

There is no built-in visitor trait. Recursive matching on the `Node` enum works well.

## Key node types

### ProgramNode (root)

| Method         | Return Type      | Description          |
| -------------- | ---------------- | -------------------- |
| `statements()` | `StatementsNode` | Top-level statements |

### StatementsNode

| Method   | Return Type | Description         |
| -------- | ----------- | ------------------- |
| `body()` | `NodeList`  | List of child nodes |

`NodeList` is iterable: `for node in &node_list` or `.iter()`.

### ModuleNode / ClassNode

Both share a similar structure:

| Method            | Return Type    | Description                                      |
| ----------------- | -------------- | ------------------------------------------------ |
| `name()`          | `ConstantId`   | Simple name (e.g., `"Bar"`)                      |
| `constant_path()` | `Node`         | Name node (ConstantReadNode or ConstantPathNode) |
| `body()`          | `Option<Node>` | Body (typically StatementsNode)                  |
| `location()`      | `Location`     | Source location                                  |

ClassNode additionally has:

| Method         | Return Type    | Description  |
| -------------- | -------------- | ------------ |
| `superclass()` | `Option<Node>` | Parent class |

### DefNode

| Method         | Return Type              | Description                               |
| -------------- | ------------------------ | ----------------------------------------- |
| `name()`       | `ConstantId`             | Method name                               |
| `receiver()`   | `Option<Node>`           | Receiver (e.g., `self` in `def self.foo`) |
| `body()`       | `Option<Node>`           | Method body                               |
| `parameters()` | `Option<ParametersNode>` | Parameters                                |
| `location()`   | `Location`               | Source location                           |

### ConstantReadNode (e.g., `Foo`)

| Method   | Return Type  | Description   |
| -------- | ------------ | ------------- |
| `name()` | `ConstantId` | Constant name |

### ConstantPathNode (e.g., `Foo::Bar`)

| Method     | Return Type          | Description                           |
| ---------- | -------------------- | ------------------------------------- |
| `parent()` | `Option<Node>`       | Left side (e.g., `Foo` in `Foo::Bar`) |
| `name()`   | `Option<ConstantId>` | Right side name (e.g., `"Bar"`)       |

### SingletonClassNode (`class << self`)

| Method         | Return Type    | Description                        |
| -------------- | -------------- | ---------------------------------- |
| `expression()` | `Node`         | The receiver (e.g., `self`)        |
| `body()`       | `Option<Node>` | Body containing method definitions |
| `location()`   | `Location`     | Source location                    |

## ConstantId (name handle)

| Method       | Return Type | Description   |
| ------------ | ----------- | ------------- |
| `as_slice()` | `&[u8]`     | Name as bytes |

Convert to `&str`:

```rust
std::str::from_utf8(id.as_slice()).unwrap()
```

## Location (source position)

| Method           | Return Type | Description                   |
| ---------------- | ----------- | ----------------------------- |
| `start_offset()` | `usize`     | Byte offset from source start |
| `end_offset()`   | `usize`     | Byte offset end               |
| `as_slice()`     | `&[u8]`     | Source text at this range     |

**No line/column API** — only byte offsets are provided.
For LSP we need to compute line/column ourselves (see `location.rs`).

## AST structure examples

### `module Foo::Bar`

```
ModuleNode
  constant_path: ConstantPathNode
    parent: Some(ConstantReadNode { name: "Foo" })
    name: Some("Bar")
  name: "Bar"
```

### `module Foo; module Bar; end; end`

```
ModuleNode { name: "Foo" }
  constant_path: ConstantReadNode { name: "Foo" }
  body: StatementsNode
    body: [
      ModuleNode { name: "Bar" }
        constant_path: ConstantReadNode { name: "Bar" }
    ]
```

The inner `Bar` node has **no knowledge** of the outer `Foo`.
We must track the namespace stack ourselves during traversal.

### `class Foo::Bar < Base`

```
ClassNode
  constant_path: ConstantPathNode
    parent: Some(ConstantReadNode { name: "Foo" })
    name: Some("Bar")
  superclass: Some(ConstantReadNode { name: "Base" })
```

## Performance notes

Prism is a C parser with Rust FFI bindings, so parsing is fast.
`ParseResult` is `!Send` / `!Sync`, but we can parse each file on a separate thread and collect results afterward.
