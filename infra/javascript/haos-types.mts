export interface Session {
    phase: string;
    base_url: string;
    browser_temporary: string;
    workflow: boolean;
    install_links: boolean;
    repository_url?: string;
}

export interface ReadySession extends Session {
    phase: 'verifying';
    url: string;
    app_slug: string;
    tokens: unknown;
}

export type Until = <T>(
    check: () => T | Promise<T>,
    label: string,
    timeout?: number,
) => Promise<NonNullable<Exclude<T, false | 0 | ''>>>;

export interface Clip {
    x: number;
    y: number;
    width: number;
    height: number;
}

export type BrowserFunction<A, R> = (argument: A) => R;

export type Action = { target: string } & (
    | { kind: 'click' | 'wait' }
    | { kind: 'scroll'; delta: number }
    | { kind: 'text'; text: string; click_first?: boolean }
    | { kind: 'key'; key: string; modifier?: 'ctrl' | 'shift' | 'alt' }
);

export interface Target {
    id: string;
    label: string;
    value?: string;
    enabled: boolean;
    bounds: [number, number, number, number] | null;
}

interface Report {
    state: string;
    completed: number;
    total: number;
    needs_background_frames: boolean;
}

declare global {
    interface Window {
        garminAutomation?: {
            targets(): Target[];
            sequence(actions: Action[], options: { run_in_background: boolean }): Report | null;
            status(): Report | null;
        };
        garminWindowControl?: {
            capture(timeout: number): Promise<[string, Uint8Array]>;
        };
    }
}
