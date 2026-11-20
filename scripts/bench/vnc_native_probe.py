"""Run synthetic native VNC texture uploads and retain CPU timings/readback checks."""
from __future__ import annotations

import argparse
import json
import platform
from pathlib import Path

from vnc_pipeline_ab import fingerprint, run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=120)
    parser.add_argument("--profile", choices=["dev", "release"], required=True)
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("samples must be positive")
    probe = args.probe.resolve()
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    report = {"platform": platform.platform(), "profile": args.profile,
              "rustc": run(["rustc", "--version"]), "probe": fingerprint(probe), "records": []}
    for width, height in [(1920, 1080), (3840, 2160)]:
        for fixture in ["full", "column"]:
            for mode in ["borrowed", "owned"]:
                row = json.loads(run([str(probe), str(width), str(height), fixture, mode, str(args.samples)], timeout=90))
                report["records"].append(row)
                output.write_text(json.dumps(report, indent=2), encoding="utf-8")
                print(json.dumps(row), flush=True)
    print(output)


if __name__ == "__main__":
    main()
