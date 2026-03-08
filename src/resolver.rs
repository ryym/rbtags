use ruby_prism::{
    CallNode, ClassNode, ConstantPathNode, ModuleNode, Node, SingletonClassNode, Visit,
};

use crate::indexer::resolve_constant_path;

fn loc_contains(loc: &ruby_prism::Location<'_>, offset: usize) -> bool {
    offset >= loc.start_offset() && offset < loc.end_offset()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    Constant {
        /// Constant name as written at the cursor (e.g. "Bar", "Foo::Bar").
        name: String,
        /// Enclosing module/class nesting at the cursor (e.g. ["A", "B"] inside `module A; module B`).
        namespace: Vec<String>,
    },
    Method {
        name: String,
        receiver: MethodReceiver,
        /// Enclosing module/class nesting at the cursor.
        namespace: Vec<String>,
    },
    InstanceVariable {
        /// Variable name without `@` prefix (e.g. "name" for `@name`).
        name: String,
        /// Enclosing module/class nesting at the cursor.
        namespace: Vec<String>,
    },
    LocalVariable {
        /// Variable name (e.g. "x").
        name: String,
        /// Byte offset of the definition (first assignment or parameter declaration).
        definition_offset: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MethodReceiver {
    None,
    SelfRef,
    Constant(String),
    Variable(String),
}

/// Given Ruby source and a byte offset (cursor position), find the reference
/// at that position and return structured information about it.
pub fn resolve_reference(source: &[u8], offset: usize) -> Option<Reference> {
    let result = ruby_prism::parse(source);
    let mut finder = ReferenceFinder {
        offset,
        namespace: Vec::new(),
        result: None,
        pending_local_var: None,
    };
    finder.visit(&result.node());

    // If the result is a pending local variable reference, do a second pass
    // to find the definition location.
    if let Some(PendingLocalVar { name, depth }) = finder.pending_local_var {
        let def_offset = find_local_var_def(&result.node(), &name, offset, depth);
        return def_offset.map(|definition_offset| Reference::LocalVariable {
            name: String::from_utf8(name).unwrap(),
            definition_offset,
        });
    }

    finder.result
}

struct PendingLocalVar {
    name: Vec<u8>,
    depth: u32,
}

struct ReferenceFinder {
    /// Byte offset of the cursor position to search for.
    offset: usize,
    /// Current module/class nesting stack during AST traversal.
    namespace: Vec<String>,
    result: Option<Reference>,
    /// Set when a local variable is detected; resolved in a second pass.
    pending_local_var: Option<PendingLocalVar>,
}

impl ReferenceFinder {
    fn found(&self) -> bool {
        self.result.is_some() || self.pending_local_var.is_some()
    }
}

impl<'pr> Visit<'pr> for ReferenceFinder {
    fn visit_module_node(&mut self, node: &ModuleNode<'pr>) {
        if self.found() {
            return;
        }
        let parts = resolve_constant_path(&node.constant_path());
        let prev_len = self.namespace.len();
        self.namespace.extend(parts);
        ruby_prism::visit_module_node(self, node);
        self.namespace.truncate(prev_len);
    }

    fn visit_class_node(&mut self, node: &ClassNode<'pr>) {
        if self.found() {
            return;
        }
        let parts = resolve_constant_path(&node.constant_path());
        let prev_len = self.namespace.len();
        self.namespace.extend(parts);
        ruby_prism::visit_class_node(self, node);
        self.namespace.truncate(prev_len);
    }

    fn visit_singleton_class_node(&mut self, node: &SingletonClassNode<'pr>) {
        if self.found() {
            return;
        }
        ruby_prism::visit_singleton_class_node(self, node);
    }

    fn visit_constant_path_node(&mut self, node: &ConstantPathNode<'pr>) {
        if self.found() {
            return;
        }
        let loc = node.location();
        if loc_contains(&loc, self.offset) {
            let node = node.as_node();
            let parts = resolve_constant_path(&node);
            if !parts.is_empty() {
                self.result = Some(Reference::Constant {
                    name: parts.join("::"),
                    namespace: self.namespace.clone(),
                });
                return;
            }
        }
        ruby_prism::visit_constant_path_node(self, node);
    }

    fn visit_call_node(&mut self, node: &CallNode<'pr>) {
        if self.found() {
            return;
        }

        // Check if cursor is on the method name
        if let Some(msg_loc) = node.message_loc()
            && loc_contains(&msg_loc, self.offset)
        {
            let name = std::str::from_utf8(node.name().as_slice())
                .unwrap()
                .to_string();
            let receiver = classify_receiver(node);
            self.result = Some(Reference::Method {
                name,
                receiver,
                namespace: self.namespace.clone(),
            });
            return;
        }

        // Continue traversal into receiver, arguments, block
        ruby_prism::visit_call_node(self, node);
    }

    fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
        if self.found() {
            return;
        }
        // Instance variable writes: @x = val, @x += val, @x &&= val, @x ||= val
        // Use name_loc() to only match when cursor is on the variable name, not the RHS.
        let ivar = node
            .as_instance_variable_write_node()
            .map(|n| (n.name(), n.name_loc()))
            .or_else(|| {
                node.as_instance_variable_operator_write_node()
                    .map(|n| (n.name(), n.name_loc()))
            })
            .or_else(|| {
                node.as_instance_variable_and_write_node()
                    .map(|n| (n.name(), n.name_loc()))
            })
            .or_else(|| {
                node.as_instance_variable_or_write_node()
                    .map(|n| (n.name(), n.name_loc()))
            });
        if let Some((name_id, loc)) = ivar
            && loc_contains(&loc, self.offset)
        {
            let name = std::str::from_utf8(name_id.as_slice()).unwrap();
            let name = name.strip_prefix('@').unwrap_or(name);
            self.result = Some(Reference::InstanceVariable {
                name: name.to_string(),
                namespace: self.namespace.clone(),
            });
            return;
        }

        // Local variable writes: x = val, x += val, x &&= val, x ||= val
        let lvar = node
            .as_local_variable_write_node()
            .map(|n| (n.name(), n.depth(), n.name_loc()))
            .or_else(|| {
                node.as_local_variable_operator_write_node()
                    .map(|n| (n.name(), n.depth(), n.name_loc()))
            })
            .or_else(|| {
                node.as_local_variable_and_write_node()
                    .map(|n| (n.name(), n.depth(), n.name_loc()))
            })
            .or_else(|| {
                node.as_local_variable_or_write_node()
                    .map(|n| (n.name(), n.depth(), n.name_loc()))
            });
        if let Some((name_id, depth, name_loc)) = lvar
            && loc_contains(&name_loc, self.offset)
        {
            self.pending_local_var = Some(PendingLocalVar {
                name: name_id.as_slice().to_vec(),
                depth,
            });
        }
    }

    fn visit_leaf_node_enter(&mut self, node: Node<'pr>) {
        if self.found() {
            return;
        }
        if let Some(n) = node.as_constant_read_node() {
            let loc = n.location();
            if loc_contains(&loc, self.offset) {
                let name = std::str::from_utf8(n.name().as_slice()).unwrap();
                self.result = Some(Reference::Constant {
                    name: name.to_string(),
                    namespace: self.namespace.clone(),
                });
            }
        } else if let Some(n) = node.as_instance_variable_read_node() {
            let loc = n.location();
            if loc_contains(&loc, self.offset) {
                let name = std::str::from_utf8(n.name().as_slice()).unwrap();
                let name = name.strip_prefix('@').unwrap_or(name);
                self.result = Some(Reference::InstanceVariable {
                    name: name.to_string(),
                    namespace: self.namespace.clone(),
                });
            }
        } else if let Some(n) = node.as_local_variable_read_node() {
            let loc = n.location();
            if loc_contains(&loc, self.offset) {
                self.pending_local_var = Some(PendingLocalVar {
                    name: n.name().as_slice().to_vec(),
                    depth: n.depth(),
                });
            }
        }
    }
}

/// Find the definition (first assignment or parameter) of a local variable.
///
/// `name` is the variable name, `cursor_offset` is where the reference was found,
/// and `depth` is from ruby-prism's LocalVariableReadNode (0 = current scope, 1 = parent, etc.).
fn find_local_var_def(
    root: &Node<'_>,
    name: &[u8],
    cursor_offset: usize,
    depth: u32,
) -> Option<usize> {
    let mut finder = LocalVarDefFinder {
        name,
        cursor_offset,
        next_scope_id: 0,
        scope_stack: Vec::new(),
        cursor_scope_stack: None,
        candidates: Vec::new(),
    };
    finder.visit(root);

    // After traversal, cursor_scope_stack is finalized.
    let cursor_stack = finder.cursor_scope_stack?;
    let target_idx = cursor_stack.len().checked_sub(1 + depth as usize)?;
    let target_scope_id = cursor_stack[target_idx];

    // Return the first (earliest) candidate in the target scope, before the cursor.
    finder
        .candidates
        .iter()
        .filter(|(offset, scope_id)| *scope_id == target_scope_id && *offset < cursor_offset)
        .map(|(offset, _)| *offset)
        .next()
}

struct LocalVarDefFinder<'a> {
    name: &'a [u8],
    cursor_offset: usize,
    /// Counter for assigning unique scope IDs.
    next_scope_id: u32,
    /// Stack of scope IDs during traversal.
    scope_stack: Vec<u32>,
    /// Snapshot of the scope stack at the cursor position, finalized after traversal.
    cursor_scope_stack: Option<Vec<u32>>,
    /// All candidate definitions: (byte_offset, scope_id).
    candidates: Vec<(usize, u32)>,
}

impl LocalVarDefFinder<'_> {
    fn enter_scope(&mut self, contains_cursor: bool) {
        let id = self.next_scope_id;
        self.next_scope_id += 1;
        self.scope_stack.push(id);
        if contains_cursor {
            self.cursor_scope_stack = Some(self.scope_stack.clone());
        }
    }

    fn leave_scope(&mut self) {
        self.scope_stack.pop();
    }

    fn current_scope_id(&self) -> u32 {
        *self.scope_stack.last().unwrap_or(&u32::MAX)
    }

    fn record_candidate(&mut self, offset: usize) {
        self.candidates.push((offset, self.current_scope_id()));
    }
}

impl<'pr> Visit<'pr> for LocalVarDefFinder<'_> {
    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
        let loc = node.location();
        self.enter_scope(loc_contains(&loc, self.cursor_offset));
        ruby_prism::visit_def_node(self, node);
        self.leave_scope();
    }

    fn visit_block_node(&mut self, node: &ruby_prism::BlockNode<'pr>) {
        let loc = node.location();
        self.enter_scope(loc_contains(&loc, self.cursor_offset));
        ruby_prism::visit_block_node(self, node);
        self.leave_scope();
    }

    fn visit_lambda_node(&mut self, node: &ruby_prism::LambdaNode<'pr>) {
        let loc = node.location();
        self.enter_scope(loc_contains(&loc, self.cursor_offset));
        ruby_prism::visit_lambda_node(self, node);
        self.leave_scope();
    }

    fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
        // Local variable writes with depth=0 are definitions in their own scope
        let lvar_write = node
            .as_local_variable_write_node()
            .filter(|n| n.depth() == 0)
            .map(|n| (n.name(), n.name_loc()))
            .or_else(|| {
                node.as_local_variable_target_node()
                    .filter(|n| n.depth() == 0)
                    .map(|n| (n.name(), n.location()))
            })
            .or_else(|| {
                node.as_local_variable_operator_write_node()
                    .filter(|n| n.depth() == 0)
                    .map(|n| (n.name(), n.name_loc()))
            })
            .or_else(|| {
                node.as_local_variable_and_write_node()
                    .filter(|n| n.depth() == 0)
                    .map(|n| (n.name(), n.name_loc()))
            })
            .or_else(|| {
                node.as_local_variable_or_write_node()
                    .filter(|n| n.depth() == 0)
                    .map(|n| (n.name(), n.name_loc()))
            });

        if let Some((name_id, loc)) = lvar_write
            && name_id.as_slice() == self.name
        {
            self.record_candidate(loc.start_offset());
            return;
        }

        // Optional parameters are branch nodes (they have default value children)
        if let Some(n) = node.as_optional_parameter_node()
            && n.name().as_slice() == self.name
        {
            self.record_candidate(n.name_loc().start_offset());
        } else if let Some(n) = node.as_optional_keyword_parameter_node()
            && n.name().as_slice() == self.name
        {
            self.record_candidate(n.name_loc().start_offset());
        }
    }

    fn visit_leaf_node_enter(&mut self, node: Node<'pr>) {
        // Parameters are leaf nodes and serve as definitions
        if let Some(n) = node.as_required_parameter_node() {
            if n.name().as_slice() == self.name {
                self.record_candidate(n.location().start_offset());
            }
        } else if let Some(n) = node.as_optional_parameter_node() {
            if n.name().as_slice() == self.name {
                self.record_candidate(n.location().start_offset());
            }
        } else if let Some(n) = node.as_rest_parameter_node() {
            if let Some(name) = n.name()
                && name.as_slice() == self.name
                && let Some(name_loc) = n.name_loc()
            {
                self.record_candidate(name_loc.start_offset());
            }
        } else if let Some(n) = node.as_block_parameter_node() {
            if let Some(name) = n.name()
                && name.as_slice() == self.name
                && let Some(name_loc) = n.name_loc()
            {
                self.record_candidate(name_loc.start_offset());
            }
        } else if let Some(n) = node.as_required_keyword_parameter_node() {
            if n.name().as_slice() == self.name {
                self.record_candidate(n.name_loc().start_offset());
            }
        } else if let Some(n) = node.as_keyword_rest_parameter_node() {
            if let Some(name) = n.name()
                && name.as_slice() == self.name
                && let Some(name_loc) = n.name_loc()
            {
                self.record_candidate(name_loc.start_offset());
            }
        } else if let Some(n) = node.as_block_local_variable_node()
            && n.name().as_slice() == self.name
        {
            self.record_candidate(n.location().start_offset());
        }
    }
}

fn classify_receiver(node: &CallNode<'_>) -> MethodReceiver {
    let Some(receiver) = node.receiver() else {
        return MethodReceiver::None;
    };

    // self.bar
    if receiver.as_self_node().is_some() {
        return MethodReceiver::SelfRef;
    }

    // Foo.bar or Foo::Bar.bar
    if receiver.as_constant_read_node().is_some() || receiver.as_constant_path_node().is_some() {
        let parts = resolve_constant_path(&receiver);
        if !parts.is_empty() {
            return MethodReceiver::Constant(parts.join("::"));
        }
    }

    // variable.bar - extract variable name
    if let Some(n) = receiver.as_local_variable_read_node() {
        let name = std::str::from_utf8(n.name().as_slice())
            .unwrap()
            .to_string();
        return MethodReceiver::Variable(name);
    }

    // Bare identifier like `user` in `user.save` - Prism parses as a CallNode
    // with no receiver and no arguments (is_variable_call flag).
    if let Some(n) = receiver.as_call_node()
        && n.receiver().is_none()
        && n.arguments().is_none()
        && n.block().is_none()
    {
        let name = std::str::from_utf8(n.name().as_slice())
            .unwrap()
            .to_string();
        return MethodReceiver::Variable(name);
    }

    // instance variable, method chain, etc. - treat as unknown
    MethodReceiver::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(src: &[u8], offset: usize) -> Option<Reference> {
        resolve_reference(src, offset)
    }

    fn constant(s: &str) -> Option<Reference> {
        Some(Reference::Constant {
            name: s.to_string(),
            namespace: vec![],
        })
    }

    fn constant_in(s: &str, ns: &[&str]) -> Option<Reference> {
        Some(Reference::Constant {
            name: s.to_string(),
            namespace: ns.iter().map(|s| s.to_string()).collect(),
        })
    }

    fn method(name: &str, receiver: MethodReceiver, namespace: &[&str]) -> Option<Reference> {
        Some(Reference::Method {
            name: name.to_string(),
            receiver,
            namespace: namespace.iter().map(|s| s.to_string()).collect(),
        })
    }

    // --- Constant tests (unchanged behavior) ---

    #[test]
    fn simple_constant() {
        let src = b"Foo";
        assert_eq!(resolve(src, 0), constant("Foo"));
        assert_eq!(resolve(src, 2), constant("Foo"));
    }

    #[test]
    fn constant_path() {
        let src = b"Foo::Bar";
        assert_eq!(resolve(src, 0), constant("Foo::Bar"));
        assert_eq!(resolve(src, 5), constant("Foo::Bar"));
        assert_eq!(resolve(src, 7), constant("Foo::Bar"));
    }

    #[test]
    fn constant_in_method_call() {
        // Cursor on constant part, not on method name
        let src = b"Foo::Bar.new";
        assert_eq!(resolve(src, 0), constant("Foo::Bar"));
        assert_eq!(resolve(src, 5), constant("Foo::Bar"));
    }

    #[test]
    fn no_reference_at_offset() {
        let src = b"x = 1";
        assert_eq!(resolve(src, 0), None);
    }

    #[test]
    fn constant_inside_class_body() {
        let src = b"\
class Foo
  Bar
end";
        assert_eq!(resolve(src, 12), constant_in("Bar", &["Foo"]));
    }

    #[test]
    fn constant_in_assignment_rhs() {
        let src = b"a = Foo::Bar";
        assert_eq!(resolve(src, 4), constant("Foo::Bar"));
        assert_eq!(resolve(src, 9), constant("Foo::Bar"));
    }

    #[test]
    fn deeply_nested_constant_path() {
        let src = b"A::B::C";
        assert_eq!(resolve(src, 0), constant("A::B::C"));
        assert_eq!(resolve(src, 3), constant("A::B::C"));
        assert_eq!(resolve(src, 6), constant("A::B::C"));
    }

    #[test]
    fn constant_in_if_condition() {
        let src = b"if Foo::Bar; end";
        assert_eq!(resolve(src, 3), constant("Foo::Bar"));
    }

    #[test]
    fn constant_in_rescue() {
        let src = b"begin; rescue Foo::Error; end";
        assert_eq!(resolve(src, 14), constant("Foo::Error"));
    }

    #[test]
    fn constant_in_block() {
        let src = b"x { Foo::Bar }";
        assert_eq!(resolve(src, 4), constant("Foo::Bar"));
    }

    #[test]
    fn constant_in_array() {
        let src = b"[Foo::Bar]";
        assert_eq!(resolve(src, 1), constant("Foo::Bar"));
    }

    #[test]
    fn constant_in_return() {
        let src = b"return Foo::Bar";
        assert_eq!(resolve(src, 7), constant("Foo::Bar"));
    }

    #[test]
    fn constant_in_or_expression() {
        let src = b"x || Foo::Bar";
        assert_eq!(resolve(src, 5), constant("Foo::Bar"));
    }

    #[test]
    fn constant_in_when() {
        let src = b"case x; when Foo::Bar; end";
        assert_eq!(resolve(src, 13), constant("Foo::Bar"));
    }

    #[test]
    fn constant_in_string_interpolation() {
        let src = b"\"#{Foo::Bar}\"";
        assert_eq!(resolve(src, 3), constant("Foo::Bar"));
    }

    #[test]
    fn constant_in_hash_value() {
        let src = b"{ a: Foo::Bar }";
        assert_eq!(resolve(src, 5), constant("Foo::Bar"));
    }

    #[test]
    fn constant_namespace_tracking() {
        let src = b"\
module A
  module B
    Bar
  end
end";
        assert_eq!(resolve(src, 24), constant_in("Bar", &["A", "B"]));
    }

    #[test]
    fn constant_path_with_namespace() {
        let src = b"\
module A
  Foo::Bar
end";
        assert_eq!(resolve(src, 12), constant_in("Foo::Bar", &["A"]));
    }

    // --- Method call tests ---

    #[test]
    fn method_on_constant_receiver() {
        // "Foo::Bar.new"
        //  0123456789ab
        let src = b"Foo::Bar.new";
        assert_eq!(
            resolve(src, 9), // cursor on "new"
            method("new", MethodReceiver::Constant("Foo::Bar".to_string()), &[])
        );
    }

    #[test]
    fn method_on_simple_constant() {
        // "User.find"
        //  012345678
        let src = b"User.find";
        assert_eq!(
            resolve(src, 5),
            method("find", MethodReceiver::Constant("User".to_string()), &[])
        );
    }

    #[test]
    fn method_on_self() {
        // "class Foo\n  self.bar\nend"
        //  0123456789012345678901
        let src = b"\
class Foo
  self.bar
end";
        assert_eq!(
            resolve(src, 17), // cursor on "bar"
            method("bar", MethodReceiver::SelfRef, &["Foo"])
        );
    }

    #[test]
    fn bare_method_call() {
        // "class Foo\n  def baz\n    bar\n  end\nend"
        //  0         1         2
        //  0123456789012345678901234567
        let src = b"\
class Foo
  def baz
    bar
  end
end";
        assert_eq!(
            resolve(src, 24), // cursor on "bar"
            method("bar", MethodReceiver::None, &["Foo"])
        );
    }

    #[test]
    fn method_on_variable() {
        // "user.save"
        //  012345678
        let src = b"user.save";
        assert_eq!(
            resolve(src, 5),
            method("save", MethodReceiver::Variable("user".to_string()), &[])
        );
    }

    #[test]
    fn method_on_variable_snake_case() {
        // "order_item.total"
        //  0123456789abcdef
        let src = b"order_item.total";
        assert_eq!(
            resolve(src, 11),
            method(
                "total",
                MethodReceiver::Variable("order_item".to_string()),
                &[]
            )
        );
    }

    #[test]
    fn method_namespace_tracking() {
        // "module A\n  class B\n    foo.bar\n  end\nend"
        let src = b"\
module A
  class B
    foo.bar
  end
end";
        assert_eq!(
            resolve(src, 27), // cursor on "bar"
            method(
                "bar",
                MethodReceiver::Variable("foo".to_string()),
                &["A", "B"]
            )
        );
    }

    #[test]
    fn method_in_chain() {
        // "foo.bar.baz"
        //  0123456789a
        let src = b"foo.bar.baz";
        // cursor on "baz" - receiver is a method chain, classify as None
        assert_eq!(resolve(src, 8), method("baz", MethodReceiver::None, &[]));
        // cursor on "bar" - receiver is variable "foo"
        assert_eq!(
            resolve(src, 4),
            method("bar", MethodReceiver::Variable("foo".to_string()), &[])
        );
    }

    // --- Instance variable tests ---

    fn ivar(name: &str, ns: &[&str]) -> Option<Reference> {
        Some(Reference::InstanceVariable {
            name: name.to_string(),
            namespace: ns.iter().map(|s| s.to_string()).collect(),
        })
    }

    #[test]
    fn instance_variable_read() {
        // "class Foo\n  def bar\n    @name\n  end\nend"
        //  0         1         2
        //  0123456789012345678901234567
        let src = b"\
class Foo
  def bar
    @name
  end
end";
        assert_eq!(resolve(src, 24), ivar("name", &["Foo"]));
    }

    #[test]
    fn instance_variable_write() {
        // "class Foo\n  def bar\n    @name = 1\n  end\nend"
        let src = b"\
class Foo
  def bar
    @name = 1
  end
end";
        assert_eq!(resolve(src, 24), ivar("name", &["Foo"]));
    }

    #[test]
    fn instance_variable_in_nested_module() {
        let src = b"\
module A
  class B
    def foo
      @val
    end
  end
end";
        // @val starts at offset 37
        assert_eq!(resolve(src, 37), ivar("val", &["A", "B"]));
    }

    #[test]
    fn instance_variable_or_write() {
        let src = b"\
class Foo
  def bar
    @cache ||= 1
  end
end";
        assert_eq!(resolve(src, 24), ivar("cache", &["Foo"]));
    }

    #[test]
    fn instance_variable_write_rhs_not_captured() {
        // Cursor on RHS of @c = expr should not resolve to @c
        let src = b"\
class Foo
  def bar(a)
    @c = a
  end
end";
        let a_in_rhs = find_offset(src, 2, b"a"); // second "a" is in RHS
        // Should resolve as local variable (parameter a), not instance variable @c
        assert_ne!(resolve(src, a_in_rhs), ivar("c", &["Foo"]));
    }

    // --- Local variable tests ---

    fn lvar(name: &str, def_offset: usize) -> Option<Reference> {
        Some(Reference::LocalVariable {
            name: name.to_string(),
            definition_offset: def_offset,
        })
    }

    #[test]
    fn local_variable_read_jumps_to_assignment() {
        let src = b"\
def foo
  x = 1
  x
end";
        let cursor = find_offset(src, 2, b"x"); // second "x" (the read)
        assert_eq!(resolve(src, cursor), lvar("x", find_offset(src, 1, b"x")));
    }

    #[test]
    fn local_variable_first_assignment_returns_none() {
        let src = b"\
def foo
  x = 1
end";
        let cursor = find_offset(src, 1, b"x");
        assert_eq!(resolve(src, cursor), None);
    }

    #[test]
    fn local_variable_second_assignment_jumps_to_first() {
        let src = b"\
def foo
  x = 1
  x = 2
end";
        let first = find_offset(src, 1, b"x");
        let second = find_offset(src, 2, b"x");
        assert_eq!(resolve(src, second), lvar("x", first));
    }

    #[test]
    fn local_variable_method_parameter() {
        let src = b"\
def foo(x)
  x
end";
        let param = find_offset(src, 1, b"x");
        let read = find_offset(src, 2, b"x");
        assert_eq!(resolve(src, read), lvar("x", param));
    }

    #[test]
    fn local_variable_scoped_to_method() {
        // In bar, x has never been assigned, so Ruby/Prism treats it as a method call
        let src = b"\
def foo
  x = 1
end
def bar
  x
end";
        let read_in_bar = find_offset(src, 2, b"x");
        assert_eq!(
            resolve(src, read_in_bar),
            method("x", MethodReceiver::None, &[])
        );
    }

    #[test]
    fn local_variable_block_parameter() {
        let src = b"\
[1].each do |item|
  item
end";
        let param = find_offset(src, 1, b"item");
        let read = find_offset(src, 2, b"item");
        assert_eq!(resolve(src, read), lvar("item", param));
    }

    #[test]
    fn local_variable_same_name_different_methods() {
        // Each method has its own scope — x in method2 should not jump to method1's x
        let src = b"\
class Foo
  def method1
    a = 1
    puts(a)
  end

  def method2
    a = 2
    puts(a)
  end
end";
        let def_in_method2 = find_offset(src, 3, b"a"); // 3rd "a" = a = 2
        let read_in_method2 = find_offset(src, 4, b"a"); // 4th "a" = puts(a) in method2
        assert_eq!(resolve(src, read_in_method2), lvar("a", def_in_method2));
    }

    #[test]
    fn local_variable_required_keyword_parameter() {
        let src = b"\
def foo(a:, b:)
  a + b
end";
        let param_a = find_offset(src, 1, b"a");
        let read_a = find_offset(src, 2, b"a");
        assert_eq!(resolve(src, read_a), lvar("a", param_a));
    }

    #[test]
    fn local_variable_optional_keyword_parameter() {
        let src = b"\
def foo(a: 1)
  a
end";
        let param = find_offset(src, 1, b"a");
        let read = find_offset(src, 2, b"a");
        assert_eq!(resolve(src, read), lvar("a", param));
    }

    #[test]
    fn local_variable_keyword_rest_parameter() {
        let src = b"\
def foo(**args)
  args
end";
        let param = find_offset(src, 1, b"args");
        let read = find_offset(src, 2, b"args");
        assert_eq!(resolve(src, read), lvar("args", param));
    }

    #[test]
    fn local_variable_optional_parameter() {
        let src = b"\
def foo(x = 1)
  x
end";
        let param = find_offset(src, 1, b"x");
        let read = find_offset(src, 2, b"x");
        assert_eq!(resolve(src, read), lvar("x", param));
    }

    #[test]
    fn local_variable_rest_parameter() {
        let src = b"\
def foo(*args)
  args
end";
        let param = find_offset(src, 1, b"args");
        let read = find_offset(src, 2, b"args");
        assert_eq!(resolve(src, read), lvar("args", param));
    }

    #[test]
    fn local_variable_in_block_from_outer_scope() {
        let src = b"\
def foo
  x = 1
  [1].each do
    x
  end
end";
        let def = find_offset(src, 1, b"x");
        let read = find_offset(src, 2, b"x");
        assert_eq!(resolve(src, read), lvar("x", def));
    }

    /// Find the byte offset of the nth occurrence of `needle` in `src` (1-indexed).
    fn find_offset(src: &[u8], nth: usize, needle: &[u8]) -> usize {
        let mut count = 0;
        for i in 0..src.len() {
            if src[i..].starts_with(needle) {
                // Ensure it's a word boundary (not part of a longer identifier)
                let before_ok = i == 0 || !src[i - 1].is_ascii_alphanumeric() && src[i - 1] != b'_';
                let end = i + needle.len();
                let after_ok =
                    end >= src.len() || !src[end].is_ascii_alphanumeric() && src[end] != b'_';
                if before_ok && after_ok {
                    count += 1;
                    if count == nth {
                        return i;
                    }
                }
            }
        }
        panic!(
            "could not find occurrence {} of {:?} in {:?}",
            nth,
            std::str::from_utf8(needle).unwrap(),
            std::str::from_utf8(src).unwrap()
        );
    }
}
