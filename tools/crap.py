#!/usr/bin/env python3
"""Compute an approximate CRAP score for Rust functions.

CRAP = complexity^2 * (1 - coverage)^3 + complexity

Complexity comes from lizard's Rust parser. Coverage comes from a
`cargo llvm-cov --json` export. LLVM's first region for a function is its
entry region and spans the whole body; we ignore it and use the remaining
regions (branches/statements) as the coverage proxy.

Usage:
    pip install lizard
    cargo llvm-cov --all-targets --json --output-path /tmp/coverage.json
    python3 tools/crap.py /tmp/coverage.json --top 40 --output docs/crap-report.md

For a gate, exclude untestable entry/FFI modules and set budgets:

    python3 tools/crap.py /tmp/coverage.json \
      --exclude 'src/transport/*' --exclude 'src/main.rs' --exclude 'src/tui/mod.rs' \
      --max-over-30 3 --max-over-100 0 --min-lines 75
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

try:
    import lizard
except ImportError:  # pragma: no cover - tooling only
    sys.exit("lizard is required: python3 -m pip install lizard")


@dataclass
class Function:
    file: str
    name: str
    line: int
    end_line: int
    complexity: int
    coverage: float

    @property
    def crap(self) -> float:
        return self.complexity**2 * (1.0 - self.coverage) ** 3 + self.complexity


# Functions that cannot execute in unit/integration tests because they own a
# hardware, terminal or process entry point. They are listed in the report so a
# 0% row is not mistaken for a missing unit test.
GLUE_NOTES = {
    ("src/main.rs", "main"): "process entry point",
    ("src/main.rs", "list_devices"): "CLI device discovery",
    ("src/transport/ble.rs", "debug"): "opt-in BLE diagnostic output",
    ("src/transport/ble.rs", "adapter"): "BLE adapter creation",
    ("src/transport/ble.rs", "scan"): "BLE scan",
    ("src/transport/ble.rs", "connect"): "BLE/CoreBluetooth connection",
    ("src/transport/ble.rs", "establish_session"): "BLE GATT session setup",
    ("src/transport/ble.rs", "drop"): "GATT teardown hook",
    ("src/transport/ble.rs", "send"): "BLE characteristic write",
    ("src/transport/serial.rs", "list_usb_devices"): "OS serial enumeration",
    ("src/transport/serial.rs", "connect"): "OS serial open",
    ("src/transport/serial.rs", "spawn_reader"): "USB serial reader thread",
    ("src/transport/serial.rs", "spawn_writer"): "USB serial writer thread",
    ("src/transport/serial.rs", "send"): "serial channel write",
    ("src/tui/app.rs", "refresh_usb"): "device discovery side effect",
    ("src/tui/app.rs", "spawn_connect"): "connection task",
    ("src/tui/app.rs", "spawn_bluetooth_by_name"): "connection task",
    ("src/tui/app.rs", "spawn_bluetooth_scan"): "scan task",
    ("src/tui/app.rs", "handle_disconnect"): "device disconnect teardown",
    ("src/tui/connection.rs", "scan_bluetooth"): "BLE scan",
    ("src/tui/connection.rs", "connect_choice"): "hardware connection dispatch",
    ("src/tui/connection.rs", "connect_by_bluetooth_name"): "hardware connection dispatch",
    ("src/tui/mod.rs", "run"): "terminal entry point",
    ("src/tui/mod.rs", "run_loop"): "terminal event loop",
    ("src/tui/mod.rs", "spawn_input_thread"): "terminal input thread",
}


def glue_note(function: Function) -> str | None:
    if function.coverage == 0.0:
        return GLUE_NOTES.get((function.file, function.name))
    return None


def load_coverage(
    path: Path,
) -> tuple[dict[str, list[tuple[int, int, float]]], float]:
    """Return function records and total line coverage percentage."""
    data = json.loads(path.read_text())["data"][0]
    line_coverage = float(data.get("totals", {}).get("lines", {}).get("percent", 0.0))
    by_file: dict[str, list[tuple[int, int, float]]] = {}
    for function in data.get("functions", []):
        files = function.get("filenames") or []
        if not files:
            continue
        regions = function.get("regions") or []
        if not regions:
            continue
        # The first region is the function entry; it covers the whole body and
        # would falsely mark every line executed.
        body = regions[1:]
        if body:
            covered = sum(1 for region in body if region[4] > 0)
            coverage = covered / len(body)
        else:
            coverage = 1.0 if function.get("count", 0) > 0 else 0.0
        start = min(region[0] for region in regions)
        end = max(region[2] for region in regions)
        by_file.setdefault(files[0], []).append((start, end, coverage))
    return by_file, line_coverage


def function_coverage(
    records: list[tuple[int, int, float]], start: int, end: int
) -> float:
    candidates = [
        coverage
        for record_start, record_end, coverage in records
        if record_start <= end and record_end >= start
        and abs(record_start - start) <= 3
    ]
    if not candidates:
        # Methods can start on the `pub fn` line while LLVM points at the body.
        candidates = [
            coverage
            for record_start, record_end, coverage in records
            if record_start <= end and record_end >= start
        ]
    return max(candidates, default=0.0)


def test_module_ranges(source: Path) -> list[tuple[int, int]]:
    """Return line ranges of top-level #[cfg(test)] modules."""
    lines = source.read_text().splitlines()
    ranges: list[tuple[int, int]] = []
    index = 0
    while index < len(lines):
        if lines[index].strip() != "#[cfg(test)]":
            index += 1
            continue
        cursor = index + 1
        while cursor < len(lines) and (
            not lines[cursor].strip() or lines[cursor].strip().startswith("#[")
        ):
            cursor += 1
        if cursor >= len(lines) or not re.match(r"\s*mod\s+\w+\s*\{", lines[cursor]):
            index += 1
            continue
        for end in range(cursor + 1, len(lines)):
            if lines[end] == "}":
                ranges.append((cursor + 1, end + 1))
                index = end
                break
        index += 1
    return ranges


def collect(
    root: Path, coverage_path: Path, excludes: tuple[str, ...] = ()
) -> list[Function]:
    coverage, _ = load_coverage(coverage_path)
    functions: list[Function] = []
    for source in sorted((root / "src").rglob("*.rs")):
        relative = str(source.relative_to(root))
        if any(fnmatch.fnmatch(relative, pattern) for pattern in excludes):
            continue
        analysis = lizard.analyze_file(str(source))
        records = coverage.get(str(source.resolve()), [])
        test_ranges = test_module_ranges(source)
        for item in analysis.function_list:
            name = item.name
            if name.startswith("test_") or name.startswith("prop_"):
                continue
            if any(start <= item.start_line <= end for start, end in test_ranges):
                continue
            line = item.start_line
            end = item.end_line
            functions.append(
                Function(
                    file=relative,
                    name=name,
                    line=line,
                    end_line=end,
                    complexity=int(item.cyclomatic_complexity),
                    coverage=function_coverage(records, line, end),
                )
            )
    return functions


def render(functions: list[Function], top: int, line_coverage: float) -> str:
    ranked = sorted(functions, key=lambda f: f.crap, reverse=True)
    lines = [
        "# CRAP report",
        "",
        "Generated by `tools/crap.py` from `cargo llvm-cov --json` and lizard.",
        "Coverage is region coverage excluding the function-entry region.",
        f"Total line coverage: **{line_coverage:.2f}%**.",
        "",
        "| CRAP | Function | File:line | Complexity | Coverage | Note |",
        "| ---: | --- | --- | ---: | ---: | --- |",
    ]
    for function in ranked[:top]:
        note = glue_note(function)
        if function.coverage == 0.0 and note is None:
            note = "**0% — unclassified test gap**"
        lines.append(
            f"| {function.crap:.2f} | `{function.name}` | "
            f"`{function.file}:{function.line}` | {function.complexity} | "
            f"{function.coverage * 100:.1f}% | {note or ''} |"
        )
    zero = [function for function in functions if function.coverage == 0.0]
    glue = [(function, glue_note(function)) for function in zero if glue_note(function)]
    unclassified = [function for function in zero if glue_note(function) is None]
    if zero:
        lines += [
            "",
            "## Zero-coverage classification",
            "",
            "`cargo llvm-cov` only records code executed in the test process. "
            "**Glue** rows own a hardware, terminal or process entry point and "
            "are excluded from the CRAP gate. **Unclassified** rows are not "
            "glue: they are low-complexity test gaps that can be covered with "
            "normal unit tests.",
            "",
            "**Glue (excluded from the gate):**",
            "",
        ]
        lines += [
            f"- `{function.name}` (`{function.file}:{function.line}`) — {note}"
            for function, note in glue
        ]
        if unclassified:
            lines += [
                "",
                "**Unclassified 0% functions — not glue; test candidates:**",
                "",
            ]
            lines += [
                f"- `{function.name}` (`{function.file}:{function.line}`) — CRAP {function.crap:.2f}"
                for function in unclassified
            ]
    lines += [
        "",
        f"Functions analysed: {len(functions)}. "
        f"CRAP > 30: {sum(1 for f in functions if f.crap > 30)}. "
        f"CRAP > 100: {sum(1 for f in functions if f.crap > 100)}.",
        "",
        "Priority: high CRAP means high complexity and/or weak coverage. "
        "Refactor pure logic and add focused tests; UI/FFI code is a poor target.",
        "",
    ]
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("coverage", type=Path)
    parser.add_argument("--root", type=Path, default=Path("."))
    parser.add_argument("--top", type=int, default=40)
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--exclude",
        action="append",
        default=[],
        metavar="GLOB",
        help="skip source files matching this glob (repeatable)",
    )
    parser.add_argument(
        "--max-over-30",
        type=int,
        help="fail when more than this many functions have CRAP > 30",
    )
    parser.add_argument(
        "--max-over-100",
        type=int,
        help="fail when more than this many functions have CRAP > 100",
    )
    parser.add_argument(
        "--min-lines",
        type=float,
        help="fail when total line coverage is below this percentage",
    )
    args = parser.parse_args()

    functions = collect(args.root.resolve(), args.coverage, tuple(args.exclude))
    _, line_coverage = load_coverage(args.coverage)
    report = render(functions, args.top, line_coverage)

    over_30 = sum(1 for function in functions if function.crap > 30)
    over_100 = sum(1 for function in functions if function.crap > 100)
    failures = []
    if args.max_over_30 is not None and over_30 > args.max_over_30:
        failures.append(
            f"CRAP > 30: {over_30} (budget {args.max_over_30})"
        )
    if args.max_over_100 is not None and over_100 > args.max_over_100:
        failures.append(
            f"CRAP > 100: {over_100} (budget {args.max_over_100})"
        )
    if args.min_lines is not None and line_coverage < args.min_lines:
        failures.append(
            f"line coverage: {line_coverage:.2f}% (minimum {args.min_lines:.2f}%)"
        )

    gate = [
        "",
        "## Quality gate",
        "",
        f"- Functions: {len(functions)}",
        f"- CRAP > 30: {over_30}"
        + (f" / budget {args.max_over_30}" if args.max_over_30 is not None else ""),
        f"- CRAP > 100: {over_100}"
        + (f" / budget {args.max_over_100}" if args.max_over_100 is not None else ""),
        f"- Line coverage: {line_coverage:.2f}%"
        + (f" / minimum {args.min_lines:.2f}%" if args.min_lines is not None else ""),
        f"- Result: {'FAIL' if failures else 'PASS'}",
    ]
    if failures:
        gate += ["", *[f"- {failure}" for failure in failures]]
    report += "\n".join(gate) + "\n"

    if args.output:
        args.output.write_text(report)
    print(report)
    if failures:
        sys.exit(1)


if __name__ == "__main__":
    main()
