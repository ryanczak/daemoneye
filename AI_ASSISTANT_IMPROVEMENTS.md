# AI Assistant (Gemini) - Requested Improvements for DaemonEye Integration

This document outlines potential improvements to the DaemonEye framework from the perspective of the integrated AI assistant (Gemini), aimed at enhancing its effectiveness, diagnostic capabilities, and overall user experience.

---

## 1. More Targeted Context Acquisition (Automated Diagnostics)

**Capability Required:** The AI should be able to specify and trigger targeted diagnostic commands (e.g., `ss -tulnp`, `cat /etc/resolv.conf`, `kubectl describe pod <pod-name>`) and receive their output as structured context. This would build upon the existing `run_terminal_command` tool, but with more intelligent context aggregation by DaemonEye.

**Need:**
*   **Reduced Friction:** Currently, if the initial context is insufficient, the AI must ask the user to run a command. This adds latency and manual steps. Automating these targeted diagnostics allows for quicker, more comprehensive data gathering.
*   **Proactive Diagnosis:** The AI could proactively gather relevant system state based on initial symptoms without explicit user prompts for each piece of information.
*   **Efficiency:** It allows the AI to "drill down" into specific layers of the stack (network, storage, process, kernel) more efficiently, replicating a human SRE's thought process.

## 2. Clearer "Active" Pane Indication

**Capability Required:** When context is provided from multiple tmux panes, there should be a clear, explicit indicator within the context dump identifying the pane that the user is currently focused on or considers the primary context for the ongoing discussion. The existing `DAEMONEYE_SOURCE_PANE` is a good start, but more explicit highlighting is needed for the AI to prioritize.

**Need:**
*   **Improved Relevance:** The AI often needs to understand which part of the user's workflow is most relevant. Without a clear "active" pane, interpreting the importance of various log messages or command outputs can be ambiguous.
*   **Better Contextual Responses:** Knowing the active pane allows the AI to tailor its responses and proposed actions more directly to the user's current focus, e.g., "In the active pane, I see..."

## 3. Structured Feedback on Command Failures

**Capability Required:** If a command proposed by the AI and executed by DaemonEye fails, the feedback provided to the AI should include more structured information about the nature of the failure (e.g., error type, exit code, common reasons for failure) in addition to the raw stderr/stdout.

**Need:**
*   **Intelligent Remediation:** Raw command output can be verbose and unstandardized. Structured error feedback (e.g., `command not found`, `permission denied`, `non-zero exit status N`, `timeout`) would allow the AI to immediately understand *why* a command failed and propose more accurate next steps or alternative solutions.
*   **Reduced Iterations:** Instead of blindly re-trying or asking for more verbose output, the AI could make an informed decision to, for example, suggest installing a missing package, using `sudo`, or modifying command arguments.

## 4. Environment Awareness

**Capability Required:** A configurable mechanism within DaemonEye (e.g., in `~/.daemoneye/config.toml` or as an environment variable) to explicitly declare the operating environment (e.g., "development", "staging", "production", "personal workstation"). This environment tag would be included in the context sent to the AI.

**Need:**
*   **Context-Sensitive Advice:** Security and operational advice often differs greatly between environments. In production, caution and impact assessment are paramount; in development, speed and experimentation might be preferred.
*   **Tailored Security Posture:** Knowing the environment allows the AI to factor in appropriate security considerations (e.g., stricter hardening recommendations for production, less sensitive data handling for local dev).
*   **Risk Assessment:** The AI can better assess the potential blast radius or impact of proposed changes or diagnostic steps based on the known environment.

---

