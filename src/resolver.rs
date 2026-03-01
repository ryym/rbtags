use ruby_prism::{ConstantPathNode, Node, Visit};

use crate::indexer::resolve_constant_path;

/// Given Ruby source and a byte offset (cursor position), find the constant
/// reference at that position and return its fully qualified name.
pub fn resolve_reference(source: &[u8], offset: usize) -> Option<String> {
    let result = ruby_prism::parse(source);
    let mut finder = ConstantFinder {
        offset,
        result: None,
    };
    finder.visit(&result.node());
    finder.result
}

struct ConstantFinder {
    offset: usize,
    result: Option<String>,
}

impl<'pr> Visit<'pr> for ConstantFinder {
    fn visit_constant_path_node(&mut self, node: &ConstantPathNode<'pr>) {
        if self.result.is_some() {
            return;
        }
        let loc = node.location();
        if self.offset >= loc.start_offset() && self.offset < loc.end_offset() {
            let node = node.as_node();
            let parts = resolve_constant_path(&node);
            if !parts.is_empty() {
                self.result = Some(parts.join("::"));
                return; // Skip children for widest match
            }
        }
        // Outside range: continue default traversal
        ruby_prism::visit_constant_path_node(self, node);
    }

    fn visit_leaf_node_enter(&mut self, node: Node<'pr>) {
        if self.result.is_some() {
            return;
        }
        if let Some(n) = node.as_constant_read_node() {
            let loc = n.location();
            if self.offset >= loc.start_offset() && self.offset < loc.end_offset() {
                let name = std::str::from_utf8(n.name().as_slice()).unwrap();
                self.result = Some(name.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_constant() {
        let src = b"Foo";
        assert_eq!(resolve_reference(src, 0), Some("Foo".to_string()));
        assert_eq!(resolve_reference(src, 2), Some("Foo".to_string()));
    }

    #[test]
    fn constant_path() {
        // "Foo::Bar"
        //  0123456789
        let src = b"Foo::Bar";
        assert_eq!(resolve_reference(src, 0), Some("Foo::Bar".to_string()));
        assert_eq!(resolve_reference(src, 5), Some("Foo::Bar".to_string()));
        assert_eq!(resolve_reference(src, 7), Some("Foo::Bar".to_string()));
    }

    #[test]
    fn constant_in_method_call() {
        // "Foo::Bar.new"
        //  0123456789...
        let src = b"Foo::Bar.new";
        assert_eq!(resolve_reference(src, 0), Some("Foo::Bar".to_string()));
        assert_eq!(resolve_reference(src, 5), Some("Foo::Bar".to_string()));
    }

    #[test]
    fn no_constant_at_offset() {
        let src = b"x = 1";
        assert_eq!(resolve_reference(src, 0), None);
    }

    #[test]
    fn constant_inside_class_body() {
        // "class Foo\n  Bar\nend"
        //  01234567890123456
        let src = b"class Foo\n  Bar\nend";
        // "Bar" starts at offset 12
        assert_eq!(resolve_reference(src, 12), Some("Bar".to_string()));
    }

    #[test]
    fn constant_in_assignment_rhs() {
        // "a = Foo::Bar"
        //  0123456789...
        let src = b"a = Foo::Bar";
        // "Foo::Bar" starts at offset 4
        assert_eq!(resolve_reference(src, 4), Some("Foo::Bar".to_string()));
        assert_eq!(resolve_reference(src, 9), Some("Foo::Bar".to_string()));
    }

    #[test]
    fn deeply_nested_constant_path() {
        // "A::B::C"
        let src = b"A::B::C";
        assert_eq!(resolve_reference(src, 0), Some("A::B::C".to_string()));
        assert_eq!(resolve_reference(src, 3), Some("A::B::C".to_string()));
        assert_eq!(resolve_reference(src, 6), Some("A::B::C".to_string()));
    }

    #[test]
    fn constant_in_if_condition() {
        let src = b"if Foo::Bar; end";
        assert_eq!(resolve_reference(src, 3), Some("Foo::Bar".to_string()));
    }

    #[test]
    fn constant_in_rescue() {
        let src = b"begin; rescue Foo::Error; end";
        assert_eq!(resolve_reference(src, 14), Some("Foo::Error".to_string()));
    }

    #[test]
    fn constant_in_block() {
        let src = b"x { Foo::Bar }";
        assert_eq!(resolve_reference(src, 4), Some("Foo::Bar".to_string()));
    }

    #[test]
    fn constant_in_array() {
        let src = b"[Foo::Bar]";
        assert_eq!(resolve_reference(src, 1), Some("Foo::Bar".to_string()));
    }

    #[test]
    fn constant_in_return() {
        let src = b"return Foo::Bar";
        assert_eq!(resolve_reference(src, 7), Some("Foo::Bar".to_string()));
    }

    #[test]
    fn constant_in_or_expression() {
        let src = b"x || Foo::Bar";
        assert_eq!(resolve_reference(src, 5), Some("Foo::Bar".to_string()));
    }

    #[test]
    fn constant_in_when() {
        let src = b"case x; when Foo::Bar; end";
        assert_eq!(resolve_reference(src, 13), Some("Foo::Bar".to_string()));
    }

    #[test]
    fn constant_in_string_interpolation() {
        let src = b"\"#{Foo::Bar}\"";
        assert_eq!(resolve_reference(src, 3), Some("Foo::Bar".to_string()));
    }

    #[test]
    fn constant_in_hash_value() {
        let src = b"{ a: Foo::Bar }";
        assert_eq!(resolve_reference(src, 5), Some("Foo::Bar".to_string()));
    }
}
