import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { runInNewContext } from 'node:vm';
import { documentationLinks, pageState } from './haos-install-links.mts';

const repository =
    'https://my.home-assistant.io/redirect/supervisor_add_addon_repository/?repository_url=https%3A%2F%2Fgithub.com%2Fkubijo%2Fgarmin-toolkit%23hass-apps';
const app = 'https://my.home-assistant.io/redirect/supervisor_app/?app=example_garmin_toolkit';
const markdown = `[hass-install]: ${repository}\n[hass-app]: ${app}\n`;
assert.deepEqual(documentationLinks(markdown), [repository, app]);
assert.equal(
    new URL(documentationLinks(markdown)[0]).searchParams.get('repository_url'),
    'https://github.com/kubijo/garmin-toolkit#hass-apps',
);
for (const invalid of [
    markdown.replace('[hass-app]:', '[different]: '),
    `${markdown}[hass-app]: ${app}\n`,
    markdown.replaceAll('https://my.home-assistant.io', 'https://unrelated.invalid'),
]) {
    assert.throws(() => documentationLinks(invalid));
}

let clicked = 0;
let revealed = 0;
const repositoryUrl = 'https://github.com/kubijo/garmin-toolkit#hass-apps';
const element = (tag: string, text: string, properties: Record<string, unknown> = {}) => ({
    tagName: tag.toUpperCase(),
    textContent: text,
    matches: (selectors: string) => selectors.split(',').some(selector => selector.trim() === tag),
    getBoundingClientRect: () => ({ width: 100 }),
    hasAttribute: () => false,
    ...properties,
});
const inspect = runInNewContext(`(${pageState.toString()})`, {
    location: { href: 'http://127.0.0.1:8123/config/apps/available' },
    document: {
        querySelectorAll: () => [
            element('ha-dialog', `Do you want to add the app repository ${repositoryUrl}?`),
            element('ha-progress-button', 'Add', { click: () => clicked++ }),
            element('ha-button', 'Install', { disabled: true }),
            element('supervisor-apps-repository', 'Garmin Toolkit', {
                repo: { source: repositoryUrl, url: 'https://github.com/kubijo/garmin-toolkit' },
                scrollIntoView: () => revealed++,
            }),
        ],
    },
}) as typeof pageState;
const page = inspect();
assert.ok(
    page.dialogs.some(text => text.includes(repositoryUrl)),
    'repository confirmation is text, not an input',
);
assert.deepEqual([...page.repositories], [repositoryUrl], 'repository identity uses its branch-bearing source');
assert.deepEqual([...page.buttons], ['Add'], 'disabled buttons are unavailable');
inspect({ click: 'Add' });
inspect({ revealRepository: repositoryUrl });
assert.equal(clicked, 1);
assert.equal(revealed, 1);
assert.throws(() => inspect({ click: 'Install' }), /unavailable/);

if (process.argv.includes('--documentation')) {
    const readme = await readFile('README.md', 'utf8');
    const quickstart = await readFile('apps/garmin-hass/addon/DOCS.md', 'utf8');
    const expected = [repository, app.replace('example_garmin_toolkit', 'af63a031_garmin_toolkit')];
    for (const document of [readme, quickstart]) {
        assert.deepEqual(documentationLinks(document), expected);
        for (const name of ['hass-install', 'hass-app']) {
            assert.match(document, new RegExp(`\\]\\[${name}\\]`), `${name} is used in the document`);
        }
    }
}
console.log('My Home Assistant documentation links: passed');
