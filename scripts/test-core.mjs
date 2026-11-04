// 便携/沙箱环境里系统 %TEMP% 未必可写，这里把临时目录固定到工作区 target/tmp，
// 保证 `pnpm test:core` 在任何机器上行为一致。
import { spawnSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const tmp = join(root, "target", "tmp");
mkdirSync(tmp, { recursive: true });

const result = spawnSync(
  "cargo",
  ["test", "-p", "launcher-core", ...process.argv.slice(2)],
  { cwd: root, stdio: "inherit", env: { ...process.env, TMP: tmp, TEMP: tmp } }
);

process.exit(result.status ?? 1);
