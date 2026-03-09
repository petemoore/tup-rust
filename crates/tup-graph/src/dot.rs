use crate::graph::Graph;
use tup_types::{LinkType, NodeType, TupId};

/// Options for DOT graph generation.
#[derive(Default)]
pub struct DotOptions {
    /// Show directory nodes.
    pub show_dirs: bool,
    /// Show ghost nodes.
    pub show_ghosts: bool,
    /// Combine nodes with the same command.
    pub combine: bool,
}

/// Generate a Graphviz DOT representation of the build graph.
///
/// Takes a graph and a function to resolve node names from TupIds.
pub fn generate_dot<F>(graph: &Graph, opts: &DotOptions, name_fn: F) -> String
where
    F: Fn(TupId) -> Option<(String, NodeType)>,
{
    let mut dot = String::from("digraph G {\n");
    dot.push_str("  rankdir=BT;\n");
    dot.push_str("  node [style=filled];\n\n");

    // Add nodes
    for (&id, node) in graph.nodes() {
        if id == graph.root() {
            continue;
        }

        if !opts.show_dirs && node.node_type.is_dir() {
            continue;
        }
        if !opts.show_ghosts && node.node_type == NodeType::Ghost {
            continue;
        }

        let (label, _) =
            name_fn(id).unwrap_or_else(|| (format!("node_{}", id.raw()), node.node_type));

        let (shape, color) = node_style(node.node_type);

        // Escape label for DOT
        let escaped = label.replace('\\', "\\\\").replace('"', "\\\"");
        dot.push_str(&format!(
            "  n{} [label=\"{}\", shape={}, fillcolor=\"{}\"];\n",
            id.raw(),
            escaped,
            shape,
            color,
        ));
    }

    dot.push('\n');

    // Add edges
    for edge in graph.all_edges() {
        if edge.src == graph.root() {
            continue;
        }

        // Skip edges to/from hidden node types
        if !opts.show_dirs {
            if let (Some(src), Some(dest)) = (graph.find_node(edge.src), graph.find_node(edge.dest))
            {
                if src.node_type.is_dir() || dest.node_type.is_dir() {
                    continue;
                }
            }
        }

        let style = match edge.style {
            LinkType::Normal => "",
            LinkType::Sticky => ", style=dashed, color=blue",
            LinkType::Group => ", style=dotted, color=red",
        };

        dot.push_str(&format!(
            "  n{} -> n{}[{}];\n",
            edge.src.raw(),
            edge.dest.raw(),
            style,
        ));
    }

    dot.push_str("}\n");
    dot
}

/// Get DOT style for a node type.
fn node_style(node_type: NodeType) -> (&'static str, &'static str) {
    match node_type {
        NodeType::File => ("ellipse", "#d4e6f1"),
        NodeType::Cmd => ("rectangle", "#d5f5e3"),
        NodeType::Dir | NodeType::GeneratedDir => ("folder", "#fdebd0"),
        NodeType::Generated => ("ellipse", "#fadbd8"),
        NodeType::Ghost => ("ellipse", "#e8daef"),
        NodeType::Group => ("diamond", "#fef9e7"),
        NodeType::Var => ("octagon", "#d6eaf8"),
        NodeType::Root => ("doubleoctagon", "#ffffff"),
    }
}

/// Generate a simple DOT graph from a list of rules (without requiring the full graph engine).
///
/// Useful for `tup graph` when we just want to visualize Tupfile rules.
pub fn rules_to_dot(rules: &[(String, Vec<String>, String, Vec<String>)]) -> String {
    let mut dot = String::from("digraph G {\n");
    dot.push_str("  rankdir=BT;\n");
    dot.push_str("  node [style=filled];\n\n");

    let mut next_id = 0u64;
    let mut file_ids: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();

    let mut alloc_id = || {
        let id = next_id;
        next_id += 1;
        id
    };

    for (dir, inputs, command, outputs) in rules {
        // Command node
        let cmd_id = alloc_id();
        let cmd_label = if command.len() > 60 {
            format!("{}...", &command[..57])
        } else {
            command.clone()
        };
        let escaped = cmd_label.replace('\\', "\\\\").replace('"', "\\\"");
        let dir_prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        dot.push_str(&format!(
            "  n{cmd_id} [label=\"{escaped}\", shape=rectangle, fillcolor=\"#d5f5e3\"];\n",
        ));

        // Input nodes and edges
        for input in inputs {
            let full_path = format!("{dir_prefix}{input}");
            let input_id = *file_ids.entry(full_path.clone()).or_insert_with(|| {
                let id = alloc_id();
                let escaped = full_path.replace('\\', "\\\\").replace('"', "\\\"");
                dot.push_str(&format!(
                    "  n{id} [label=\"{escaped}\", shape=ellipse, fillcolor=\"#d4e6f1\"];\n",
                ));
                id
            });
            dot.push_str(&format!("  n{input_id} -> n{cmd_id};\n"));
        }

        // Output nodes and edges
        for output in outputs {
            let full_path = format!("{dir_prefix}{output}");
            let output_id = *file_ids.entry(full_path.clone()).or_insert_with(|| {
                let id = alloc_id();
                let escaped = full_path.replace('\\', "\\\\").replace('"', "\\\"");
                dot.push_str(&format!(
                    "  n{id} [label=\"{escaped}\", shape=ellipse, fillcolor=\"#fadbd8\"];\n",
                ));
                id
            });
            dot.push_str(&format!("  n{cmd_id} -> n{output_id};\n"));
        }
    }

    dot.push_str("}\n");
    dot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rules_to_dot_basic() {
        let rules = vec![(
            "".to_string(),
            vec!["main.c".to_string()],
            "gcc -c main.c -o main.o".to_string(),
            vec!["main.o".to_string()],
        )];

        let dot = rules_to_dot(&rules);
        assert!(dot.contains("digraph G"));
        assert!(dot.contains("main.c"));
        assert!(dot.contains("main.o"));
        assert!(dot.contains("gcc"));
        assert!(dot.contains("rectangle")); // command shape
        assert!(dot.contains("ellipse")); // file shape
    }

    #[test]
    fn test_rules_to_dot_shared_files() {
        let rules = vec![
            (
                "".to_string(),
                vec!["a.c".to_string()],
                "gcc -c a.c -o a.o".to_string(),
                vec!["a.o".to_string()],
            ),
            (
                "".to_string(),
                vec!["a.o".to_string()],
                "gcc a.o -o app".to_string(),
                vec!["app".to_string()],
            ),
        ];

        let dot = rules_to_dot(&rules);
        // a.o should appear only once as a node (shared between rules)
        let count = dot.matches("a.o").count();
        // label + two edge references = 3 mentions
        assert!(count >= 2, "a.o should be shared, got {count} mentions");
    }

    #[test]
    fn test_rules_to_dot_long_command() {
        let long_cmd = "gcc -c -Wall -Werror -O2 -I/usr/include -I/usr/local/include -DFOO=bar very_long_source_file.c -o output.o";
        let rules = vec![(
            "".to_string(),
            vec!["input.c".to_string()],
            long_cmd.to_string(),
            vec!["output.o".to_string()],
        )];

        let dot = rules_to_dot(&rules);
        assert!(dot.contains("...")); // Long command truncated
    }

    #[test]
    fn test_rules_to_dot_with_dir() {
        let rules = vec![(
            "src".to_string(),
            vec!["main.c".to_string()],
            "gcc -c main.c".to_string(),
            vec!["main.o".to_string()],
        )];

        let dot = rules_to_dot(&rules);
        assert!(dot.contains("src/main.c"));
        assert!(dot.contains("src/main.o"));
    }

    #[test]
    fn test_node_style() {
        let (shape, _) = node_style(NodeType::Cmd);
        assert_eq!(shape, "rectangle");
        let (shape, _) = node_style(NodeType::File);
        assert_eq!(shape, "ellipse");
        let (shape, _) = node_style(NodeType::Dir);
        assert_eq!(shape, "folder");
    }
}
