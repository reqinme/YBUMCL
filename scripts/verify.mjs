// 工程门禁：界面检查 → 前端构建 → Rust 检查。
// 用法：pnpm verify [--skip-build] [--skip-rust]
// 任何一步失败立即退出并返回非零码。
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { homedir, platform } from 'node:os';
import { join } from 'node:path';

const args = new Set(process.argv.slice(2));
const isWindows = platform() === 'win32';
const started = Date.now();
const steps = [];

function run(label, command, commandArgs) {
  console.log(`\n=== ${label} ===`);
  const begin = Date.now();
  const result = spawnSync(command, commandArgs, {
    stdio: 'inherit',
    shell: isWindows,
    env: process.env
  });
  const seconds = ((Date.now() - begin) / 1000).toFixed(1);
  if (result.error) {
    console.error(`✗ ${label} 无法启动：${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    console.error(`✗ ${label} 失败（exit ${result.status}，${seconds}s）`);
    process.exit(result.status ?? 1);
  }
  steps.push(`${label} ${seconds}s`);
  console.log(`✓ ${label}（${seconds}s）`);
}

// 1. Svelte 类型与语法检查
run('svelte-kit sync', 'pnpm', ['exec', 'svelte-kit', 'sync']);
run('svelte-check', 'pnpm', ['exec', 'svelte-check', '--tsconfig', './tsconfig.json']);

// 2. 前端构建：确保 .svelte 改动真的能进 exe
if (!args.has('--skip-build')) {
  run('vite build', 'pnpm', ['exec', 'vite', 'build']);
}

// 3. Rust workspace 检查（没有工具链时明确跳过，而不是静默通过）
if (!args.has('--skip-rust')) {
  const cargo = findCargo();
  if (!cargo) {
    console.warn('\n⚠ 未找到 cargo，跳过 Rust 检查（安装工具链后重跑 pnpm verify）');
  } else {
    run('cargo check', cargo, ['check', '--workspace']);
  }
}

console.log(`\n全部通过（${((Date.now() - started) / 1000).toFixed(1)}s）：${steps.join(' · ')}`);

function findCargo() {
  const candidates = isWindows
    ? [join(homedir(), '.cargo', 'bin', 'cargo.exe'), join(homedir(), '.cargo', 'bin', 'cargo')]
    : [join(homedir(), '.cargo', 'bin', 'cargo')];
  for (const candidate of candidates) {
    if (existsSync(candidate)) return candidate;
  }
  const probe = spawnSync('cargo', ['--version'], { shell: isWindows, stdio: 'ignore' });
  return probe.status === 0 ? 'cargo' : null;
}
