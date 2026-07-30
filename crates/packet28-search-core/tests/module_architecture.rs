//! Mechanical source-architecture contract for the search implementation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};
use syn::{Attribute, File, Item, ItemMod, ItemUse, Path as SynPath, UseTree, Visibility};

const EXPECTED_ROOT_EXPORTS: &[&str] = &[
    "Result",
    "SearchError",
    "RegexIndexManifest",
    "RegexIndexRuntime",
    "load_runtime",
    "rebuild_full_index",
    "rebuild_full_index_with_progress",
    "update_overlay_index",
    "clear_index",
    "guarded_fallback_reason",
    "indexed_search",
    "shared_scan",
];

#[derive(Clone, Copy)]
struct ModulePolicy {
    max_lines: usize,
    allowed_dependencies: &'static [&'static str],
}

fn policies() -> BTreeMap<&'static str, ModulePolicy> {
    BTreeMap::from([
        (
            "error",
            ModulePolicy {
                max_lines: 300,
                allowed_dependencies: &[],
            },
        ),
        (
            "generation",
            ModulePolicy {
                max_lines: 1_025,
                allowed_dependencies: &[
                    "error",
                    "git_process",
                    "layer",
                    "model",
                    "paths",
                    "publication",
                    "support",
                    "weights",
                    "workspace",
                ],
            },
        ),
        (
            "git_process",
            ModulePolicy {
                max_lines: 175,
                allowed_dependencies: &[],
            },
        ),
        (
            "layer",
            ModulePolicy {
                max_lines: 750,
                allowed_dependencies: &["error", "model", "paths", "postings", "support"],
            },
        ),
        (
            "model",
            ModulePolicy {
                max_lines: 550,
                allowed_dependencies: &[],
            },
        ),
        (
            "paths",
            ModulePolicy {
                max_lines: 325,
                allowed_dependencies: &["error", "model"],
            },
        ),
        (
            "postings",
            ModulePolicy {
                max_lines: 500,
                allowed_dependencies: &["error", "model", "support", "weights"],
            },
        ),
        (
            "publication",
            ModulePolicy {
                max_lines: 300,
                allowed_dependencies: &["error", "layer", "model", "paths", "support"],
            },
        ),
        (
            "query",
            ModulePolicy {
                max_lines: 1_500,
                allowed_dependencies: &[
                    "error",
                    "model",
                    "paths",
                    "postings",
                    "support",
                    "workspace",
                ],
            },
        ),
        (
            "shared_scan",
            ModulePolicy {
                max_lines: 750,
                allowed_dependencies: &[
                    "error",
                    "generation",
                    "git_process",
                    "layer",
                    "model",
                    "paths",
                    "postings",
                    "publication",
                    "support",
                    "weights",
                    "workspace",
                ],
            },
        ),
        (
            "support",
            ModulePolicy {
                max_lines: 150,
                allowed_dependencies: &["error"],
            },
        ),
        (
            "weights",
            ModulePolicy {
                max_lines: 100,
                allowed_dependencies: &[],
            },
        ),
        (
            "workspace",
            ModulePolicy {
                max_lines: 400,
                allowed_dependencies: &["error", "git_process", "model", "paths"],
            },
        ),
    ])
}

#[derive(Default)]
struct SourceFacts {
    dependencies: BTreeSet<String>,
    errors: Vec<String>,
}

struct SourceVisitor<'a> {
    module: &'a str,
    known_modules: &'a BTreeSet<&'static str>,
    facts: SourceFacts,
}

impl SourceVisitor<'_> {
    fn record_segments(&mut self, segments: &[String]) {
        let Some(first) = segments.first().map(String::as_str) else {
            return;
        };
        if first == "super" && segments.len() > 1 {
            self.facts.errors.push(format!(
                "{} imports through `super`; use the owning crate module",
                self.module
            ));
            return;
        }
        if first != "crate" {
            return;
        }
        let Some(owner) = segments.get(1).map(String::as_str) else {
            return;
        };
        if !self.known_modules.contains(owner) {
            self.facts.errors.push(format!(
                "{} reaches root export `{owner}`; import from its owning module",
                self.module
            ));
            return;
        }
        if owner != self.module {
            self.facts.dependencies.insert(owner.to_string());
        }
    }
}

impl<'ast> Visit<'ast> for SourceVisitor<'_> {
    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        if is_exact_cfg_test(&item.attrs) {
            return;
        }
        visit::visit_item_mod(self, item);
    }

    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        if is_exact_cfg_test(&item.attrs) {
            return;
        }
        let mut imports = Vec::new();
        flatten_use_tree(&item.tree, &mut Vec::new(), &mut imports);
        for import in imports {
            if import.is_glob {
                self.facts.errors.push(format!(
                    "{} contains a production wildcard import",
                    self.module
                ));
            }
            self.record_segments(&import.segments);
        }
    }

    fn visit_path(&mut self, path: &'ast SynPath) {
        let segments = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        self.record_segments(&segments);
        visit::visit_path(self, path);
    }
}

struct ImportPath {
    segments: Vec<String>,
    is_glob: bool,
}

fn flatten_use_tree(tree: &UseTree, prefix: &mut Vec<String>, output: &mut Vec<ImportPath>) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            flatten_use_tree(&path.tree, prefix, output);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let mut segments = prefix.clone();
            segments.push(name.ident.to_string());
            output.push(ImportPath {
                segments,
                is_glob: false,
            });
        }
        UseTree::Rename(rename) => {
            let mut segments = prefix.clone();
            segments.push(rename.ident.to_string());
            output.push(ImportPath {
                segments,
                is_glob: false,
            });
        }
        UseTree::Glob(_) => output.push(ImportPath {
            segments: prefix.clone(),
            is_glob: true,
        }),
        UseTree::Group(group) => {
            for item in &group.items {
                flatten_use_tree(item, prefix, output);
            }
        }
    }
}

fn is_exact_cfg_test(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && matches!(
                &attribute.meta,
                syn::Meta::List(list) if list.tokens.to_string() == "test"
            )
    })
}

fn parse_source(path: &Path) -> File {
    let source = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    syn::parse_file(&source)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn source_facts(module: &str, file: &File, known: &BTreeSet<&'static str>) -> SourceFacts {
    let mut visitor = SourceVisitor {
        module,
        known_modules: known,
        facts: SourceFacts::default(),
    };
    visitor.visit_file(file);
    visitor.facts
}

fn source_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn public_root_exports(file: &File) -> Result<BTreeSet<String>, String> {
    let mut exports = BTreeSet::new();
    for item in &file.items {
        match item {
            Item::Use(item) if matches!(item.vis, Visibility::Public(_)) => {
                let mut imports = Vec::new();
                flatten_use_tree(&item.tree, &mut Vec::new(), &mut imports);
                for import in imports {
                    let Some(name) = import.segments.last() else {
                        return Err("public root wildcard export is forbidden".to_string());
                    };
                    exports.insert(name.clone());
                }
            }
            Item::Mod(item) if matches!(item.vis, Visibility::Public(_)) => {
                exports.insert(item.ident.to_string());
            }
            item if is_public_item(item) => {
                return Err("lib.rs may expose only reviewed re-exports".to_string());
            }
            _ => {}
        }
    }
    Ok(exports)
}

fn is_public_item(item: &Item) -> bool {
    match item {
        Item::Const(item) => matches!(item.vis, Visibility::Public(_)),
        Item::Enum(item) => matches!(item.vis, Visibility::Public(_)),
        Item::ExternCrate(item) => matches!(item.vis, Visibility::Public(_)),
        Item::Fn(item) => matches!(item.vis, Visibility::Public(_)),
        Item::Static(item) => matches!(item.vis, Visibility::Public(_)),
        Item::Struct(item) => matches!(item.vis, Visibility::Public(_)),
        Item::Trait(item) => matches!(item.vis, Visibility::Public(_)),
        Item::TraitAlias(item) => matches!(item.vis, Visibility::Public(_)),
        Item::Type(item) => matches!(item.vis, Visibility::Public(_)),
        Item::Union(item) => matches!(item.vis, Visibility::Public(_)),
        _ => false,
    }
}

fn assert_acyclic(graph: &BTreeMap<String, BTreeSet<String>>, label: &str) {
    fn visit(
        node: &str,
        graph: &BTreeMap<String, BTreeSet<String>>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        if visited.contains(node) {
            return Ok(());
        }
        if !visiting.insert(node.to_string()) {
            return Err(format!("cycle reaches {node}"));
        }
        for dependency in graph.get(node).into_iter().flatten() {
            visit(dependency, graph, visiting, visited)?;
        }
        visiting.remove(node);
        visited.insert(node.to_string());
        Ok(())
    }

    let mut visited = BTreeSet::new();
    for node in graph.keys() {
        visit(node, graph, &mut BTreeSet::new(), &mut visited)
            .unwrap_or_else(|error| panic!("{label} dependency graph has a {error}"));
    }
}

#[test]
fn search_core_source_architecture_stays_reviewed() {
    let source_dir = source_dir();
    let policies = policies();
    let known = policies.keys().copied().collect::<BTreeSet<_>>();
    let expected_files = known
        .iter()
        .map(|module| format!("{module}.rs"))
        .chain([
            "generated_pair_weights.rs".to_string(),
            "lib.rs".to_string(),
            "tests.rs".to_string(),
        ])
        .collect::<BTreeSet<_>>();
    let actual_files = fs::read_dir(&source_dir)
        .expect("search source directory")
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "rs")
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual_files, expected_files,
        "unreviewed search source file"
    );

    let lib_source = fs::read_to_string(source_dir.join("lib.rs")).expect("search facade");
    assert!(
        lib_source.lines().count() <= 150,
        "search facade exceeded 150 lines"
    );
    let exports = public_root_exports(&parse_source(&source_dir.join("lib.rs")))
        .expect("reviewed root exports");
    assert_eq!(
        exports,
        EXPECTED_ROOT_EXPORTS
            .iter()
            .map(|name| (*name).to_string())
            .collect()
    );

    let generated = fs::read_to_string(source_dir.join("generated_pair_weights.rs"))
        .expect("generated pair weights");
    assert!(
        generated.starts_with("// @generated by cargo run -p packet28-search-core"),
        "large generated source lost its provenance marker"
    );
    let tests = fs::read_to_string(source_dir.join("tests.rs")).expect("search tests");
    assert!(
        tests.lines().count() <= 1_650,
        "search unit tests exceeded 1,650 lines"
    );

    let mut observed = BTreeMap::new();
    let mut reviewed = BTreeMap::new();
    for (module, policy) in &policies {
        let path = source_dir.join(format!("{module}.rs"));
        let source = fs::read_to_string(&path).expect("reviewed search module");
        assert!(
            source.starts_with("//!"),
            "{module}.rs must explain its owned responsibility"
        );
        assert!(
            source.lines().count() <= policy.max_lines,
            "{module}.rs exceeded its reviewed {}-line ceiling",
            policy.max_lines
        );
        let facts = source_facts(module, &parse_source(&path), &known);
        assert!(facts.errors.is_empty(), "{}", facts.errors.join("\n"));
        let allowed = policy
            .allowed_dependencies
            .iter()
            .map(|dependency| (*dependency).to_string())
            .collect::<BTreeSet<_>>();
        let forbidden = facts.dependencies.difference(&allowed).collect::<Vec<_>>();
        assert!(
            forbidden.is_empty(),
            "{module} has forbidden dependencies: {forbidden:?}"
        );
        observed.insert((*module).to_string(), facts.dependencies);
        reviewed.insert((*module).to_string(), allowed);
    }
    assert_acyclic(&reviewed, "reviewed");
    assert_acyclic(&observed, "observed");
}

#[test]
fn source_parser_handles_grouped_paths_and_ignores_lexical_decoys() {
    let known = policies().keys().copied().collect::<BTreeSet<_>>();
    let file = syn::parse_file(
        r#"
            use crate::model::{LoadedIndex, SearchPlan};
            const COMMENT: &str = "use super::*; crate::generation::hidden";
            fn check(_: crate::postings::PostingRow) {}
            #[cfg(test)]
            mod tests { use super::*; }
        "#,
    )
    .expect("fixture syntax");
    let facts = source_facts("query", &file, &known);
    assert_eq!(
        facts.dependencies,
        BTreeSet::from(["model".into(), "postings".into()])
    );
    assert!(facts.errors.is_empty(), "{:?}", facts.errors);
}

#[test]
fn source_parser_rejects_wildcards_root_shortcuts_and_forbidden_edges() {
    let known = policies().keys().copied().collect::<BTreeSet<_>>();
    let file = syn::parse_file(
        "use std::prelude::*; use crate::RegexIndexRuntime; use crate::generation::load_runtime;",
    )
    .expect("fixture syntax");
    let facts = source_facts("query", &file, &known);
    assert_eq!(facts.dependencies, BTreeSet::from(["generation".into()]));
    assert_eq!(facts.errors.len(), 2);
    assert!(!facts.dependencies.is_subset(
        &policies()["query"]
            .allowed_dependencies
            .iter()
            .map(|dependency| (*dependency).to_string())
            .collect()
    ));
}

#[test]
#[should_panic(expected = "dependency graph has a cycle")]
fn cycle_guard_rejects_two_node_cycles() {
    assert_acyclic(
        &BTreeMap::from([
            ("left".to_string(), BTreeSet::from(["right".to_string()])),
            ("right".to_string(), BTreeSet::from(["left".to_string()])),
        ]),
        "fixture",
    );
}
