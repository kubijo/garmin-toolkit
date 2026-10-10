CREATE TRIGGER abort_route_import
BEFORE INSERT ON route_import_receipts
BEGIN
    SELECT raise(ABORT, 'injected import receipt failure');
END;
