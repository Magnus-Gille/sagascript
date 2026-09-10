#!/usr/bin/env node

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ISO_TIMESTAMP =
  /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d{1,3})?(?:Z|[+-]\d{2}:\d{2})$/;

const compareStrings = (left, right) => (left < right ? -1 : left > right ? 1 : 0);

function isoTimestamp(value, label) {
  if (typeof value !== "string") {
    throw new TypeError(`${label} must be an ISO 8601 timestamp`);
  }

  const parts = value.match(ISO_TIMESTAMP);
  const milliseconds = Date.parse(value);
  if (!parts || !Number.isFinite(milliseconds)) {
    throw new TypeError(`${label} must be an ISO 8601 timestamp with a time zone`);
  }

  const [, year, month, day, hour, minute, second] = parts.map(Number);
  const calendarDate = new Date(Date.UTC(year, month - 1, day));
  const validCalendarDate =
    calendarDate.getUTCFullYear() === year &&
    calendarDate.getUTCMonth() === month - 1 &&
    calendarDate.getUTCDate() === day;
  if (!validCalendarDate || hour > 23 || minute > 59 || second > 59) {
    throw new TypeError(`${label} must be a valid ISO 8601 timestamp`);
  }

  return new Date(milliseconds).toISOString();
}

function nonNegativeInteger(value, label) {
  if (!Number.isInteger(value) || value < 0) {
    throw new TypeError(`${label} must be a non-negative integer`);
  }
  return value;
}

function normalizeAsset(asset, label) {
  if (asset === null || typeof asset !== "object" || Array.isArray(asset)) {
    throw new TypeError(`${label} must be an object`);
  }
  if (typeof asset.name !== "string" || asset.name.length === 0) {
    throw new TypeError(`${label}.name must be a non-empty string`);
  }
  if (typeof asset.content_type !== "string") {
    throw new TypeError(`${label}.content_type must be a string`);
  }

  return {
    name: asset.name,
    content_type: asset.content_type,
    size: nonNegativeInteger(asset.size, `${label}.size`),
    download_count: nonNegativeInteger(asset.download_count, `${label}.download_count`),
  };
}

function normalizeRelease(release, index) {
  const label = `releases[${index}]`;
  if (release === null || typeof release !== "object" || Array.isArray(release)) {
    throw new TypeError(`${label} must be an object`);
  }
  if (typeof release.tag_name !== "string" || release.tag_name.length === 0) {
    throw new TypeError(`${label}.tag_name must be a non-empty string`);
  }
  if (typeof release.prerelease !== "boolean") {
    throw new TypeError(`${label}.prerelease must be a boolean`);
  }
  if (!Array.isArray(release.assets)) {
    throw new TypeError(`${label}.assets must be an array`);
  }

  return {
    tag_name: release.tag_name,
    published_at: isoTimestamp(release.published_at, `${label}.published_at`),
    prerelease: release.prerelease,
    assets: release.assets
      .map((asset, assetIndex) => normalizeAsset(asset, `${label}.assets[${assetIndex}]`))
      .sort((left, right) => compareStrings(left.name, right.name)),
  };
}

function calculateTotals(releases) {
  const totals = {
    all_assets: 0,
    app_downloads: 0,
    dmg_downloads: 0,
    updater_downloads: 0,
    windows_downloads: 0,
  };

  for (const { assets } of releases) {
    for (const asset of assets) {
      const downloads = asset.download_count;
      const windows = /\.(?:exe|msi)$/i.test(asset.name);
      const dmg = asset.name === "Sagascript.dmg";
      const updater = asset.name === "Sagascript.app.tar.gz";

      totals.all_assets += downloads;
      if (dmg) totals.dmg_downloads += downloads;
      if (updater) totals.updater_downloads += downloads;
      if (windows) totals.windows_downloads += downloads;
      if (dmg || updater || windows) totals.app_downloads += downloads;
    }
  }

  return totals;
}

export function createSnapshot(input, repository, capturedAt = new Date().toISOString()) {
  if (!Array.isArray(input)) {
    throw new TypeError("GitHub releases input must be an array");
  }
  if (typeof repository !== "string" || !/^[^/\s]+\/[^/\s]+$/.test(repository)) {
    throw new TypeError("repository must have the form owner/repo");
  }

  const captured_at = isoTimestamp(capturedAt, "captured-at");
  const releases = input
    .filter((release) => release?.draft === false)
    .map(normalizeRelease)
    .sort(
      (left, right) =>
        compareStrings(left.published_at, right.published_at) ||
        compareStrings(left.tag_name, right.tag_name),
    );

  return {
    schema_version: 1,
    repository,
    captured_at,
    snapshot_date: captured_at.slice(0, 10),
    totals: calculateTotals(releases),
    releases,
  };
}

function parseArguments(arguments_) {
  const allowed = new Set(["--input", "--output", "--repository", "--captured-at"]);
  const required = ["--input", "--output", "--repository"];
  const parsed = new Map();

  for (let index = 0; index < arguments_.length; index += 2) {
    const flag = arguments_[index];
    const value = arguments_[index + 1];
    if (!allowed.has(flag)) throw new Error(`Unknown flag: ${flag ?? "<missing>"}`);
    if (parsed.has(flag)) throw new Error(`Duplicate flag: ${flag}`);
    if (value === undefined || value.startsWith("--")) throw new Error(`Missing value for ${flag}`);
    parsed.set(flag, value);
  }

  for (const flag of required) {
    if (!parsed.has(flag)) throw new Error(`Missing required flag: ${flag}`);
  }
  return parsed;
}

function main() {
  const arguments_ = parseArguments(process.argv.slice(2));
  const inputPath = arguments_.get("--input");
  const outputPath = arguments_.get("--output");
  const input = JSON.parse(readFileSync(inputPath, "utf8"));
  const snapshot = createSnapshot(
    input,
    arguments_.get("--repository"),
    arguments_.get("--captured-at") ?? new Date().toISOString(),
  );

  mkdirSync(dirname(outputPath), { recursive: true });
  writeFileSync(outputPath, `${JSON.stringify(snapshot, null, 2)}\n`, "utf8");
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(error instanceof Error ? error.stack : String(error));
    process.exitCode = 1;
  }
}
