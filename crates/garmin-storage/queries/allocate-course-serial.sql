UPDATE course_serial_counter
SET last_serial = last_serial + 1
WHERE singleton = 1 AND last_serial < 4294967295
RETURNING last_serial;
