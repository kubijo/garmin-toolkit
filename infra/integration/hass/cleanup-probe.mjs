import assert from 'node:assert/strict';
import { access } from 'node:fs/promises';
import { Fixture, listening } from './fixture.mjs';

const instance = new Fixture('intentional-failure');
let injected = false;
try {
    await instance.start();
    injected = true;
    assert.fail('Intentional failure after host and browser startup');
} finally {
    await instance.dispose();
    assert.equal(await listening(), false);
    await assert.rejects(access(instance.work), { code: 'ENOENT' });
    if (injected) console.log('INTENTIONAL_FAILURE_CLEANUP_VERIFIED');
}
