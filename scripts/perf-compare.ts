#!/usr/bin/env bun
/**
 * perf-compare — nightly performance regression gate.
 *
 * STATUS: STUB (M0-11). Exits 0 unconditionally.
 *
 * Wired into .github/workflows/nightly-perf.yml now so the runner, the artifact
 * path and the failure path are all proven before there is anything to measure.
 * The real implementation lands with the st-bench harness in M2 (M2-10); see
 * docs/plan/06-testing-perf-ci.md §3 for the scenario list and metric names.
 *
 * Contract, once implemented:
 *
 *   bun run scripts/perf-compare.ts --dir docs/perf --window 7 --threshold 0.15
 *
 *   --dir        directory of per-run JSON files written by `just perf`
 *                (one file per run, named <ISO-date>.json)
 *   --window     how many days of history form the comparison baseline
 *   --threshold  fractional regression that fails the run (0.15 = 15%)
 *
 * Algorithm:
 *   1. Load every run in --dir; take the newest as `current` and the runs from
 *      the preceding --window days as `history`.
 *   2. Group by (scenario, platform, metric). Q47: macOS decides the gate;
 *      Linux/WSL2 numbers are recorded and reported but never fail the run,
 *      and are only flagged if they drift beyond 2x the macOS number.
 *   3. Baseline = median of `history` per group. The median, not the mean, so a
 *      single noisy night cannot move the bar.
 *   4. Regression = (current - baseline) / baseline for latency-style metrics
 *      (lower is better) and its negation for throughput-style metrics. Fail if
 *      any macOS group exceeds --threshold.
 *   5. Groups with fewer than 3 history points are skipped: not enough data to
 *      call a regression, and a new scenario must not fail its own first night.
 *   6. Print a table of every group with its delta, then exit 1 if any gating
 *      group regressed, 0 otherwise.
 *
 * TODO(M2-10): implement the above against the real st-bench JSON schema.
 */

const args = process.argv.slice(2);

function flag(name: string, fallback: string): string {
    const i = args.indexOf(`--${name}`);
    return i >= 0 && i + 1 < args.length ? args[i + 1]! : fallback;
}

const dir = flag("dir", "docs/perf");
const window = Number(flag("window", "7"));
const threshold = Number(flag("threshold", "0.15"));

console.log(
    `perf-compare: STUB — no comparison performed ` +
        `(dir=${dir}, window=${window}d, threshold=${(threshold * 100).toFixed(0)}%).`,
);
console.log("perf-compare: TODO(M2-10) implement the regression gate; see the header comment.");

process.exit(0);
