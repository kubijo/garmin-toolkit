CREATE TEMP TRIGGER abort_course_generation
BEFORE INSERT ON course_generations
BEGIN
    SELECT raise(ABORT, 'injected generation write failure');
END;
