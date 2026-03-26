#!/usr/bin/env python3
"""Rename NAPI-produced .node files from index.* to pgmicro.*"""
import glob, os, sys

os.chdir(sys.argv[1] if len(sys.argv) > 1 else ".")
files = glob.glob("*.node")
if not files:
    print("ERROR: no .node file found")
    sys.exit(1)
for f in files:
    target = "pgmicro." + f.split(".", 1)[1]
    print(f"  {f} -> {target}")
    os.rename(f, target)
