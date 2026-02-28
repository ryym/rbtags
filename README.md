# rbtags

A Ruby definition index tool built with [Prism](https://github.com/ruby/prism). Resolves fully qualified names for nested modules/classes, which ctags can't do.

Work in progress.

## Usage

```sh
# Dump definitions
cargo run -- dump path/to/ruby/files

# Start LSP server (textDocument/definition, workspace/symbol)
cargo run -- lsp
```
