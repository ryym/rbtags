# Rbtags

A fast, lightweight Ruby LSP server focused on **go-to-definition** for constants and methods.

## Motivation

[Ctags]-based definition jump can't handle Ruby's nested module/class definitions properly.
For example, when `Bar` is defined inside `module Foo; module Bar; end; end`, Ctags only sees `Bar`, not `Foo::Bar`.
This makes it impossible to distinguish between `A::Bar`, `B::Bar`, and `Foo::Bar`.
[ripper-tags] does a better job but Vim's builtin tag jump doesn't work well for Ruby's module paths anyway.

[Ctags]: https://ctags.io/
[ripper-tags]: https://github.com/tmm1/ripper-tags

Full-featured Ruby LSPs (e.g., ruby-lsp, Solargraph) solve this but tend to be heavy, especially on large repositories.

Rbtags is like a Ruby-specialized Ctags built as LSP.
It focuses on accurate-enough definition jump with fast initialization based on a simple static analysis.

## Trade-offs

- **Static analysis only** — like Ctags, no runtime evaluation or type inference
- **False positives are possible** — when a constant or method name is ambiguous, Rbtags may return multiple candidates (ranked by heuristics like namespace nesting and file distance)
- **No dynamic definitions** — `define_method`, `class_eval`, Rails DSLs (`has_many`, etc.) are not recognized

## Features

- **Constant definition jump** — resolves fully qualified names even for nested definitions
- **Nesting-aware resolution** — `Bar` inside `module Foo` correctly resolves to `Foo::Bar`
- **Method definition jump** — prioritizes by receiver class, enclosing class, and variable name guessing
- **Workspace symbol search** — find definitions by FQN substring
- **File distance ranking** — closer definitions are prioritized among equal candidates

## Usage

```sh
# Build
cargo build --release

# Start LSP server (communicates over stdio)
cargo run --release -- lsp

# Dump definition index for debugging
cargo run --release -- dump path/to/ruby/files

# Install as a single binary
cargo install --path .
```

### LSP capabilities

| Method                    | Description                                 |
| ------------------------- | ------------------------------------------- |
| `textDocument/definition` | Jump to constant or method definition       |
| `workspace/symbol`        | Search definitions by FQN substring         |
| `rbtags/bestDefinition`   | Like definition, but returns only top match |

### `rbtags/bestDefinition`

A custom LSP request that works like `textDocument/definition` but returns only the single best candidate instead of a list.
This is useful for editor plugins that want to jump directly without showing a selection UI.

- Accepts the same `GotoDefinitionParams` as `textDocument/definition`
- Returns a single `Location` or `null`
- Uses the same resolution and priority logic

## Editor setup example: Neovim

```lua
-- Define the rbtags LSP server.
vim.lsp.config("rbtags", {
    cmd = { "rbtags", "lsp" },
    filetypes = { "ruby" },
    root_markers = { '.git' },
})
vim.lsp.enable("rbtags")

-- To use the rbtags/bestDefinition command, define the handler.
local function rbtags_best_definition()
    local clients = vim.lsp.get_clients({ bufnr = 0, name = 'rbtags' })
    if #clients == 0 then
        vim.notify('rbtags client not found', vim.log.levels.ERROR)
        return
    end
    local params = vim.lsp.util.make_position_params()
    clients[1]:request('rbtags/bestDefinition', params, function(err, result)
        if err then
            vim.notify('rbtags/bestDefinition: ' .. err.message, vim.log.levels.ERROR)
            return
        end
        if result == nil then
            vim.notify('No location found', vim.log.levels.INFO)
            return
        end
        vim.lsp.util.show_document(result, 'utf-8')
    end)
end

vim.api.nvim_create_autocmd('LspAttach', {
    group = 'vimrc',
    callback = function(event)
        -- ...

        local filetype = vim.bo[event.buf].filetype
        if filetype == 'ruby' then
            vim.keymap.set('n', '<C-]>', rbtags_best_definition, { buffer = event.buf })
        end
    end,
})
```
