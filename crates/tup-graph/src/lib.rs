// tup-graph: DAG graph engine for the tup build system
//
// This crate implements the dependency graph that drives tup's build
// system. Nodes represent files, commands, directories, and groups.
// Edges represent dependencies between them.

mod graph;

pub use graph::{Edge, Graph, GraphNode, NodeState, TransientState};
