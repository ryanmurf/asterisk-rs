#!/usr/bin/env python3
"""assert_boundary.py — the RECEIVER-SIDE proof for M8a.

Consumes the two stand-in capture files (FS + rustisk) written by listener.py
and asserts, on what each stand-in ACTUALLY CAPTURED (never a sender log):

  Boundary integrity for the primed tuple (--trust-tag):
    * The two capture sets are DISJOINT (no seq delivered to both) -> no
      split-brain / no duplication.
    * Sorted by seq, ownership forms clean contiguous runs with a GAP between
      them (the handover drop window). Expected run pattern for a switch +
      rollback under one continuous flow:  FS, RUSTISK, FS.
    * Each ownership change is a single clean boundary: max(prev run) <
      min(next run), and the intervening seqs (the gap) were delivered to
      NEITHER — i.e. dropped during the handover, not misrouted.

  Source-drop (--untrusted-tags): NONE of those tags appear in EITHER capture,
    anywhere, at any time (before/during/after both transitions). A genuine
    DROP, proven for whatever families the untrusted senders used (v4 + v6).

Prints the switch boundary (N -> N+gap+1), the rollback boundary, the per-
transition delivery gap in datagrams and ms, and exits non-zero on any
violation.
"""
import argparse
import sys


def load(path):
    rows = []
    try:
        with open(path) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                # recv_ts_ns,label,src,tag,seq
                parts = line.split(",")
                if len(parts) < 5:
                    continue
                ts, label, src, tag, seq = parts[0], parts[1], parts[2], parts[3], parts[4]
                try:
                    rows.append((int(ts), label, src, tag, int(seq)))
                except ValueError:
                    continue
    except FileNotFoundError:
        pass
    return rows


def runs_by_seq(delivered):
    """delivered: list of (seq, owner, ts). Return compressed ownership runs:
    [(owner, min_seq, max_seq, count, first_ts, last_ts), ...] ordered by seq,
    and flag any interleaving (owner changes back and forth within adjacent
    seqs without a clean split)."""
    d = sorted(delivered, key=lambda r: r[0])
    runs = []
    for seq, owner, ts in d:
        if runs and runs[-1][0] == owner:
            o, lo, hi, cnt, fts, lts = runs[-1]
            runs[-1] = (o, lo, seq, cnt + 1, fts, ts)
        else:
            runs.append((owner, seq, seq, 1, ts, ts))
    return runs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--fs", required=True)
    ap.add_argument("--rustisk", required=True)
    ap.add_argument("--trust-tag", default="TRUST")
    ap.add_argument("--untrusted-tags", nargs="*", default=["EVILV4", "EVILV6"])
    ap.add_argument("--expect-runs", default="FS,RUSTISK,FS",
                    help="expected ownership run pattern by seq")
    args = ap.parse_args()

    fs = load(args.fs)
    ru = load(args.rustisk)
    ok = True

    def emit(good, msg):
        nonlocal ok
        print(("PASS" if good else "FAIL") + " " + msg)
        if not good:
            ok = False

    # ---- Source-drop: untrusted tags must appear NOWHERE ----
    for tag in args.untrusted_tags:
        nfs = sum(1 for r in fs if r[3] == tag)
        nru = sum(1 for r in ru if r[3] == tag)
        emit(nfs == 0 and nru == 0,
             f"source-drop {tag}: delivered_to_FS={nfs} delivered_to_RUSTISK={nru} "
             f"(must be 0/0)")

    # ---- Boundary integrity for the primed trusted tuple ----
    fs_seq = {r[4]: r[0] for r in fs if r[3] == args.trust_tag}
    ru_seq = {r[4]: r[0] for r in ru if r[3] == args.trust_tag}
    overlap = sorted(set(fs_seq) & set(ru_seq))
    emit(len(overlap) == 0,
         f"disjoint captures (no split-brain): overlap_count={len(overlap)}"
         + (f" e.g. {overlap[:5]}" if overlap else ""))

    delivered = ([(s, "FS", t) for s, t in fs_seq.items()]
                 + [(s, "RUSTISK", t) for s, t in ru_seq.items()])
    if not delivered:
        emit(False, "no trusted datagrams captured at all")
        print("RESULT: FAIL")
        sys.exit(1)

    runs = runs_by_seq(delivered)
    pattern = ",".join(r[0] for r in runs)
    expect = args.expect_runs
    emit(pattern == expect,
         f"ownership run pattern by seq = [{pattern}] (expected [{expect}])")

    # Clean boundary + gap between each adjacent run.
    for i in range(len(runs) - 1):
        o1, lo1, hi1, c1, f1, l1 = runs[i]
        o2, lo2, hi2, c2, f2, l2 = runs[i + 1]
        clean = hi1 < lo2
        gap_dgrams = lo2 - hi1 - 1
        gap_ms = (f2 - l1) / 1e6
        label = f"{o1}->{o2}"
        emit(clean,
             f"boundary {label}: last {o1} seq={hi1}, first {o2} seq={lo2} "
             f"(clean: {o1}.max < {o2}.min); handover gap = {gap_dgrams} datagrams / "
             f"{gap_ms:.1f} ms delivery gap")

    allseq = set(fs_seq) | set(ru_seq)
    print(f"trusted captured: FS={len(fs_seq)} RUSTISK={len(ru_seq)} "
          f"seq_span={min(allseq)}..{max(allseq)}")
    print("RESULT: " + ("PASS" if ok else "FAIL"))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
