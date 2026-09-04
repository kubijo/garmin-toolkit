INSERT INTO route_plan_points (
    revision_id,
    position,
    latitude_degrees,
    longitude_degrees,
    elevation_m
)
VALUES (?, ?, ?, ?, ?)
ON CONFLICT (revision_id, position) DO UPDATE SET
    revision_id = excluded.revision_id
WHERE route_plan_points.latitude_degrees = excluded.latitude_degrees
AND route_plan_points.longitude_degrees = excluded.longitude_degrees
AND route_plan_points.elevation_m IS excluded.elevation_m
RETURNING position
