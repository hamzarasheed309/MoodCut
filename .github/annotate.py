#!/usr/bin/env python3
"""Turns `cargo test` output into GitHub annotations: compiler errors and failing tests show up on
the run's summary page (and through the public check-run annotations API) without opening the log.

    cargo test --release 2>&1 | tee test.log; python3 .github/annotate.py test.log

GitHub keeps at most 10 error annotations per step, so blocks are packed several to an annotation.
"""
import re
import sys

MAX_ANNOTATIONS = 10
MAX_CHARS = 3800


def esc(s: str) -> str:
    return s.replace("%", "%25").replace("\r", "").replace("\n", "%0A")


def compiler_errors(lines):
    """`error[E0308]: ...` blocks up to the next blank line, with their first ` --> file:line:col`."""
    out = []
    i = 0
    while i < len(lines):
        if re.match(r"^error(\[E\d+\])?: ", lines[i]) and not lines[i].startswith("error: could not compile") and not lines[i].startswith("error: test failed"):
            block = [lines[i]]
            j = i + 1
            while j < len(lines) and lines[j].strip() != "" and not re.match(r"^(error|warning)(\[\w+\])?: ", lines[j]):
                block.append(lines[j])
                j += 1
            loc = next((m for m in (re.search(r"--> ([^:]+):(\d+):(\d+)", b) for b in block) if m), None)
            out.append((loc.group(1).strip() if loc else None, int(loc.group(2)) if loc else None, "\n".join(block[:40])))
            i = j
        else:
            i += 1
    return out


def test_failures(lines):
    """`---- name stdout ----` blocks (the panic message and location)."""
    out = []
    i = 0
    while i < len(lines):
        m = re.match(r"^---- (\S+) stdout ----$", lines[i])
        if m:
            block = [f"test {m.group(1)} failed"]
            j = i + 1
            while j < len(lines) and not lines[j].startswith("---- ") and lines[j].strip() != "failures:":
                block.append(lines[j])
                j += 1
            loc = next((mm for mm in (re.search(r"panicked at ([^:]+\.rs):(\d+):\d+", b) for b in block) if mm), None)
            out.append((loc.group(1) if loc else None, int(loc.group(2)) if loc else None, "\n".join(block[:60])))
            i = j
        else:
            i += 1
    return out


def main():
    text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
    lines = text.splitlines()
    items = compiler_errors(lines) or test_failures(lines)
    if not items:
        tail = "\n".join(lines[-40:])
        print(f"::error title=cargo test failed::{esc(tail)}")
        return
    kind = "compile error" if compiler_errors(lines) else "test failure"
    # one annotation per block while there is room, then packed
    per = max(1, -(-len(items) // MAX_ANNOTATIONS))
    for k in range(0, len(items), per):
        group = items[k : k + per]
        f, ln, _ = group[0]
        body = "\n\n".join(b for _, _, b in group)[:MAX_CHARS]
        prop = lambda v: esc(str(v)).replace(":", "%3A").replace(",", "%2C")
        where = f"file={prop(f)},line={ln}," if f and ln else ""
        print(f"::error {where}title={prop(f'{len(items)} {kind}s #{k + 1}-{k + len(group)}')}::{esc(body)}")


if __name__ == "__main__":
    main()
