#!/usr/bin/env python3
"""Measure downstream cargo check on a graph of beans, excluding dependency builds."""

import argparse
import json
import os
from pathlib import Path
import shutil
from statistics import median
import subprocess
from tempfile import TemporaryDirectory
from time import perf_counter


def positive(value):
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("expected a positive integer")
    return number


def graph(size, deps):
    """Bean `i` depends on the `deps` beans registered just before it."""
    lines = ['#![recursion_limit = "{}"]'.format(max(512, 4 * size)),
             "use r2e::prelude::*;"]
    for i in range(size):
        params = ", ".join(f"_d{j}: Bean{j}" for j in range(max(0, i - deps), i))
        lines.extend([f"#[derive(Clone)] struct Bean{i};",
                      f"#[bean] impl Bean{i} {{ fn new({params}) -> Self {{ Self }} }}"])
    lines.append("pub async fn boot() { let _ = AppBuilder::new()")
    lines.extend(f"    .register::<Bean{i}>()" for i in range(size))
    lines.append("    .build_state().await; }")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1],
                        help="R2E checkout to benchmark")
    parser.add_argument("--runs", type=positive, default=3)
    parser.add_argument("--deps", type=positive, nargs="+", default=[1, 5, 10],
                        help="dependencies per bean (one measurement per value)")
    parser.add_argument("sizes", type=positive, nargs="*", default=[100, 250])
    args = parser.parse_args()
    repo = args.repo.resolve()
    with TemporaryDirectory(prefix="r2e-di-check-") as tmp:
        project = Path(tmp)
        manifest = project / "Cargo.toml"
        manifest.write_text(
            '[package]\nname = "r2e-di-check-bench"\nversion = "0.0.0"\nedition = "2024"\n'
            f'[dependencies]\nr2e = {{ path = {json.dumps(str(repo / "r2e"))} }}\n'
        )
        shutil.copyfile(repo / "Cargo.lock", project / "Cargo.lock")
        source = project / "src/lib.rs"
        source.parent.mkdir()
        source.write_text("")
        # Run from the checkout to use its rust-toolchain.toml. Compile the
        # dependencies once, then force Cargo to recheck only the consumer.
        cmd = ["cargo", "check", "--quiet", "--manifest-path", str(manifest)]
        env = dict(os.environ, CARGO_INCREMENTAL="0", RUSTC_WRAPPER="")
        subprocess.run(cmd, cwd=repo, env=env, check=True)
        for size in args.sizes:
            for deps in args.deps:
                source_text = graph(size, deps)
                edges = sum(min(deps, i) for i in range(size))
                samples = []
                for run in range(args.runs):
                    source.write_text(f"{source_text}\n// Measurement {run}\n")
                    start = perf_counter()
                    subprocess.run(cmd, cwd=repo, env=env, check=True)
                    samples.append(perf_counter() - start)
                print(f"{size} beans x {deps} deps ({edges} edges): "
                      f"{median(samples):.2f}s median "
                      f"({', '.join(f'{s:.2f}s' for s in samples)})", flush=True)

if __name__ == "__main__":
    main()
