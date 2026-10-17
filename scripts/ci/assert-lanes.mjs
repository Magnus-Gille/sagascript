#!/usr/bin/env node
// The body of the check-macos, check-windows and check-linux aggregators of
// .github/workflows/ci.yml (#358). Branch protection requires check-macos and
// check-windows by name; each job `needs:` the scope job and its OS's lanes and
// runs this script with `NEEDS: ${{ toJSON(needs) }}`.
//
// A lane that did not run is only acceptable when the scope job trusted its own
// classification and found nothing but documentation changed; in every other case
// every lane must have succeeded. Everything else fails closed: a missing or
// malformed input, an untrusted or unsuccessful scope, an unknown docs_only value,
// a lane that failed, was cancelled or was skipped on a full run, and an aggregator
// that waits for no lane at all.
import { pathToFileURL } from "node:url";

/** Returns the problems with a `needs` context; an empty list means the lanes are accepted. */
export function laneProblems(needs) {
  if (needs === null || typeof needs !== "object" || Array.isArray(needs)) return ["NEEDS must be the JSON object of the needs context"];
  const problems = [];
  const scope = needs.scope;
  if (scope?.result !== "success") problems.push(`scope: result is ${JSON.stringify(scope?.result)}, expected "success"`);
  if (scope?.outputs?.scope_trusted !== "true")
    problems.push(`scope: scope_trusted is ${JSON.stringify(scope?.outputs?.scope_trusted)}, expected "true"`);
  const lanes = Object.entries(needs).filter(([id]) => id !== "scope");
  if (lanes.length === 0) problems.push("no lane job in needs: this aggregator would gate nothing");
  const docsOnly = scope?.outputs?.docs_only;
  if (docsOnly !== "true" && docsOnly !== "false") {
    problems.push(`scope: docs_only is ${JSON.stringify(docsOnly)}, expected "true" or "false"`);
  } else {
    const expected = docsOnly === "true" ? "skipped" : "success";
    for (const [id, lane] of lanes) {
      if (lane?.result !== expected)
        problems.push(`${id}: result is ${JSON.stringify(lane?.result)}, expected ${JSON.stringify(expected)} (docs_only=${docsOnly})`);
    }
  }
  return problems;
}

function main() {
  let needs;
  try {
    needs = JSON.parse(process.env.NEEDS ?? "");
  } catch (error) {
    console.error(`NEEDS is not valid JSON: ${error.message}`);
    return 1;
  }
  const problems = laneProblems(needs);
  for (const [id, entry] of Object.entries(needs ?? {})) console.log(`${id}: ${entry?.result}`);
  for (const problem of problems) console.error(`::error title=CI lanes::${problem}`);
  return problems.length === 0 ? 0 : 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) process.exit(main());
