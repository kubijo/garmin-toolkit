INSERT INTO route_import_receipts (
    acquisition_id, artifact_id, owner_id, plan_id, revision_id,
    candidate_kind, candidate_index, segment_index, parser_name, parser_version
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?);
