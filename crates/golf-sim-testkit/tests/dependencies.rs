//! The core starts no thread, so no core crate may depend on a thread pool.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const CORE: &[&str] = &[
    "golf-sim-base",
    "golf-sim-physics",
    "golf-sim-course",
    "golf-sim-conditions",
    "golf-sim-swing",
    "golf-sim-stroke",
    "golf-sim-rules",
    "golf-sim",
];

const THREAD_POOLS: &[&str] = &[
    "rayon",
    "rayon-core",
    "threadpool",
    "scoped_threadpool",
    "crossbeam-deque",
    "tokio",
    "async-std",
];

/// Each package in Cargo.lock with the names of its dependencies.
fn lock_graph(lock: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for block in lock.split("[[package]]").skip(1) {
        let mut name = None;
        let mut deps = BTreeSet::new();
        let mut in_deps = false;
        for line in block.lines().map(str::trim) {
            if let Some(value) = line.strip_prefix("name = ") {
                name = Some(value.trim_matches('"').to_owned());
            } else if line.starts_with("dependencies = [") {
                in_deps = true;
            } else if in_deps && line == "]" {
                in_deps = false;
            } else if in_deps {
                let dep = line.trim_matches(|c| c == '"' || c == ',');
                deps.insert(dep.split(' ').next().unwrap_or(dep).to_owned());
            }
        }
        let name = name.expect("every package has a name");
        graph.entry(name).or_default().extend(deps);
    }
    graph
}

#[test]
fn no_core_crate_depends_on_a_thread_pool() {
    let lock_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let graph = lock_graph(&std::fs::read_to_string(lock_path).unwrap());
    for root in CORE {
        assert!(graph.contains_key(*root), "{root} is not in Cargo.lock");
        let mut seen = BTreeSet::new();
        let mut stack = vec![root.to_string()];
        while let Some(name) = stack.pop() {
            assert!(
                !THREAD_POOLS.contains(&name.as_str()),
                "{root} depends on the thread pool {name}"
            );
            if seen.insert(name.clone()) {
                stack.extend(graph.get(&name).into_iter().flatten().cloned());
            }
        }
    }
}

#[test]
fn the_graph_walk_sees_transitive_dependencies() {
    let lock = "[[package]]\nname = \"a\"\ndependencies = [\n \"b 1.0.0\",\n]\n\n[[package]]\nname = \"b\"\n";
    let graph = lock_graph(lock);
    assert_eq!(graph["a"], BTreeSet::from(["b".to_owned()]));
    assert!(graph["b"].is_empty());
}
