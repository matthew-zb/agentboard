"""Interactive Claude Code session in a ConPTY (pywinpty), like AgentBoard's panes.

python pty_interactive.py <name> <settings.json> <hookDir> <outDir> <cwd> [extra claude args]

Scenario:
 1. spawn `claude --model haiku --name <name> --settings <settings>` with a clean env
 2. wait for the SessionStart hook dump, inject a message into the inbox pipe (inject.mjs)
 3. wait for Stop, then type a prompt that needs a Write permission -> expect Notification(permission_prompt)
 4. press Esc to deny, wait ~70 s idle -> expect Notification(idle_prompt)
 5. terminate. Raw PTY output goes to <outDir>/pty.log.
"""
import os, sys, time, glob, threading, subprocess, json
from winpty import PtyProcess

name, settings, hook_dir, out_dir, cwd = sys.argv[1:6]
extra = ' '.join(sys.argv[6:])  # e.g. --permission-mode default
os.makedirs(out_dir, exist_ok=True)
here = os.path.dirname(os.path.abspath(__file__))
env = {k: v for k, v in os.environ.items() if not (k.startswith('CLAUDE') or k == 'ANTHROPIC_BASE_URL')}
log = open(os.path.join(out_dir, 'pty.log'), 'w', encoding='utf-8', errors='replace')
steps = open(os.path.join(out_dir, 'steps.log'), 'w', encoding='utf-8')


def step(msg):
    line = f"[{time.strftime('%H:%M:%S')}] {msg}"
    print(line, flush=True)
    steps.write(line + '\n'); steps.flush()


cmd = f'claude --model haiku --name {name} --settings {settings} {extra}'  # pywinpty keeps quotes literally; paths must not contain spaces
p = PtyProcess.spawn(cmd, cwd=cwd, env=env, dimensions=(40, 140))
step(f'spawned pid={p.pid}: {cmd}')


def pump():
    while True:
        try:
            data = p.read(4096)
        except Exception:
            break
        log.write(data); log.flush()


threading.Thread(target=pump, daemon=True).start()


def wait_for(pattern, timeout, pred=None):
    end = time.time() + timeout
    while time.time() < end:
        for f in sorted(glob.glob(os.path.join(hook_dir, pattern))):
            if pred is None or pred(json.load(open(f, encoding='utf-8'))):
                return f
        time.sleep(0.5)
    return None


try:
    ss = wait_for('SessionStart-*.json', 60)
    step(f'SessionStart hook: {ss}')
    if not ss:
        raise SystemExit('no SessionStart')
    time.sleep(8)
    r = subprocess.run(['node', os.path.join(here, 'inject.mjs'), ss,
                        'pty-pipe-msg-444: reply with exactly GOT pty-pipe-msg-444'], capture_output=True, text=True)
    step('inject: ' + (r.stdout + r.stderr).strip())
    st = wait_for('Stop-*.json', 60)
    step(f'Stop after inject: {st}')
    if st:
        step('last_assistant_message: ' + json.load(open(st, encoding='utf-8'))['stdin'].get('last_assistant_message', '')[:200])
    time.sleep(2)
    p.write('Create a file named abspike-should-not-exist.txt containing hi, using the Write tool.')
    time.sleep(1)
    p.write('\r')
    step('typed permission-needing prompt')
    n = wait_for('Notification-*.json', 60, lambda d: d['stdin'].get('notification_type') == 'permission_prompt')
    step(f'permission Notification: {n}')
    p.write('\x1b')
    step('sent Esc')
    n2 = wait_for('Notification-*.json', 110, lambda d: d['stdin'].get('notification_type') == 'idle_prompt')
    step(f'idle Notification: {n2}')
finally:
    try:
        subprocess.run(['taskkill', '/PID', str(p.pid), '/T', '/F'], capture_output=True)
    except Exception:
        pass
    try:
        p.terminate(force=True)
    except Exception:
        pass
    step('terminated')
