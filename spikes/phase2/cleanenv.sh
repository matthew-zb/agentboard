#!/usr/bin/env bash
# Run a command without any CLAUDE*/CLAUDECODE/ANTHROPIC_BASE_URL variables inherited from a parent
# Claude Code session (so the spike session behaves like one launched by AgentBoard).
unset_args=()
for v in $(env | grep -oE '^(CLAUDE[A-Z0-9_]*|CLAUDECODE|ANTHROPIC_BASE_URL)=' | sed 's/=$//'); do unset_args+=(-u "$v"); done
exec env "${unset_args[@]}" "$@"
