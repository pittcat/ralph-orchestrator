## Command Hang Prevention Rules

1. Never run infinite-follow commands directly.
   Forbidden examples:
   - tail -f
   - tail -F
   - journalctl -f
   - adb logcat
   - dmesg -w
   - watch
   - while true

2. If follow mode is necessary, always wrap it with timeout:
   - timeout 30s tail -f <file>
   - timeout 60s adb logcat
   - timeout 30s journalctl -f

3. Prefer bounded commands:
   - tail -n 200 <file>
   - grep -n "ERROR" <file> | head -100
   - journalctl -n 300 --no-pager
   - dmesg | tail -200

4. For large files, never cat the whole file.
   Use:
   - wc -l <file>
   - tail -n 200 <file>
   - head -n 100 <file>
   - grep -n "keyword" <file> | head -50

5. Every external command that may block must have timeout.
