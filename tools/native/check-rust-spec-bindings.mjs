import { execFileSync } from 'node:child_process';
import { checkSpecBindings } from './check-spec-bindings.mjs';
// Inventory the retained desktop engine selectors; page/CLI scenarios are retired.
const raw = execFileSync('cargo', ['test', '-p', 'hagency', '-p', 'hagency-agent-local', '-p', 'hagency-core', '-p', 'hagency-crypto-proof', '-p', 'hagency-execution', '-p', 'hagency-files', '-p', 'hagency-matrix', '-p', 'hagency-matrix-format', '-p', 'hagency-media', '-p', 'hagency-media-store', '-p', 'hagency-metering', '-p', 'hagency-palpo', '-p', 'hagency-permissions', '-p', 'hagency-platform', '-p', 'hagency-progress', '-p', 'hagency-progress-runtime', '-p', 'hagency-runtime', '-p', 'hagency-store', '--all-targets', '--all-features', '--locked', '--', '--list'], {
  encoding: 'utf8', timeout: 600_000, maxBuffer: 32 * 1024 * 1024,
});
// mdurl is an external path dependency with no features. Cargo rejects
// --all-features for excluded packages, so inventory its actual default build
// separately while preserving the engine's existing all-feature inventory.
const mdurlRaw = execFileSync('cargo', ['test', '-p', 'mdurl', '--all-targets', '--locked', '--', '--list'], {
  encoding: 'utf8', timeout: 600_000, maxBuffer: 32 * 1024 * 1024,
});
const inventory = `${raw}\n${mdurlRaw}`.split('\n').filter(line => line.endsWith(': test')).map(line => ({name: line.slice(0, -6)}));
if (!inventory.length) throw new Error('Cargo returned no registered tests');
const result = checkSpecBindings(inventory, { runtime: 'rust' });
console.log(JSON.stringify(result, null, 2));
if (!result.count || result.missing.length) process.exitCode = 1;
