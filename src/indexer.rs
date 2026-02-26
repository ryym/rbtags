use ruby_prism::Node;

#[derive(Debug, PartialEq, Eq)]
pub enum DefinitionKind {
    Module,
    Class,
    Method,
}

#[derive(Debug)]
pub struct Definition {
    pub fqn: String,
    pub kind: DefinitionKind,
    pub offset: usize,
}

pub fn index_source(source: &[u8]) -> Vec<Definition> {
    let result = ruby_prism::parse(source);
    let mut defs = Vec::new();
    let namespace: Vec<String> = Vec::new();
    visit(&result.node(), &namespace, false, &mut defs);
    defs
}

fn visit(node: &Node<'_>, namespace: &[String], in_singleton: bool, defs: &mut Vec<Definition>) {
    if let Some(n) = node.as_program_node() {
        let stmts = n.statements();
        for child in &stmts.body() {
            visit(&child, namespace, in_singleton, defs);
        }
    } else if let Some(n) = node.as_statements_node() {
        for child in &n.body() {
            visit(&child, namespace, in_singleton, defs);
        }
    } else if let Some(n) = node.as_module_node() {
        let parts = resolve_constant_path(&n.constant_path());
        let fqn = build_fqn(namespace, &parts);
        defs.push(Definition {
            fqn,
            kind: DefinitionKind::Module,
            offset: n.location().start_offset(),
        });
        let new_namespace = build_namespace(namespace, &parts);
        if let Some(body) = n.body() {
            visit(&body, &new_namespace, false, defs);
        }
    } else if let Some(n) = node.as_class_node() {
        let parts = resolve_constant_path(&n.constant_path());
        let fqn = build_fqn(namespace, &parts);
        defs.push(Definition {
            fqn,
            kind: DefinitionKind::Class,
            offset: n.location().start_offset(),
        });
        let new_namespace = build_namespace(namespace, &parts);
        if let Some(body) = n.body() {
            visit(&body, &new_namespace, false, defs);
        }
    } else if let Some(n) = node.as_singleton_class_node() {
        if let Some(body) = n.body() {
            visit(&body, namespace, true, defs);
        }
    } else if let Some(n) = node.as_def_node() {
        let method_name = std::str::from_utf8(n.name().as_slice()).unwrap();
        let separator = if in_singleton || n.receiver().is_some() {
            "."
        } else {
            "#"
        };
        let owner = namespace.join("::");
        let fqn = if owner.is_empty() {
            format!("{separator}{method_name}")
        } else {
            format!("{owner}{separator}{method_name}")
        };
        defs.push(Definition {
            fqn,
            kind: DefinitionKind::Method,
            offset: n.location().start_offset(),
        });
    }
}

fn resolve_constant_path(node: &Node<'_>) -> Vec<String> {
    if let Some(n) = node.as_constant_read_node() {
        vec![std::str::from_utf8(n.name().as_slice()).unwrap().to_string()]
    } else if let Some(n) = node.as_constant_path_node() {
        let mut parts = match n.parent() {
            Some(parent) => resolve_constant_path(&parent),
            None => Vec::new(),
        };
        if let Some(name) = n.name() {
            parts.push(std::str::from_utf8(name.as_slice()).unwrap().to_string());
        }
        parts
    } else {
        Vec::new()
    }
}

fn build_fqn(namespace: &[String], parts: &[String]) -> String {
    if namespace.is_empty() {
        parts.join("::")
    } else {
        format!("{}::{}", namespace.join("::"), parts.join("::"))
    }
}

fn build_namespace(namespace: &[String], parts: &[String]) -> Vec<String> {
    let mut new_ns = namespace.to_vec();
    new_ns.extend(parts.iter().cloned());
    new_ns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_modules() {
        let defs = index_source(b"module Foo\n  module Bar\n  end\nend");
        assert_eq!(defs[0].fqn, "Foo");
        assert_eq!(defs[0].kind, DefinitionKind::Module);
        assert_eq!(defs[1].fqn, "Foo::Bar");
        assert_eq!(defs[1].kind, DefinitionKind::Module);
    }

    #[test]
    fn inline_constant_path() {
        let defs = index_source(b"class Foo::Bar < Base\nend");
        assert_eq!(defs[0].fqn, "Foo::Bar");
        assert_eq!(defs[0].kind, DefinitionKind::Class);
    }

    #[test]
    fn methods_in_class() {
        let defs = index_source(b"class Foo\n  def bar\n  end\nend");
        assert_eq!(defs[1].fqn, "Foo#bar");
        assert_eq!(defs[1].kind, DefinitionKind::Method);
    }

    #[test]
    fn singleton_method() {
        let defs = index_source(b"class Foo\n  def self.bar\n  end\nend");
        assert_eq!(defs[1].fqn, "Foo.bar");
    }

    #[test]
    fn deeply_nested() {
        let defs = index_source(b"module A\n  module B\n    class C\n    end\n  end\nend");
        let fqns: Vec<&str> = defs.iter().map(|d| d.fqn.as_str()).collect();
        assert_eq!(fqns, ["A", "A::B", "A::B::C"]);
    }

    #[test]
    fn mixed_inline_and_nested() {
        let defs = index_source(b"module A\n  class B::C\n  end\nend");
        let fqns: Vec<&str> = defs.iter().map(|d| d.fqn.as_str()).collect();
        assert_eq!(fqns, ["A", "A::B::C"]);
    }

    #[test]
    fn reopened_class() {
        let src = b"class Foo\n  def a\n  end\nend\nclass Foo\n  def b\n  end\nend";
        let defs = index_source(src);
        assert!(defs.iter().any(|d| d.fqn == "Foo#a"));
        assert!(defs.iter().any(|d| d.fqn == "Foo#b"));
    }
}
