import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { constants } from 'node:os';
import { resolve } from 'node:path';

const directories = new Set();

function remove(directory) {
    rmSync(directory, { recursive: true, force: true, maxRetries: 3, retryDelay: 10 });
    directories.delete(directory);
}

// Exit callbacks must be synchronous, including when a signal interrupts a test.
process.on('exit', () => {
    for (const directory of directories) {
        try {
            remove(directory);
        } catch (error) {
            console.error(`Failed to remove test directory ${directory}:`, error);
            if (!process.exitCode) process.exitCode = 1;
        }
    }
});
for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
    process.once(signal, () => process.exit(128 + constants.signals[signal]));
}

export function testDirectory(t, prefix) {
    mkdirSync('.tmp', { recursive: true });
    // Allocate and register without an await where shutdown could lose ownership.
    const directory = mkdtempSync(resolve('.tmp', prefix));
    directories.add(directory);
    t.after(() => remove(directory));
    return directory;
}
