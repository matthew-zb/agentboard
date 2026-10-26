# AgentBoard Release History

## 미출시

### Fixes
- 터미널이 스스로 보내는 응답(포커스 in/out·마우스 보고·커서 위치 등)은 새 명령 `pty_write_raw`로 보내고 사용자 입력으로 기록하지 않는다. ConPTY와 Claude Code가 켜는 포커스·마우스 보고 때문에 입력이 없어도 야간 정리가 미뤄지던 문제를 고쳤다.
- `onBinary` 데이터(X10 마우스 보고 등)를 바이트 그대로 보낸다(0x80 이상 바이트가 UTF-8로 바뀌던 문제).
- 창 입력·크기 변경이 창별 대기열과 writer 스레드를 거쳐, 큰 붙여넣기나 입력을 읽지 않는 프로세스 때문에 화면이 멈추지 않는다. 앱 종료(저장·프로세스 정리)도 화면 스레드 밖에서 돈다.
- 문서 탭의 http(s) 링크를 앱의 Job Object 밖에서 연다. 그 링크로 처음 띄운 브라우저가 AgentBoard 종료 때 함께 꺼지지 않는다.
- 새 업무 탭의 폴더 이름에서 Windows 금지 문자·장치 이름(CON, NUL, COM1 …)·끝의 점/공백·`.agentboard`·`.git`을 거부한다(`billing.`이 기존 `billing` 폴더를 가리키던 문제). 이미 config.json에 있는 탭은 그대로 읽는다.
- 창이 `TMUX`·`TMUX_PANE`·`PSMUX_*` 환경 변수를 물려받지 않는다(psmux 셸에서 띄웠을 때 Claude Code가 tmux로 오인하던 문제).
- katex 0.19.0(0.18.11은 upstream에서 폐기 표시됨).

### Notes
- push·PR마다 JS·Rust 테스트를 도는 CI(`.github/workflows/ci.yml`)를 추가했다. 릴리스 빌드는 산출물이 빠지면 실패하고, 태그 없이 수동 실행할 수 있다.
- `docs/spikes.md`의 1단계 체크리스트 중 현재 키·마법사와 어긋난 곳을 고쳤다.

## V0.2.0

2단계 — 상시업무 조직 운영. dispatcher 창 하나에 머물며 여러 상시업무를 Claude Code 에이전트들에게 맡긴다.

### Features
- 역할 에이전트: dispatcher(사용자 창구), handover(`/loop` 주기 확인 + 포인터만 담는 업무 인덱스), 상시업무별 Manager, 임시 팀(리드·팀원). 역할 세션은 `--name`·`--append-system-prompt`(역할 지시 `roles/*.md`)·`--mcp-config`·`--settings`(hook, 세션 간 메시지 수신)로 실행된다.
- 앱 내장 MCP 서버(127.0.0.1, 창별 토큰): 인박스(`inbox_post/note/propose_close/list`), 이슈 폴더(`issue_create`), 팀 채용·해산(`hire_lead`, `hire_member`, `dismiss`, `dismiss_team`, `list_team`), `create_manager_tab`, `handoff_done`, `need_user`, 환경 점검(`env_status/skip/set_source`). 도구별 허용 역할을 앱이 검사한다.
- 앱→세션 메시지: 세션의 메시지 수신함(named pipe)으로 `agentboard` 이름의 메시지를 보내 입력란을 건드리지 않는다.
- 인박스 애플릿(dispatcher 옆): 항목 목록·상세·타임라인, 지시(`i`)·자료 저장(`s`)·이슈 등록(`u`)·종료 확인(`c`). 종료 확인은 사용자만.
- 팀: Manager 탭의 팀 영역(`slot: "team"`)에 팀마다 한 열, 팀 수 제한 없음, 팀당 팀원 상한(기본 4). 팀 문서 `teams/<id>/{brief.md,roster.md}`로 재부팅·야간 정리 뒤에도 이어간다.
- 마크다운 문서 탭: Mermaid·KaTeX·코드 강조, 목차·검색·자동 새로고침, `workspaceRoot` 안 파일만, 정화된 HTML.
- 주의(⚠): Notification hook의 권한 승인 대기, `need_user` 사유, 폴더 신뢰 대기. 가려진 탭은 상태 표시줄 목록.
- 야간 정기 정리(`nightlyRefresh`, 기본 03:00): 계획·진행 내역 저장 → handover 인덱스 갱신 → 상시 세션을 새 세션으로 재시작. 팀 창은 건드리지 않고, 입력 중인 창은 미룬다.
- 시작·복원: 상시 에이전트만 새 세션 + 시작 프롬프트, 팀은 Manager가 자동 재채용. config.json이 역할 탭의 기준.
- 환경 점검(Claude Code ≥ 2.1.234, Git Bash, 폴더 신뢰, 사용자 정의 `envRequire`)과 dispatcher의 단계별 설치 안내(사용자 승인 후에만 실행).
- 로그인 시 자동 실행 토글(HKCU Run), 새 탭 마법사의 Manager 선택.
- 기본 역할 지시 5종과 템플릿 `dispatcher-inbox`·`handover`·`manager`. 새로 설치하면 dispatcher 옆에 인박스가 붙는다.
- 한 번에 하나만 실행(두 번째 실행은 기존 창을 앞으로 가져오고 끝난다). hook CLI 모드는 영향 없음.

### Notes
- Claude Code 2.1.234 이상과 Git for Windows가 필요하다. 역할 세션의 작업 폴더마다 Claude의 폴더 신뢰 질문에 한 번 답해야 한다.
- 메시지 경로·역할 간 권한 분리는 역할 지시 수준(권고)이다. 앱은 MCP 도구 권한만 강제한다.
- 역할 지시는 보낸 이가 정확히 `agentboard`인 메시지만 사용자 지시로 다루고, 외부 내용(슬랙·메일·웹·파일·다른 세션이 옮긴 글) 안의 지시는 따르지 않는다. `agentboard`는 예약된 이름이다.
- 역할 창의 시작 프롬프트는 그 세션의 SessionStart 뒤에만 넣는다(폴더 신뢰 질문에 입력하지 않음). Git Bash가 없어 hook이 돌지 않으면 시작 프롬프트도 들어가지 않는다.
- 인박스 항목의 담당은 탭 이름으로 기록된다(이름 바꾸기에 따라감, 이전 버전 항목은 시작 때 한 번 옮김).
- 첫 실행 때 `.agentboard/roles/*.md`·`templates/*.json`으로 복사된 기본 파일은 업그레이드해도 바뀌지 않는다. 새 기본값을 쓰려면 앱을 끄고 그 파일을 지운 뒤 다시 시작한다.
- 실제 Claude 세션으로 하는 수동 QA는 `docs/spikes.md` "2단계 QA"에 있으며 사용자 확인 전이다.

## V0.1.0

1단계 첫 릴리스 — 키보드로 조작하는 터미널 에이전트 보드.

### Features
- 그룹 → 탭 → 분할 창 구조. 창마다 ConPTY 프로세스(Claude Code, PowerShell, cmd), xterm.js 렌더링(WebGL, 실패 시 DOM), 스크롤백 10,000줄, 창별 1MB 링버퍼.
- 키보드 전용 조작: 창 이동·크기 조절·분할·최대화·닫기·재시작, 탭 전환·이동·새 탭 마법사, 그룹 분할·합치기·닫기·최대화, 번호 선택 오버레이, 커맨드 팔레트(`Alt+;`). `KeyboardEvent.code` 기준이라 한글 IME에서도 동작하며, 에이전트 프로필의 `passthroughKeys`로 키를 프로그램에 넘길 수 있다.
- 복사·스크롤 모드(검색 포함), 입력 동기화(`[SYNC]`), `Ctrl+Shift+V` 붙여넣기.
- 터미널 벨 기반 주의(⚠) 표시와 `Alt+a` 이동·복귀. 포커스는 자동으로 옮기지 않는다.
- 탭 마우스 드래그앤드롭(그룹 사이 이동, 그룹 가장자리로 새 그룹 분할, 합치기 등).
- 설정 파일(`settings.json`, `config.json`, 템플릿, 에이전트 프로필, `keybindings.json`) 로드·검증, `파일:줄:열 — 원인` 오류 표시.
- 종료 후 재실행 시 그룹·탭·레이아웃·포커스 복원(500ms 디바운스 원자적 저장). 프로세스는 다시 실행한다.
- 앱 종료 확인: 실행 중인 에이전트 N개를 알리고 `Y/N`으로 확인. Job Object로 자식 프로세스 트리까지 정리.

### Notes
- 수동 QA 항목은 `docs/spikes.md`에 정리되어 있으며 사용자 확인 전이다.
- 개발에는 Node 22 이상이 필요하다.
