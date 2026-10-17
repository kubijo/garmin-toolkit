SELECT course_generation_deletions.owner_id FROM course_generation_deletions
INNER JOIN course_generation_operations
    ON
        course_generation_deletions.id
        = course_generation_operations.generation_id
WHERE course_generation_operations.operation_id = ?;
