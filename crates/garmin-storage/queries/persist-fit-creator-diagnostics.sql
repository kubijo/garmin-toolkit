INSERT INTO fit_creator_diagnostics (
    observation_id,
    manufacturer_id,
    product_id,
    serial_number,
    product_name,
    software_version_hundredths
)
VALUES (?, ?, ?, ?, ?, ?)
ON CONFLICT DO UPDATE SET observation_id = excluded.observation_id
WHERE fit_creator_diagnostics.manufacturer_id = excluded.manufacturer_id
AND fit_creator_diagnostics.product_id IS excluded.product_id
AND fit_creator_diagnostics.serial_number IS excluded.serial_number
AND fit_creator_diagnostics.product_name IS excluded.product_name
AND fit_creator_diagnostics.software_version_hundredths
IS excluded.software_version_hundredths
RETURNING observation_id;
