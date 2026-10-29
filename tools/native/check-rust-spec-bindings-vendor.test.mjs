import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
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

test('real workspace layout admits only registered excluded vendor manifests', () => {
  const python = process.env.HAGENCY_LAYOUT_PYTHON || (process.platform === 'win32' ? 'python' : 'python3');
  const command = ['tools/check-workspace-layout.py'];
  const result = JSON.parse(execFileSync(python, command, options));
  assert.equal(result.packages, graph.workspace_members.length);
  assert.deepEqual(result.vendored_patches, ['vendor/mdurl/Cargo.toml']);
  // Exercise the real tracked/untracked discovery and Cargo metadata, including
  // a nested impostor under the otherwise permitted vendor directory.
  for (const parent of ['vendor', 'vendor/mdurl']) {
    const directory = mkdtempSync(path.join(root, parent, 'layout-check-'));
    try {
      const manifest = path.join(directory, 'Cargo.toml');
      writeFileSync(manifest, '[package]\nname = "layout-impostor"\nversion = "0.0.0"\nedition = "2024"\n');
      const rejected = spawnSync(python, command, options);
      assert.equal(rejected.status, 1, rejected.stderr);
      assert.ok(rejected.stderr.includes(`Crate outside crates/: ${path.relative(root, manifest).split(path.sep).join('/')}`), rejected.stderr);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  }
});

test('a real Cargo-accepted directory alias cannot bypass exact vendor exclusion', () => {
  const fixture = mkdtempSync(path.join(tmpdir(), 'hagency-layout-alias-'));
  const python = process.env.HAGENCY_LAYOUT_PYTHON || (process.platform === 'win32' ? 'python' : 'python3');
  const fixtureOptions = { ...options, cwd: fixture };
  const manifest = vendorPath => `[workspace]\nmembers = ["crates/hagency", "crates/hagency-app"]\nresolver = "3"\nexclude = ["vendor/mdurl"]\n[patch.crates-io]\nmdurl = { path = "${vendorPath}" }\n`;
  try {
    const files = {
      'Cargo.toml': manifest('vendor/mdurl'),
      'crates/hagency/Cargo.toml': '[package]\nname = "hagency"\nversion = "0.0.0"\nedition = "2024"\n',
      'crates/hagency/src/lib.rs': '',
      'crates/hagency-app/Cargo.toml': '[package]\nname = "hagency-app"\nversion = "0.0.0"\nedition = "2024"\ndefault-run = "hagency"\n[[bin]]\nname = "hagency"\npath = "src/main.rs"\n[dependencies]\nhagency = { path = "../hagency" }\nmdurl = "=0.3.1"\n',
      'crates/hagency-app/src/main.rs': 'fn main() {}\n',
      'vendor/mdurl/Cargo.toml': '[package]\nname = "mdurl"\nversion = "0.3.1"\nedition = "2024"\n',
      'vendor/mdurl/src/lib.rs': '',
    };
    for (const [name, contents] of Object.entries(files)) {
      const destination = path.join(fixture, name);
      mkdirSync(path.dirname(destination), { recursive: true });
      writeFileSync(destination, contents);
    }
    mkdirSync(path.join(fixture, 'tools'));
    copyFileSync(path.join(root, 'tools/check-workspace-layout.py'), path.join(fixture, 'tools/check-workspace-layout.py'));
    execFileSync('git', ['init', '-q'], fixtureOptions);
    execFileSync('git', ['add', '.'], fixtureOptions);
    execFileSync('cargo', ['generate-lockfile', '--offline'], fixtureOptions);
    assert.equal(JSON.parse(execFileSync(python, ['tools/check-workspace-layout.py'], fixtureOptions)).packages, 2);
    symlinkSync(path.join(fixture, 'vendor/mdurl'), path.join(fixture, 'vendor/alias'), process.platform === 'win32' ? 'junction' : 'dir');
    writeFileSync(path.join(fixture, 'Cargo.toml'), manifest('vendor/alias'));
    // Cargo accepts this canonical alias; the repository's stricter source
    // boundary must still reject its unexcluded lexical patch location.
    execFileSync('cargo', ['metadata', '--no-deps', '--locked', '--format-version', '1'], fixtureOptions);
    let rejected = spawnSync(python, ['tools/check-workspace-layout.py'], fixtureOptions);
    assert.equal(rejected.status, 1, rejected.stderr);
    assert.ok(rejected.stderr.includes('Vendor patch not explicitly excluded'), rejected.stderr);
    writeFileSync(path.join(fixture, 'Cargo.toml'), manifest('vendor/alias').replace('exclude = ["vendor/mdurl"]', 'exclude = ["vendor/mdurl", "vendor/alias"]'));
    rejected = spawnSync(python, ['tools/check-workspace-layout.py'], fixtureOptions);
    assert.equal(rejected.status, 1, rejected.stderr);
    assert.ok(rejected.stderr.includes('Symlinked vendor patch directory'), rejected.stderr);
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});
