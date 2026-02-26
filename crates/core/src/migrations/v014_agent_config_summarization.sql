-- Add optional summarization model/provider overrides to agent_configs.
-- When set, summarization uses a cheaper model instead of the main model.
ALTER TABLE agent_configs ADD COLUMN summarization_model TEXT DEFAULT NULL;
ALTER TABLE agent_configs ADD COLUMN summarization_provider TEXT DEFAULT NULL;
