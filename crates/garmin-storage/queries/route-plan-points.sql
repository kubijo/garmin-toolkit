SELECT
    latitude_degrees,
    longitude_degrees,
    elevation_m
FROM route_plan_points
WHERE revision_id = ?
ORDER BY position
