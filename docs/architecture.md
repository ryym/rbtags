# rbtags - Architecture

## Overview

rbtags is a Ruby-specific definition index tool that runs as an LSP server.
It solves a problem ctags can't: resolving fully qualified constant names (e.g., `Foo::Bar`) to their definitions even when defined in the nested form (`module Foo; module Bar; end; end`).

See [goal.md](goal.md) for motivation.

## Module structure

```
src/
  main.rs       CLI entry point (subcommands: dump, lsp)
  lib.rs        Library root, file collection utility
  indexer.rs    Definition-side: Ruby source -> list of FQN definitions
  resolver.rs   Reference-side: cursor position -> FQN string
  server.rs     LSP server, glues indexer + resolver together
  location.rs   Byte offset <-> line/column conversion
  log.rs        File-based logging (/tmp/rbtags.log)
```

## Two-phase design

The core problem is split into two independent phases:

### Phase 1: Indexing (definition side) — `indexer.rs`

Answers: "What is defined and where?"

Parses Ruby source with Prism and walks the AST to extract definitions (modules, classes, methods) with their fully qualified names (FQNs) and byte offsets.

The key technique is maintaining a **namespace stack** during recursive traversal. When entering a `module Foo; module Bar` nesting, the stack grows to `["Foo", "Bar"]`, so inner definitions are recorded as `Foo::Bar::*`. For inline forms like `class Foo::Bar`, the constant path parts `["Foo", "Bar"]` are all pushed at once.

Input: `&[u8]` (Ruby source) → Output: `Vec<Definition>` (FQN + kind + byte offset)

### Phase 2: Resolution (reference side) — `resolver.rs`

Answers: "What constant is the cursor on?"

Given a cursor byte offset in a Ruby file, parses the file with Prism, walks the AST to find the `ConstantPathNode` or `ConstantReadNode` at that position, and returns its fully qualified form as a string.

The key design choice is **widest match**: for `Foo::Bar`, regardless of where in the token the cursor sits, the entire `Foo::Bar` is returned rather than just `Foo` or `Bar`.

Input: `(&[u8], usize)` (source + byte offset) → Output: `Option<String>` (FQN)

### How they connect — `server.rs`

The LSP server ties the two phases together:

```
[Editor] --textDocument/definition--> [server.rs]
  1. Read the file from disk
  2. Convert LSP (line, character) to byte offset  (location.rs)
  3. Resolve cursor position to FQN string          (resolver.rs)
  4. Look up FQN in the workspace index             (WorkspaceIndex, built by indexer.rs)
  5. Return definition locations to editor
```

The workspace index (`WorkspaceIndex`) is built once at startup by scanning all `.rb` files in the workspace root and indexing each with `indexer::index_source`. It stores a `HashMap<String, Vec<LocationInfo>>` mapping FQN → file locations.

## Key design decisions

### AST traversal strategies

`indexer.rs` and `resolver.rs` use different traversal strategies suited to their needs.

- **indexer.rs** uses explicit node matching (`if let` chains) because it only cares about a small set of structural nodes (`ProgramNode`, `StatementsNode`, `ModuleNode`, `ClassNode`, `DefNode`, `SingletonClassNode`). Manual enumeration is sufficient here.
- **resolver.rs** uses ruby-prism's `Visit` trait, which auto-generates traversal over all node types. This is necessary because constant references can appear in any expression context (if/unless, case/when, rescue, blocks, arrays, etc.). The `Visit` trait eliminates the need to manually enumerate every possible containing node type. See [ruby-prism-usage.md](ruby-prism-usage.md#no-generic-child-iteration) for background on why generic iteration isn't available otherwise.

### Static analysis only

Like ctags, rbtags relies purely on static source analysis. There is no runtime evaluation, type inference, or understanding of dynamic definitions (`define_method`, `class_eval`, Rails DSLs, etc.).

### Byte offset as the common currency

Prism provides only byte offsets (no line/column). LSP requires 0-based line and character. `location::LineIndex` bridges this gap by pre-computing a line start offset table and using binary search for conversion.
