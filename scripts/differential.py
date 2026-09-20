#!/usr/bin/env python3
"""Compare this crate's verdicts against independent ABNF implementations.

    python scripts/differential.py [--samples N] [--seed N] [--redefine-core]

Differential testing is the one check that does not rest on my own reading of RFC 5234. Every
other test in this repository was written by the same person who wrote the parser, against the
same understanding of the spec; a misreading would be invisible to all of them, and consistently
so. Another implementation, written by someone else from the same RFC, has different blind spots.

Three directions are compared, per implementation, on every fixture it will load:

  we generate   -> they accept   a disagreement means one of us generates or accepts wrongly
  they generate -> we accept     the same question from the other side; only some can generate
  corpus        -> both decide   inputs whose correct verdict is already known

Findings are printed as they are found and summarised at the end. A disagreement is a finding
whichever way it goes, including one where this crate is the more permissive: being different
from every other tool is a defect in an oracle even when it is defensible.

Setup:
    cargo build --release --features cli
    python -m venv .venv && .venv/Scripts/pip install abnf     # python-abnf
    cd scripts/goharness && go build -o goharness.exe .        # go-abnf
"""

from __future__ import annotations

import argparse
import os
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
GRAMMARS = ROOT / "tests" / "grammars"
CORPUS = ROOT / "tests" / "corpus"

# Grammars that say nothing useful when generated from in bulk, because every rule produces one
# character. Skipped for speed, not for correctness.
TRIVIAL = {"rfc5234-core.abnf"}

# Disagreements that have been investigated and attributed, so a run stays green on what is
# already understood while any *new* disagreement still fails it. Keyed by (implementation,
# grammar, rule); the value is the explanation, and `scripts/DIFFERENTIAL.md` carries the
# evidence. Nothing goes in here that has not been reduced to a minimal case and checked
# against a third implementation.
ATTRIBUTED = {
    ("go-abnf", "rfc7405-case-sensitivity.abnf", "both"): (
        "go-abnf v0.5.1 treats an alternation as case-sensitive throughout when its FIRST "
        "branch carries %s. `%s\"x\" / %i\"x\"` rejects \"X\"; reversing the branches to "
        "`%i\"x\" / %s\"x\"` accepts it. RFC 7405 makes %i and unmarked strings "
        "case-insensitive regardless of what precedes them, and python-abnf agrees with this "
        "crate on all four orderings, so the bug is go-abnf's."
    ),
}


def executable(path: pathlib.Path) -> pathlib.Path:
    return path.with_suffix(".exe") if os.name == "nt" else path


def oracle(*args: str) -> subprocess.CompletedProcess[str]:
    """Runs this crate's CLI."""
    binary = executable(ROOT / "target" / "release" / "abnf-oracle")
    if not binary.exists():
        sys.exit(f"{binary} not found.\nBuild it:  cargo build --release --features cli")
    return subprocess.run([str(binary), *args], capture_output=True, text=True, encoding="utf-8")


def read_exact(path: pathlib.Path) -> str:
    """Reads a file without translating line endings.

    `newline=""` matters on Windows: without it Python turns CRLF into LF on read, which would
    make every rule ending in `CRLF` look like a disagreement. That bug produced about forty
    phantom findings on RFC 5322 before it was spotted.
    """
    return path.read_text(encoding="utf-8", newline="")


def crlf(text: str) -> str:
    return text.replace("\r\n", "\n").replace("\n", "\r\n")


class Implementation:
    """One ABNF implementation to compare against."""

    name = "?"
    version: str | None = None
    available = False
    can_generate = False

    def loads(self, path: pathlib.Path) -> tuple[bool, str]:
        """Whether it will load this grammar, and why not if it will not."""
        raise NotImplementedError

    def accepts(self, path: pathlib.Path, rule: str, text: str) -> bool | None:
        """Its verdict, or `None` if it could not produce one."""
        raise NotImplementedError

    def generate(self, path: pathlib.Path, rule: str, seed: int, count: int) -> list[str]:
        return []

    def accepts_all(
        self, path: pathlib.Path, rule: str, texts: list[str]
    ) -> list[bool | None]:
        """Verdicts for many inputs.

        The default is one call per input. Implementations that pay a large fixed cost per
        grammar override it -- parsing RFC 5322 dominates everything else, so deciding its
        inputs one process at a time made a run take minutes rather than seconds.
        """
        return [self.accepts(path, rule, text) for text in texts]


class PythonAbnf(Implementation):
    """python-abnf: a recursive-descent parser. Cannot generate."""

    name = "python-abnf"

    def __init__(self) -> None:
        self._loaded: dict[pathlib.Path, object] = {}
        try:
            import abnf

            self.available = True
            self.version = abnf.__version__
        except ImportError:
            pass

    def _grammar(self, path: pathlib.Path):
        if path not in self._loaded:
            from abnf import Rule

            namespace = type("G_" + path.stem.replace("-", "_"), (Rule,), {})
            namespace.load_grammar(crlf(read_exact(path)))
            self._loaded[path] = namespace
        return self._loaded[path]

    def loads(self, path: pathlib.Path) -> tuple[bool, str]:
        try:
            self._grammar(path)
            return True, ""
        except Exception as error:
            return False, f"{type(error).__name__}: {error}"

    def accepts(self, path: pathlib.Path, rule: str, text: str) -> bool | None:
        from abnf import ParseError

        try:
            grammar = self._grammar(path)
        except Exception:
            return None
        try:
            grammar(rule).parse_all(text)
            return True
        except ParseError:
            return False
        except Exception:
            # A crash or a recursion limit is not a verdict.
            return None


class GoAbnf(Implementation):
    """go-abnf, through `scripts/goharness`. A GLL parser that also generates."""

    name = "go-abnf"
    can_generate = True

    def __init__(self, redefine_core: bool) -> None:
        self.binary = executable(ROOT / "scripts" / "goharness" / "goharness")
        self.available = self.binary.exists()
        self.redefine_core = redefine_core
        if self.available:
            self.version = self._module_version()

    def _module_version(self) -> str:
        for line in read_exact(ROOT / "scripts" / "goharness" / "go.mod").splitlines():
            if "pandatix/go-abnf" in line:
                return line.split()[-1]
        return "unknown"

    def _run(self, *args: str) -> subprocess.CompletedProcess[str]:
        environment = dict(os.environ)
        if self.redefine_core:
            environment["GOHARNESS_REDEFINE_CORE"] = "1"
        return subprocess.run(
            [str(self.binary), *args],
            capture_output=True,
            text=True,
            encoding="utf-8",
            env=environment,
        )

    def loads(self, path: pathlib.Path) -> tuple[bool, str]:
        result = self._run("load", str(path))
        return result.returncode == 0, result.stderr.strip()

    def accepts(self, path: pathlib.Path, rule: str, text: str) -> bool | None:
        with tempfile.TemporaryDirectory() as directory:
            target = pathlib.Path(directory) / "input"
            target.write_text(text, encoding="utf-8", newline="")
            result = self._run("match", str(path), rule, str(target))
        return {0: True, 1: False}.get(result.returncode)

    def generate(self, path: pathlib.Path, rule: str, seed: int, count: int) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            result = self._run("gen", str(path), rule, str(seed), str(count), directory)
            if result.returncode != 0:
                return []
            return [read_exact(p) for p in sorted(pathlib.Path(directory).glob("*.txt"))]

    def accepts_all(
        self, path: pathlib.Path, rule: str, texts: list[str]
    ) -> list[bool | None]:
        if not texts:
            return []
        with tempfile.TemporaryDirectory() as directory:
            for index, text in enumerate(texts):
                target = pathlib.Path(directory) / f"{index:06d}"
                target.write_text(text, encoding="utf-8", newline="")
            result = self._run("matchdir", str(path), rule, directory)
        if result.returncode != 0:
            return [None] * len(texts)

        by_name: dict[str, bool | None] = {}
        for line in result.stdout.splitlines():
            name, _, word = line.partition("	")
            by_name[name] = {"ACCEPT": True, "REJECT": False}.get(word)
        return [by_name.get(f"{index:06d}") for index in range(len(texts))]


class Report:
    def __init__(self) -> None:
        self.compared = 0
        self.agreed = 0
        self.findings: list[str] = []
        self.notes: list[str] = []
        self.known: list[str] = []

    def finding(self, message: str) -> None:
        self.findings.append(message)
        print(f"  DISAGREEMENT {message}")

    def note(self, message: str) -> None:
        self.notes.append(message)
        print(f"  note: {message}")

    def attributed(self, message: str) -> None:
        """A disagreement already investigated and blamed; reported once, does not fail."""
        if message not in self.known:
            self.known.append(message)
            print(f"  KNOWN {message}")

    def verdict(self, agreed: bool) -> None:
        self.compared += 1
        self.agreed += int(agreed)


def our_generation(path: pathlib.Path, rule: str, count: int, seed: int) -> list[str]:
    """Generates with this crate, one file per string so line endings survive."""
    with tempfile.TemporaryDirectory() as directory:
        result = oracle(
            "gen", str(path), "--rule", rule, "--count", str(count),
            "--seed", str(seed), "--coverage", "--out", directory,
        )
        if result.returncode != 0:
            return []
        return [read_exact(p) for p in sorted(pathlib.Path(directory).glob("*.txt"))]


def our_verdicts(path: pathlib.Path, rule: str, texts: list[str]) -> list[bool | None]:
    """Our verdicts for many inputs, in one call.

    `match --dir` exists for exactly this. One process per input made a run take minutes: the
    cost is in parsing the grammar, and doing it once per string wastes nearly all of it.
    """
    if not texts:
        return []
    with tempfile.TemporaryDirectory() as directory:
        for index, text in enumerate(texts):
            target = pathlib.Path(directory) / f"{index:06d}"
            target.write_text(text, encoding="utf-8", newline="")
        result = oracle(
            "match", str(path), "--rule", rule, "--dir", directory, "--max-depth", "4096"
        )

    by_name: dict[str, bool | None] = {}
    for line in result.stdout.splitlines():
        parts = line.split()
        if len(parts) >= 2:
            by_name[parts[0]] = {"ACCEPT": True, "REJECT": False}.get(parts[1])
    return [by_name.get(f"{index:06d}") for index in range(len(texts))]


def sample_rules(rules: list[str], limit: int) -> list[str]:
    """At most `limit` rules, spread evenly across the grammar.

    RFC 5322 has 133 generatable rules and RFC 9110 has 117; comparing every one against a
    slower implementation costs minutes for little extra signal, since neighbouring rules
    exercise the same constructs. Evenly spaced rather than the first N, so the sample reaches
    the obsolete-syntax rules at the end of RFC 5322 as well as the current ones at the front.
    """
    if limit <= 0 or len(rules) <= limit:
        return rules
    step = len(rules) / limit
    return [rules[int(index * step)] for index in range(limit)]


def usable_rules(path: pathlib.Path) -> list[str]:
    """The rules this crate can generate from, as `rules` reports them."""
    listed = oracle("rules", str(path))
    if listed.returncode != 0:
        return []
    found = []
    for line in listed.stdout.splitlines()[1:]:
        parts = line.split()
        if parts and "not a usable start rule" not in line and "matches nothing" not in line:
            found.append(parts[0])
    return found


def compare(
    other: Implementation, report: Report, samples: int, seed: int, rule_limit: int
) -> None:
    print(f"\n=== {other.name} {other.version}: generated strings ===")

    for path in sorted(GRAMMARS.glob("*.abnf")):
        loaded, why = other.loads(path)
        if not loaded:
            report.note(f"{path.name}: {other.name} will not load it -- {why}")
            continue
        if path.name in TRIVIAL:
            continue

        ours_ok = theirs_ok = undecided = 0
        for rule in sample_rules(usable_rules(path), rule_limit):
            # Direction 1: we generate, they decide.
            ours = our_generation(path, rule, samples, seed)
            for text, verdict in zip(ours, other.accepts_all(path, rule, ours)):
                if verdict is None:
                    undecided += 1
                    continue
                known = ATTRIBUTED.get((other.name, path.name, rule))
                report.verdict(verdict or known is not None)
                if verdict:
                    ours_ok += 1
                elif known:
                    report.attributed(f"{other.name} {path.name} {rule}: {known}")
                else:
                    report.finding(
                        f"{path.name} {rule}: we generated {text!r}, {other.name} rejects it"
                    )

            # Direction 2: they generate, we decide. Only some implementations can.
            if other.can_generate:
                theirs = other.generate(path, rule, seed, samples)
                for text, verdict in zip(theirs, our_verdicts(path, rule, theirs)):
                    if verdict is None:
                        undecided += 1
                        continue
                    report.verdict(verdict)
                    if verdict:
                        theirs_ok += 1
                    else:
                        report.finding(
                            f"{path.name} {rule}: {other.name} generated {text!r}, we reject it"
                        )

        summary = f"{ours_ok} of ours accepted"
        if other.can_generate:
            summary += f", {theirs_ok} of theirs accepted"
        if undecided:
            # Printed rather than swallowed. A batch that silently returns nothing looks
            # identical to perfect agreement in the totals, which is how a broken harness hides.
            summary += f", {undecided} undecided"
        print(f"  {path.name:<34} {summary}")


def compare_corpus(other: Implementation, report: Report) -> None:
    """Both implementations decide inputs whose correct verdict is already known."""
    print(f"\n=== {other.name}: corpus ===")

    for corpus in sorted(p for p in CORPUS.iterdir() if p.is_dir()):
        fixture = next(GRAMMARS.glob(f"{corpus.name}-*.abnf"), None)
        if fixture is None:
            continue
        loaded, why = other.loads(fixture)
        if not loaded:
            report.note(f"{corpus.name}: {other.name} will not load {fixture.name} -- {why}")
            continue

        rule = usable_rules(fixture)[0]
        agreed = undecided = 0
        for bucket, expected in (("accept", True), ("reject", False)):
            directory = corpus / bucket
            if not directory.is_dir():
                continue

            names, texts = [], []
            for path in sorted(directory.iterdir()):
                if not path.is_file() or path.suffix == ".md":
                    continue
                try:
                    texts.append(read_exact(path))
                except UnicodeDecodeError:
                    continue  # neither implementation sees this as text
                names.append(path.name)

            for name, verdict in zip(names, other.accepts_all(fixture, rule, texts)):
                if verdict is None:
                    undecided += 1
                    continue
                report.verdict(verdict == expected)
                if verdict == expected:
                    agreed += 1
                else:
                    report.finding(
                        f"{corpus.name}/{bucket}/{name}: "
                        f"we say {expected}, {other.name} says {verdict}"
                    )
        print(f"  {corpus.name:<34} {agreed} agreed, {undecided} it could not decide")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=3, help="strings per rule per direction")
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--no-corpus", action="store_true", help="skip the corpus comparison")
    parser.add_argument(
        "--max-rules", type=int, default=20,
        help="rules sampled per grammar, evenly spread; 0 for all",
    )
    parser.add_argument(
        "--redefine-core",
        action="store_true",
        help="let go-abnf load grammars that shadow a core rule (see scripts/DIFFERENTIAL.md)",
    )
    args = parser.parse_args()

    implementations = [PythonAbnf(), GoAbnf(args.redefine_core)]
    print("abnf-oracle differential run")
    for implementation in implementations:
        state = implementation.version if implementation.available else "NOT AVAILABLE"
        print(f"  {implementation.name:<14} {state}")

    present = [i for i in implementations if i.available]
    if not present:
        print("\nNothing to compare against; see the module docstring for setup.")
        return 0

    report = Report()
    for implementation in present:
        compare(implementation, report, args.samples, args.seed, args.max_rules)
        if not args.no_corpus:
            compare_corpus(implementation, report)

    print("\n=== summary ===")
    print(f"  {report.agreed}/{report.compared} verdicts agreed")
    print(
        f"  {len(report.findings)} new disagreements, "
        f"{len(report.known)} known, {len(report.notes)} notes"
    )
    if report.findings:
        print("\nDisagreements must be understood before being waived (SCOPE.md 8, M4):")
        for finding in report.findings:
            print(f"  - {finding}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
