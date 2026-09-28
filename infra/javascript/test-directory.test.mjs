import assert from 'node:assert/strict';
import { fork } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { constants } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { testDirectory } from './test-directory.mjs';

const mode = process.argv[2];
if (mode) {
    let roots;
    test('child fixture', async t => {
        roots = [testDirectory(t, 'web-assets-test-'), testDirectory(t, 'web-assets-test-')];
        for (const root of roots) {
            mkdirSync(join(root, 'nested'));
            writeFileSync(join(root, 'nested', 'fixture'), 'test data');
        }
        process.send(roots);
        if (mode === 'failure') assert.fail('intentional fixture failure');
        if (mode === 'exit') process.exit(7);
        if (mode !== 'success') await once(process, 'message');
    });
    test('previous test already cleaned its fixtures', () => {
        assert.ok(roots.every(root => !existsSync(root)));
        process.disconnect();
    });
} else {
    for (const outcome of ['success', 'failure', 'exit', 'SIGINT', 'SIGTERM', 'SIGHUP']) {
        test(`test directories are removed after ${outcome}`, { timeout: 10_000 }, async t => {
            const sibling = testDirectory(t, 'cleanup-control-');
            const sentinel = join(sibling, 'keep');
            writeFileSync(sentinel, 'unrelated fixture');
            const child = fork(new URL(import.meta.url), [outcome], { silent: true });
            let roots = [];
            t.after(async () => {
                if (child.exitCode === null && child.signalCode === null) {
                    const stopped = once(child, 'exit');
                    child.kill('SIGKILL');
                    await stopped;
                }
                for (const root of roots) rmSync(root, { recursive: true, force: true });
            });
            let output = '';
            child.stdout.on('data', chunk => {
                output += chunk;
            });
            child.stderr.on('data', chunk => {
                output += chunk;
            });
            const exited = once(child, 'exit');
            [roots] = await once(child, 'message');
            if (outcome.startsWith('SIG')) child.kill(outcome);
            const [code, signal] = await exited;
            const expected = outcome.startsWith('SIG')
                ? 128 + constants.signals[outcome]
                : outcome === 'exit'
                  ? 7
                  : outcome === 'failure'
                    ? 1
                    : 0;
            assert.equal(signal, null, output);
            assert.equal(code, expected, output);
            assert.ok(
                roots.every(root => !existsSync(root)),
                output,
            );
            assert.ok(existsSync(sentinel), 'cleanup must preserve other processes’ fixtures');
        });
    }
}
