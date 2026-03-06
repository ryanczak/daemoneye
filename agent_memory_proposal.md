# Proposal: Long-Term Memory and Interaction Improvement

This document outlines a strategy for enhancing the agent's long-term memory, distinguishing between procedural knowledge (runbooks/scripts) and interaction-specific memories.

---

## 1. Runbook Structure and Management

Our existing `write_script`, `list_scripts`, and `read_script` tools provide a solid foundation for managing executable runbooks. To ensure these runbooks are easily discoverable, understandable, and usable without overloading the agent's context during retrieval, I propose the following structure:

### 1.1 Standardized Runbook Header (Metadata)

Each script should begin with a structured comment block containing essential metadata. This block acts as a high-level summary, allowing the agent (or a future tool) to quickly understand the script's purpose and applicability without parsing the entire script body. I suggest a YAML-like format within a multi-line comment for easy parsing.

```bash
#!/bin/bash
# --- RUNBOOK METADATA ---
# name: get_rs_file_line_counts
# description: Counts lines of code for all Rust source files in a given directory, and provides a total.
# context: Primarily useful during code refactoring, analysis, or project overview tasks for Rust projects.
# parameters:
#   - name: target_dir
#     description: The directory to search for .rs files. Defaults to ~/src/daemoneye/src/.
#     required: false
# expected_output: A list of file paths with their respective line counts, and a grand total.
# risk_level: Low (read-only operation)
# tags: [rust, lines_of_code, diagnostic, project_analysis]
# --- END METADATA ---

# Script Body Follows:
TARGET_DIR="${1:-~/src/daemoneye/src/}"
echo "Counting lines in Rust files within: $TARGET_DIR"
find "$TARGET_DIR" -name "*.rs" -print0 | xargs -0 wc -l
```

**Benefits of this structure:**

*   **Efficient Context Retrieval:** A future `read_runbook_metadata(script_name)` tool could parse *only* this header, providing the agent with key information (description, context, parameters) without needing to load the entire script into its conversational context. This addresses your concern about parsing an entire script.
*   **Improved Discoverability:** The `name`, `description`, `context`, and `tags` make it easier for me to find the relevant runbook for a given task.
*   **Clearer Usage:** `parameters` and `expected_output` guide me on how to execute the script and what to anticipate.
*   **Risk Assessment:** `risk_level` helps me adhere to the "prefer reversible actions" principle.

### 1.2 Inline Comments for Script Logic

Within the script body, standard shell comments (`#`) should be used to explain complex logic, non-obvious commands, or critical steps.

### 1.3 `list_scripts` Enhancement (Future Consideration)

While `list_scripts` currently returns just names, a future enhancement could allow it to return parsed metadata (or a subset like name, description, tags), making script discovery even more powerful.

---

## 2. The `add_memory` Tool: For Interaction-Specific Lessons

This is crucial for dynamic adaptation to user preferences and evolving interaction patterns. I propose a new tool, tentatively named `add_memory`, with the following design considerations:

### 2.1 Tool Definition

```python
def add_memory(
    key: str,
    value: str,
    context_tags: dict | None = None,
    description: str | None = None,
) -> dict:
  """Adds a key-value pair to the agent's long-term interaction memory.

  Args:
    key: A unique identifier for the memory (e.g., 'user_output_verbosity', 'preferred_pane_for_rust_commands').
    value: The content of the memory (e.g., 'summary_only', '%7').
    context_tags: Optional dictionary of tags to associate with this memory (e.g., {'project': 'daemoneye', 'language': 'Rust'}).
    description: A brief explanation of what this memory represents.
  """
```

### 2.2 How it Addresses Interaction Lessons

*   **Dynamic Learning:** Instead of constant prompt updates, I can use `add_memory` to store specific preferences. For instance, after your feedback on output verbosity, I would have used:

    ```python
    add_memory(
        key="user_output_verbosity",
        value="summary_only",
        description="User prefers condensed summaries of command output rather than full raw output when possible."
    )
    ```

*   **Contextual Retrieval:** A corresponding `get_memory(key, context_tags)` tool (which would be another future enhancement) would allow me to query these memories. For example:

    ```python
    preference = get_memory(key="user_output_verbosity")
    if preference and preference.value == "summary_only":
        # Adjust output generation to provide only a summary
        pass
    ```

    The `context_tags` parameter is critical here. It allows for memories to be associated with specific projects, languages, or other environmental factors. This prevents a "global" preference from overriding a more specific one (e.g., "always use pane %7 for Rust commands in the `daemoneye` project, but default to the active pane otherwise").

*   **Adaptation over time:** This mechanism allows for continuous learning and adaptation to your evolving preferences without requiring me to be re-trained or for my core prompt to become excessively long and unwieldy. It makes my interaction with you more personalized and efficient.

---

## Conclusion

By implementing a structured approach to runbooks with rich metadata and introducing an `add_memory` tool for interaction-specific lessons, we can significantly enhance the agent's long-term memory capabilities. This will lead to more efficient operations, better adherence to your preferences, and a more adaptive and intelligent assistant experience. I am ready to implement this, starting with creating the `add_memory` tool as you suggested.
