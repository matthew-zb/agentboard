# AgentBoard 2단계 — 상시업무 조직 운영 설계

- **Date:** 2026-10-25
- **Repo:** `C:/workspace/agentboard` (1단계 완료, master `be2e1f2` 이후)
- **Status:** Draft (브레인스토밍 완료, 계획 전)
- **선행 문서:** 1단계 스펙 `docs/superpowers/specs/2026-10-24-agentboard-design.md`, 1단계 후속 과제 `docs/superpowers/phase1-followups.md`

> 표기: **[사용자 결정]** = 사용자가 직접 정한 것, **[설계자 판단]** = 설계자가 정한 것(빌더가 바꿀 수 있음, 바꾸면 기록).

## 1. 의도

사용자는 PC를 거의 끄지 않고 하루 종일 **dispatcher 창에만 머무르며** 여러 상시업무를 Claude Code 에이전트들에게 맡긴다. 2단계는 1단계 멀티플렉서 위에 이 운영 방식을 올린다:

- 상시업무마다 탭 하나, 탭마다 **Manager** 에이전트 하나가 항상 떠 있다. **[사용자 결정]**
- 바쁠 때만 Manager가 **팀 리드**를 채용하고, 팀 리드가 **팀원**을 채용해 같은 탭 안 분할 창에서 일한다. 팀 리드의 핵심 역할은 팀원 결과물을 **검증하고 지적**해 품질을 올리는 것. 일이 끝나면 해산. **[사용자 결정]**
- 모든 보고는 **dispatcher**로 모이고, 사용자는 dispatcher 옆 **인박스 애플릿**에서 항목을 보고 지시·종료 확인한다. **[사용자 결정]**
- 사용자가 퇴근한 새벽에 **야간 정기 정리**로 Manager 세션을 새로 고친다. 대화 이어가기(`--resume`)는 쓰지 않고 문서로 이어간다. **[사용자 결정]**

### 성공 기준
- 아침에 dispatcher 창만 보고 있어도 밤사이·오전에 들어온 일의 "확인할 것 / 리스크"가 인박스에 정리돼 있고, 거기서 지시·저장·이슈 등록·종료 확인을 끝낼 수 있다.
- 집중 업무 때 "팀 구성해" 한 마디로 팀이 같은 탭에 뜨고, 끝나면 정리된다.
- 며칠·몇 주 켜 두어도 Manager 맥락이 흐려지지 않는다(야간 정리), 예고 없는 재부팅 후에도 문서로 이어간다.

### 비목표 / 반목표
- 비목표: Codex 등 Claude 외 에이전트 지원(프로필 구조만 유지), 앱이 에이전트 대신 주기 작업 실행(handover는 Claude Code `/loop` 사용), `--resume` 대화 복원, 슬랙·메일 연동(handover가 자체 도구로 처리), 앱 레벨의 메시지 경로 강제.
- **반목표 (이러면 실패):** 사용자가 dispatcher에 타이핑 중인 입력에 다른 에이전트의 텍스트가 섞여 들어가는 것 · 앱이 스스로 포커스나 탭을 옮기는 것(1단계 원칙 유지) · handover 인덱스 문서가 내용 요약으로 지저분해지는 것 · 야간 정리가 가동 중인 팀 작업을 끊는 것.

## 2. 역할과 흐름

| 역할 | 수명 | 하는 일 |
|---|---|---|
| **dispatcher** | 상시 | 사용자 창구. handover가 전달한 메시지를 판단해 담당 Manager에게 넘김. 사용자 지시를 담당 Manager에게 전달. 이슈·자료 정리 지원 |
| **handover** | 상시 | `/loop`로 슬랙·메일을 주기 확인(현재 10분, 바꿀 수 있음) → 새 메시지를 dispatcher에게 전달. **업무 인덱스 문서**(포인터만) 유지 |
| **Manager** (상시업무별, 현재 약 5개) | 상시 | 자기 업무 처리. 검토 결과를 **인박스 항목**으로 올림. 필요 시 팀 리드 채용. 야간 정리 때 진행 내역 저장 → handover 보고 |
| **팀 리드** | 임시 | Manager가 정해 준 업무를 팀원에게 배분, 결과물 검증·지적, Manager에게 보고 |
| **팀원** | 임시 | 팀 리드의 지시 수행, 팀 리드에게만 보고 |

**메시지 경로 [사용자 결정]**: handover → dispatcher → 담당 Manager → (인박스 항목) → 사용자 → **지시는 dispatcher를 거쳐** 담당 Manager. 팀이 있으면 Manager ↔ 팀 리드 ↔ 팀원.
**경로 강제 [설계자 판단]**: 앱이 막지 않는다. 역할 지시(시스템 프롬프트)로 지키게 한다. Claude Code 세션 간 메시징이 대상 제한을 지원하지 않기 때문이며, 사용자 혼자 쓰는 환경이라 위반 비용이 낮다.

## 3. 에이전트 간 메시지 — Claude Code 세션 간 메시징 사용

[Claude Code cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging)을 쓴다. 같은 PC 세션끼리 `SendMessage`/`ListAgents`, 받는 세션이 작업 중이면 도구 호출 사이에, 쉬고 있으면 새 턴으로 전달되며 **입력란을 건드리지 않는다**(반목표 충족).

- **요구 버전**: Windows 네이티브 Claude Code **v2.1.234 이상**. 현재 이 PC는 2.1.215 → 업데이트 필요. 앱은 시작 시 `claude --version`을 확인해 미달이면 상태 표시줄 오류를 띄운다. **이때도 role 탭은 정상으로 띄우고 MCP·인박스도 동작한다**; 에이전트끼리 메시지만 안 되며, 앱→세션 메시지는 §3 대안(PTY) 경로를 쓴다 **[설계자 판단]**.
- **세션 이름**: 앱이 각 에이전트를 `--name <에이전트 이름>`으로 띄운다. 이름 규칙 **[설계자 판단]**: 상시 에이전트는 탭 이름(`dispatcher`, `handover`, `billing` …), 팀은 `<탭>-<팀id>-lead`, `<탭>-<팀id>-m1`, `<탭>-<팀id>-m2` … (팀id는 Manager가 정하는 짧은 이름, 예: `batch`). 다른 세션이 이 이름으로 메시지를 보낸다.
- **수신 설정**: 사용자는 auto 모드 위주로 쓰므로 기본값으로 전달된다. 앱이 띄우는 세션에는 `--settings`로 `crossSessionInbound: "accept"`를 명시해 모드와 무관하게 받게 한다. **[설계자 판단]**
- **앱 → 세션 메시지** (사용자 지시 전달, 야간 정리 신호): 세션의 inbox 소켓(Windows named pipe)에 쓴다. Windows는 연결 첫 줄에 `CLAUDE_CODE_MESSAGING_TOKEN` 인증이 필요하므로, 앱이 각 세션에 **SessionStart hook**을 넣어 hook이 `CLAUDE_CODE_MESSAGING_SOCKET`·`CLAUDE_CODE_MESSAGING_TOKEN`을 앱의 로컬 엔드포인트로 보고하게 한다. 앱은 그 소켓·토큰으로 메시지를 쓴다. **[설계자 판단 — 스파이크 P1로 먼저 검증]**
  - 대안(P1 실패 시): 대상 세션이 idle(Stop hook)이고 **입력란이 비어 있다고 확인될 때만** PTY로 입력·제출. 확인 불가 시 대기. 사용자 입력과 섞이면 안 된다(반목표).

## 4. 앱 내장 MCP 서버

- 127.0.0.1의 HTTP MCP 서버를 앱이 띄운다. 각 에이전트는 `--mcp-config`로 `http://127.0.0.1:<port>/mcp` + `Authorization: Bearer <에이전트별 토큰>`을 받는다. 토큰으로 호출자(이름·역할·탭)를 식별한다. **[설계자 판단]**
- 도구별 허용 역할을 앱이 검사한다(메시지 경로와 달리 **채용·해산·인박스는 앱이 강제**).

| 도구 | 허용 역할 | 동작 |
|---|---|---|
| `inbox_post(title, check[], risk[], source, refs?)` | Manager | 항목 생성, 담당 = 호출자. 반환: 항목 id |
| `inbox_note(item, text)` | 담당 Manager | 항목 타임라인에 기록 추가 |
| `inbox_propose_close(item, final_opinion)` | 담당 Manager | 상태 → 종료 제안됨, 최종 의견 기록 |
| `inbox_list(state?)` | 모두 | 자기 담당(또는 dispatcher는 전체) 항목 조회 |
| `issue_create(item?, title)` | Manager | `<Manager 작업폴더>/issues/<id>/{input,output}` 생성, 항목이 있으면 연결. 반환: 경로 |
| `hire_lead(team, brief_path, extra_prompt?)` | Manager | 자기 탭에 팀 `team`을 만들고 팀 리드 창 추가(팀 영역 분할). 한 Manager가 **팀을 개수 제한 없이** 동시에 둘 수 있다 [사용자 결정] |
| `hire_member(extra_prompt?)` | 팀 리드 | 자기 팀 영역에 팀원 창 추가 (팀당 상한 `roles.member.maxPerTeam`) |
| `dismiss(agent)` / `dismiss_team(team)` | Manager(자기 탭의 리드·팀원·팀 전체), 팀 리드(자기 팀원) | 해당 창(들) 종료, 팀 해산 시 팀 영역 제거 |
| `list_team(team?)` | Manager·팀 리드 | 탭 안 팀들(또는 한 팀)의 구성과 상태 |
| `handoff_done()` | Manager·dispatcher·handover | 야간 정리 완료 신호(§7) |
| `create_manager_tab(name, dir, template?)` | dispatcher | 새 상시업무 탭 + Manager를 실행 중에 생성하고 `config.json`의 `tabs`에 role `manager`로 추가. 탭은 dispatcher가 있지 않은 그룹(기본 `defaultGroup`)에 열리고 포커스·활성 탭을 바꾸지 않는다(1단계 R12) |

- **상시업무 추가 [원래 요구사항]**: 새 업무 영역이 생기면 dispatcher가 `create_manager_tab`으로 탭과 Manager를 만들고 설정을 갱신한다. 사용자도 1단계 `Alt+n`으로 만들 수 있다(마법사에 역할 선택 추가). **상시업무 제거 [설계자 판단]**: 2단계 범위 밖 — 사용자가 탭을 닫고 `config.json`에서 지운다. 남은 이슈 폴더·인박스 항목은 그대로 둔다.
- **팀 창의 프로필·인자 [설계자 판단]**: `roles.<역할>`에 `agent`(기본 `claude`)·`args`를 둔다. 채용 도구는 이 기본값을 쓰고, `hire_*`에 `args` 덮어쓰기는 두지 않는다. 탭당 팀원 상한 `roles.member.maxPerTeam`(기본 4).

- 채용·해산은 1단계 `WsCommand`(SplitPane/ClosePane)를 재사용한다. 1단계 후속 과제의 "Tauri 명령 안의 apply→submit→publish를 `launcher::execute`로 분리"를 먼저 한다.
- 1단계 후속 과제 중 **창 재시작 경합(spawn generation)**은 MCP가 동시에 명령을 보내므로 2단계 착수 시 반드시 해결한다.
- **팀 업무 문서 [사용자 결정 → 형식은 설계자 판단]**: Manager는 팀 리드를 채용하기 전에 팀 업무 문서(`<작업폴더>/teams/<팀id>/brief.md`)를 쓰고 `hire_lead`에 경로를 넘긴다. 팀 구성(팀id, 팀원 수, 각 역할 한 줄)도 같은 폴더의 `roster.md`에 Manager가 기록한다. 팀 리드는 그 문서를 기준으로 일한다. 야간 정리로 Manager가 새로 떠도 이 문서로 팀을 이어 관리한다.

## 5. 인박스 애플릿 (C안: 인박스 + 상세) [사용자 결정]

dispatcher 탭을 분할해 오른쪽에 둔다. 터미널이 아닌 **앱 화면 창**(새 창 종류 `applet:inbox`). 레이아웃은 템플릿에 `{ "applet": { "name": "inbox", "kind": "inbox" } }`로 둔다.

- **목록**: 처리할 항목, 안 읽음 굵게, 상태 표시(새 항목 / 진행 중 / **종료 제안됨** 강조). 완료 항목은 숨김(토글로 보기 **[설계자 판단]**).
- **상세**: 제목, 담당 업무, 출처(슬랙/메일·시각), 경로(handover→dispatcher→담당), **확인할 것**, **리스크 포인트**, **타임라인**(지시·메모·자료 저장·종료 제안 기록), 연결된 이슈 폴더.
- **항목 수명 [사용자 결정]**: 새 항목 → 진행 중 → 종료 제안됨(Manager가 최종 의견과 함께 제안) → 완료(**사용자가 최종 의견 확인 후 종료 확인**). 종료 제안됨에서 다시 지시하면 진행 중으로 되돌아감. 종료까지 어떤 일이 몇 번 오갈지 모른다.
- **버튼(키보드 단축키 포함) [동작은 사용자 결정, 키는 설계자 판단]**
  - **지시(`i`)**: 입력창 → 앱이 "항목 정보 + 지시"를 **dispatcher에게** 메시지로 보냄 → dispatcher가 담당 Manager에게 전달. 타임라인에 기록. 상태 → 진행 중.
  - **자료 저장(`s`)**: 파일 경로(여러 개) 입력 → 앱이 항목의 이슈 폴더 `input/` 또는 `output/`(선택)에 복사. 이슈가 없으면 먼저 만들지 묻는다. 타임라인 기록 + 담당 Manager에게 dispatcher 경유로 알림.
  - **이슈 등록(`u`)**: dispatcher 경유로 담당 Manager에게 "이 항목을 이슈로 등록하라" 전달 → Manager가 `issue_create`. **[설계자 판단: 이슈는 Manager가 만든다 — 담당이 알아야 하므로]**
  - **종료 확인(`c`)**: 종료 제안됨 상태에서만. 완료로 전환, 담당 Manager에게 알림.
  - 목록 `j/k`, Enter 상세, `Esc` 목록으로. 터미널↔애플릿은 1단계 `Alt+h/l`.
- **저장 위치 [설계자 판단]**: `<workspaceRoot>/.agentboard/inbox/<id>.json`(항목 + 타임라인). 원자적 쓰기. 애플릿은 1단계 스냅샷 방식으로 변경 이벤트를 받아 다시 그린다.

## 5-1. 보이지 않는 창의 주의 표시 [사용자 결정]

- 사용자는 dispatcher 창에 머무르므로, 다른 탭 에이전트가 **권한 승인 대기·질문 대기**로 멈추면 그 **탭에 ⚠ 표시**로 알린다(인박스 항목으로 올리지 않는다).
- 감지: 1단계 터미널 벨에 더해 Claude Code **Notification hook**(권한 요청)을 앱이 각 세션에 넣어 SetAttention을 정확히 보낸다. 권한 요청 알림은 권한 모드에 따라 오지 않을 수 있다(auto 모드에서는 거의 없음 — 스파이크 결과).
- **사용자 결정이 필요한 에이전트도 탭에 표시 [사용자 결정]**: 에이전트가 사용자의 판단·결정이 필요하면 MCP 도구 `need_user(reason)`을 호출한다(모든 역할 허용). 앱은 그 창에 attention을 켜고 사유를 ⚠ 툴팁·상태 표시줄 목록에 보여 준다. 사용자가 그 창에 포커스하면 해제된다. 역할 지시에 "사용자 결정이 필요하면 질문을 화면에 쓰고 `need_user`를 호출하라"를 넣는다. `idle_prompt` 알림은 상시 에이전트가 쉬는 동안 늘 발생하므로 주의 표시에 쓰지 않는다 **[설계자 판단]**.
- **탭은 항상 보여야 한다 [사용자 결정]**: dispatcher 화면에서도 모든 탭의 ⚠가 보여야 한다. 그룹이 최대화되어 다른 그룹 탭 바가 가려지면, 상태 표시줄에 ⚠ 탭 목록(`⚠ billing, api`)을 띄운다 **[설계자 판단]**. `Alt+a`로 이동(1단계 기능).

## 6. 이슈 폴더 [사용자 결정]

```
<담당 Manager 작업폴더>/issues/<이슈 id>/
   input/    ← 전달받은 자료(슬랙·메일 첨부, 사용자 파일)
   output/   ← Manager·팀이 정리한 문서·결과물
```
이슈 id 형식 **[설계자 판단]**: `YYYYMMDD-<짧은 제목 슬러그>` (중복 시 `-2`). 이슈로 승격하지 않은 가벼운 항목은 폴더 없이 인박스 기록만.

## 7. 야간 정기 정리 [사용자 결정]

- `config.json`에 `"nightlyRefresh": "03:00"`(사용자가 퇴근한 뒤 시각, 바꿀 수 있음).
- 순서:
  1. 앱이 각 Manager(그리고 dispatcher·handover)에게 "마감 정리" 메시지를 보낸다.
  2. 각 Manager는 작업 계획·진행 내역 파일을 갱신하고 **팀 작업을 방해하지 않는 범위에서** 저장한 뒤, handover에게 파일 위치를 보고하고 `handoff_done()`.
  3. handover는 인덱스 문서의 포인터만 갱신한 뒤 `handoff_done()`.
  4. 모든 대상이 완료했거나 제한 시간(기본 20분 **[설계자 판단]**)이 지나면, 앱이 Manager·dispatcher·handover 세션을 **새 세션으로** 재시작(1단계 RestartPane + 시작 프롬프트 재주입). 시간 초과한 에이전트는 인박스에 "야간 정리 미완료" 항목으로 남긴다.
- **팀 세션(리드·팀원)은 재시작하지 않는다** — 팀은 하던 일을 계속한다. [사용자 결정]
- 사용자가 그 시각에 dispatcher에 입력 중이면(최근 N분 내 입력) 정리를 미루고 다음 기회에 재시도 **[설계자 판단]**.

## 8. handover 업무 인덱스 문서 [사용자 결정]

- **포인터만** 담는다. 업무마다 한 줄: 업무 이름, 작업 계획 파일 경로, 진행 내역 파일 경로, 마지막 갱신 시각. 요약·대화 내용 금지.
- 관리 주체는 handover 에이전트(역할 지시). 앱은 경로만 설정으로 안다(`config.json`의 `handoverIndex`, workspaceRoot 기준 상대경로, 기본 `HANDOVER.md` **[설계자 판단]**).
- **handover의 작업공간은 dispatcher와 같이 루트(`workspaceRoot`)다 [사용자 결정]**. config에서 handover 탭은 `dir: ""`(빈 값 = 루트)로 둔다. 각 Manager의 작업 폴더를 모두 볼 수 있는 위치다.

## 9. 시작과 자동 실행

- **자동 실행 [사용자 결정]**: PC 로그인 시 AgentBoard 실행. 구현은 HKCU `Run` 레지스트리 키, 설정 화면/팔레트에서 켜고 끔 **[설계자 판단]**.
- 시작 시 **상시 에이전트만** 띄운다(dispatcher, handover, 각 Manager). 팀 영역은 비어 있음. 모두 새 세션. [사용자 결정]
- **환경 점검과 dispatcher 설치 안내 [사용자 결정]**: 시작 시 앱이 필요한 도구를 점검한다(처음 항목: Claude Code ≥ 2.1.234, Git for Windows(Git Bash), 각 role 작업 폴더의 Claude Code 신뢰 상태). 결과는 MCP 도구 `env_status()`(모든 역할)로 조회하며, 항목마다 상태·필요 이유·없으면 빠지는 기능·설치/조치 방법을 준다. dispatcher는 시작 리뷰 때 이를 확인하고 빠진 항목을 사용자에게 하나씩 안내한다. **설치·변경 명령은 사용자 승인 후에만** dispatcher가 실행한다. 사용자가 "필요 없다"고 하면 dispatcher가 `env_skip(item)`을 호출해 `config.json`의 `envSkip`에 기록하고, 이후 다시 묻지 않는다. 건너뛴 항목의 기능만 빠지고 앱은 계속 동작하며, 상태 표시줄에 작게 표시한다.
- **사용자 정의 필수 도구 [사용자 결정]**: 점검 항목은 `config.json`의 `envRequire`로 추가할 수 있다(`{ "id", "check": "<명령>", "fix": "<설치·조치 안내>", "why", "required": true|false }`, check 명령이 종료 코드 0이면 정상, 타임아웃 10초). 기본 설정에 **holla**(`@circlesac/holla`, handover의 슬랙 연동)를 넣는다: check `holla slack version`(설치 + 슬랙 인증 확인), `required: true`. **required 항목은 건너뛸 수 없다** — `env_skip`이 거부하고, dispatcher는 해결될 때까지 안내를 반복하며 상태 표시줄에 강조 표시한다(회사 업무에 슬랙 연동이 필수).
- 각 상시 에이전트의 시작 프롬프트: "handover 인덱스(경로)를 읽고 내 작업 계획·진행 내역을 리뷰해 진행 준비하라" + 역할별 추가 지시. handover의 시작 프롬프트에는 `/loop <간격> <확인 작업>`이 들어간다(사용자가 템플릿에서 관리).
- 앱이 강제 종료·재부팅된 경우에도 같은 시작 절차(문서 기반)로 이어간다.
- **재부팅 후 팀 자동 재채용 [사용자 결정]**: 재시작 때 팀 창은 띄우지 않는다. 각 Manager는 시작 리뷰 중 `teams/*/brief.md`·`roster.md`에 진행 중인 팀이 있으면 **자동으로 팀 리드를 다시 채용**하고, 팀 리드는 roster대로 팀원을 다시 채용해 brief와 산출물을 보고 이어서 일한다. 완료된 팀은 Manager가 brief에 `상태: 완료`를 적어 재채용 대상에서 뺀다.
- **1단계식 탭(role 없음)** 은 1단계 방식 그대로 `workspace.json`에서 복원한다. role 탭은 복원 시 레이아웃·탭 위치만 복원하고 프로세스는 위 시작 절차(새 세션 + 시작 프롬프트)로 띄우며 팀 영역은 비운다 **[설계자 판단]**.

## 10. 설정 추가

```json
{
  "tabs": [
    { "name": "dispatcher", "role": "dispatcher", "template": "dispatcher-inbox" },
    { "name": "handover",   "role": "handover",   "dir": "",         "template": "handover" },
    { "name": "billing",    "role": "manager",    "dir": "billing",  "template": "manager" }
  ],
  "nightlyRefresh": "03:00",
  "nightlyTimeoutMin": 20,
  "handoverIndex": "HANDOVER.md",
  "roles": {
    "dispatcher": { "systemPromptFile": "roles/dispatcher.md" },
    "handover":   { "systemPromptFile": "roles/handover.md" },
    "manager":    { "systemPromptFile": "roles/manager.md" },
    "lead":       { "systemPromptFile": "roles/lead.md" },
    "member":     { "systemPromptFile": "roles/member.md" }
  }
}
```
- 템플릿의 팀 영역은 1단계 `slot`을 쓴다(`"slot": "team"`). 팀 리드·팀원 창은 그 slot에 추가된다.
- 역할 지시 파일 내용(경로 규칙, 보고 형식 "확인할 것/리스크", MCP 도구 사용법, 야간 정리 절차)은 2단계에서 **동작에 필요한 최소판**을 기본 제공하고, 사용자가 다듬는다 **[설계자 판단 — 1단계 계획의 3단계를 2단계로 흡수]**.

## 11. 빌더에게 맡기는 것

- MCP 서버 구현 방식(HTTP 라이브러리, SSE/streamable HTTP), 토큰 발급·전달 방식의 세부.
- 인박스 저장 포맷의 세부 필드, 애플릿 화면의 세부 디자인(1단계 스타일 유지).
- 팀 영역 분할 방향·크기 기본값.
- 이름 충돌·재채용 시 번호 매기기 세부.

## 12. 위험 요소와 초기 스파이크

| # | 확인 | 실패 시 |
|---|---|---|
| P1 | 앱이 SessionStart hook으로 받은 소켓·토큰으로 Windows named pipe에 메시지를 써서 세션에 전달되는지(own-child 인증) | §3 대안: idle + 빈 입력란 확인 후 PTY 주입 |
| P2 | Claude Code v2.1.234+ Windows에서 `--name` 세션끼리 `SendMessage` 전달, auto 모드에서 hold 없이 도착 | `crossSessionInbound` 설정 조정, 그래도 안 되면 P1 경로로 앱이 중계 |
| P3 | `--mcp-config` HTTP + Authorization 헤더로 에이전트별 신원 식별 | URL 경로에 토큰 포함 |
| P4 | 인박스 애플릿 창이 1단계 키 라우터·포커스·드래그와 충돌 없이 공존 | 애플릿을 별도 오버레이로 |

## 13. 테스트

- Rust 단위: 역할별 도구 권한 검사, 인박스 상태 전이(종료 제안↔진행 중, 사용자만 완료), 이슈 폴더 생성·id 규칙·경로 이탈 방지, 야간 정리 상태 기계(완료/시간 초과/입력 중 연기), 팀 채용·해산이 탭 slot에만 영향.
- 통합: MCP 서버에 가짜 에이전트 토큰으로 호출, 앱→세션 메시지(P1 경로는 가짜 named pipe 서버로).
- JS 단위: 애플릿 목록·상세 상태, 키 처리.
- 수동 QA(사용자): 실제 Claude 세션들로 아침 시작 → handover 전달 → 인박스 항목 → 지시 → 종료 제안 → 종료 확인, 팀 채용·해산, 야간 정리(시각을 가깝게 설정해 확인).
