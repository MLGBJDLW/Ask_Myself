const DISABLE_LLM_STREAMING_ENV: &str = "NEXA_DISABLE_LLM_STREAMING";
const LEGACY_DISABLE_LLM_STREAMING_ENV: &str = "ASK_MYSELF_DISABLE_LLM_STREAMING";

fn env_flag_enabled(name: &str) -> bool {
    let Ok(value) = std::env::var(name) else {
        return false;
    };
    let normalized = value.trim().to_ascii_lowercase();
    !normalized.is_empty()
        && !matches!(
            normalized.as_str(),
            "0" | "false" | "off" | "no" | "disabled"
        )
}

pub fn llm_streaming_disabled_by_env() -> bool {
    env_flag_enabled(DISABLE_LLM_STREAMING_ENV)
        || env_flag_enabled(LEGACY_DISABLE_LLM_STREAMING_ENV)
}
