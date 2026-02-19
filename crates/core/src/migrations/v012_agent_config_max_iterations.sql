-- Add max_iterations column to agent_configs for configurable tool iteration limit.
ALTER TABLE agent_configs ADD COLUMN max_iterations INTEGER;
