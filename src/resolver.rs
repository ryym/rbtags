use ruby_prism::Node;

use crate::indexer::resolve_constant_path;

/// Given Ruby source and a byte offset (cursor position), find the constant
/// reference at that position and return its fully qualified name.
pub fn resolve_reference(source: &[u8], offset: usize) -> Option<String> {
    let result = ruby_prism::parse(source);
    find_constant_at(&result.node(), offset)
}

/// Recursively search the AST for a ConstantReadNode or ConstantPathNode
/// whose location spans the given byte offset.
fn find_constant_at(node: &Node<'_>, offset: usize) -> Option<String> {
    // Check if this node is a constant reference spanning the offset.
    // For ConstantPathNode, resolve the full path (e.g., Foo::Bar).
    // For ConstantReadNode, return the simple name.
    if let Some(n) = node.as_constant_path_node() {
        let loc = n.location();
        if offset >= loc.start_offset() && offset < loc.end_offset() {
            // Check children first for more specific matches (not applicable
            // for constant paths, but recurse into parent to see if cursor is
            // on a prefix like "Foo" in "Foo::Bar").
            // However, we want the *widest* constant path that contains the
            // offset, so we return the full path from this node.
            let parts = resolve_constant_path(node);
            if !parts.is_empty() {
                return Some(parts.join("::"));
            }
        }
    } else if let Some(n) = node.as_constant_read_node() {
        let loc = n.location();
        if offset >= loc.start_offset() && offset < loc.end_offset() {
            let name = std::str::from_utf8(n.name().as_slice()).unwrap();
            return Some(name.to_string());
        }
    }

    // Recurse into child nodes.
    visit_children(node, offset)
}

fn visit_children(node: &Node<'_>, offset: usize) -> Option<String> {
    if let Some(n) = node.as_program_node() {
        let stmts = n.statements();
        for child in &stmts.body() {
            if let Some(result) = find_constant_at(&child, offset) {
                return Some(result);
            }
        }
    } else if let Some(n) = node.as_statements_node() {
        for child in &n.body() {
            if let Some(result) = find_constant_at(&child, offset) {
                return Some(result);
            }
        }
    } else if let Some(n) = node.as_module_node() {
        if let Some(result) = find_constant_at(&n.constant_path(), offset) {
            return Some(result);
        }
        if let Some(body) = n.body()
            && let Some(result) = find_constant_at(&body, offset)
        {
            return Some(result);
        }
    } else if let Some(n) = node.as_class_node() {
        if let Some(result) = find_constant_at(&n.constant_path(), offset) {
            return Some(result);
        }
        if let Some(superclass) = n.superclass()
            && let Some(result) = find_constant_at(&superclass, offset)
        {
            return Some(result);
        }
        if let Some(body) = n.body()
            && let Some(result) = find_constant_at(&body, offset)
        {
            return Some(result);
        }
    } else if let Some(n) = node.as_def_node() {
        if let Some(body) = n.body()
            && let Some(result) = find_constant_at(&body, offset)
        {
            return Some(result);
        }
    } else if let Some(n) = node.as_singleton_class_node() {
        if let Some(body) = n.body()
            && let Some(result) = find_constant_at(&body, offset)
        {
            return Some(result);
        }
    } else if let Some(n) = node.as_call_node() {
        if let Some(receiver) = n.receiver()
            && let Some(result) = find_constant_at(&receiver, offset)
        {
            return Some(result);
        }
        if let Some(args) = n.arguments() {
            for arg in &args.arguments() {
                if let Some(result) = find_constant_at(&arg, offset) {
                    return Some(result);
                }
            }
        }
    } else if let Some(n) = node.as_local_variable_write_node() {
        if let Some(result) = find_constant_at(&n.value(), offset) {
            return Some(result);
        }
    } else if let Some(n) = node.as_instance_variable_write_node() {
        if let Some(result) = find_constant_at(&n.value(), offset) {
            return Some(result);
        }
    } else if let Some(n) = node.as_class_variable_write_node() {
        if let Some(result) = find_constant_at(&n.value(), offset) {
            return Some(result);
        }
    } else if let Some(n) = node.as_constant_write_node() {
        if let Some(result) = find_constant_at(&n.value(), offset) {
            return Some(result);
        }
    } else if let Some(n) = node.as_global_variable_write_node() {
        if let Some(result) = find_constant_at(&n.value(), offset) {
            return Some(result);
        }
    }

    None
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
}
