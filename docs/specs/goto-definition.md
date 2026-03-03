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

### Fallback

- [x] If none of the above yields any candidates, return all definitions matching the method name (ctags-equivalent behavior)
