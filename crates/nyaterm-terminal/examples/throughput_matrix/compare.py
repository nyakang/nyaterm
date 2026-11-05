"""Check PR #1139 performance gates against matching CSV cases."""
import argparse
import csv
import math


def read(path):
    with open(path, encoding="utf-8-sig", newline="") as source:
        return {
            (row["kind"], int(row["rows"]), int(row["history"]), int(row["chunk"])): row
            for row in csv.DictReader(source)
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("main_csv")
    parser.add_argument("pr_csv")
    parser.add_argument("fixed_csv")
    args = parser.parse_args()
    baseline, original, fixed = map(read, (args.main_csv, args.pr_csv, args.fixed_csv))
    if baseline.keys() != original.keys() or baseline.keys() != fixed.keys():
        raise SystemExit("CSV matrices have different cases")
    gates = {"plain/SGR 16 KiB vs main": [], "per-line vs PR": []}
    for key, row in fixed.items():
        kind, rows, history, chunk = key
        speed = float(row["mbps"])
        if kind in ("plain", "sgr") and chunk == 16384:
            gates["plain/SGR 16 KiB vs main"].append((speed / float(baseline[key]["mbps"]), key))
        if chunk < 16384:
            gates["per-line vs PR"].append((speed / float(original[key]["mbps"]), key))
    failed = False
    for name, cases in gates.items():
        if not cases:
            print(f"{name}: no cases")
            failed = True
            continue
        ratios = [ratio for ratio, _ in cases]
        worst, key = min(cases)
        geomean = math.exp(sum(map(math.log, ratios)) / len(ratios))
        print(f"{name}: n={len(cases)} min={worst:.1%} geomean={geomean:.1%} worst={key}")
        for ratio, key in sorted(cases):
            if ratio < 0.9:
                failed = True
                print(f"  FAIL {key}: {ratio:.1%}")
    raise SystemExit(int(failed))


if __name__ == "__main__":
    main()
