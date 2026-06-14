ALTER TABLE backup_configs
ADD COLUMN email_to TEXT[] NOT NULL DEFAULT '{}',
ADD COLUMN email_cc TEXT[] NOT NULL DEFAULT '{}',
ADD COLUMN email_notify_on VARCHAR(20) NOT NULL DEFAULT 'never';

ALTER TABLE backup_configs
ADD CONSTRAINT backup_configs_email_notify_on_check
CHECK (email_notify_on IN ('never', 'failure', 'always'));
