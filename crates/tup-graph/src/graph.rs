use std::collections::{BTreeMap, BTreeSet, VecDeque};

use tup_types::{LinkType, NodeType, TupId};

/// State of a node in the graph processing pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
    /// Node created, not yet processed.
    Initialized,
    /// Node dependencies are being discovered. Re-encountering this
    /// state indicates a circular dependency.
    Processing,
    /// Node processing complete.
    Finished,
    /// Node marked for removal.
    Removing,
}

/// Transient file state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransientState {
    /// Regular node.
    None,
    /// Transient generated file being evaluated.
    Processing,
    /// Unused transient file, can be deleted.
    Delete,
}

/// An edge in the dependency graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// Source node (dependency provider).
    pub src: TupId,
    /// Destination node (dependent).
    pub dest: TupId,
    /// Edge type (normal, sticky, or group).
    pub style: LinkType,
}

/// A node in the dependency graph.
///
/// Corresponds to `struct node` in C's graph.h.
#[derive(Debug)]
pub struct GraphNode {
    /// Unique node ID (matches TupEntry.id).
    pub id: TupId,
    /// Node type from the database.
    pub node_type: NodeType,
    /// Processing state.
    pub state: NodeState,
    /// Transient state (for generated files).
    pub transient: TransientState,
    /// Whether this node has been used in processing.
    pub already_used: bool,
    /// Whether dependencies have been expanded.
    pub expanded: bool,
    /// Whether node is currently being parsed.
    pub parsing: bool,
    /// Whether node is marked for retention during pruning.
    pub marked: bool,
    /// Whether to skip this node (initially true, set false if reachable).
    pub skip: bool,
    /// Whether this node has been counted in statistics.
    pub counted: bool,
}

impl GraphNode {
    /// Create a new graph node.
    fn new(id: TupId, node_type: NodeType) -> Self {
        GraphNode {
            id,
            node_type,
            state: NodeState::Initialized,
            transient: TransientState::None,
            already_used: false,
            expanded: false,
            parsing: false,
            marked: false,
            skip: true,
            counted: false,
        }
    }
}

/// The dependency graph (DAG).
///
/// Corresponds to `struct graph` in C's graph.h.
pub struct Graph {
    /// All nodes indexed by TupId.
    nodes: BTreeMap<TupId, GraphNode>,

    /// Outgoing edges for each node: src → [(dest, style)].
    edges_out: BTreeMap<TupId, Vec<(TupId, LinkType)>>,

    /// Incoming edges for each node: dest → [(src, style)].
    edges_in: BTreeMap<TupId, Vec<(TupId, LinkType)>>,

    /// Finished/active nodes list (ordered).
    node_list: VecDeque<TupId>,

    /// Pending nodes awaiting expansion.
    plist: VecDeque<TupId>,

    /// Nodes being removed.
    removing_list: Vec<TupId>,

    /// The virtual root node (tupid=0).
    root: TupId,

    /// Node type filter for counting (e.g., only count CMDs).
    count_flags: NodeType,

    /// Number of nodes matching count_flags.
    num_nodes: usize,

    /// Total mtime of counted nodes.
    #[allow(dead_code)]
    total_mtime: i64,
}

impl Graph {
    /// Create a new empty graph.
    ///
    /// `count_flags` determines which node types are counted in `num_nodes`.
    /// Use `NodeType::Root` to count all node types.
    pub fn new(count_flags: NodeType) -> Self {
        let root_id = TupId::new(0);
        let mut nodes = BTreeMap::new();
        nodes.insert(root_id, GraphNode::new(root_id, NodeType::Root));

        let mut node_list = VecDeque::new();
        node_list.push_back(root_id);

        Graph {
            nodes,
            edges_out: BTreeMap::new(),
            edges_in: BTreeMap::new(),
            node_list,
            plist: VecDeque::new(),
            removing_list: Vec::new(),
            root: root_id,
            count_flags,
            num_nodes: 0,
            total_mtime: 0,
        }
    }

    /// Get the root node ID.
    pub fn root(&self) -> TupId {
        self.root
    }

    /// Find a node by TupId.
    pub fn find_node(&self, id: TupId) -> Option<&GraphNode> {
        self.nodes.get(&id)
    }

    /// Find a mutable node by TupId.
    pub fn find_node_mut(&mut self, id: TupId) -> Option<&mut GraphNode> {
        self.nodes.get_mut(&id)
    }

    /// Check if a node exists in the graph.
    pub fn contains(&self, id: TupId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// Create a new node in the graph.
    ///
    /// Returns the node ID. If the node already exists, returns its ID
    /// without modification.
    pub fn create_node(&mut self, id: TupId, node_type: NodeType) -> TupId {
        if self.nodes.contains_key(&id) {
            return id;
        }

        let node = GraphNode::new(id, node_type);
        self.nodes.insert(id, node);
        self.node_list.push_back(id);

        // Update counting
        if self.should_count(node_type) {
            self.num_nodes += 1;
        }

        id
    }

    /// Create an edge between two nodes.
    ///
    /// Returns an error if the destination is in Processing state
    /// (circular dependency detected).
    pub fn create_edge(
        &mut self,
        src: TupId,
        dest: TupId,
        style: LinkType,
    ) -> Result<(), CircularDependencyError> {
        // Check for circular dependency
        if let Some(dest_node) = self.nodes.get(&dest) {
            if dest_node.state == NodeState::Processing {
                return Err(CircularDependencyError { src, dest, style });
            }
        }

        // Add outgoing edge
        self.edges_out.entry(src).or_default().push((dest, style));

        // Add incoming edge
        self.edges_in.entry(dest).or_default().push((src, style));

        Ok(())
    }

    /// Remove all edges for a node (both incoming and outgoing).
    pub fn remove_edges(&mut self, id: TupId) {
        // Remove outgoing edges and their incoming counterparts
        if let Some(out_edges) = self.edges_out.remove(&id) {
            for (dest, _) in &out_edges {
                if let Some(in_edges) = self.edges_in.get_mut(dest) {
                    in_edges.retain(|(src, _)| *src != id);
                }
            }
        }

        // Remove incoming edges and their outgoing counterparts
        if let Some(in_edges) = self.edges_in.remove(&id) {
            for (src, _) in &in_edges {
                if let Some(out_edges) = self.edges_out.get_mut(src) {
                    out_edges.retain(|(dest, _)| *dest != id);
                }
            }
        }
    }

    /// Remove a node from the graph entirely.
    pub fn remove_node(&mut self, id: TupId) {
        self.remove_edges(id);
        if let Some(node) = self.nodes.remove(&id) {
            if self.should_count(node.node_type) && node.counted {
                self.num_nodes = self.num_nodes.saturating_sub(1);
            }
        }
        self.node_list.retain(|n| *n != id);
        self.plist.retain(|n| *n != id);
        self.removing_list.retain(|n| *n != id);
    }

    /// Get outgoing edges for a node.
    pub fn outgoing_edges(&self, id: TupId) -> &[(TupId, LinkType)] {
        self.edges_out.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get incoming edges for a node.
    pub fn incoming_edges(&self, id: TupId) -> &[(TupId, LinkType)] {
        self.edges_in.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Check if the graph has no real nodes (only root).
    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1 // Just root
    }

    /// Get the number of nodes (excluding root).
    pub fn node_count(&self) -> usize {
        self.nodes.len().saturating_sub(1)
    }

    /// Get the number of counted nodes.
    pub fn counted_nodes(&self) -> usize {
        self.num_nodes
    }

    /// Push a node to the processing list (plist).
    pub fn push_plist(&mut self, id: TupId) {
        self.plist.push_front(id);
    }

    /// Pop a node from the processing list.
    pub fn pop_plist(&mut self) -> Option<TupId> {
        self.plist.pop_front()
    }

    /// Check if the processing list is empty.
    pub fn plist_is_empty(&self) -> bool {
        self.plist.is_empty()
    }

    /// Move a node to the finished list.
    pub fn finish_node(&mut self, id: TupId) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.state = NodeState::Finished;
        }
        self.plist.retain(|n| *n != id);
        if !self.node_list.contains(&id) {
            self.node_list.push_back(id);
        }
    }

    /// Get all node IDs in the finished list.
    pub fn finished_nodes(&self) -> &VecDeque<TupId> {
        &self.node_list
    }

    /// Iterate over all nodes.
    pub fn nodes(&self) -> impl Iterator<Item = (&TupId, &GraphNode)> {
        self.nodes.iter()
    }

    /// Iterate over all nodes mutably.
    pub fn nodes_mut(&mut self) -> impl Iterator<Item = (&TupId, &mut GraphNode)> {
        self.nodes.iter_mut()
    }

    /// Get all edges in the graph.
    pub fn all_edges(&self) -> Vec<Edge> {
        let mut edges = Vec::new();
        for (src, dests) in &self.edges_out {
            for (dest, style) in dests {
                edges.push(Edge {
                    src: *src,
                    dest: *dest,
                    style: *style,
                });
            }
        }
        edges
    }

    /// Trim the graph by repeatedly removing leaf nodes.
    ///
    /// A leaf node has no incoming edges or no outgoing edges.
    /// After trimming, remaining nodes form cycles.
    ///
    /// Corresponds to `trim_graph()` in C.
    pub fn trim(&mut self) {
        loop {
            let to_remove: Vec<TupId> = self
                .nodes
                .keys()
                .filter(|&&id| id != self.root)
                .filter(|&&id| {
                    let has_in = self.edges_in.get(&id).is_some_and(|e| !e.is_empty());
                    let has_out = self.edges_out.get(&id).is_some_and(|e| !e.is_empty());
                    !has_in || !has_out
                })
                .copied()
                .collect();

            if to_remove.is_empty() {
                break;
            }

            for id in to_remove {
                self.remove_node(id);
            }
        }
    }

    /// Prune the graph to only include nodes reachable from the given targets.
    ///
    /// Corresponds to `prune_graph()` in C. Marks target nodes and their
    /// transitive dependencies, then removes everything else.
    ///
    /// Returns the number of nodes pruned.
    pub fn prune(&mut self, targets: &[TupId]) -> usize {
        if targets.is_empty() {
            return 0;
        }

        // Mark all target nodes and their transitive dependencies
        let mut marked: BTreeSet<TupId> = BTreeSet::new();
        let mut stack: Vec<TupId> = targets.to_vec();

        while let Some(id) = stack.pop() {
            if marked.contains(&id) {
                continue;
            }
            marked.insert(id);

            // Mark all dependencies (incoming edges = things this node depends on)
            for &(dep, _) in self.incoming_edges(id) {
                if !marked.contains(&dep) {
                    stack.push(dep);
                }
            }

            // For CMD nodes, also mark all outputs (outgoing edges)
            if let Some(node) = self.nodes.get(&id) {
                if node.node_type == NodeType::Cmd {
                    for &(out, _) in self.outgoing_edges(id) {
                        if !marked.contains(&out) {
                            stack.push(out);
                        }
                    }
                }
            }
        }

        // Always keep root
        marked.insert(self.root);

        // Remove unmarked nodes
        let to_remove: Vec<TupId> = self
            .nodes
            .keys()
            .filter(|id| !marked.contains(id))
            .copied()
            .collect();

        let count = to_remove.len();
        for id in to_remove {
            self.remove_node(id);
        }

        count
    }

    /// Get all nodes that have no dependencies (leaf inputs).
    pub fn leaf_nodes(&self) -> Vec<TupId> {
        self.nodes
            .keys()
            .filter(|&&id| id != self.root)
            .filter(|&&id| self.incoming_edges(id).is_empty())
            .copied()
            .collect()
    }

    /// Get all nodes ready to execute (all dependencies finished).
    pub fn ready_nodes(&self) -> Vec<TupId> {
        self.nodes
            .iter()
            .filter(|(&id, node)| {
                id != self.root
                    && node.state == NodeState::Initialized
                    && self.incoming_edges(id).iter().all(|&(dep, _)| {
                        self.nodes
                            .get(&dep)
                            .map(|n| n.state == NodeState::Finished)
                            .unwrap_or(true)
                    })
            })
            .map(|(&id, _)| id)
            .collect()
    }

    /// Generate a Graphviz DOT representation.
    pub fn to_dot(&self) -> String {
        let mut dot = String::from("digraph G {\n");
        dot.push_str("  rankdir=TB;\n");

        for (id, node) in &self.nodes {
            if *id == self.root {
                continue;
            }
            let shape = match node.node_type {
                NodeType::Cmd => "rectangle",
                NodeType::Dir | NodeType::GeneratedDir => "folder",
                NodeType::Group => "diamond",
                _ => "ellipse",
            };
            dot.push_str(&format!(
                "  n{} [label=\"{}\", shape={}];\n",
                id.raw(),
                id,
                shape,
            ));
        }

        for edge in self.all_edges() {
            if edge.src == self.root {
                continue;
            }
            let style = match edge.style {
                LinkType::Normal => "",
                LinkType::Sticky => ", style=dashed",
                LinkType::Group => ", style=dotted",
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

    /// Check if `should_count` applies to a node type based on count_flags.
    fn should_count(&self, node_type: NodeType) -> bool {
        if self.count_flags == NodeType::Root {
            // Root means count everything
            true
        } else {
            node_type == self.count_flags
        }
    }

    /// Perform a topological sort of the graph.
    ///
    /// Returns nodes in dependency order (dependencies before dependents).
    /// Returns None if the graph has cycles.
    pub fn topological_sort(&self) -> Option<Vec<TupId>> {
        let mut in_degree: BTreeMap<TupId, usize> = BTreeMap::new();

        // Initialize in-degrees
        for &id in self.nodes.keys() {
            in_degree.insert(id, 0);
        }
        for edges in self.edges_in.values() {
            for (_, _) in edges {
                // Each incoming edge contributes to in-degree
            }
        }
        for (dest, edges) in &self.edges_in {
            if let Some(deg) = in_degree.get_mut(dest) {
                *deg = edges.len();
            }
        }

        // Start with nodes that have no incoming edges
        let mut queue: VecDeque<TupId> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut result = Vec::new();

        while let Some(id) = queue.pop_front() {
            result.push(id);

            if let Some(out_edges) = self.edges_out.get(&id) {
                for (dest, _) in out_edges {
                    if let Some(deg) = in_degree.get_mut(dest) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            queue.push_back(*dest);
                        }
                    }
                }
            }
        }

        if result.len() == self.nodes.len() {
            Some(result)
        } else {
            None // Cycle detected
        }
    }

    /// Detect if there are any cycles in the graph.
    pub fn has_cycles(&self) -> bool {
        self.topological_sort().is_none()
    }

    /// Find nodes that form cycles (after trimming leaves).
    pub fn find_cycle_nodes(&self) -> BTreeSet<TupId> {
        let mut trimmed = Graph::new(self.count_flags);

        // Copy all nodes and edges
        for (&id, node) in &self.nodes {
            if id == self.root {
                continue;
            }
            trimmed.create_node(id, node.node_type);
        }
        for (src, dests) in &self.edges_out {
            for (dest, style) in dests {
                if *src != self.root {
                    let _ = trimmed.create_edge(*src, *dest, *style);
                }
            }
        }

        // Trim leaves — remaining nodes are in cycles
        trimmed.trim();

        trimmed
            .nodes
            .keys()
            .filter(|&&id| id != trimmed.root)
            .copied()
            .collect()
    }
}

/// Error returned when a circular dependency is detected.
#[derive(Debug)]
pub struct CircularDependencyError {
    pub src: TupId,
    pub dest: TupId,
    pub style: LinkType,
}

impl std::fmt::Display for CircularDependencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "circular dependency: {} -> {} ({})",
            self.src, self.dest, self.style
        )
    }
}

impl std::error::Error for CircularDependencyError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_graph() {
        let g = Graph::new(NodeType::Cmd);
        assert!(g.is_empty());
        assert_eq!(g.node_count(), 0);
        assert!(g.contains(TupId::new(0))); // Root exists
    }

    #[test]
    fn test_create_node() {
        let mut g = Graph::new(NodeType::Cmd);
        g.create_node(TupId::new(1), NodeType::File);
        g.create_node(TupId::new(2), NodeType::Cmd);

        assert_eq!(g.node_count(), 2);
        assert!(g.contains(TupId::new(1)));
        assert!(g.contains(TupId::new(2)));
        assert!(!g.contains(TupId::new(99)));
    }

    #[test]
    fn test_create_node_idempotent() {
        let mut g = Graph::new(NodeType::Cmd);
        g.create_node(TupId::new(1), NodeType::File);
        g.create_node(TupId::new(1), NodeType::File);
        assert_eq!(g.node_count(), 1);
    }

    #[test]
    fn test_create_edge() {
        let mut g = Graph::new(NodeType::Cmd);
        g.create_node(TupId::new(1), NodeType::File);
        g.create_node(TupId::new(2), NodeType::Cmd);

        g.create_edge(TupId::new(1), TupId::new(2), LinkType::Normal)
            .unwrap();

        let out = g.outgoing_edges(TupId::new(1));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], (TupId::new(2), LinkType::Normal));

        let inc = g.incoming_edges(TupId::new(2));
        assert_eq!(inc.len(), 1);
        assert_eq!(inc[0], (TupId::new(1), LinkType::Normal));
    }

    #[test]
    fn test_circular_dependency_detection() {
        let mut g = Graph::new(NodeType::Cmd);
        let a = TupId::new(1);
        let b = TupId::new(2);
        g.create_node(a, NodeType::Cmd);
        g.create_node(b, NodeType::Cmd);

        // Set node a to Processing state
        g.find_node_mut(a).unwrap().state = NodeState::Processing;

        // Trying to create edge to a Processing node should fail
        let result = g.create_edge(b, a, LinkType::Normal);
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_node() {
        let mut g = Graph::new(NodeType::Cmd);
        g.create_node(TupId::new(1), NodeType::File);
        g.create_node(TupId::new(2), NodeType::Cmd);
        g.create_edge(TupId::new(1), TupId::new(2), LinkType::Normal)
            .unwrap();

        g.remove_node(TupId::new(1));
        assert!(!g.contains(TupId::new(1)));
        assert!(g.incoming_edges(TupId::new(2)).is_empty());
    }

    #[test]
    fn test_remove_edges() {
        let mut g = Graph::new(NodeType::Cmd);
        g.create_node(TupId::new(1), NodeType::File);
        g.create_node(TupId::new(2), NodeType::Cmd);
        g.create_node(TupId::new(3), NodeType::Generated);
        g.create_edge(TupId::new(1), TupId::new(2), LinkType::Normal)
            .unwrap();
        g.create_edge(TupId::new(2), TupId::new(3), LinkType::Normal)
            .unwrap();

        g.remove_edges(TupId::new(2));
        assert!(g.outgoing_edges(TupId::new(2)).is_empty());
        assert!(g.incoming_edges(TupId::new(2)).is_empty());
        // Other nodes should still exist
        assert!(g.contains(TupId::new(1)));
        assert!(g.contains(TupId::new(3)));
    }

    #[test]
    fn test_plist_operations() {
        let mut g = Graph::new(NodeType::Cmd);
        let a = TupId::new(1);
        let b = TupId::new(2);
        g.create_node(a, NodeType::Cmd);
        g.create_node(b, NodeType::Cmd);

        assert!(g.plist_is_empty());

        g.push_plist(a);
        g.push_plist(b);
        assert!(!g.plist_is_empty());

        // LIFO order
        assert_eq!(g.pop_plist(), Some(b));
        assert_eq!(g.pop_plist(), Some(a));
        assert!(g.plist_is_empty());
    }

    #[test]
    fn test_finish_node() {
        let mut g = Graph::new(NodeType::Cmd);
        let id = TupId::new(1);
        g.create_node(id, NodeType::Cmd);
        g.push_plist(id);

        g.finish_node(id);
        assert_eq!(g.find_node(id).unwrap().state, NodeState::Finished);
        assert!(g.plist_is_empty());
    }

    #[test]
    fn test_topological_sort_simple() {
        let mut g = Graph::new(NodeType::Cmd);
        let a = TupId::new(1);
        let b = TupId::new(2);
        let c = TupId::new(3);
        g.create_node(a, NodeType::File);
        g.create_node(b, NodeType::Cmd);
        g.create_node(c, NodeType::Generated);
        g.create_edge(a, b, LinkType::Normal).unwrap();
        g.create_edge(b, c, LinkType::Normal).unwrap();

        let sorted = g.topological_sort().unwrap();
        let pos_a = sorted.iter().position(|&x| x == a).unwrap();
        let pos_b = sorted.iter().position(|&x| x == b).unwrap();
        let pos_c = sorted.iter().position(|&x| x == c).unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    #[test]
    fn test_topological_sort_cycle() {
        let mut g = Graph::new(NodeType::Cmd);
        let a = TupId::new(1);
        let b = TupId::new(2);
        g.create_node(a, NodeType::Cmd);
        g.create_node(b, NodeType::Cmd);
        // Manually create cycle (bypass Processing check)
        g.edges_out
            .entry(a)
            .or_default()
            .push((b, LinkType::Normal));
        g.edges_in.entry(b).or_default().push((a, LinkType::Normal));
        g.edges_out
            .entry(b)
            .or_default()
            .push((a, LinkType::Normal));
        g.edges_in.entry(a).or_default().push((b, LinkType::Normal));

        assert!(g.has_cycles());
        assert!(g.topological_sort().is_none());
    }

    #[test]
    fn test_find_cycle_nodes() {
        let mut g = Graph::new(NodeType::Cmd);
        let a = TupId::new(1);
        let b = TupId::new(2);
        let c = TupId::new(3); // Not in cycle
        g.create_node(a, NodeType::Cmd);
        g.create_node(b, NodeType::Cmd);
        g.create_node(c, NodeType::File);

        // Create a→b→a cycle
        g.edges_out
            .entry(a)
            .or_default()
            .push((b, LinkType::Normal));
        g.edges_in.entry(b).or_default().push((a, LinkType::Normal));
        g.edges_out
            .entry(b)
            .or_default()
            .push((a, LinkType::Normal));
        g.edges_in.entry(a).or_default().push((b, LinkType::Normal));

        // c is a leaf, not in cycle
        g.edges_out
            .entry(c)
            .or_default()
            .push((a, LinkType::Normal));
        g.edges_in.entry(a).or_default().push((c, LinkType::Normal));

        let cycle_nodes = g.find_cycle_nodes();
        assert!(cycle_nodes.contains(&a));
        assert!(cycle_nodes.contains(&b));
        assert!(!cycle_nodes.contains(&c));
    }

    #[test]
    fn test_trim_removes_leaves() {
        let mut g = Graph::new(NodeType::Cmd);
        let a = TupId::new(1);
        let b = TupId::new(2);
        let c = TupId::new(3);
        g.create_node(a, NodeType::File);
        g.create_node(b, NodeType::Cmd);
        g.create_node(c, NodeType::Generated);
        g.create_edge(a, b, LinkType::Normal).unwrap();
        g.create_edge(b, c, LinkType::Normal).unwrap();

        // All nodes are leaves (a has no incoming, c has no outgoing)
        g.trim();
        assert!(g.is_empty()); // Only root left
    }

    #[test]
    fn test_trim_preserves_cycles() {
        let mut g = Graph::new(NodeType::Cmd);
        let a = TupId::new(1);
        let b = TupId::new(2);
        g.create_node(a, NodeType::Cmd);
        g.create_node(b, NodeType::Cmd);

        // Create cycle
        g.edges_out
            .entry(a)
            .or_default()
            .push((b, LinkType::Normal));
        g.edges_in.entry(b).or_default().push((a, LinkType::Normal));
        g.edges_out
            .entry(b)
            .or_default()
            .push((a, LinkType::Normal));
        g.edges_in.entry(a).or_default().push((b, LinkType::Normal));

        g.trim();
        assert!(g.contains(a));
        assert!(g.contains(b));
    }

    #[test]
    fn test_counting() {
        let mut g = Graph::new(NodeType::Cmd);
        g.create_node(TupId::new(1), NodeType::File);
        g.create_node(TupId::new(2), NodeType::Cmd);
        g.create_node(TupId::new(3), NodeType::Cmd);
        g.create_node(TupId::new(4), NodeType::Generated);

        // Only CMD nodes counted
        assert_eq!(g.counted_nodes(), 2);
    }

    #[test]
    fn test_counting_all() {
        let mut g = Graph::new(NodeType::Root); // Root = count all
        g.create_node(TupId::new(1), NodeType::File);
        g.create_node(TupId::new(2), NodeType::Cmd);
        assert_eq!(g.counted_nodes(), 2);
    }

    #[test]
    fn test_all_edges() {
        let mut g = Graph::new(NodeType::Cmd);
        g.create_node(TupId::new(1), NodeType::File);
        g.create_node(TupId::new(2), NodeType::Cmd);
        g.create_node(TupId::new(3), NodeType::Generated);
        g.create_edge(TupId::new(1), TupId::new(2), LinkType::Normal)
            .unwrap();
        g.create_edge(TupId::new(2), TupId::new(3), LinkType::Normal)
            .unwrap();
        g.create_edge(TupId::new(1), TupId::new(2), LinkType::Sticky)
            .unwrap();

        let edges = g.all_edges();
        assert_eq!(edges.len(), 3);
    }

    #[test]
    fn test_to_dot() {
        let mut g = Graph::new(NodeType::Cmd);
        g.create_node(TupId::new(1), NodeType::File);
        g.create_node(TupId::new(2), NodeType::Cmd);
        g.create_edge(TupId::new(1), TupId::new(2), LinkType::Normal)
            .unwrap();

        let dot = g.to_dot();
        assert!(dot.contains("digraph G"));
        assert!(dot.contains("n1"));
        assert!(dot.contains("n2"));
        assert!(dot.contains("n1 -> n2"));
    }

    #[test]
    fn test_node_state_transitions() {
        let mut g = Graph::new(NodeType::Cmd);
        let id = TupId::new(1);
        g.create_node(id, NodeType::Cmd);

        assert_eq!(g.find_node(id).unwrap().state, NodeState::Initialized);

        g.find_node_mut(id).unwrap().state = NodeState::Processing;
        assert_eq!(g.find_node(id).unwrap().state, NodeState::Processing);

        g.finish_node(id);
        assert_eq!(g.find_node(id).unwrap().state, NodeState::Finished);
    }

    #[test]
    fn test_diamond_dependency() {
        // A → B, A → C, B → D, C → D
        let mut g = Graph::new(NodeType::Cmd);
        let a = TupId::new(1);
        let b = TupId::new(2);
        let c = TupId::new(3);
        let d = TupId::new(4);
        g.create_node(a, NodeType::File);
        g.create_node(b, NodeType::Cmd);
        g.create_node(c, NodeType::Cmd);
        g.create_node(d, NodeType::Generated);
        g.create_edge(a, b, LinkType::Normal).unwrap();
        g.create_edge(a, c, LinkType::Normal).unwrap();
        g.create_edge(b, d, LinkType::Normal).unwrap();
        g.create_edge(c, d, LinkType::Normal).unwrap();

        let sorted = g.topological_sort().unwrap();
        let pos_a = sorted.iter().position(|&x| x == a).unwrap();
        let pos_b = sorted.iter().position(|&x| x == b).unwrap();
        let pos_c = sorted.iter().position(|&x| x == c).unwrap();
        let pos_d = sorted.iter().position(|&x| x == d).unwrap();

        assert!(pos_a < pos_b);
        assert!(pos_a < pos_c);
        assert!(pos_b < pos_d);
        assert!(pos_c < pos_d);
    }

    #[test]
    fn test_prune_keeps_target_and_deps() {
        // a.c → gcc → a.o
        // b.c → gcc2 → b.o
        let mut g = Graph::new(NodeType::Cmd);
        let a = TupId::new(1);
        let cmd1 = TupId::new(2);
        let ao = TupId::new(3);
        let b = TupId::new(4);
        let cmd2 = TupId::new(5);
        let bo = TupId::new(6);

        g.create_node(a, NodeType::File);
        g.create_node(cmd1, NodeType::Cmd);
        g.create_node(ao, NodeType::Generated);
        g.create_node(b, NodeType::File);
        g.create_node(cmd2, NodeType::Cmd);
        g.create_node(bo, NodeType::Generated);

        g.create_edge(a, cmd1, LinkType::Normal).unwrap();
        g.create_edge(cmd1, ao, LinkType::Normal).unwrap();
        g.create_edge(b, cmd2, LinkType::Normal).unwrap();
        g.create_edge(cmd2, bo, LinkType::Normal).unwrap();

        // Prune to only a.o
        let pruned = g.prune(&[ao]);
        assert_eq!(pruned, 3); // b, cmd2, bo removed
        assert!(g.contains(a));
        assert!(g.contains(cmd1));
        assert!(g.contains(ao));
        assert!(!g.contains(b));
        assert!(!g.contains(cmd2));
        assert!(!g.contains(bo));
    }

    #[test]
    fn test_prune_empty_targets() {
        let mut g = Graph::new(NodeType::Cmd);
        g.create_node(TupId::new(1), NodeType::File);
        let pruned = g.prune(&[]);
        assert_eq!(pruned, 0);
    }

    #[test]
    fn test_leaf_nodes() {
        let mut g = Graph::new(NodeType::Cmd);
        let a = TupId::new(1);
        let b = TupId::new(2);
        let c = TupId::new(3);
        g.create_node(a, NodeType::File);
        g.create_node(b, NodeType::Cmd);
        g.create_node(c, NodeType::Generated);
        g.create_edge(a, b, LinkType::Normal).unwrap();
        g.create_edge(b, c, LinkType::Normal).unwrap();

        let leaves = g.leaf_nodes();
        assert_eq!(leaves, vec![a]); // a has no incoming
    }

    #[test]
    fn test_ready_nodes() {
        let mut g = Graph::new(NodeType::Cmd);
        let a = TupId::new(1);
        let b = TupId::new(2);
        let c = TupId::new(3);
        g.create_node(a, NodeType::File);
        g.create_node(b, NodeType::Cmd);
        g.create_node(c, NodeType::Cmd);
        g.create_edge(a, b, LinkType::Normal).unwrap();
        g.create_edge(a, c, LinkType::Normal).unwrap();

        // Nothing is finished, so only nodes with no deps are ready
        // a has no incoming, so it's ready. b and c depend on a.
        let ready = g.ready_nodes();
        assert_eq!(ready, vec![a]);

        // Finish a
        g.finish_node(a);
        let ready = g.ready_nodes();
        assert!(ready.contains(&b));
        assert!(ready.contains(&c));
    }
}
