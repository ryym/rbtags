# Goto Definition

Specification for `textDocument/definition` in rbtags.

Status legend:

- [x] Implemented
- [x] Not yet implemented

## Constant Jump

### Indexing

- [x] Index constant assignments (`ABC = 1`) as definitions with their FQN
- [x] Index path-qualified constant assignments (`Foo::BAR = 2`)
- [ ] Index constants in multi-assignment (`A, B = 1, 2`)

### Basic Behavior

- [x] Resolve the constant reference at the cursor position to a fully qualified name (FQN) and jump to its definition
- [x] Resolve the entire `Foo::Bar` regardless of where the cursor sits within the token (widest match)
- [x] Correctly resolve nested definitions (`module Foo; module Bar; end; end`)
- [x] Correctly resolve inline definitions (`class Foo::Bar`)
- [x] Return all candidates when multiple files define the same FQN

### Nesting-Aware Resolution

When the constant reference is not fully qualified, resolve it using the enclosing namespace context, emulating Ruby's constant nesting lookup.

- [x] Walk outward from the current namespace (e.g., `Bar` inside `A::B` tries `A::B::Bar` → `A::Bar` → `Bar`)
- [x] Partially qualified paths also use nesting (e.g., `Foo::Bar` inside `X` tries `X::Foo::Bar` → `Foo::Bar`)

```ruby
class Foo
  class Bar; end
  def foo
    Bar.new  # resolves to Foo::Bar via nesting
  end
end
```

### Suffix Match Fallback

When nesting resolution finds no candidates, fall back to matching any FQN ending with the constant name.

- [x] Match any FQN ending with `::{name}` (e.g., `Bar` matches `Foo::Bar`, `Baz::Bar`, etc.)
- [x] Sort suffix-matched candidates by file distance

### File Distance Priority

- [x] Among candidates at the same nesting/suffix tier, prioritize definitions closer to the current file

## Method Jump

### Basic Behavior

- [x] Extract the method name from the method call at the cursor position and search the index for matching method definitions
- [x] When there are multiple candidates, sort them by the priority rules below

### Constant Receiver

When the receiver is a constant, match directly against class method definitions.

- [x] `User.find` → return `User.find`
- [x] `Foo::Bar.create` → return `Foo::Bar.create`

### Same Class/Module Priority

When the cursor is inside a class or module body, prioritize methods belonging to that class.

- [x] `self.bar` → prioritize current class method (`Foo.bar` or `Foo#bar`)
- [x] Bare `bar` (no receiver) → prioritize `Foo#bar` of the current class

```ruby
class Foo
  def bar; end
  def baz
    bar       # prioritize Foo#bar
    self.bar  # prioritize Foo#bar
  end
end
```

### Class Guess from Variable Name

When the receiver is a variable, convert the variable name to CamelCase to guess the class.

- [x] `user.save` → prioritize `User#save`
- [x] `order_item.total` → prioritize `OrderItem#total`
- [x] Fall back to all candidates if the guess does not match any class in the index

### File Distance Priority

When the above rules do not narrow down candidates, prioritize definitions closer to the current file.

- [x] Definitions in the same file are highest priority
- [x] Definitions in the same directory are next
- [x] Otherwise, sort by longest common path prefix

### Symbol as Method Name

When the cursor is on a symbol literal (e.g., `:do_something`, `&:do_something`), treat it as a method name reference and search the index for matching method definitions.

- [x] `:foo` → search for method `foo`
- [x] `items.map(&:bar)` → search for method `bar`
- [x] Only the value part of the symbol is matched (cursor on `:` alone does not trigger)
- [x] Same priority and fallback rules as regular method calls apply

### Fallback

- [x] If none of the above yields any candidates, return all definitions matching the method name (ctags-equivalent behavior)

## Instance Variable Jump

### Indexing

- [x] Index instance variable assignments (`@name = value`) inside method bodies with owning class FQN (e.g., `User#@name`)
- [x] Index operator assignments (`@count += 1`), or-assignments (`@cache ||= val`), and-assignments (`@flag &&= val`)
- [x] Each assignment is indexed separately (same `@var` in multiple methods produces multiple index entries)

### Basic Behavior

- [x] Detect `@var` at cursor (both reads and writes) and resolve to the owning class's instance variable definitions
- [x] Use nesting-aware resolution: `@name` inside `User` → try `User#@name` first, then walk outward

### Nesting-Aware Resolution

Same approach as constants: walk outward from the current namespace.

- [x] `@name` inside `A::B` tries `A::B#@name` → `A#@name` → `#@name`

### Suffix Match Fallback

- [x] When nesting resolution finds no match, fall back to any FQN ending with `#@{name}`
- [x] Sort fallback candidates by file distance

### Limitations

- Does not track inheritance (won't find `@var` defined in a superclass)
- Only indexes assignments, not bare reads

## Local Variable Jump

Local variable definitions are resolved entirely within the current file at query time — they are not part of the workspace index.

### Basic Behavior

- [x] Detect local variable reads and writes at cursor position
- [x] Jump to the first assignment (`x = val`) or parameter declaration in the enclosing scope
- [x] When the cursor is on the first (defining) assignment itself, return no result (already at definition)

### Parameters

- [x] Method parameters (`def foo(x)`) are treated as definitions
- [x] Optional parameters (`def foo(x = 1)`)
- [x] Rest parameters (`def foo(*args)`)
- [ ] Block parameters (`def foo(&block)`)
- [x] Required keyword parameters (`def foo(a:)`)
- [x] Optional keyword parameters (`def foo(a: 1)`)
- [x] Keyword rest parameters (`def foo(**opts)`)
- [x] Block local variables (`arr.each { |item; local| }`)

### Scope Awareness

- [x] `def` creates a hard scope boundary — variables from other methods are not visible
- [x] Blocks (`do..end`, `{ }`) create soft boundaries — outer variables are accessible
- [x] Use ruby-prism's `depth` field to determine which scope a variable belongs to
- [x] Lambda nodes create scope boundaries (same as blocks)

```ruby
def foo
  x = 1
  [1].each do
    x  # depth=1 → resolves to x = 1 in the outer method scope
  end
end
```

### Limitations

- Variables not previously assigned in the current scope are parsed as method calls by ruby-prism (correct Ruby semantics)
- No cross-file resolution (local variables are file-scoped by definition)
- Pattern matching variable bindings are not yet supported

## Custom Request: `rbtags/bestDefinition`

A custom LSP request that accepts the same params as `textDocument/definition` but returns only the single best match.

- [x] Accepts `GotoDefinitionParams` (same as `textDocument/definition`)
- [x] Returns the top-priority `Location` or `null`
- [x] Uses the same resolution and priority logic as `textDocument/definition`
