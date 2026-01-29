# AI Integration References

## Inspiration

- Terminus (Terminal-Bench): https://www.tbench.ai/terminus

## Local CLI Notes (Jan 29, 2026)

These notes are from local CLI help outputs and on-disk session files. They are
meant as working references for adapter design, not as canonical specs.

### pi

- CLI flags: `--mode rpc`, `--session <path>`, `--session-dir <dir>`,
  `--continue`, `--resume`.
- Session root: `$PI_CODING_AGENT_DIR` (default `~/.pi/agent`).
- Session files: `~/.pi/agent/sessions/--<cwd>--/*.jsonl`.
- JSONL begins with a `{"type":"session","version":3,...}` record.

### codex

- Resume command: `codex resume <SESSION_ID>` (also supports `--last`).
- Session files: `~/.codex/sessions/YYYY/MM/DD/*.jsonl`.
- JSONL begins with a `{"id":"...","timestamp":"...",...}` record.
- `~/.codex/history.jsonl` exists but is not a full transcript.

### claude (Claude Code)

- Resume command: `claude --resume <sessionId>` (also `--continue`).
- Session files (per project): `~/.claude/projects/-<cwd>/agent-*.jsonl`.
- JSONL includes `sessionId`, `type`, and `message` objects.

## Notes for v1 adapters

- Prefer **streaming JSONL** over stdio when available.
- For drivers without stable stdin/stdout streaming, use **per-message spawn**
  and **session-file tailing**.
