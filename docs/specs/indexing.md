# Indexing

Specification for workspace indexing behavior.

Status legend:

- [x] Implemented
- [ ] Not yet implemented

## Performance

- [x] Parse and index files in parallel using a thread pool (number of threads = logical CPU cores)
- [x] Log total build time for diagnostics

## Instance Variables

- [x] Index instance variable write nodes (`@x = val`, `@x += val`, `@x ||= val`, `@x &&= val`) inside method bodies
- [x] Associate instance variables with the enclosing class/module namespace (e.g., `User#@name`)

## Exclude Paths

Allow users to specify directories to exclude from indexing via a configuration file (separate from `.gitignore`).

- [ ] Support a configuration file (e.g., `.rbtags.toml` or similar) for rbtags-specific settings
- [ ] Allow specifying directory patterns to exclude from indexing
- [ ] Excluded directories are skipped during the file walk phase
