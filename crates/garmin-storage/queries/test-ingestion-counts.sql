SELECT
    (SELECT count(*) FROM artifact_blobs) AS blob_count,
    (SELECT count(*) FROM artifacts) AS artifact_count,
    (SELECT count(*) FROM acquisitions) AS acquisition_count,
    (SELECT count(*) FROM observations) AS observation_count,
    (SELECT count(*) FROM association_groups) AS association_count,
    (
        SELECT count(*)
        FROM association_group_members
    ) AS association_member_count;
