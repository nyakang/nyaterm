"""Run each matrix case in a fresh process, rotating the three version orders."""
import argparse
import csv
import ctypes
import io
import os
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("main_exe", type=Path)
    parser.add_argument("pr_exe", type=Path)
    parser.add_argument("fixed_exe", type=Path)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("--quick", action="store_true")
    parser.add_argument("--cpu", type=int, help="Pin this runner and its children to a Windows logical CPU")
    args = parser.parse_args()
    if args.cpu is not None:
        if os.name != "nt":
            parser.error("--cpu currently requires Windows")
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.GetCurrentProcess.restype = ctypes.c_void_p
        kernel.SetProcessAffinityMask.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        if not kernel.SetProcessAffinityMask(kernel.GetCurrentProcess(), 1 << args.cpu):
            raise ctypes.WinError(ctypes.get_last_error())
    args.output_dir.mkdir(parents=True, exist_ok=True)
    versions = [("main", args.main_exe), ("pr", args.pr_exe), ("fixed", args.fixed_exe)]
    # Obtain exact per-line byte lengths from the executable, not a second copy
    # of its workload definitions.
    listing = subprocess.run([str(args.fixed_exe), "--matrix", "--list"],
                             check=True, capture_output=True, text=True).stdout
    cases = list(csv.DictReader(io.StringIO(listing)))
    outputs = {}
    try:
        for name, _ in versions:
            outputs[name] = (args.output_dir / f"{name}.csv").open("w", encoding="utf-8", newline="")
        for index, case in enumerate(cases):
            key = ",".join(case[field] for field in ("kind", "rows", "history", "chunk"))
            if args.quick and (case["kind"] not in ("plain", "sgr") or case["rows"] == "200" or case["chunk"] != "16384"):
                continue
            offset = index % len(versions)
            for name, executable in versions[offset:] + versions[:offset]:
                result = subprocess.run([str(executable), "--matrix", f"--case={key}"],
                                        check=True, capture_output=True, text=True,
                                        creationflags=subprocess.ABOVE_NORMAL_PRIORITY_CLASS if os.name == "nt" else 0)
                lines = result.stdout.strip().splitlines()
                if len(lines) != 2:
                    raise RuntimeError(f"Unexpected CSV for {name} {key}: {result.stdout}")
                if outputs[name].tell() == 0:
                    outputs[name].write(lines[0] + "\n")
                outputs[name].write(lines[1] + "\n")
                outputs[name].flush()
            print(f"{index + 1}/{len(cases)} {key}", flush=True)
    finally:
        for output in outputs.values():
            output.close()


if __name__ == "__main__":
    main()
