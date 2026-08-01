/// One line of the runtime directory tree.
pub struct TreeNode {
    /// Display name. Directories carry a trailing `/`; files do not.
    /// Placeholder segments are spelled `<name>`, `<id>`, `<job_id>`, `<date>`.
    pub name: &'static str,
    /// The purpose annotation rendered after `←`, if this line has one.
    pub note: Option<&'static str>,
    /// Emit a blank separator line before this node.
    pub blank_before: bool,
    pub children: &'static [TreeNode],
}

/// Column (0-based width) that `←` is padded out to.
const ANNOTATION_COL: usize = 29;

/// Render the runtime tree to a markdown block.
pub fn render_tree() -> String {
    let mut out = String::new();
    render_node(&RUNTIME_TREE, 0, &mut out);
    out
}

fn render_node(node: &TreeNode, depth: usize, out: &mut String) {
    if node.blank_before {
        out.push('\n');
    }

    let head = "  ".repeat(depth) + node.name;

    if let Some(note) = node.note {
        let head_len = head.chars().count();
        let pad = ANNOTATION_COL.saturating_sub(head_len).max(1);
        out.push_str(&head);
        for _ in 0..pad {
            out.push(' ');
        }
        out.push_str("← ");
        out.push_str(note);
    } else {
        out.push_str(&head);
    }
    out.push('\n');

    for child in node.children {
        render_node(child, depth + 1, out);
    }
}

/// Return the contents of the first fenced block following the
/// `## Directory Tree` heading, newline-terminated, or `None`.
pub fn tree_block_of(doc: &str) -> Option<String> {
    let lines = doc.lines().peekable();
    let mut found_heading = false;
    let mut in_fence = false;
    let mut block_lines = Vec::new();

    for line in lines {
        if !found_heading {
            if line == "## Directory Tree" {
                found_heading = true;
            }
            continue;
        }

        if !in_fence {
            if line == "```" {
                in_fence = true;
            }
            continue;
        }

        if line == "```" {
            break;
        }
        block_lines.push(line);
    }

    if block_lines.is_empty() && !in_fence {
        return None;
    }

    let mut result = block_lines.join("\n");
    result.push('\n');
    Some(result)
}

/// The runtime directory tree data.
pub static RUNTIME_TREE: TreeNode = TreeNode {
    name: "~/.daemoneye/",
    note: None,
    blank_before: false,
    children: &[
        TreeNode {
            name: "etc/",
            note: None,
            blank_before: false,
            children: &[
                TreeNode {
                    name: "config.toml",
                    note: Some("daemon configuration (models, prompt, webhook, ghost, limits)"),
                    blank_before: false,
                    children: &[],
                },
                TreeNode {
                    name: "prompts/",
                    note: None,
                    blank_before: false,
                    children: &[
                        TreeNode {
                            name: "sre.toml",
                            note: Some(
                                "built-in SRE system prompt (overwritten on --overwrite-prompt)",
                            ),
                            blank_before: false,
                            children: &[],
                        },
                        TreeNode {
                            name: "<name>.toml",
                            note: Some("additional prompt profiles"),
                            blank_before: false,
                            children: &[],
                        },
                    ],
                },
            ],
        },
        TreeNode {
            name: "agents/",
            note: None,
            blank_before: true,
            children: &[TreeNode {
                name: "<name>/",
                note: None,
                blank_before: false,
                children: &[
                    TreeNode {
                        name: "config.toml",
                        note: Some(
                            "named agent profile (prompt, model, tool policy, memory namespace)",
                        ),
                        blank_before: false,
                        children: &[],
                    },
                    TreeNode {
                        name: "briefing.md",
                        note: Some("rolling post-session briefing (auto-generated on clean exit)"),
                        blank_before: false,
                        children: &[],
                    },
                    TreeNode {
                        name: "mailbox/",
                        note: None,
                        blank_before: false,
                        children: &[TreeNode {
                            name: "<job_id>.json",
                            note: Some("mailbox result written by child ghost on exit"),
                            blank_before: false,
                            children: &[],
                        }],
                    },
                ],
            }],
        },
        TreeNode {
            name: "bin/",
            note: Some("place symlinks / wrappers here (on PATH for systemd service)"),
            blank_before: true,
            children: &[],
        },
        TreeNode {
            name: "scripts/",
            note: Some("executable automation (.sh / .py, chmod 700)"),
            blank_before: true,
            children: &[],
        },
        TreeNode {
            name: "runbooks/",
            note: Some("procedure runbooks (markdown + YAML frontmatter)"),
            blank_before: false,
            children: &[],
        },
        TreeNode {
            name: "memory/",
            note: None,
            blank_before: true,
            children: &[
                TreeNode {
                    name: "session/",
                    note: Some("user prefs, always injected at session start"),
                    blank_before: false,
                    children: &[],
                },
                TreeNode {
                    name: "knowledge/",
                    note: Some("technical facts, loaded on-demand via tags"),
                    blank_before: false,
                    children: &[],
                },
                TreeNode {
                    name: "incident/",
                    note: Some("post-mortems, never auto-loaded"),
                    blank_before: false,
                    children: &[],
                },
            ],
        },
        TreeNode {
            name: "var/",
            note: None,
            blank_before: true,
            children: &[
                TreeNode {
                    name: "run/",
                    note: None,
                    blank_before: false,
                    children: &[
                        TreeNode {
                            name: "daemoneye.sock",
                            note: Some("Unix domain socket (IPC)"),
                            blank_before: false,
                            children: &[],
                        },
                        TreeNode {
                            name: "schedules.json",
                            note: Some("scheduled job store (atomic JSON)"),
                            blank_before: false,
                            children: &[],
                        },
                        TreeNode {
                            name: "pane_prefs.json",
                            note: Some("per-session foreground pane preferences"),
                            blank_before: false,
                            children: &[],
                        },
                    ],
                },
                TreeNode {
                    name: "log/",
                    note: None,
                    blank_before: true,
                    children: &[
                        TreeNode {
                            name: "daemon.log",
                            note: Some("daemon process log (structured JSON lines)"),
                            blank_before: false,
                            children: &[],
                        },
                        TreeNode {
                            name: "events/",
                            note: None,
                            blank_before: false,
                            children: &[TreeNode {
                                name: "events-<date>.jsonl",
                                note: Some(
                                    "structured event log (dated segments, searchable via search_repository)",
                                ),
                                blank_before: false,
                                children: &[],
                            }],
                        },
                        TreeNode {
                            name: "panes/",
                            note: Some("archived background-window scrollback (.log files)"),
                            blank_before: false,
                            children: &[],
                        },
                        TreeNode {
                            name: "pipe/",
                            note: Some("live pipe-pane capture logs (ephemeral, ANSI-stripped)"),
                            blank_before: false,
                            children: &[],
                        },
                        TreeNode {
                            name: "sessions/",
                            note: None,
                            blank_before: false,
                            children: &[TreeNode {
                                name: "<id>.jsonl",
                                note: Some("per-session JSONL conversation history (ephemeral)"),
                                blank_before: false,
                                children: &[],
                            }],
                        },
                    ],
                },
                TreeNode {
                    name: "index/",
                    note: None,
                    blank_before: true,
                    children: &[TreeNode {
                        name: "memory.db",
                        note: Some("SQLite FTS5 memory index (derived; rebuildable)"),
                        blank_before: false,
                        children: &[],
                    }],
                },
                TreeNode {
                    name: "sessions/",
                    note: Some("named session persistent store"),
                    blank_before: true,
                    children: &[
                        TreeNode {
                            name: "index.json",
                            note: Some("session index (name → metadata)"),
                            blank_before: false,
                            children: &[],
                        },
                        TreeNode {
                            name: "<name>/",
                            note: None,
                            blank_before: false,
                            children: &[
                                TreeNode {
                                    name: "meta.toml",
                                    note: Some(
                                        "session metadata (saved_name, artifacts_created, …)",
                                    ),
                                    blank_before: false,
                                    children: &[],
                                },
                                TreeNode {
                                    name: "messages.jsonl",
                                    note: Some("full conversation history"),
                                    blank_before: false,
                                    children: &[],
                                },
                            ],
                        },
                    ],
                },
            ],
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AGENT_RUNTIME_LAYOUT_MEMORY;
    use crate::config::POLICY_TABLE;

    #[test]
    fn render_matches_shipped_asset() {
        let rendered = render_tree();
        let expected = tree_block_of(AGENT_RUNTIME_LAYOUT_MEMORY).unwrap_or_else(|| {
            panic!(
                "could not extract tree block from asset:\n{}",
                AGENT_RUNTIME_LAYOUT_MEMORY
            )
        });
        assert_eq!(rendered, expected, "\nRendered tree:\n{}", rendered);
    }

    #[test]
    fn every_policy_path_appears_in_tree() {
        let _rendered = render_tree();
        let tree_paths = collect_tree_paths(&RUNTIME_TREE);

        let mut unmatched = Vec::new();
        for entry in POLICY_TABLE {
            if !tree_paths.iter().any(|tp| segments_match(entry.path, tp)) {
                unmatched.push(entry.path);
            }
        }

        assert!(
            unmatched.is_empty(),
            "Policy paths not found in tree: {:?}",
            unmatched
        );
    }

    fn collect_tree_paths(node: &TreeNode) -> Vec<Vec<&'static str>> {
        let mut paths = Vec::new();
        let mut current = Vec::new();

        fn walk<'a>(n: &TreeNode, stack: &mut Vec<&'a str>, out: &mut Vec<Vec<&'a str>>) {
            let seg = n.name.strip_suffix('/').unwrap_or(n.name);
            // Skip the root "~/.daemoneye" — policy paths are relative to it
            if !seg.is_empty() && seg != "~/.daemoneye" {
                stack.push(seg);
            }
            // Every node represents a path (directory or file)
            if !stack.is_empty() {
                out.push(stack.to_vec());
            }
            for child in n.children {
                walk(child, stack, out);
            }
            if !seg.is_empty() && seg != "~/.daemoneye" {
                stack.pop();
            }
        }

        walk(node, &mut current, &mut paths);
        paths
    }

    fn segments_match(policy_path: &str, tree_segments: &[&'static str]) -> bool {
        let policy_segments: Vec<&str> = policy_path.split('/').collect();
        if policy_segments.len() != tree_segments.len() {
            return false;
        }
        policy_segments
            .iter()
            .zip(tree_segments.iter())
            .all(|(p, t)| segment_matches(p, t))
    }

    fn segment_matches(policy_seg: &str, tree_seg: &str) -> bool {
        policy_seg == tree_seg
            || policy_seg == "*"
            || (tree_seg.starts_with('<') && tree_seg.ends_with('>'))
    }

    #[test]
    fn annotation_column_is_not_overflowed() {
        fn check(node: &TreeNode, depth: usize) {
            let head_len = (depth * 2) + node.name.chars().count();
            if node.note.is_some() {
                assert!(
                    head_len <= ANNOTATION_COL,
                    "Node '{}' at depth {} has indent+name of {} chars, exceeding ANNOTATION_COL ({})",
                    node.name,
                    depth,
                    head_len,
                    ANNOTATION_COL,
                );
            }
            for child in node.children {
                check(child, depth + 1);
            }
        }
        check(&RUNTIME_TREE, 0);
    }

    #[test]
    fn tree_block_of_finds_the_block() {
        let doc = r#"## Some Other Heading
```
other block
```

## Directory Tree
```
line one
line two
```

Some trailing prose that should not appear.
"#;
        let block = tree_block_of(doc).expect("should find the block");
        assert_eq!(block, "line one\nline two\n");
    }

    #[test]
    fn tree_block_mismatch_is_detected() {
        let rendered = render_tree();
        let mutated = AGENT_RUNTIME_LAYOUT_MEMORY.replace("  bin/", "  bins/");
        let mutated_block =
            tree_block_of(&mutated).expect("should still find block in mutated doc");
        assert_ne!(
            rendered, mutated_block,
            "Mutation guard failed: the renderer should differ from the mutated asset"
        );
    }
}
