use ruby_prism::{Node, Visit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionKind {
    Module,
    Class,
    Method,
    Constant,
    InstanceVariable,
}

#[derive(Debug)]
pub struct Definition {
    /// Fully qualified name (e.g. "Foo::Bar", "Foo#bar", "Foo.bar").
    pub fqn: String,
    pub kind: DefinitionKind,
    /// Byte offset of the definition in the source file.
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
    } else if let Some(n) = node.as_constant_write_node() {
        let name = std::str::from_utf8(n.name().as_slice())
            .unwrap()
            .to_string();
        let fqn = build_fqn(namespace, &[name]);
        defs.push(Definition {
            fqn,
            kind: DefinitionKind::Constant,
            offset: n.location().start_offset(),
        });
    } else if let Some(n) = node.as_constant_path_write_node() {
        let target = n.target();
        let parts = resolve_constant_path(&target.as_node());
        if !parts.is_empty() {
            let fqn = build_fqn(namespace, &parts);
            defs.push(Definition {
                fqn,
                kind: DefinitionKind::Constant,
                offset: n.location().start_offset(),
            });
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
        if let Some(body) = n.body() {
            collect_ivar_defs(&body, namespace, defs);
        }
    }
}

/// Walk a method body using the Visit trait to find instance variable assignments.
fn collect_ivar_defs(node: &Node<'_>, namespace: &[String], defs: &mut Vec<Definition>) {
    let owner = namespace.join("::");
    let mut collector = IvarCollector {
        owner,
        defs: Vec::new(),
    };
    collector.visit(node);
    defs.append(&mut collector.defs);
}

struct IvarCollector {
    owner: String,
    defs: Vec<Definition>,
}

impl IvarCollector {
    fn record(&mut self, name_bytes: &[u8], offset: usize) {
        let name = std::str::from_utf8(name_bytes).unwrap();
        let name = name.strip_prefix('@').unwrap_or(name);
        let fqn = if self.owner.is_empty() {
            format!("#@{name}")
        } else {
            format!("{}#@{name}", self.owner)
        };
        self.defs.push(Definition {
            fqn,
            kind: DefinitionKind::InstanceVariable,
            offset,
        });
    }
}

impl<'pr> Visit<'pr> for IvarCollector {
    fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
        if let Some(n) = node.as_instance_variable_write_node() {
            self.record(n.name().as_slice(), n.location().start_offset());
        } else if let Some(n) = node.as_instance_variable_operator_write_node() {
            self.record(n.name().as_slice(), n.location().start_offset());
        } else if let Some(n) = node.as_instance_variable_and_write_node() {
            self.record(n.name().as_slice(), n.location().start_offset());
        } else if let Some(n) = node.as_instance_variable_or_write_node() {
            self.record(n.name().as_slice(), n.location().start_offset());
        }
    }
}

pub fn resolve_constant_path(node: &Node<'_>) -> Vec<String> {
    if let Some(n) = node.as_constant_read_node() {
        vec![
            std::str::from_utf8(n.name().as_slice())
                .unwrap()
                .to_string(),
        ]
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
        let defs = index_source(
            b"\
module Foo
  module Bar
  end
end",
        );
        assert_eq!(defs[0].fqn, "Foo");
        assert_eq!(defs[0].kind, DefinitionKind::Module);
        assert_eq!(defs[1].fqn, "Foo::Bar");
        assert_eq!(defs[1].kind, DefinitionKind::Module);
    }

    #[test]
    fn inline_constant_path() {
        let defs = index_source(
            b"\
class Foo::Bar < Base
end",
        );
        assert_eq!(defs[0].fqn, "Foo::Bar");
        assert_eq!(defs[0].kind, DefinitionKind::Class);
    }

    #[test]
    fn methods_in_class() {
        let defs = index_source(
            b"\
class Foo
  def bar
  end
end",
        );
        assert_eq!(defs[1].fqn, "Foo#bar");
        assert_eq!(defs[1].kind, DefinitionKind::Method);
    }

    #[test]
    fn singleton_method() {
        let defs = index_source(
            b"\
class Foo
  def self.bar
  end
end",
        );
        assert_eq!(defs[1].fqn, "Foo.bar");
    }

    #[test]
    fn deeply_nested() {
        let defs = index_source(
            b"\
module A
  module B
    class C
    end
  end
end",
        );
        let fqns: Vec<&str> = defs.iter().map(|d| d.fqn.as_str()).collect();
        assert_eq!(fqns, ["A", "A::B", "A::B::C"]);
    }

    #[test]
    fn mixed_inline_and_nested() {
        let defs = index_source(
            b"\
module A
  class B::C
  end
end",
        );
        let fqns: Vec<&str> = defs.iter().map(|d| d.fqn.as_str()).collect();
        assert_eq!(fqns, ["A", "A::B::C"]);
    }

    #[test]
    fn constant_in_class() {
        let defs = index_source(
            b"\
class Foo
  ABC = 1
end",
        );
        assert_eq!(defs[1].fqn, "Foo::ABC");
        assert_eq!(defs[1].kind, DefinitionKind::Constant);
    }

    #[test]
    fn top_level_constant() {
        let defs = index_source(b"ABC = 1");
        assert_eq!(defs[0].fqn, "ABC");
        assert_eq!(defs[0].kind, DefinitionKind::Constant);
    }

    #[test]
    fn constant_path_write() {
        let defs = index_source(b"Foo::BAR = 2");
        assert_eq!(defs[0].fqn, "Foo::BAR");
        assert_eq!(defs[0].kind, DefinitionKind::Constant);
    }

    #[test]
    fn constant_in_nested_module() {
        let defs = index_source(
            b"\
module A
  module B
    X = 1
  end
end",
        );
        let fqns: Vec<&str> = defs.iter().map(|d| d.fqn.as_str()).collect();
        assert_eq!(fqns, ["A", "A::B", "A::B::X"]);
        assert_eq!(defs[2].kind, DefinitionKind::Constant);
    }

    #[test]
    fn reopened_class() {
        let src = b"\
class Foo
  def a
  end
end
class Foo
  def b
  end
end";
        let defs = index_source(src);
        assert!(defs.iter().any(|d| d.fqn == "Foo#a"));
        assert!(defs.iter().any(|d| d.fqn == "Foo#b"));
    }

    #[test]
    fn instance_variable_in_method() {
        let src = b"\
class User
  def initialize(name)
    @name = name
  end
end";
        let ivars: Vec<_> = index_source(src)
            .into_iter()
            .filter(|d| d.kind == DefinitionKind::InstanceVariable)
            .collect();
        assert_eq!(ivars.len(), 1);
        assert_eq!(ivars[0].fqn, "User#@name");
    }

    #[test]
    fn instance_variable_multiple_methods() {
        let src = b"\
class Foo
  def a
    @x = 1
  end
  def b
    @y = 2
  end
end";
        let ivars: Vec<_> = index_source(src)
            .into_iter()
            .filter(|d| d.kind == DefinitionKind::InstanceVariable)
            .collect();
        let fqns: Vec<&str> = ivars.iter().map(|d| d.fqn.as_str()).collect();
        assert!(fqns.contains(&"Foo#@x"));
        assert!(fqns.contains(&"Foo#@y"));
    }

    #[test]
    fn instance_variable_operator_write() {
        let src = b"\
class Foo
  def bar
    @count += 1
  end
end";
        let ivars: Vec<_> = index_source(src)
            .into_iter()
            .filter(|d| d.kind == DefinitionKind::InstanceVariable)
            .collect();
        assert_eq!(ivars.len(), 1);
        assert_eq!(ivars[0].fqn, "Foo#@count");
    }

    #[test]
    fn instance_variable_or_write() {
        let src = b"\
class Foo
  def bar
    @cache ||= compute
  end
end";
        let ivars: Vec<_> = index_source(src)
            .into_iter()
            .filter(|d| d.kind == DefinitionKind::InstanceVariable)
            .collect();
        assert_eq!(ivars.len(), 1);
        assert_eq!(ivars[0].fqn, "Foo#@cache");
    }

    #[test]
    fn instance_variable_in_nested_class() {
        let src = b"\
module A
  class B
    def foo
      @val = 1
    end
  end
end";
        let ivars: Vec<_> = index_source(src)
            .into_iter()
            .filter(|d| d.kind == DefinitionKind::InstanceVariable)
            .collect();
        assert_eq!(ivars.len(), 1);
        assert_eq!(ivars[0].fqn, "A::B#@val");
    }

    #[test]
    fn instance_variable_duplicate_across_methods() {
        let src = b"\
class Foo
  def a
    @x = 1
  end
  def b
    @x = 2
  end
end";
        let ivars: Vec<_> = index_source(src)
            .into_iter()
            .filter(|d| d.kind == DefinitionKind::InstanceVariable)
            .collect();
        // Both assignments are indexed (same FQN, different offsets)
        assert_eq!(ivars.len(), 2);
        assert!(ivars.iter().all(|d| d.fqn == "Foo#@x"));
    }
}
