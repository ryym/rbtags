use ruby_prism::{CallNode, ClassNode, ConstantPathNode, ModuleNode, Node, SingletonClassNode, Visit};

use crate::indexer::resolve_constant_path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    Constant {
        name: String,
        namespace: Vec<String>,
    },
    Method {
        name: String,
        receiver: MethodReceiver,
        namespace: Vec<String>,
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
    };
    finder.visit(&result.node());
    finder.result
}

struct ReferenceFinder {
    offset: usize,
    namespace: Vec<String>,
    result: Option<Reference>,
}

impl ReferenceFinder {
    fn found(&self) -> bool {
        self.result.is_some()
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
        if self.offset >= loc.start_offset() && self.offset < loc.end_offset() {
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
            && self.offset >= msg_loc.start_offset()
            && self.offset < msg_loc.end_offset()
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

    fn visit_leaf_node_enter(&mut self, node: Node<'pr>) {
        if self.found() {
            return;
        }
        if let Some(n) = node.as_constant_read_node() {
            let loc = n.location();
            if self.offset >= loc.start_offset() && self.offset < loc.end_offset() {
                let name = std::str::from_utf8(n.name().as_slice()).unwrap();
                self.result = Some(Reference::Constant {
                    name: name.to_string(),
                    namespace: self.namespace.clone(),
                });
            }
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
        let name = std::str::from_utf8(n.name().as_slice()).unwrap().to_string();
        return MethodReceiver::Variable(name);
    }

    // Bare identifier like `user` in `user.save` - Prism parses as a CallNode
    // with no receiver and no arguments (is_variable_call flag).
    if let Some(n) = receiver.as_call_node()
        && n.receiver().is_none()
        && n.arguments().is_none()
        && n.block().is_none()
    {
        let name = std::str::from_utf8(n.name().as_slice()).unwrap().to_string();
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
        let src = b"class Foo\n  Bar\nend";
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
        let src = b"module A\n  module B\n    Bar\n  end\nend";
        assert_eq!(resolve(src, 24), constant_in("Bar", &["A", "B"]));
    }

    #[test]
    fn constant_path_with_namespace() {
        let src = b"module A\n  Foo::Bar\nend";
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
        let src = b"class Foo\n  self.bar\nend";
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
        let src = b"class Foo\n  def baz\n    bar\n  end\nend";
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
        let src = b"module A\n  class B\n    foo.bar\n  end\nend";
        assert_eq!(
            resolve(src, 27), // cursor on "bar"
            method("bar", MethodReceiver::Variable("foo".to_string()), &["A", "B"])
        );
    }

    #[test]
    fn method_in_chain() {
        // "foo.bar.baz"
        //  0123456789a
        let src = b"foo.bar.baz";
        // cursor on "baz" - receiver is a method chain, classify as None
        assert_eq!(
            resolve(src, 8),
            method("baz", MethodReceiver::None, &[])
        );
        // cursor on "bar" - receiver is variable "foo"
        assert_eq!(
            resolve(src, 4),
            method("bar", MethodReceiver::Variable("foo".to_string()), &[])
        );
    }
}
