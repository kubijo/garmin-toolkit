SELECT
    (SELECT count(*) FROM sources) AS sources,
    (SELECT count(*) FROM artifact_blobs) AS blobs,
    (SELECT count(*) FROM artifacts) AS artifacts,
    (SELECT count(*) FROM acquisitions) AS acquisitions,
    (SELECT count(*) FROM route_plans) AS plans,
    (SELECT count(*) FROM route_plan_revisions) AS revisions,
    (SELECT count(*) FROM route_plan_heads) AS heads,
    (SELECT count(*) FROM route_import_receipts) AS receipts;
