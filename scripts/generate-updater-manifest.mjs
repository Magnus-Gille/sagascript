import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { basename } from 'node:path';

const [version, bundlePath, signaturePath, outputPath] = process.argv.slice(2);
if (!version || !bundlePath || !signaturePath || !outputPath) {
  throw new Error('Usage: generate-updater-manifest.mjs VERSION BUNDLE SIGNATURE OUTPUT');
}
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  throw new Error(`Updater version must be a stable release: ${version}`);
}
if (!existsSync(bundlePath)) {
  throw new Error(`Updater bundle is missing: ${bundlePath}`);
}
if (!existsSync(signaturePath)) {
  throw new Error(`Updater signature is missing: ${signaturePath}`);
}
const signature = readFileSync(signaturePath, 'utf8').trim();
if (!signature) {
  throw new Error('Updater signature is empty');
}
const filename = 'Sagascript.app.tar.gz';
if (basename(bundlePath) !== filename) {
  throw new Error(`Expected ${filename} as the release asset`);
}
const manifest = {
  version,
  platforms: {
    'darwin-aarch64': {
      url: `https://github.com/Magnus-Gille/sagascript/releases/download/v${version}/${filename}`,
      signature,
    },
  },
};
writeFileSync(outputPath, `${JSON.stringify(manifest, null, 2)}\n`);
