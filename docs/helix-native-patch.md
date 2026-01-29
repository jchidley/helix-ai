# Helix-Native Patch: Pi Integration + Window Expansion

This document sketches what the repository would look like if the current
Steel-based plugin was replaced by a single Helix patch that implements only:

- Pi coding agent integration
- Window expansion / focus mode for the Pi split

It deliberately ignores the Steel extension system itself and any broader
plugin architecture.

## Current Baseline (from this repo)

Today the integration is implemented as Steel plugins:

- `src/pi.scm` wires Helix UI and editor APIs to a Pi RPC subprocess
- `src/pi-core.scm` holds the pure session logic, RPC event handling, and
  formatting
- Two scratch buffers are created (`[pi/output]` and `[pi/input]`) in a split
- Commands are exposed via `helix.scm` (`:pi-start`, `:pi-send`, etc.)
- A custom Helix build is required, including PR #8546 for window resizing /
  focus mode

So the patch loadout today is:

- Helix fork with Steel enabled
- Additional Helix PR for window expansion
- Steel runtime + plugin load path
- This repo's `.scm` files copied into `~/.config/helix/cogs/pi/`

## Goal: One Helix Patch Only

Replace all Steel code with a single Helix-side feature patch that:

1. Provides the same Pi commands and split-buffer UX
2. Handles the Pi RPC stream internally
3. Implements or embeds the window expansion feature (focus/resize)

No Steel runtime, no `.scm` files, no `helix.scm`/`init.scm` changes.

---

## If It Must Be Minimally Invasive and Self-Contained

If the patch has to be easy to apply to upstream Helix (and reapply later),
the design shifts toward **additive**, **isolated**, and **gated** changes:

### Design Constraints

- **Additive-only**: new files/modules, minimal edits to existing ones
- **Gated behavior**: no behavior change unless Pi commands are invoked
- **Single touchpoints**: only touch command registry + config + layout API
- **No global hooks**: avoid modifying the general event loop unless necessary

### Practical Changes vs. the "full native" approach

1. **Isolated module**
   - Keep all implementation in a new module (e.g., `pi/`), referenced from a
     single command registration file.
   - No deep refactors or shared abstractions in core Helix subsystems.

2. **Command-only integration**
   - Pi functionality only activates when `:pi-*` commands are called.
   - No global UI widgets or background tasks until a session starts.

3. **Minimal layout changes**
   - If window expansion is required, implement it as a *small helper function*
     in the view/layout module and keep it unused by default.
   - Pi calls that helper; the rest of Helix remains unchanged.

4. **Config or feature flag**
   - Add a simple config stanza (e.g., `[pi] enabled = true`) or a compile-time
     feature flag.
   - This reduces risk for upstream and avoids surprising users.

### Net effect

The patch becomes a clean, self-contained "feature bundle":

- A new `pi` module
- A few command registrations
- A tiny layout helper for window expansion
- Optional config gating

This makes reapplying the patch much easier, because conflicts are limited to
very few, stable files.

---

## Driver-Agnostic Plan (No MCP, CLI + JSONL Only)

You want to drive the LLMs via CLI and JSON/JSONL streams, **not MCP**. That
means the Helix patch should expose a minimal, provider-agnostic driver layer
with two transport modes:

1. **Streaming JSONL over stdio**  
   Use a single long-running process that accepts JSON on stdin and emits JSONL
   on stdout (ideal).

2. **Per-message spawn + JSONL tail**  
   Spawn the CLI per message and tail its session JSONL file for output
   (fallback when full-duplex streaming isn't practical).

This keeps the patch self-contained while still supporting multiple drivers.

---

## Concrete Patch Plan (Self-Contained, Reappliable)

This is the "v1" plan that matches your constraints: minimal, driver-agnostic,
CLI-only, JSON/JSONL input/output, and no MCP.

### 1) New Module: `helix-term/src/ai/`

Add a small, isolated AI driver module. Suggested files:

- `helix-term/src/ai/mod.rs` (public entry point)
- `helix-term/src/ai/config.rs` (load `~/.config/helix/ai.toml`)
- `helix-term/src/ai/driver.rs` (driver adapter API)
- `helix-term/src/ai/session.rs` (session picker, file listing)
- `helix-term/src/ai/stream.rs` (process I/O, JSONL reader)
- `helix-term/src/ai/format.rs` (pretty rendering + raw fallback)
- `helix-term/src/ai/ui.rs` (buffer creation, reuse, output append)

### 2) Command Registration (minimal touchpoints)

Add only what is required to `helix-term/src/commands/typed.rs`:

- `:ai-start [driver]`
- `:ai-resume [driver]` (picker from session files)
- `:ai-send`
- `:ai-grow` / `:ai-shrink`
- `:ai-quit`

### 3) Window Sizing Helper (tiny, isolated)

Port the minimal resize helper used in the fork:

- `helix-view/src/tree.rs`: add `resize_buffer` and `toggle` helpers
- `helix-view/src/editor.rs`: add thin wrappers

This is ~200-300 LOC, and already known to work.

---

## Config: `~/.config/helix/ai.toml`

Keep this outside the main Helix config schema to minimize upstream surface.

Example (v1):

```toml
[drivers.pi]
mode = "stream"                 # stream | tail
cmd = ["pi", "--mode", "rpc"]
session_dir = "~/.pi/agent/sessions"
session_glob = "--*--/*.jsonl"

[drivers.codex]
mode = "tail"
cmd = ["codex", "resume"]
session_dir = "~/.codex/sessions"
session_glob = "**/*.jsonl"

[drivers.claude]
mode = "tail"
cmd = ["claude", "--resume"]
session_dir = "~/.claude/projects"
session_glob = "-*/agent-*.jsonl"
```

Notes:
- `session_glob` is used by the picker.
- For `tail` mode, the driver is spawned per message with a session id.
- For `stream` mode, the driver is long-running (stdin/stdout JSONL).

---

## Driver Adapter Interface (v1)

Each driver implements a small interface:

```
start(driver, session_opt)
resume(driver, session_path_or_id)
send(driver, prompt_text)
list_sessions(driver) -> [path]
stream(driver) -> JSONL events
```

### Transport Modes

1) **stream** (preferred):  
   A long-running process reads JSON from stdin, emits JSONL on stdout.

2) **tail** (fallback):  
   Spawn the driver per message, then follow its session JSONL file to
   render output.

---

## Session Picker Logic

### pi
- Session files are in `~/.pi/agent/sessions/--<cwd>--/*.jsonl`
- Resume uses `--session <path>` or `--session-dir <dir>`

### codex
- Session files are in `~/.codex/sessions/YYYY/MM/DD/*.jsonl`
- Resume uses `codex resume <SESSION_ID>`
- SESSION_ID can be parsed from filename (UUID at end of name)

### claude
- Session files are in `~/.claude/projects/-<cwd>/agent-*.jsonl`
- Resume uses `claude --resume <sessionId>`
- `sessionId` is available inside JSONL; read the first line to extract it

---

## Output Parsing Rules (v1)

Goal: "pretty by default, raw on failure."

### Rules

1) Parse each JSONL line.
2) Attempt to extract message text via common keys:
   - `message.content[*].text` (Claude)
   - `content[*].text` (Pi)
   - `text` or `display` (Codex / history)
3) If it looks like a tool block, wrap in fences:
   - e.g., `tool_execution_*` → `**tool**` + code block
4) If parsing fails, append raw line.

This allows minimally acceptable output for all drivers without deep,
driver-specific parsers.

---

## Default Window Layout (small chat-style split)

Behavior:

1) Reuse existing `[ai/output]` + `[ai/input]` buffers if they exist.
2) If new, open a horizontal split and immediately shrink input.
3) Provide `:ai-grow` / `:ai-shrink` commands using resize helpers.

YAGNI: no focus/fullscreen toggle in v1.

---

## Rough LOC Estimate

Self-contained patch, no Steel or MCP:

- New `helix-term/src/ai/*`: ~1,200–2,400 LOC
- Commands wiring: ~150–300 LOC
- Resize helpers: ~200–300 LOC

Total: **~1,500–3,000 LOC** in a handful of files.

---

## Option B (Ultra-Minimal Variant)

If you want the smallest possible Helix delta:

- No long-running driver.
- Only per-message spawns + JSONL tailing.
- No stdin protocol.

This reduces the Helix code but increases reliance on session file formats.

---

## Summary

This plan keeps Helix changes small and reapplyable, uses only CLI + JSONL, and
works across pi, codex, and claude. Streaming is used when available; file
tailing is the fallback that keeps v1 simple.

---

## File-by-File Patch Checklist (Upstream Helix)

This is the concrete, reapplyable change list. Everything else stays untouched.

### New files (self-contained module)

- `helix-term/src/ai/mod.rs`  
  - public API + command entry points
  - holds global state (running?, active driver, buffers)

- `helix-term/src/ai/config.rs`  
  - load and parse `~/.config/helix/ai.toml`
  - expand `~` in paths

- `helix-term/src/ai/driver.rs`  
  - driver trait / interface
  - per-driver command templates (pi/codex/claude)

- `helix-term/src/ai/session.rs`  
  - list sessions for picker (glob + sort)
  - parse session IDs from files (codex/claude)

- `helix-term/src/ai/stream.rs`  
  - process spawn
  - stdio JSONL reader
  - file tailer for session files

- `helix-term/src/ai/format.rs`  
  - pretty output + raw fallback
  - message extraction helpers

- `helix-term/src/ai/ui.rs`  
  - buffer creation/reuse
  - append to output buffer
  - read/clear input buffer

### Touch existing files (minimal)

- `helix-term/src/commands/typed.rs`  
  - register: `ai-start`, `ai-resume`, `ai-send`, `ai-grow`, `ai-shrink`, `ai-quit`
  - small command docs

- `helix-view/src/tree.rs`  
  - add `Resize`/`Dimension`
  - add `resize_buffer` helper

- `helix-view/src/editor.rs`  
  - expose `resize_buffer` (thin wrapper)

Optional (only if needed):

- `helix-term/src/ui/editor.rs`  
  - if a UI tick / poll hook is needed for tailing

---

## `ai.toml` Schema (v1)

Minimal schema, no integration with Helix core config.

```toml
[general]
default_driver = "pi"          # used if no driver specified
output_buffer = "ai/output"
input_buffer = "ai/input"

[drivers.pi]
mode = "stream"                 # stream | tail
cmd = ["pi", "--mode", "rpc"]
session_dir = "~/.pi/agent/sessions"
session_glob = "--*--/*.jsonl"

[drivers.codex]
mode = "tail"
cmd = ["codex", "resume"]
session_dir = "~/.codex/sessions"
session_glob = "**/*.jsonl"

[drivers.claude]
mode = "tail"
cmd = ["claude", "--resume"]
session_dir = "~/.claude/projects"
session_glob = "-*/agent-*.jsonl"
```

Defaults (if config missing):

- `default_driver = "pi"`
- `output_buffer = "ai/output"`
- `input_buffer = "ai/input"`
- `pi` driver only (stream mode)

---

## v1 Parser Pseudocode (Pretty + Raw Fallback)

### Common helpers

```
fn append_output(text) { ... }
fn is_json(line) -> bool { ... }
fn parse_json(line) -> Value { ... }
```

### 1) JSONL parse loop

```
for line in stream:
  if !is_json(line):
     append_output(line + "\n")
     continue

  let v = parse_json(line)
  if try_render_message(v) { continue }
  if try_render_tool(v) { continue }

  // fallback
  append_output(line + "\n")
```

### 2) Render message (driver-agnostic)

```
fn try_render_message(v):
  // Claude style: v.message.content[*].text
  if let Some(content) = v["message"]["content"]:
     for part in content:
        if part["type"] == "text":
           append_output(part["text"])
     return true

  // Pi style: v.content[*].text
  if let Some(content) = v["content"]:
     for part in content:
        if part["type"] == "text":
           append_output(part["text"])
     return true

  // Codex history style: v["text"] or v["display"]
  if let Some(text) = v["text"]:
     append_output(text + "\n")
     return true
  if let Some(text) = v["display"]:
     append_output(text + "\n")
     return true

  return false
```

### 3) Render tools (optional v1)

```
fn try_render_tool(v):
  if v["type"] in ["tool_execution_start","tool_execution_update","tool_execution_end"]:
     append_output("\n**tool**\n```\n")
     // append tool output if present
     append_output("```\n\n")
     return true
  return false
```

This is intentionally minimal: it gives readable output quickly without deep
driver coupling. Later you can add structured renderers per driver.

---

## Driver-Specific Resume Logic (v1)

### pi (streaming)
```
start: pi --mode rpc
resume: pi --mode rpc --session <path>
send: JSON RPC "prompt" request
```

### codex (tail)
```
start: codex [prompt]
resume: codex resume <SESSION_ID> [prompt]
session_id: parse from filename (UUID suffix)
```

### claude (tail)
```
start: claude [prompt]
resume: claude --resume <sessionId> [prompt]
sessionId: read first JSONL lines to extract sessionId
```

---

## Patch Size Estimate (per file)

- `helix-term/src/ai/*`: ~1,200–2,400 LOC
- `helix-term/src/commands/typed.rs`: +150–300 LOC
- `helix-view/src/tree.rs`: +200–300 LOC
- `helix-view/src/editor.rs`: +30–60 LOC

Total: **~1,600–3,100 LOC**

---

## Implementation Order (Low Risk First)

1) **Window resize helpers**  
   - Port `Resize/Dimension` + `resize_buffer` to `helix-view/src/tree.rs`  
   - Add wrapper in `helix-view/src/editor.rs`  
   - Wire `:ai-grow` / `:ai-shrink` (no dependency on driver logic)

2) **UI buffers + commands (stubbed)**  
   - Create `[ai/output]` + `[ai/input]` buffers  
   - Implement `:ai-start` / `:ai-quit` with no driver process yet  
   - Ensure buffer reuse and default split works

3) **Config loader (`ai.toml`)**  
   - Read config and resolve `~` paths  
   - Provide defaults if missing

4) **Session picker**  
   - Implement `list_sessions` per driver  
   - Plug into `:ai-resume`

5) **Driver process + stream**  
   - Implement `stream` mode (pi)  
   - Append JSONL to output

6) **Tail mode**  
   - File tailer for session JSONL  
   - Used by codex/claude in v1

7) **Minimal formatting**  
   - Try JSON parse → pretty output  
   - Fallback to raw line

---

## Risk Checklist (v1)

- **Session id parsing**  
  Codex uses filename UUIDs; Claude uses `sessionId` inside JSONL. Ensure both
  are extracted robustly.

- **Tailing reliability**  
  File tailing can miss writes if the CLI buffers output. Prefer line-buffered
  or explicit flushes when possible.

- **JSONL shape drift**  
  These CLIs may change JSONL fields. Keep parsing permissive and fallback to
  raw output.

- **Buffer reuse bugs**  
  Ensure doc IDs are cached and checked (avoid duplicate buffers).

- **Resize heuristics**  
  Grow/shrink should be bounded so layout can’t collapse.


## Observed Driver Capabilities (Local)

These are the CLI and session formats currently on disk:

### pi

- CLI supports: `--mode rpc`, `--session <path>`, `--session-dir <dir>`,
  `--continue`, `--resume`
- Session files: `~/.pi/agent/sessions/--<cwd>--/*.jsonl`
- JSONL begins with:
  - `{"type":"session","version":3,...}`

### codex

- CLI supports: `codex resume [SESSION_ID] [PROMPT]`, `--last`, `--all`
- Session files: `~/.codex/sessions/YYYY/MM/DD/*.jsonl`
- JSONL begins with:
  - `{"id":"...","timestamp":"...","instructions":...,"git":{...}}`

### claude (Claude Code)

- CLI supports: `--resume [sessionId]`, `--continue`, `--session-id <uuid>`
- Session files (per project): `~/.claude/projects/-<cwd>/agent-*.jsonl`
- JSONL includes: `sessionId`, `type`, `message`, etc.

This suggests **pi** can run in streaming mode (`--mode rpc`), while **codex**
and **claude** can be supported via *per-message spawn + session tailing* for
v1, then upgraded to streaming if their stdin/stdout JSONL modes are stable.

---

## What The Native Patch Would Contain

This is a conceptual layout, not exact file paths.

### 1. A Pi Subsystem in Helix

**Responsibilities (Rust):**

- Spawn and manage the `pi --mode rpc` subprocess
- Read stdout/stderr lines and parse JSON events
- Maintain session state (streaming flag, request IDs, tool output lengths)
- Format output text the same way `pi-core.scm` does
- Expose commands that match the Steel ones

**Suggested shape:**

- `PiSession` struct in editor state
- `PiClient` for process I/O and JSON parsing
- An async task or background thread that forwards events to the UI thread
- A simple formatter for `message_*` and `tool_execution_*` events

This is a direct port of `pi-core.scm` and parts of `pi.scm` into Rust.

### 2. Built-in Pi Buffers

Use two scratch documents internally:

- `[pi/output]` (read-only, streaming output)
- `[pi/input]` (editable prompt buffer)

Both would be managed by Helix itself instead of by a plugin. The patch would
maintain doc IDs and ensure they survive session restarts, just like the
current `pi-ui` state does.

### 3. Commands (Native)

Implement the same command surface as in `docs/commands.md`:

- `:pi-start`, `:pi-send`, `:pi-quit`, `:pi-abort`, `:pi-continue`, `:pi-resume`
- `:pi-model`, `:pi-thinking`, `:pi-compact`, `:pi-new`
- `:pi-steer`, `:pi-follow`, `:pi-status`, `:pi-recover`, `:pi-sessions`

In Helix terms this likely means new commands in the command registry and a
small config block to set default keybindings.

### 4. Session File Handling

Reuse the same on-disk session convention from `pi-core.scm`:

- Session directory derived from `$PWD`
- `~/.pi/agent/sessions/<path-id>/...` jsonl files
- Ability to render full history into `[pi/output]`

### 5. Window Expansion / Focus Mode

The Pi layout today depends on Helix PR #8546 (window resize/focus mode). A
native patch would include this directly and hook it into the Pi layout.

Two likely behaviors:

- **Focus toggle**: expand the currently focused Pi pane (input/output)
- **Default split**: on `:pi-start`, open output + input split and give input
  focus

Because this is inside Helix, the patch can manipulate the view tree directly,
not via plugin callbacks, which makes expansion more predictable and less
fragile.

---

## What Would Change (User-Facing)

### Removed

- No Steel runtime dependency
- No `~/.config/helix/helix.scm` plugin wiring
- No `~/.config/helix/cogs/pi/` copies
- No plugin-only APIs or Steel limitations

### Added / Different

- Built-in Pi commands and buffers (available out of the box)
- Window expansion built into core Helix layout logic
- Pi process management handled by Helix, not a plugin layer

---

## Strengths of a Single Helix Patch

1. **Cleaner install and distribution**
   - One Helix build, no extra runtime or `cogs/` setup

2. **Tighter UI integration**
   - Full access to view/layout internals for focus/resize behavior

3. **Lower runtime overhead**
   - No Steel VM in the critical path
   - Fewer cross-language boundaries

4. **More robust threading model**
   - Direct use of Helix's async runtime and event loop

5. **Better long-term ergonomics**
   - Commands, keybindings, and config behave like built-in Helix features

---

## Weaknesses / Tradeoffs

1. **Slower iteration**
   - Every change requires rebuilding Helix

2. **Larger patch surface**
   - More Rust code in Helix core, harder to keep up with upstream changes

3. **Harder to upstream**
   - Pi is a specific integration; core Helix might not want it built-in

4. **Reduced flexibility**
   - Users cannot easily tweak logic the way they can edit `.scm` files

5. **More responsibility in editor process**
   - Bugs in the Pi integration are now editor bugs, not plugin bugs

---

## Minimal Porting Plan (Conceptual)

1. **Port `pi-core.scm` to Rust**
   - Keep the same event parsing and output formatting

2. **Add a Pi client to Helix**
   - Spawn `pi --mode rpc`, read JSON lines, route events to session state

3. **Create two scratch docs**
   - `[pi/output]` read-only, `[pi/input]` editable

4. **Wire commands**
   - Implement all `:pi-*` commands

5. **Integrate focus/resize feature**
   - Incorporate PR #8546 and add a Pi-focused toggle

---

## Summary

If this repo were replaced by a single Helix patch, the functionality would be
nearly identical, but the implementation would move from Steel into Rust and
from plugin space into core editor space. The biggest wins would be a cleaner
install path and more reliable window expansion. The biggest costs would be
slower iteration and a larger, less upstream-friendly patch.
