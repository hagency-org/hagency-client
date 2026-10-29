import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { checkSpecBindings } from './check-spec-bindings.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const options = { cwd: root, encoding: 'utf8', timeout: 600_000, maxBuffer: 64 * 1024 * 1024 };
const graph = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--format-version', '1'], options));
// Use the actual locked graph and test binaries, not invented selector names or
// workspace membership as a proxy for whether this path dependency is testable.
const raw = execFileSync('cargo', ['test', '--locked', '-p', 'mdurl', '-p', 'hagency-matrix-format', '--all-targets', '--', '--list'], options);
const inventory = raw.split('\n').filter(line => line.endsWith(': test')).map(line => ({ name: line.slice(0, -6) }));
const quality = inventory.filter(entry => entry.name.includes('quality_idna_'));
const withContract = check => {
  const directory = mkdtempSync(path.join(tmpdir(), 'hagency-vendor-bindings-'));
  try {
    copyFileSync(path.join(root, 'specs/task-client-idna-dependency.spec.md'), path.join(directory, 'idna.spec.md'));
    check(directory);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
};

test('the locked external mdurl package is the local patch and has real registered tests', () => {
  const packages = graph.packages.filter(pkg => pkg.name === 'mdurl');
  assert.equal(packages.length, 1);
  const [mdurl] = packages;
  assert.equal(realpathSync(mdurl.manifest_path), realpathSync(path.join(root, 'vendor/mdurl/Cargo.toml')));
  assert.ok(!graph.workspace_members.includes(mdurl.id), 'vendor must remain outside workspace membership');
  assert.equal(quality.length, 4, 'all four public API regressions must actually compile/register');
  assert.equal(inventory.filter(entry => entry.name.includes('native_matrix_format_')).length, 6);
  withContract(directory => {
    const result = checkSpecBindings(inventory, { runtime: 'rust', directory });
    assert.equal(result.count, 2);
    assert.deepEqual(result.missing, []);
  });
});

test('removing the actual vendor selectors leaves the new contract visibly unbound', () => {
  assert.equal(quality.length, 4);
  withContract(directory => {
    const result = checkSpecBindings(inventory.filter(entry => !entry.name.includes('quality_idna_')), { runtime: 'rust', directory });
    assert.equal(result.count, 2);
    assert.deepEqual(result.missing.map(entry => entry.selector), ['quality_idna_']);
  });
});
