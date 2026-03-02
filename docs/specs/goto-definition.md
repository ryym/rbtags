# Goto Definition

Specification for `textDocument/definition` in rbtags.

Status legend:

- [x] Implemented
- [ ] Not yet implemented

## Constant Jump

- [x] Resolve the constant reference at the cursor position to a fully qualified name (FQN) and jump to its definition
- [x] Resolve the entire `Foo::Bar` regardless of where the cursor sits within the token (widest match)
- [x] Correctly resolve nested definitions (`module Foo; module Bar; end; end`)
- [x] Correctly resolve inline definitions (`class Foo::Bar`)
- [x] Return all candidates when multiple files define the same FQN

## Method Jump

### Basic Behavior

- [ ] Extract the method name from the method call at the cursor position and search the index for matching method definitions
- [ ] When there are multiple candidates, sort them by the priority rules below

### Constant Receiver

When the receiver is a constant, match directly against class method definitions.

- [ ] `User.find` → return `User.find`
- [ ] `Foo::Bar.create` → return `Foo::Bar.create`

### Same Class/Module Priority

When the cursor is inside a class or module body, prioritize methods belonging to that class.

- [ ] `self.bar` → prioritize current class method (`Foo.bar` or `Foo#bar`)
- [ ] Bare `bar` (no receiver) → prioritize `Foo#bar` of the current class

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

- [ ] `user.save` → prioritize `User#save`
- [ ] `order_item.total` → prioritize `OrderItem#total`
- [ ] Fall back to all candidates if the guess does not match any class in the index

### File Distance Priority

When the above rules do not narrow down candidates, prioritize definitions closer to the current file.

- [ ] Definitions in the same file are highest priority
- [ ] Definitions in the same directory are next
- [ ] Otherwise, sort by longest common path prefix

### Fallback

- [ ] If none of the above yields any candidates, return all definitions matching the method name (ctags-equivalent behavior)
