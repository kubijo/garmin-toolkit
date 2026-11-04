WITH RECURSIVE related (id) AS (
    SELECT observations.id
    FROM observations
    INNER JOIN observations AS selected
        ON observations.owner_id = selected.owner_id
    WHERE
        selected.owner_id = ?1
        AND selected.id = ?2
        AND selected.kind = 'activity'
        AND observations.kind = 'activity'
        AND observations.fingerprint_schema_name
        = selected.fingerprint_schema_name
        AND observations.fingerprint_schema_version
        = selected.fingerprint_schema_version
        AND observations.fingerprint_digest = selected.fingerprint_digest
    UNION
    SELECT sibling.observation_id
    FROM related
    INNER JOIN
        association_group_members AS member
        ON related.id = member.observation_id
    INNER JOIN association_group_members AS sibling
        ON member.association_group_id = sibling.association_group_id
    WHERE sibling.owner_id = ?1 AND sibling.kind = 'activity'
)

SELECT
    observations.id AS observation_id,
    observations.normalization_run_id,
    observations.fingerprint_schema_name,
    observations.fingerprint_schema_version,
    observations.fingerprint_digest,
    normalization_runs.acquisition_id,
    normalization_runs.artifact_id,
    normalization_runs.parser_name,
    normalization_runs.parser_version,
    normalization_runs.schema_version,
    acquisitions.operation_id,
    acquisitions.source_id,
    acquisitions.source_identity,
    acquisitions.acquired_at,
    sources.label AS source_label,
    sources.device_id,
    artifacts.media_type,
    artifact_blobs.digest,
    artifact_blobs.byte_count,
    artifact_blobs.bytes
FROM related
INNER JOIN observations ON related.id = observations.id
INNER JOIN
    normalization_runs
    ON observations.normalization_run_id = normalization_runs.id
INNER JOIN acquisitions ON normalization_runs.acquisition_id = acquisitions.id
INNER JOIN sources ON acquisitions.source_id = sources.id
INNER JOIN artifacts ON normalization_runs.artifact_id = artifacts.id
INNER JOIN artifact_blobs ON artifacts.digest = artifact_blobs.digest
WHERE observations.owner_id = ?1 AND observations.kind = 'activity'
ORDER BY observations.id;
