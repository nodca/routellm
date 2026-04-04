ALTER TABLE channels
ADD COLUMN protocol TEXT NOT NULL DEFAULT 'responses';

UPDATE model_routes
SET routing_strategy = 'priority'
WHERE routing_strategy = 'weighted';
