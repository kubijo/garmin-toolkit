export async function readOrMissing<T>(read: () => Promise<T>, missing: T): Promise<T> {
    try {
        return await read();
    } catch (error: unknown) {
        if (typeof error === 'object' && error !== null && 'code' in error && error.code === 'ENOENT') {
            return missing;
        }
        throw error;
    }
}
