import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const root = fileURLToPath(new URL('../', import.meta.url));
const args = process.argv.slice(2);
const mode = args[0] && !args[0].startsWith('-') ? args.shift() : process.platform === 'darwin' ? 'metal' : 'cpu';
if (!['metal', 'cpu'].includes(mode) || (mode === 'metal' && process.platform !== 'darwin')) {
  console.error('Choose cpu, or metal on macOS.');
  process.exit(2);
}
function run(command, argv) {
  const result = spawnSync(command, argv, { cwd: root, stdio: 'inherit' });
  if (result.error) console.error(result.error.message);
  if (result.error || result.status !== 0) process.exit(result.status || 1);
}
for (const manifest of ['src-tauri/Cargo.toml', 'src-tauri/sidecars/smooth-diarize/Cargo.toml']) {
  run('cargo', ['clean', '--manifest-path', manifest]);
}
run(process.execPath, [path.join(root, 'node_modules/@tauri-apps/cli/tauri.js'), 'build', ...args, '--', '--no-default-features', ...(mode === 'metal' ? ['--features', 'metal'] : [])]);
