//! Topological sort of domains by their `depends on` declarations.
//!
//! This ensures that when the code generator iterates domains, referenced
//! types from dependency domains are already emitted.

use super::parser::Domain;

/// Sort `domains` in-place so that each domain appears after all domains it
/// declares a `depends on` relationship with.
///
/// The sort is stable with respect to the original order when no dependency
/// constraint forces a reorder (i.e. domains with no deps keep their original
/// relative positions).
///
/// Domains with circular or missing dependencies are placed at the end without
/// panicking — the generator can handle forward references via `super::` paths.
pub fn topological_sort(domains: &mut Vec<Domain>) {
    let n = domains.len();
    let names: Vec<String> = domains.iter().map(|d| d.name.clone()).collect();

    // Build adjacency: for each domain, which indices must come before it.
    // dep_indices[i] = indices of domains that domain[i] depends on.
    let dep_indices: Vec<Vec<usize>> = domains
        .iter()
        .map(|d| {
            d.dependencies.iter().filter_map(|dep| names.iter().position(|n| n == dep)).collect()
        })
        .collect();

    // Kahn's algorithm
    let mut in_degree = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![vec![]; n]; // adj[i] = list of indices that depend on i

    for (i, deps) in dep_indices.iter().enumerate() {
        in_degree[i] += deps.len();
        for &dep in deps {
            adj[dep].push(i);
        }
    }

    let mut queue: std::collections::VecDeque<usize> =
        (0..n).filter(|&i| in_degree[i] == 0).collect();

    let mut order: Vec<usize> = Vec::with_capacity(n);

    while let Some(idx) = queue.pop_front() {
        order.push(idx);
        for &next in &adj[idx] {
            in_degree[next] -= 1;
            if in_degree[next] == 0 {
                queue.push_back(next);
            }
        }
    }

    // If there were cycles, append remaining (unresolved) nodes in original order.
    // Use a HashSet for O(n) membership test rather than O(n²) linear scan.
    if order.len() < n {
        let ordered: std::collections::HashSet<usize> = order.iter().copied().collect();
        for i in 0..n {
            if !ordered.contains(&i) {
                order.push(i);
            }
        }
    }

    // Reconstruct domains in the computed order
    let mut original = std::mem::take(domains);
    // Move domains out in order
    let mut result: Vec<Option<Domain>> = original.drain(..).map(Some).collect();
    *domains = order
        .into_iter()
        .map(|i| {
            result[i]
                .take()
                .expect("invariant: each index appears exactly once in topological order")
        })
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain(name: &str, deps: &[&str]) -> Domain {
        Domain {
            name: name.to_string(),
            description: None,
            experimental: false,
            deprecated: false,
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            types: vec![],
            commands: vec![],
            events: vec![],
        }
    }

    fn names(domains: &[Domain]) -> Vec<&str> {
        domains.iter().map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn no_deps_preserves_order() {
        let mut domains = vec![domain("A", &[]), domain("B", &[]), domain("C", &[])];
        topological_sort(&mut domains);
        assert_eq!(names(&domains), ["A", "B", "C"]);
    }

    #[test]
    fn simple_chain() {
        // C depends on B which depends on A — input in reverse order.
        let mut domains = vec![domain("C", &["B"]), domain("B", &["A"]), domain("A", &[])];
        topological_sort(&mut domains);
        let order = names(&domains);
        let pos = |n: &str| order.iter().position(|&x| x == n).unwrap();
        assert!(pos("A") < pos("B"));
        assert!(pos("B") < pos("C"));
    }

    #[test]
    fn diamond_dependency() {
        // D depends on B and C; B and C both depend on A.
        let mut domains = vec![
            domain("D", &["B", "C"]),
            domain("B", &["A"]),
            domain("C", &["A"]),
            domain("A", &[]),
        ];
        topological_sort(&mut domains);
        let order = names(&domains);
        let pos = |n: &str| order.iter().position(|&x| x == n).unwrap();
        assert!(pos("A") < pos("B"));
        assert!(pos("A") < pos("C"));
        assert!(pos("B") < pos("D"));
        assert!(pos("C") < pos("D"));
    }

    #[test]
    fn circular_dependency_does_not_panic() {
        // A → B → A forms a cycle; both must appear in output without panic.
        let mut domains = vec![domain("A", &["B"]), domain("B", &["A"])];
        topological_sort(&mut domains);
        assert_eq!(domains.len(), 2);
        let order = names(&domains);
        assert!(order.contains(&"A") && order.contains(&"B"));
    }

    #[test]
    fn missing_dependency_does_not_panic() {
        // A claims to depend on "Ghost" which is not in the list.
        let mut domains = vec![domain("A", &["Ghost"]), domain("B", &[])];
        topological_sort(&mut domains);
        assert_eq!(domains.len(), 2);
    }

    #[test]
    fn empty_input() {
        let mut domains: Vec<Domain> = vec![];
        topological_sort(&mut domains);
        assert!(domains.is_empty());
    }

    #[test]
    fn single_domain() {
        let mut domains = vec![domain("Lone", &[])];
        topological_sort(&mut domains);
        assert_eq!(names(&domains), ["Lone"]);
    }
}
