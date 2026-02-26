# Product Requirements Document (PRD) Details for T1000

This document specifies the functional and non-functional requirements for T1000, building upon the Product Definition.

## 1. Functional Requirements

### 1.1 Terminal & UI

- **FR-1.1.1**: The application MUST render a desktop terminal window on Linux (X11 and Wayland).
- **FR-1.1.2**: The application MUST support Terminator-style tiling (horizontal and vertical splits) to manage multiple panes in a single window.
- **FR-1.1.3**: The UI MUST provide a global hotkey to instantly open a dedicated Gemini AI pane adjacent to the active terminal pane.

### 1.2 tmux Integration

- **FR-1.2.1**: The application MUST use `tmux` as its native backend for terminal session management.
- **FR-1.2.2**: The application MUST be fully backwards compatible with existing `tmux` configurations (`.tmux.conf`).
- **FR-1.2.3**: Operators MUST be able to import existing tmux configurations seamlessly.
- **FR-1.2.4**: The application MUST allow users to attach to existing, long-running `tmux` sessions.
- **FR-1.2.5**: Window layouts and terminal panes MUST persist across application restarts or remote connection drops.

### 1.3 Gemini AI Integration

- **FR-1.3.1**: The AI agent MUST act as an expert sysadmin and security expert, capable of determining the right tools to use.
- **FR-1.3.2**: When activated, the application MUST capture terminal context (visible output, backscroll history, environment state) and provide it to the Gemini AI.
- **FR-1.3.3**: The AI MUST be able to analyze stack traces, crash logs, failing services, and security scan outputs to provide root cause analysis and remediation strategies.
- **FR-1.3.4**: The AI MUST be able to generate scripts or on-the-fly commands.
- **FR-1.3.5**: The application MUST allow the AI to directly execute commands in the sysadmin's terminal upon user request.

### 1.4 Prompt Library

- **FR-1.4.1**: The application MUST include a library of pre-defined prompts for common tasks.
- **FR-1.4.2**: Users MUST be able to create, save, and manage their own custom prompts.
- **FR-1.4.3**: All user-defined and standard prompts MUST be stored in the user's home directory under `~/.t1000/prompts`.

### 1.5 Authentication & Security

- **FR-1.5.1**: The application MUST have native support for `ssh-agent`.
- **FR-1.5.2**: The application MUST provide seamless ssh-key management capabilities.
- **FR-1.5.3**: Sensitive data (passwords, secret keys, PII) MUST be masked or filtered from the terminal buffer before being transmitted to the Gemini API.
- **FR-1.5.4**: Users MUST have explicit controls over what terminal context is sent to the LLM.

### 1.6 Extensibility

- **FR-1.6.1**: The application MUST include a native plugin architecture for community extensions.
- **FR-1.6.2**: The application MUST allow plugins to hook into AI prompt lifecycles, UI rendering, and third-party APIs (e.g., AWS/GCP/Azure).

### 1.7 Terminal Emulation Compatibility

The rendering engine MUST achieve high compatibility with VT100, VT220, and xterm standards to correctly display modern CLI tools.

#### 1.7.1 Text Attributes (SGR — Tier 1 Critical)

- **FR-1.7.1**: The terminal MUST render SGR text attribute codes: bold (`1`), dim (`2`), italic (`3`), underline (`4`), blink (`5`), reverse video (`7`), and strikethrough (`9`), along with their respective reset codes (`22`/`23`/`24`/`25`/`27`/`29`).

#### 1.7.2 Screen Management (Tier 1 Critical)

- **FR-1.7.2**: The terminal MUST support the alternate screen buffer (`CSI ?1049h` to enter, `CSI ?1049l` to exit), saving and restoring the primary screen content. This is required for `vim`, `less`, `htop`, and `man`.
- **FR-1.7.3**: The terminal MUST support cursor save/restore (`ESC 7`/`ESC 8`, `DECSC`/`DECRC`, and `CSI s`/`CSI u`), preserving cursor position and current SGR attributes.

#### 1.7.3 Device Communication (Tier 1 Critical)

- **FR-1.7.4**: The terminal MUST respond to Device Status Reports (`CSI 5n` → `CSI 0n`) and Cursor Position Reports (`CSI 6n` → `CSI row;colR`) sent back to the PTY. Failure to reply causes programs like `vim` to hang.
- **FR-1.7.5**: The terminal MUST respond to Primary Device Attributes (`CSI c` → `CSI ?1;2c`) and Secondary DA (`CSI > c`) to identify as an xterm-compatible terminal.

#### 1.7.4 Input Handling (Tier 2 High)

- **FR-1.7.6**: The terminal MUST support tab stops: setting (`ESC H`), clearing (`CSI 0g`/`CSI 3g`), and horizontal tab movement (`CSI nI` forward, `CSI nZ` backward). The `\t` character MUST advance to the next tab stop (default: every 8 columns).
- **FR-1.7.7**: The terminal MUST support Insert Mode (`CSI ?4h`/`CSI ?4l`), shifting existing characters right when typing.
- **FR-1.7.8**: The terminal MUST support Bracketed Paste Mode (`CSI ?2004h`/`l`), wrapping paste events in `\x1b[200~`/`\x1b[201~` delimiters.
- **FR-1.7.9**: The terminal MUST support mouse event forwarding modes (`CSI ?1000h/l` basic, `?1002h/l` button-motion, `?1006h/l` SGR extended) for tools like `vim` and `tmux` mouse integration.

#### 1.7.5 Scrollback & Display (Tier 2 High)

- **FR-1.7.10**: The terminal MUST maintain a scrollback buffer of at least 10,000 lines, accessible via keyboard shortcuts or GTK scroll events.
- **FR-1.7.11**: The terminal MUST support `CSI n X` (Erase Character) and `CSI n S`/`CSI n T` (Scroll Up/Down region) for efficient ncurses redraws.
- **FR-1.7.12**: The terminal MUST support cursor visibility toggling (`CSI ?25h` show, `CSI ?25l` hide) used by TUIs to prevent cursor flickering during redraws.

#### 1.7.6 Character Sets (Tier 2 High)

- **FR-1.7.13**: The terminal MUST support DEC Special Graphics character set selection (`ESC ( 0` vs `ESC ( B`), translating DEC line-drawing glyphs (e.g., `j`=`┘`, `k`=`┐`, `q`=`─`) to Unicode equivalents. This is required for correct rendering of `tmux` pane borders in fallback ASCII mode.

#### 1.7.7 OSC & Window Management (Tier 1/3)

- **FR-1.7.14**: The terminal MUST process OSC window title sequences (`OSC 0;title ST`, `OSC 2;title ST`), propagating the title to the GTK window title bar.
- **FR-1.7.15** *(Stretch)*: The terminal SHOULD support the OS clipboard OSC sequence (`OSC 52`) for clipboard integration with `vim` and `tmux`.

---

## 2. Non-Functional Requirements

### 2.1 Performance

- **NFR-2.1.1**: Terminal rendering MUST be highly performant, with low latency typing and rapid scrollback rendering comparable to Alacritty or Kitty.
- **NFR-2.1.2**: Capturing tmux buffers and transmitting to Gemini MUST NOT block the main terminal rendering thread.
- **NFR-2.1.3**: Terminal emulation MUST pass at least 90% of the standard `vttest` test suite, and 100% of Tier 1 critical tests, at each tagged release.

### 2.2 Compatibility & Environment

- **NFR-2.2.1**: The application MUST run on standard modern Linux distributions (Ubuntu, Fedora, Arch, etc.).
- **NFR-2.2.2**: The application requires `tmux` to be available in the system PATH.

### 2.3 Usability

- **NFR-2.3.1**: The application MUST NOT require users to memorize complex `tmux` keyboard shortcuts for basic window management (tiling, resizing, closing panes).
