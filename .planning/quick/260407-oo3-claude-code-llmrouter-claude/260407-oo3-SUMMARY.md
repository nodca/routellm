# Quick Task 260407-oo3 Summary

## Task

Close the first Claude Code compatibility gap by making `/v1/messages` failures diagnosable from `request_logs` and by returning message-path errors in a more Anthropic-compatible shape.

## Result

- Added `request_logs.downstream_client_request_id` and `request_logs.claude_request_fingerprint`, and exposed both through `/api/routes/{route_id}/logs`.
- The Claude fingerprint now stores only an allowlisted request shape:
  - selected Claude headers (`anthropic-beta`, `x-app`, `x-client-app`, `x-client-request-id`, `user-agent`)
  - presence-only markers for `X-Claude-Code-Session-Id` and `x-anthropic-additional-protection`
  - top-level request keys, message count, `system`/`tools`/`tool_choice`/`thinking`/`context_management` presence, `stream`, and token knobs
- `/v1/messages` and `/messages` now return Anthropic-style error envelopes with a `request-id` header for auth failures, local validation/adapter failures, no-route failures, and upstream transport/5xx failures.
- `/v1/responses` and `/v1/chat/completions` keep their existing OpenAI-style error responses.

## Commits

- `8879c08` `test(260407-oo3): add failing claude diagnostics tests`
- `8874152` `feat(260407-oo3): persist claude request fingerprints in logs`
- `df5482c` `feat(260407-oo3): add claude message correlation errors`
- `ac38b5c` `refactor(260407-oo3): format claude diagnostics changes`

## Verification

- `cargo test --lib`

## Deviations

- Plan file named the migration `migrations/0007_request_log_claude_request_snapshot.sql`, but the repo already contained `migrations/0007_channel_needs_reprobe.sql`.
- Added `migrations/0008_request_log_claude_request_snapshot.sql` instead so SQLx migrations remain uniquely versioned and the suite can run.

## Notes

- The new fingerprint does not persist raw prompt text, system prompt text, tool arguments, auth tokens, or arbitrary headers.
- `cargo test --lib` still emits the existing `unused_assignments` warnings in the SSE tool-stream conversion code inside `src/http.rs`; this quick task did not change that behavior.

## Self-Check

- Summary file present at `.planning/quick/260407-oo3-claude-code-llmrouter-claude/260407-oo3-SUMMARY.md`
- `STATE.md` updated for this quick task
- Commits `8879c08`, `8874152`, `df5482c`, and `ac38b5c` verified in git history
