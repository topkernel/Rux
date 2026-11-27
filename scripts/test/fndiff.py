#!/usr/bin/env python3
"""fndiff.py — statement-level equivalence audit for unsafe-collection
rewrites. For each changed file of a commit, extract every function
body (brace-matched) from the pre and post versions and flag:

  1. big statement-count drops (post < pre * 0.75, diff >= 2 ';')
  2. lost tail actions: call-like tokens in the pre version's LAST 5
     statements that appear NOWHERE in the post body of the same
     function (the ipv4_send_src_ttl signature: the send/ip_output
     tail vanished).

Imperfect parsing is acceptable — this flags candidates for manual
review, it does not prove equivalence.
"""
import re
import subprocess
import sys

COMMIT = sys.argv[1]
REPO = sys.argv[2] if len(sys.argv) > 2 else "/home/william/rux-agents/mainline"

def files_changed(commit):
    out = subprocess.run(["git", "-C", REPO, "show", commit,
                          "--name-only", "--format="],
                         capture_output=True, text=True).stdout
    return [f for f in out.splitlines() if f.endswith(".rs")]

def get_src(commit, path):
    r = subprocess.run(["git", "-C", REPO, "show", f"{commit}:{path}"],
                       capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None

FN_RE = re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)', re.M)

def extract_fns(src):
    """Return dict name -> body text (brace-matched, comments stripped)."""
    src = re.sub(r'//[^\n]*', '', src)
    src = re.sub(r'/\*.*?\*/', '', src, flags=re.S)
    fns = {}
    for m in FN_RE.finditer(src):
        name = m.group(1)
        j = src.find('{', m.end())
        if j == -1:
            continue
        depth = 0
        k = j
        in_str = False
        while k < len(src):
            c = src[k]
            if in_str:
                if c == '\\':
                    k += 2
                    continue
                if c == '"':
                    in_str = False
            else:
                if c == '"':
                    in_str = True
                elif c == '{':
                    depth += 1
                elif c == '}':
                    depth -= 1
                    if depth == 0:
                        break
            k += 1
        body = src[j:k + 1]
        if name not in fns or len(body) > len(fns[name]):
            fns[name] = body
    return fns

def stmts(body):
    return body.count(';')

CALL_RE = re.compile(r'([A-Za-z_][A-Za-z0-9_]*)\s*[(!]')

def tail_tokens(body, k=5):
    """Call-like identifiers in the last k statements."""
    parts = [p for p in body.split(';') if p.strip()]
    tail = ';'.join(parts[-k:])
    toks = set()
    for m in CALL_RE.finditer(tail):
        t = m.group(1)
        if t not in ('if', 'while', 'for', 'match', 'return', 'let',
                     'else', 'loop', 'break', 'Some', 'None', 'Ok', 'Err',
                     'unsafe', 'as'):
            toks.add(t)
    return toks

def main():
    changed = files_changed(COMMIT)
    for path in changed:
        pre_src = get_src(f"{COMMIT}^", path)
        post_src = get_src(COMMIT, path)
        if pre_src is None or post_src is None:
            continue
        pre = extract_fns(pre_src)
        post = extract_fns(post_src)
        for name, pbody in pre.items():
            if name not in post:
                continue
            nbody = post[name]
            ps, ns = stmts(pbody), stmts(nbody)
            if ns < ps * 0.75 and (ps - ns) >= 2:
                print(f"STMT-DROP {path}::{name}: {ps} -> {ns} ';'")
            ptails = tail_tokens(pbody)
            ntails = tail_tokens(nbody)
            lost = ptails - ntails
            if lost:
                absent = {t for t in lost if not re.search(r'\b' + re.escape(t) + r'\b', nbody)}
                if absent:
                    print(f"TAIL-LOST {path}::{name}: absent-from-post {sorted(absent)}")

if __name__ == "__main__":
    main()
