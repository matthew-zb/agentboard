# 스파이크 S1–S3 (Task 3)

상태: 빌드·실행 확인만 자동으로 끝냈다. 아래 수동 항목은 모두 **사용자 확인 필요**이며, 결과 칸은 비어 있다(관찰하지 않은 결과는 적지 않는다).

자동 확인 결과: `cargo build`·`cargo test`(15 통과)·`npm run test:js`(통과) 정상. `npx tauri dev`로 `agentboard.exe`가 기동되고 콘솔에 `webview settings failed`/`with_webview failed`/패닉 로그 없음(= `SetAreBrowserAcceleratorKeysEnabled(false)` 호출이 오류 없이 반환됨).

준비: `npx tauri dev` 실행 → 창이 뜨면 `p-test` 창(PowerShell)이 보인다.

## 기본 동작 확인 (Step 2)
- [ ] 사용자 확인 필요 — PowerShell 프롬프트에 입력이 된다.
- [ ] 사용자 확인 필요 — `dir` 출력이 어긋나지 않는다.
- [ ] 사용자 확인 필요 — `echo 한글테스트`가 올바른 폭(글자 겹침/빈칸 없음)으로 표시된다.
- [ ] 사용자 확인 필요 — 창 크기를 바꾸면 프롬프트 줄바꿈 폭이 따라간다(`$Host.UI.RawUI.WindowSize`로 열·행 확인).

## S3 — ConPTY + xterm.js에서 Claude Code TUI
같은 창에서 `claude` 실행 후 확인.
- [ ] 사용자 확인 필요 — 색, 커서 위치, 상자 문자(┌─┐│└┘)가 깨지지 않는다.
- [ ] 사용자 확인 필요 — 창 리사이즈 시 TUI가 새 크기로 다시 그려진다.
- [ ] 사용자 확인 필요 — 한글 입력(IME 조합 중 글자가 커서 위치에 보이고 확정 시 입력됨).
- [ ] 사용자 확인 필요 — `Ctrl+V`로 이미지 붙여넣기 시 이미지 칩이 나타난다(클립보드에 이미지 복사 후).

기대 결과: 위 4개 모두 통과.
실패 시 대안(스펙 §10 S3): `pane.js`의 unicode11 addon 설정 조정, ConPTY 플래그 조정.
결과: (미기록)

## S2 — WebView2 브라우저 단축키 비활성화
`src-tauri/src/webview.rs`가 적용된 상태에서 `claude` 창에서 확인.
- [ ] 사용자 확인 필요 — `Ctrl+R`: 페이지 새로고침이 아니라 Claude 기록 검색이 열린다.
- [ ] 사용자 확인 필요 — `Ctrl+P`: 인쇄 대화가 뜨지 않는다.
- [ ] 사용자 확인 필요 — `F5`: 새로고침되지 않는다(화면·세션 유지).
- [ ] 사용자 확인 필요 — 우클릭 시 기본 컨텍스트 메뉴가 뜨지 않는다.

기대 결과: 위 항목 모두 통과.
실패 시 대안(스펙 §10 S2): document keydown capture에서 해당 키를 `preventDefault` 한 뒤 PTY로 직접 전송(`api.write`).
결과: (미기록)

## S1 — Alt 키 처리
`src/app.js`에 임시 프로브가 있다: 문서 keydown capture에서 `Alt+H`를 `preventDefault`(`// TEMP(Task 9)`).
- [ ] 사용자 확인 필요 — `Alt`만 눌렀다 뗐을 때 xterm 포커스가 유지된다(이후 바로 타이핑됨).
- [ ] 사용자 확인 필요 — `Alt`만 눌렀다 뗐을 때 경고음이 나지 않는다.
- [ ] 사용자 확인 필요 — `Alt+H`(프로브 적용 상태)에서 경고음이 나지 않는다.
- [ ] 사용자 확인 필요 — 그 외 `Alt+글자`(예: `Alt+J`)에서 경고음 여부 기록.

기대 결과: 포커스 유지, 경고음 없음.
실패 시 대안(스펙 §10 S1, 순서대로): (1) keyup의 Alt에 `preventDefault` 적용, (2) 그래도 안 되면 Rust 서브클래싱으로 `WM_SYSKEYDOWN/WM_SYSCHAR` 처리.
적용한 대안: (미기록)
