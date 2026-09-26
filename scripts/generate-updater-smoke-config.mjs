// Tauri deserializes plugins.updater before the Rust plugin builder supplies
// its key, so the HTTP-only smoke config must also contain the current key.
import { readFileSync, writeFileSync } from 'node:fs';

const [source, output] = process.argv.slice(2);
const pubkey = process.env.SAGASCRIPT_UPDATER_PUBKEY?.trim();
if (!source || !output || !pubkey) {
  throw new Error('Usage: SAGASCRIPT_UPDATER_PUBKEY=... generate-updater-smoke-config.mjs SOURCE OUTPUT');
}

const config = JSON.parse(readFileSync(source, 'utf8'));
if (config.plugins?.updater?.dangerousInsecureTransportProtocol !== true) {
  throw new Error('Smoke config must explicitly allow the loopback update feed');
}
config.plugins.updater.pubkey = pubkey;
writeFileSync(output, JSON.stringify(config));
