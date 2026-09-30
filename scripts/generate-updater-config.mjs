// Tauri deserializes plugins.updater before the Rust plugin builder supplies
// its key. The shipped app and both sides of the isolated upgrade need a config.
import { readFileSync, writeFileSync } from 'node:fs';

const [source, output] = process.argv.slice(2);
const pubkey = process.env.SAGASCRIPT_UPDATER_PUBKEY?.trim();
if (!source || !output || !pubkey) {
  throw new Error('Usage: SAGASCRIPT_UPDATER_PUBKEY=... generate-updater-config.mjs SOURCE OUTPUT');
}

const config = JSON.parse(readFileSync(source, 'utf8'));
if (typeof config.plugins?.updater?.dangerousInsecureTransportProtocol !== 'boolean') {
  throw new Error('Smoke config must explicitly specify whether insecure transport is allowed');
}
config.plugins.updater.pubkey = pubkey;
writeFileSync(output, JSON.stringify(config));
