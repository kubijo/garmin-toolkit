SELECT
    last_serial AS serial,
    (SELECT count(*) FROM artifact_blobs) AS blobs,
    (SELECT count(*) FROM artifacts) AS artifacts,
    (SELECT count(*) FROM course_generations) AS generations
FROM course_serial_counter
WHERE singleton = 1;
