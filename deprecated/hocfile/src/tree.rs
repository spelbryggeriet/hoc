use std::cmp::Ordering;
use std::hash::Hash;

use indexmap::IndexSet;

#[derive(PartialEq, Eq, Copy, Clone, Hash, Debug)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
}

#[derive(Debug)]
pub struct Tree<V> {
    nodes: IndexSet<V>,
    edges: IndexSet<Edge>,
}

impl<V> Tree<V>
where
    V: Eq + Hash,
{
    pub fn new(nodes: IndexSet<V>, mut edges: IndexSet<Edge>) -> Result<Tree<V>, Vec<V>> {
        // L ← Empty list that will contain the sorted nodes
        // while exists nodes without a permanent mark do
        //     select an unmarked node n
        //     visit(n)
        //
        // function visit(node n)
        //     if n has a permanent mark then
        //         return
        //     if n has a temporary mark then
        //         stop   (not a DAG)
        //
        //     mark n with a temporary mark
        //
        //     for each node m with an edge from n to m do
        //         visit(m)
        //
        //     remove temporary mark from n
        //     mark n with a permanent mark
        //     add n to head of L

        #[derive(Clone, Copy, Eq, PartialEq, Debug)]
        enum Mark {
            None,
            Temp,
            Perm,
        }

        let mut marks = vec![Mark::None; nodes.len()];

        fn visit_node(
            node: usize,
            edges: &IndexSet<Edge>,
            marks: &mut Vec<Mark>,
            topological_order: &mut Vec<usize>,
        ) -> bool {
            match marks[node] {
                Mark::Perm => return true,
                Mark::Temp => return false,
                _ => (),
            }

            marks[node] = Mark::Temp;

            for next_node in edges
                .iter()
                .filter(|edge| edge.from == node)
                .map(|edge| edge.to)
            {
                if !visit_node(next_node, edges, marks, topological_order) {
                    return false;
                }
            }

            marks[node] = Mark::Perm;
            topological_order.insert(0, node);

            true
        }

        let mut topological_order = Vec::with_capacity(nodes.len());
        while let Some((node, _)) = marks
            .iter()
            .enumerate()
            .find(|(_, mark)| **mark == Mark::None)
        {
            if !visit_node(node, &edges, &mut marks, &mut topological_order) {
                return Err(nodes
                    .into_iter()
                    .zip(marks)
                    .filter(|(_, mark)| *mark == Mark::Temp)
                    .map(|(node, _)| node)
                    .collect());
            }
        }

        let sort_by = |a, b| {
            let first = topological_order.iter().find_map(|sorted| {
                Some(a)
                    .filter(|a| a == sorted)
                    .or(Some(b).filter(|b| b == sorted))
            });

            if first == Some(a) {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        };

        let mut nodes: Vec<_> = nodes
            .into_iter()
            .enumerate()
            .map(|(i, node)| {
                (
                    topological_order.iter().position(|sorted| *sorted == i),
                    node,
                )
            })
            .collect();
        nodes.sort_unstable_by_key(|(key, _)| *key);
        let nodes: IndexSet<_> = nodes.into_iter().map(|(_, node)| node).collect();

        edges.sort_by(|a, b| sort_by(a.from, b.from));

        Ok(Tree { nodes, edges })
    }

    pub fn nodes(&self) -> impl Iterator<Item = &V> {
        self.nodes.iter()
    }

    pub fn edges(&self) -> impl Iterator<Item = Edge> + '_ {
        self.edges.iter().copied()
    }
}

// pub trait IntoTree<V> {
//     fn into_tree(self) -> Result<Tree<V>, Vec<V>>;
// }

// impl<I, V> IntoTree<V> for I
// where
//     I: IntoIterator<Item = Edge<V>>,
//     V: Eq + Clone + Hash,
// {
//     fn into_tree(self) -> Result<Tree<V>, Vec<V>> {}
// }
