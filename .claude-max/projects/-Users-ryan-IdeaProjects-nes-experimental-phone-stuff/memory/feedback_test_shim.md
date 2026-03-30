---
name: test pjsip shim after build
description: User wants an agent to test the pjsip-shim C API library as soon as it's built — compile C tests against it, run pjproject's own tests
type: feedback
---

When the pjsip-shim agent completes, immediately launch a test agent that:
1. Builds libpjsip_rs.dylib
2. Writes and compiles C test programs against it
3. Extracts pjproject source and tries to compile/run pjproject's own tests against our .so
4. Reports pass/fail

**Why:** User wants to verify binary compatibility with real C consumers.

**How to apply:** Watch for pjsip-shim agent completion, then launch test agent.
