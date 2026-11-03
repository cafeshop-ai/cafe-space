// Preserve a separately compiled, behavior-compatible daemon only on a disposable hosted runner.
import assert from 'node:assert/strict';
import { constants } from 'node:fs';
import { appendFile, copyFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
assert.equal(process.env.GITHUB_ACTIONS, 'true');
assert.equal(process.env.RUNNER_ENVIRONMENT, 'github-hosted');
assert.ok(process.env.GITHUB_ENV && process.env.RUNNER_TEMP);
const suffix = process.platform === 'win32' ? '.exe' : '';
const source = fileURLToPath(new URL(`../../target/debug/cafe-spaced-dev${suffix}`, import.meta.url));
const target = path.join(process.env.RUNNER_TEMP, `cafe-space-m0-old${suffix}`);
assert.ok(path.isAbsolute(target) && !/[\r\n]/.test(target));
await copyFile(source, target, constants.COPYFILE_EXCL);
await appendFile(process.env.GITHUB_ENV, `CAFE_SPACE_M0_OLD_BINARY=${target}\n`, 'utf8');
