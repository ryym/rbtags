# rbtags - Implementation Plan

## Architecture overview

```
src/
  lib.rs        -- Library root, public API
  indexer.rs    -- Core: parse Ruby source and extract definitions
  location.rs   -- Byte offset to line/column conversion
  main.rs       -- CLI entry point (later: LSP server)
```

The core logic lives in the library crate so it can be unit-tested independently of the CLI/LSP layer.

## Data model

```rust
enum DefinitionKind {
    Module,
    Class,
    Method,
}

struct Definition {
    fqn: String,            // e.g., "Foo::Bar", "Foo::Bar#baz"
    kind: DefinitionKind,
    offset: usize,          // byte offset in source
}

// Per-file result
struct FileIndex {
    path: PathBuf,
    definitions: Vec<Definition>,
}
```

## How the Prism AST maps to FQNs

The key challenge is that `module Foo; module Bar` produces two separate `ModuleNode`s where the inner one only knows its own name `Bar`.
We resolve this by maintaining a namespace stack during recursive AST traversal.

### AST traversal algorithm

```
fn visit(node, namespace_stack) -> Vec<Definition>:
    match node:
        ProgramNode:
            visit(node.statements(), [])

        StatementsNode:
            for child in node.body():
                visit(child, namespace_stack)

        ModuleNode | ClassNode:
            // 1. Resolve the name parts from constant_path
            parts = resolve_constant_path(node.constant_path())
            //   "module Foo"       -> ["Foo"]
            //   "class Foo::Bar"   -> ["Foo", "Bar"]

            // 2. Build FQN by combining stack + parts
            fqn = join(namespace_stack + parts, "::")

            // 3. Record the definition
            record(fqn, node.location())

            // 4. Recurse into body with the full FQN as new stack
            visit(node.body(), namespace_stack + parts)

        DefNode:
            method_name = node.name()
            owner_fqn = join(namespace_stack, "::")
            record(owner_fqn + "#" + method_name, node.location())

fn resolve_constant_path(node) -> Vec<String>:
    match node:
        ConstantReadNode:
            [node.name()]
        ConstantPathNode:
            parent_parts = match node.parent():
                Some(p) => resolve_constant_path(p)
                None => []  // leading "::" (absolute)
            parent_parts + [node.name()]
```

### Example: how different patterns produce the same FQN

**Nested form:**

```ruby
module Foo      # stack=[], parts=["Foo"]       -> FQN "Foo"
  module Bar    # stack=["Foo"], parts=["Bar"]   -> FQN "Foo::Bar"
  end
end
```

**Inline form:**

```ruby
class Foo::Bar  # stack=[], parts=["Foo","Bar"]  -> FQN "Foo::Bar"
end
```

Both produce `Foo::Bar`.

## Byte offset to line/column

Prism's `Location` only provides byte offsets (`start_offset`, `end_offset`).
LSP requires 0-based line and character (UTF-16 code unit offset).

Approach:

1. Pre-compute a line start offset table: scan source bytes for `\n`, record each line's start offset.
2. For a given byte offset, binary search the table to find the line number.
3. Compute the column as the difference from the line start.

This is a well-understood problem and can be implemented and tested independently.

## Implementation steps

### Step 1: Core indexer (`indexer.rs`)

The heart of the project. Parse a single Ruby source string and return a list of definitions with FQNs and byte offsets.

```rust
pub fn index_source(source: &[u8]) -> Vec<Definition>
```

**Test strategy**: Unit tests with Ruby source string literals as input.

```rust
#[test]
fn nested_modules() {
    let defs = index_source(b"module Foo\n  module Bar\n  end\nend");
    assert_eq!(defs[0].fqn, "Foo");
    assert_eq!(defs[1].fqn, "Foo::Bar");
}

#[test]
fn inline_constant_path() {
    let defs = index_source(b"class Foo::Bar < Base\nend");
    assert_eq!(defs[0].fqn, "Foo::Bar");
}

#[test]
fn methods_in_class() {
    let defs = index_source(b"class Foo\n  def bar\n  end\nend");
    assert_eq!(defs[1].fqn, "Foo#bar");
}

#[test]
fn singleton_method() {
    let defs = index_source(b"class Foo\n  def self.bar\n  end\nend");
    assert_eq!(defs[1].fqn, "Foo.bar");
}

#[test]
fn deeply_nested() {
    let defs = index_source(b"module A\n  module B\n    class C\n    end\n  end\nend");
    let fqns: Vec<&str> = defs.iter().map(|d| d.fqn.as_str()).collect();
    assert_eq!(fqns, ["A", "A::B", "A::B::C"]);
}

#[test]
fn mixed_inline_and_nested() {
    let defs = index_source(b"module A\n  class B::C\n  end\nend");
    let fqns: Vec<&str> = defs.iter().map(|d| d.fqn.as_str()).collect();
    assert_eq!(fqns, ["A", "A::B::C"]);
}

#[test]
fn reopened_class() {
    let src = b"class Foo\n  def a\n  end\nend\nclass Foo\n  def b\n  end\nend";
    let defs = index_source(src);
    // Both definitions of Foo and both methods should appear
    assert!(defs.iter().any(|d| d.fqn == "Foo#a"));
    assert!(defs.iter().any(|d| d.fqn == "Foo#b"));
}
```

This is the most important step. Get this right and well-tested before moving on.

### Step 2: Line/column conversion (`location.rs`)

```rust
pub struct LineIndex { /* line start offsets */ }

impl LineIndex {
    pub fn new(source: &[u8]) -> Self;
    pub fn line_col(&self, offset: usize) -> (usize, usize);
}
```

**Test strategy**: Unit tests with known source strings.

### Step 3: CLI dump tool (`main.rs`)

- Accept a directory or file path as argument
- Recursively find `.rb` files
- Run `index_source` on each
- Print results in a readable format: `FQN\tKIND\tFILE:LINE`

**Purpose**: Validate against real Ruby/Rails projects by eyeballing the output.
No automated tests needed at this stage; the core logic is already covered by Step 1 tests.

Example usage:

```
$ cargo run -- /path/to/rails/app
Foo             module  app/models/foo.rb:1
Foo::Bar        class   app/models/foo/bar.rb:3
Foo::Bar#save   method  app/models/foo/bar.rb:10
```

### Step 4: LSP server

Add an LSP crate (e.g., `lsp-server` or `tower-lsp`) and implement:

1. **`initialize`**: Scan the workspace, build the full index in memory.
2. **`textDocument/definition`**: Given cursor position, determine the constant reference at that position, look it up in the index, return the location.

The reference-side analysis (Step 4a below) is needed to determine _what symbol_ the cursor is on.

#### Step 4a: Reference-side symbol extraction

Given a cursor position in a Ruby file, determine the fully qualified constant name at that position.

For example, with cursor on `Bar` in `Foo::Bar.new`:

- Parse the file with Prism
- Find the `ConstantPathNode` that spans the cursor offset
- Resolve it to `Foo::Bar`

This is the reverse of the definition-side work: instead of building FQNs from definitions, we build them from references.

This can also be unit-tested independently.

## Test strategy summary

| Layer                | What                        | How                                  |
| -------------------- | --------------------------- | ------------------------------------ |
| `index_source`       | Ruby source -> FQN + offset | Unit tests with string literals      |
| `LineIndex`          | Byte offset -> line/col     | Unit tests                           |
| CLI dump             | File traversal + formatting | Manual verification on real projects |
| LSP definition       | Request -> response         | Manual verification in Vim           |
| Reference extraction | Cursor position -> FQN      | Unit tests with string literals      |

## Dependencies

- `ruby-prism`: Ruby parser (already added)
- `lsp-server` or `tower-lsp`: LSP protocol (added in Step 4)
- No other dependencies expected for the core logic

## Open questions

- **Index persistence format**: JSON? Bincode? Not critical for now; can start with in-memory only.
- **File watching**: For the LSP, should we watch for file changes and re-index? Or just re-scan on command? Can defer this.
- **Singleton class (`class << self`)**: Methods defined inside should be treated as class methods. Need to handle `SingletonClassNode` in the traversal.
