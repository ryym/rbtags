# rbtags - Goal

## Problem

When developing in Ruby/Rails, ctags-based definition jump in Vim is the primary navigation tool.
LSP (e.g., ruby-lsp) tends to be heavy on large repositories without providing significantly better UX for this particular use case.

However, ctags has a fundamental limitation as a language-agnostic tool: it cannot properly handle Ruby's nested module/class definitions.

### The core issue

Given a reference like `Foo::Bar`, ctags can jump to:

```ruby
class Foo::Bar  # Works - ctags recognizes "Foo::Bar" as a tag
end
```

But fails with the nested form:

```ruby
module Foo
  module Bar  # ctags only sees "Bar", not "Foo::Bar"
  end
end
```

When `Bar` exists under multiple namespaces (e.g., `A::Bar`, `B::Bar`, `Foo::Bar`), ctags offers all of them as candidates with no way to distinguish.

(To be precise, this is not the solo ctags issue. It has `--extras=+q` for qualified names but Vim's default tag jump doesn't handle it well)

## Goal

Build a Ruby-specific definition index tool, ideally as an LSP server, that resolves this problem.

### Primary objective

**Accurate definition jump for fully qualified constants**: When the cursor is on `Foo::Bar`,
jump directly to the definition of `Foo::Bar` regardless of how it's defined (inline `class Foo::Bar` or nested `module Foo; module Bar`).

### Scope and non-goals

- **Static analysis only**: Like ctags, rely purely on static source analysis. No runtime evaluation, no type inference.
- **No dynamic definitions**: `define_method`, `class_eval`, Rails DSLs like `has_many` are out of scope.
- **Imperfect is fine**: When the reference is just `Bar` (without qualifying namespace), disambiguation may not be possible. That's acceptable.
- **Definition jump first**: Code completion, hover information, and other LSP features are secondary.
- **Method definitions**: Track which methods belong to which class/module (e.g., `Foo::Bar#baz`), but method call resolution on the reference side is hard without type inference, so improvement over ctags is limited here.

## Technical approach

- **Language**: Rust
- **Parser**: [Prism](https://github.com/ruby/prism) via its Rust binding (`ruby-prism` crate). Prism is the official Ruby parser, so syntax coverage is guaranteed.
- **Index**: Full scan of all `.rb` files, building a map of fully qualified names to source locations. Stored in a file. No incremental/differential updates needed initially.
- **Interface**: Start as a CLI tool, then wrap as an LSP server (`textDocument/definition`).
