"""Compare two prebuilt desktop test binaries using the same native probe.

Build each revision with the current terminal_surface_entity/native_probe.rs
and the same Cargo profile. Run on an idle Windows desktop, without builds.
Only synthetic terminal data is used. Results are resumable after each sample.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import platform

from keyword_highlight_native_ab import probe


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--optimized", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--profile", required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    binaries = {"baseline": args.baseline.resolve(), "optimized": args.optimized.resolve()}
    metadata = {
        "platform": platform.platform(), "profile": args.profile, "repeats": args.repeats,
        "binaries": {label: {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                     for label, path in binaries.items()},
    }
    metadata_path = args.output / "metadata.json"
    if metadata_path.exists() and json.loads(metadata_path.read_text(encoding="utf-8")) != metadata:
        raise SystemExit("Metadata changed; use a new output directory for different binaries or settings")
    metadata_path.write_text(json.dumps(metadata, indent=2), encoding="utf-8")
    result_path = args.output / "results.json"
    records = json.loads(result_path.read_text(encoding="utf-8")) if result_path.exists() else []
    completed = {(r["variant"], r["repeat"], r["case"], r["custom_rules"], r["panes"]) for r in records}
    for repeat in range(args.repeats):
        for rules in [0, 200, 1000]:
            for panes in [1, 4]:
                for case in ["output", "scroll"]:
                    labels = list(binaries) if repeat % 2 == 0 else list(reversed(binaries))
                    for label in labels:
                        key = (label, repeat, case, rules, panes)
                        if key in completed:
                            continue
                        result = probe(binaries[label], case, True, "github-dark", rules=rules, panes=panes)
                        assert result["custom_rules"] == rules and result["panes"] == panes, "binary lacks current probe"
                        records.append(dict(result, variant=label, repeat=repeat))
                        result_path.write_text(json.dumps(records, indent=2), encoding="utf-8")
                        print(f"{len(records)}/{args.repeats * 24}: {key}; handler p95={result['handler']['p95_us']} us", flush=True)
    print(result_path)


if __name__ == "__main__":
    main()
