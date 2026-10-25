# 2단계 스파이크 (P1–P3) — 버리는 코드

빌드 대상 아님. 결과는 `docs/spikes.md`의 "2단계 스파이크" 절에 있다. `samples/`에 실제로 받은 hook stdin·MCP 요청 예가 있다(토큰은 `<redacted>`).

모두 Git Bash 기준. `S`는 아무 임시 폴더(경로에 공백 없게). 세션 이름은 항상 `abspike-<난수>-…`로 다른 세션과 겹치지 않게 한다. Claude Code 세션 안에서 돌릴 때는 부모 세션의 `CLAUDE_*` 환경 변수가 새지 않도록 `cleanenv.sh`로 감싼다.

| 파일 | 역할 |
|---|---|
| `mcp-server.mjs` | P3: 평문 JSON 응답(SSE 없음) HTTP MCP 서버. 모든 요청을 JSONL로 기록. `Authorization: Bearer x`가 아니면 401 |
| `hooklog.mjs` | hook 명령. stdin JSON + `CLAUDE_CODE_MESSAGING_SOCKET/TOKEN` 등 환경 변수를 `<dir>/<event>-<n>.json`으로 떨군다 |
| `mksettings.sh` | `--settings` 파일 생성: SessionStart/UserPromptSubmit/Stop/Notification hook + `crossSessionInbound`(`accept`/`hold`/`refuse`/`none`=키 생략) |
| `worker.mjs` | `claude -p --input-format stream-json --output-format stream-json --verbose …` 장기 워커. stdin을 열어 둔 채 N초 유지 후 프로세스 트리 kill |
| `inject.mjs` | SessionStart 덤프에서 소켓·토큰을 읽어 named pipe에 auth 줄 + 메시지 줄을 쓴다 |
| `pty_interactive.py` | pywinpty(ConPTY)로 대화형 세션을 띄워 파이프 주입·권한 프롬프트·idle 알림을 확인 |
| `cleanenv.sh` | `CLAUDE*`/`CLAUDECODE`/`ANTHROPIC_BASE_URL` 변수를 지우고 명령 실행 |

## P3 — MCP HTTP + 헤더

```bash
node spikes/phase2/mcp-server.mjs 47811 "$S/req.jsonl" &
echo '{"mcpServers":{"ab":{"type":"http","url":"http://127.0.0.1:47811/mcp","headers":{"Authorization":"Bearer x"}}}}' > "$S/mcp.json"
spikes/phase2/cleanenv.sh claude -p --model haiku --strict-mcp-config --mcp-config "$S/mcp.json" \
  --allowedTools mcp__ab__echo_who "Call mcp__ab__echo_who with text 'ping' and reply with the result" < /dev/null
kill %1; cat "$S/req.jsonl"
```

## P1 — SessionStart hook → named pipe 주입

```bash
R=$RANDOM
spikes/phase2/mksettings.sh "$S/settings.json" "$S/hooks" accept     # 또는 none
spikes/phase2/cleanenv.sh node spikes/phase2/worker.mjs "$S/a" 100 "Reply ready. For each later message reply GOT: <text>." \
  -- --model haiku --name abspike-$R-a --settings "$S/settings.json" &
# hooks/SessionStart-0.json 생기면:
node spikes/phase2/inject.mjs "$S/hooks/SessionStart-0.json" "pipe-hello"
# 보낸 이 표시를 붙이려면 세 번째 인자로 줄 전체를 준다:
node spikes/phase2/inject.mjs "$S/hooks/SessionStart-0.json" x \
  '{"type":"user","message":{"role":"user","content":"<cross-session-message from-name=\"agentboard\">\nhello\n</cross-session-message>"}}'
```

## P2 — `--name` 세션끼리 SendMessage

수신자 2개(`--permission-mode auto`, 하나는 `accept`, 하나는 키 없음)를 `worker.mjs`로 띄우고, 송신 워커에 "SendMessage로 `abspike-$R-r1`, `abspike-$R-r2`에 보내라"를 준다. 각 워커의 `stdout.jsonl`·`hooks/*`와 `~/.claude/projects/<cwd>/<session>.jsonl`의 `origin`을 본다.

## 대화형(ConPTY)

```bash
spikes/phase2/mksettings.sh "$S/settings.json" "$S/hooks" accept
python spikes/phase2/pty_interactive.py abspike-$R-i "$S/settings.json" "$S/hooks" "$S/out" C:/workspace [--permission-mode default]
```
`pywinpty`가 있는 파이썬이 필요하다. cwd는 이미 신뢰된 폴더여야 한다(신뢰 대화 상자를 피하기 위해).
