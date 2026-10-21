// Home Assistant's repository and app-store screens have no Garmin semantic controls.
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import type { DevTools } from './haos-devtools.mts';
import type { ReadySession, Until } from './haos-types.mts';

export function documentationLinks(markdown: string): [string, string] {
    const link = (name: string) => {
        const matches = [...markdown.matchAll(new RegExp(`^\\[${name}\\]:\\s+(\\S+)$`, 'gm'))];
        assert.equal(matches.length, 1, `one ${name} link is documented`);
        const value = matches[0]?.[1];
        assert.ok(value, `${name} has a destination`);
        const url = new URL(value);
        assert.equal(url.origin, 'https://my.home-assistant.io');
        return url.href;
    };
    return [link('hass-install'), link('hass-app')];
}

export function pageState(action?: { click?: string; revealRepository?: string }) {
    function collect(root: Document | ShadowRoot): Element[] {
        return [...root.querySelectorAll('*')].flatMap(element => {
            const child = element.tagName === 'IFRAME' ? (element as HTMLIFrameElement).contentDocument : null;
            return [
                element,
                ...(element.shadowRoot ? collect(element.shadowRoot) : []),
                ...(child ? collect(child) : []),
            ];
        });
    }
    const elements = collect(document);
    const repositories = elements.filter(
        element =>
            'repo' in element && typeof element.repo === 'object' && element.repo !== null && 'source' in element.repo,
    ) as (HTMLElement & { repo: { source: string } })[];
    const buttons = elements.filter(
        element =>
            element.matches('button, ha-button, ha-progress-button, mwc-button') &&
            element.getBoundingClientRect().width > 0 &&
            !element.hasAttribute('disabled') &&
            !('disabled' in element && element.disabled),
    ) as HTMLElement[];
    if (action?.click) {
        const button = buttons.find(element => element.textContent?.trim() === action.click);
        if (!button) throw Error(`Visible ${action.click} button is unavailable`);
        button.click();
    }
    if (action?.revealRepository) {
        const repository = repositories.find(element => element.repo.source === action.revealRepository);
        if (!repository) throw Error('Published repository card is unavailable');
        repository.scrollIntoView({ block: 'center' });
    }
    return {
        url: location.href,
        buttons: buttons.map(element => element.textContent?.trim()),
        repositories: repositories.map(element => element.repo.source),
        dialogs: elements
            .filter(element => element.matches('ha-dialog, dialog'))
            .map(element => element.textContent ?? ''),
        garminName: elements.some(element => element.textContent?.trim() === 'Garmin Toolkit'),
        app: elements.some(
            element =>
                element.id === 'garmin-toolkit' &&
                element.tagName === 'CANVAS' &&
                (element as HTMLCanvasElement).width > 0 &&
                (element as HTMLCanvasElement).height > 0,
        ),
    };
}

export async function verifyInstallLinks(client: DevTools, state: ReadySession, evidencePath: string, until: Until) {
    assert.ok(state.repository_url, 'the repository URL is available');
    const repositoryUrl = state.repository_url;
    const links = documentationLinks(await readFile(resolve('README.md'), 'utf8'));
    assert.deepEqual(links, documentationLinks(await readFile(resolve('apps/garmin-hass/addon/DOCS.md'), 'utf8')));
    const repository = new URL(links[0]);
    assert.equal(repository.pathname, '/redirect/supervisor_add_addon_repository/');
    assert.equal(repository.searchParams.get('repository_url'), state.repository_url);
    const app = new URL(links[1]);
    assert.equal(app.pathname, '/redirect/supervisor_app/');
    assert.equal(app.searchParams.get('app'), state.app_slug);

    await client.call('Emulation.setAutoDarkModeOverride', { enabled: true });
    await client.call('Page.navigate', { url: links[0] });
    await until(
        () =>
            client.evaluate(base => {
                if (location.origin !== 'https://my.home-assistant.io') return false;
                localStorage.setItem('hassUrl', base);
                return true;
            }, state.base_url),
        'owned My Home Assistant instance preference',
    );
    for (const [index, url] of links.entries()) {
        await client.call('Emulation.setAutoDarkModeOverride', { enabled: true });
        await client.call('Page.navigate', { url });
        const destination = await until(
            () => client.evaluate(() => document.querySelector<HTMLAnchorElement>('a.open-link')?.href),
            'My Home Assistant open-instance link',
        );
        assert.equal(new URL(destination).origin, state.base_url);
        if (index === 0) await client.capture('install-redirect.png');
        await client.call('Emulation.setAutoDarkModeOverride', { enabled: false });
        await client.evaluate(() => {
            const link = document.querySelector<HTMLAnchorElement>('a.open-link');
            if (!link) throw Error('My Home Assistant open-instance link is unavailable');
            link.click();
        });
        await until(
            async () => {
                const page = await client.evaluate(pageState);
                if (new URL(page.url).origin !== state.base_url) return false;
                return index === 0
                    ? page.dialogs.some(text => text.includes(repositoryUrl)) && page.buttons.includes('Add')
                    : new URL(page.url).pathname.includes(state.app_slug) && page.garminName;
            },
            index === 0 ? 'repository confirmation' : 'published Garmin Toolkit app page',
        );
        await client.capture(index === 0 ? 'install-repository.png' : 'install-app.png');
        if (index === 0) {
            await until(() => client.evaluate(pageState, { click: 'Add' }), 'confirm public repository');
            await until(
                async () => (await client.evaluate(pageState)).repositories.includes(repositoryUrl),
                'added repository',
            );
            console.log('Documentation repository link: confirmed and added.');
        } else {
            await until(() => client.evaluate(pageState, { click: 'Install' }), 'install published app');
            await until(
                async () => (await client.evaluate(pageState)).buttons.includes('Start'),
                'installed app',
                180_000,
            );
            await client.evaluate(pageState, { click: 'Start' });
            await until(
                async () => (await client.evaluate(pageState)).buttons.includes('Stop'),
                'started app',
                120_000,
            );
            await client.capture('install-started.png');
            console.log('Documentation app link: installed and started.');
        }
    }
    await client.call('Page.navigate', { url: links[0] });
    const existingDestination = await until(
        () => client.evaluate(() => document.querySelector<HTMLAnchorElement>('a.open-link')?.href),
        'existing-repository redirect',
    );
    assert.equal(new URL(existingDestination).origin, state.base_url);
    await client.evaluate(() => document.querySelector<HTMLAnchorElement>('a.open-link')?.click());
    await until(() => client.evaluate(pageState, { revealRepository: repositoryUrl }), 'existing public repository');
    await client.capture('install-existing-repository.png');
    await client.call('Page.navigate', { url: state.url });
    await until(async () => (await client.evaluate(pageState)).app, 'ingress after installation link checks');
    await writeFile(
        resolve(evidencePath, 'install-links.json'),
        JSON.stringify({ passed: true, links, installed: true, started: true, existing_repository: true }),
    );
    console.log(
        'README and quickstart links add the public repository, install/start the app, and reopen an existing repository.',
    );
}
