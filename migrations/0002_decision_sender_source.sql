-- Which Source produced the Sender category; type_signal covers the Email type.
ALTER TABLE decision ADD COLUMN sender_source TEXT;
