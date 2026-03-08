// tup-graph: DAG graph engine for the tup build system
//
// This crate implements the dependency graph that drives tup's build
// system. Nodes represent files, commands, directories, and groups.
// Edges represent dependencies between them.

mod dot;
mod graph;

pub use dot::{generate_dot, rules_to_dot, DotOptions};
pub use graph::{Edge, Graph, GraphNode, NodeState, TransientState};
