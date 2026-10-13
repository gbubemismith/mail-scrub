-- Nothing reads these; the Client supplies them again on any Full Run.
ALTER TABLE message DROP COLUMN thread_id;
ALTER TABLE message DROP COLUMN internal_date;
