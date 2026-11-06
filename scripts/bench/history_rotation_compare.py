"""Compare the five-run CPU history-rotation benchmark logs."""
import argparse
import re
import statistics
from pathlib import Path

ROW = re.compile(
    r"history_rotation limit=(\d+) scrolled=(true|false) repeat=(\d+) "
    r"bytes_per_sec=([\d.]+) update_p95_ns=(\d+) recoveries=(\d+)"
)


def read_runs(path: Path) -> dict:
    runs = {}
    for match in ROW.finditer(path.read_text(encoding="utf-8")):
        limit, scrolled, repeat, throughput, p95, recoveries = match.groups()
        key = (int(limit), scrolled == "true")
        group = runs.setdefault(key, {})
        if int(repeat) in group:
            raise ValueError(f"{path}: repeated run {key}/{repeat}")
        group[int(repeat)] = (float(throughput), int(p95), int(recoveries))
    expected = {(limit, scrolled) for limit in (5000, 100000) for scrolled in (False, True)}
    if runs.keys() != expected or any(set(group) != set(range(5)) for group in runs.values()):
        raise ValueError(f"{path}: expected five complete runs of each scenario")
    return runs


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    args = parser.parse_args()
    baseline, candidate = read_runs(args.baseline), read_runs(args.candidate)
    passed = True
    for key in sorted(baseline):
        old = [statistics.median(row[i] for row in baseline[key].values()) for i in range(3)]
        new = [statistics.median(row[i] for row in candidate[key].values()) for i in range(3)]
        throughput_change = (new[0] / old[0] - 1) * 100
        p95_change = (new[1] / old[1] - 1) * 100
        print(
            f"limit={key[0]} scrolled={key[1]} "
            f"bytes_per_sec={old[0]:.1f}->{new[0]:.1f} ({throughput_change:+.2f}%) "
            f"update_p95_ns={old[1]:.0f}->{new[1]:.0f} ({p95_change:+.2f}%) "
            f"recoveries={old[2]:.0f}->{new[2]:.0f}"
        )
        passed &= p95_change <= 10 if key[1] else throughput_change >= -5
    print("PASS" if passed else "FAIL")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
