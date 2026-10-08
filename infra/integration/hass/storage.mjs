import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';

export class Storage {
    constructor(base) {
        this.root = join(base, 'demo');
    }

    path() {
        const { active } = JSON.parse(readFileSync(join(this.root, 'storage-selection.json'), 'utf8'));
        if (active === 'Original') return join(this.root, 'storage.sqlite3');
        assert.match(active.Generation, /^[a-f0-9-]{36}$/);
        return join(this.root, 'storage-generations', active.Generation, 'storage.sqlite3');
    }

    rows(sql, ...params) {
        const db = new DatabaseSync(this.path(), { readOnly: true });
        try {
            return db
                .prepare(sql)
                .all(...params)
                .map(row => ({ ...row }));
        } finally {
            db.close();
        }
    }

    counts() {
        return this.rows(`SELECT
            (SELECT count(*) FROM users) AS profiles,
            (SELECT count(*) FROM activity_projections) AS activities,
            (SELECT count(*) FROM route_plans) AS routes,
            (SELECT count(*) FROM course_generations) AS courses`)[0];
    }

    routes(name) {
        return this.rows(
            `SELECT r.plan_id, r.id AS revision, r.sport, r.source_artifact_id
            FROM route_plan_revisions r JOIN route_plan_heads h ON h.revision_id = r.id
            WHERE r.name = ?`,
            name,
        );
    }

    courses(name) {
        return this.rows(
            `SELECT c.id, c.version, c.serial, c.artifact_id
            FROM course_generations c JOIN route_plan_revisions r ON r.id = c.revision_id
            WHERE r.name = ? ORDER BY c.version`,
            name,
        );
    }

    artifact(id) {
        const rows = this.rows(
            `SELECT b.bytes FROM artifacts a
            JOIN artifact_blobs b ON a.digest = b.digest WHERE a.id = ?`,
            id,
        );
        assert.equal(rows.length, 1, `Missing artifact ${id}`);
        return Buffer.from(rows[0].bytes);
    }

    snapshot() {
        return {
            counts: this.counts(),
            routes: this.rows('SELECT * FROM route_plan_revisions ORDER BY id'),
            courses: this.rows('SELECT * FROM course_generations ORDER BY id'),
            artifacts: this.rows('SELECT id, hex(digest) AS digest FROM artifacts ORDER BY id'),
        };
    }
}
