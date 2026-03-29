---
name: v1 release plan
description: User wants a v1 git commit once all 1,079 Asterisk test suite tests pass
type: project
---

User requested a v1 commit/tag once the Asterisk test suite is fully passing.

**Why:** This is the milestone marking production readiness — all 1,079 official tests green.

**How to apply:** After all test suite agents complete and tests pass:
1. `git init` the asterisk-rs repo (it's not a git repo yet)
2. Create a comprehensive commit with the full codebase
3. Tag as `v1.0.0`
4. Include a summary of what's in v1 in the commit message
