#!/usr/bin/env bash
# detector-selftest.sh — proves assert_boundary.py itself has TEETH: it must
# ACCEPT a clean capture and REJECT every crafted pathology (split-brain/overlap,
# interleaving, a missing rollback boundary, and an untrusted-source delivery).
# This guarantees the listener-swap PASS is meaningful, not a rigged always-pass.
set -uo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RUN="${RUN:-/m8a/run}"; D="${RUN}/selftest"; rm -rf "${D}"; mkdir -p "${D}"

gen() { python3 - "$@" <<'PY'
import sys
mode=sys.argv[1]; fs=open(sys.argv[2],"w"); ru=open(sys.argv[3],"w")
def row(f,label,tag,seq): f.write(f"{seq*1000000},{label},10.9.0.2:41002,{tag},{seq}\n")
if mode=="good":            # FS 1..100 , RU 201..300 , FS 401..500  (clean FS,RU,FS)
    for s in range(1,101):   row(fs,"FS","TRUST",s)
    for s in range(201,301): row(ru,"RUSTISK","TRUST",s)
    for s in range(401,501): row(fs,"FS","TRUST",s)
elif mode=="overlap":       # split-brain: RU shares seqs with FS
    for s in range(1,101):   row(fs,"FS","TRUST",s)
    for s in range(50,151):  row(ru,"RUSTISK","TRUST",s)
elif mode=="interleave":    # port flapping: odd->FS even->RU
    for s in range(1,201):
        (row(fs,"FS","TRUST",s) if s%2 else row(ru,"RUSTISK","TRUST",s))
elif mode=="norollback":    # switched but never returned to FS
    for s in range(1,101):   row(fs,"FS","TRUST",s)
    for s in range(201,401): row(ru,"RUSTISK","TRUST",s)
elif mode=="untrusted":     # clean boundary but an untrusted datagram slipped in
    for s in range(1,101):   row(fs,"FS","TRUST",s)
    for s in range(201,301): row(ru,"RUSTISK","TRUST",s)
    for s in range(401,501): row(fs,"FS","TRUST",s)
    row(ru,"RUSTISK","EVILV4",250)
fs.close(); ru.close()
PY
}

run_case() { # <mode> <expected: PASS|FAIL>
  local mode="$1" want="$2"
  gen "${mode}" "${D}/${mode}_fs.csv" "${D}/${mode}_ru.csv"
  python3 "${HERE}/assert_boundary.py" --fs "${D}/${mode}_fs.csv" --rustisk "${D}/${mode}_ru.csv" \
      --trust-tag TRUST --untrusted-tags EVILV4 EVILV6 --expect-runs FS,RUSTISK,FS >/dev/null 2>&1
  local rc=$?
  local got; [ "${rc}" -eq 0 ] && got=PASS || got=FAIL
  if [ "${got}" = "${want}" ]; then echo "  detector[$mode]: expected ${want}, got ${got}  OK"; return 0
  else echo "  detector[$mode]: expected ${want}, got ${got}  MISMATCH"; return 1; fi
}

echo "=================== DETECTOR SELF-TEST (assert_boundary teeth) ==================="
rc=0
run_case good       PASS || rc=1
run_case overlap    FAIL || rc=1
run_case interleave FAIL || rc=1
run_case norollback FAIL || rc=1
run_case untrusted  FAIL || rc=1
[ "${rc}" -eq 0 ] && echo "detector self-test: ALL OK (accepts clean, rejects every pathology)" \
                  || echo "detector self-test: FAILED"
exit "${rc}"
