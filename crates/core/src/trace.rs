//! Agent trace collection — structured telemetry for every agent turn.
//!
//! Records tool calls, durations, token usage, context fullness, and outcomes
//! without impacting agent execution. Trace data is persisted as JSON in
//! SQLite for later analysis ("Harness as Dataset").

use serde::{Deserialize, Serialize};

/// A single agent session trace (one user message → full agent response).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTrace {
    pub id: String,
    pub conversation_id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// First 200 chars of user input.
    pub user_message_preview: String,
    pub total_iterations: u32,
    pub total_tool_calls: u32,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    /// Configured context window size.
    pub context_window_size: usize,
    /// Highest context usage percentage during the session.
    pub peak_context_usage_pct: f32,
    /// How many tool definitions were sent to the LLM.
    pub tools_offered: u32,
    /// Whether the answer was served from cache.
    pub cache_hit: bool,
    /// Deterministic route selected for the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_kind: Option<String>,
    /// Typed task plan injected into the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_plan: Option<serde_json::Value>,
    /// How many times context was compacted during the session.
    pub compaction_count: u32,
    pub outcome: TraceOutcome,
    pub error_message: Option<String>,
    /// Which LLM model was used.
    pub model_id: String,
    pub steps: Vec<TraceStep>,
}

/// Outcome of an agent session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TraceOutcome {
    /// `set_done` was called — normal completion.
    Success,
    /// Hit the iteration limit.
    MaxIterations,
    /// Agent failed with an error.
    Error,
    /// User cancelled the request.
    Cancelled,
}

/// One iteration in the agent loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStep {
    pub iteration: u32,
    pub tool_name: Option<String>,
    pub tool_duration_ms: Option<u64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub context_usage_pct: f32,
    pub was_compacted: bool,
}

impl AgentTrace {
    /// Start a new trace for an agent session.
    pub fn begin(
        conversation_id: &str,
        user_message: &str,
        model_id: &str,
        context_window_size: usize,
    ) -> Self {
        let preview = if user_message.len() > 200 {
            let mut end = 200;
            while !user_message.is_char_boundary(end) {
                end -= 1;
            }
            user_message[..end].to_string()
        } else {
            user_message.to_string()
        };

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            conversation_id: conversation_id.to_string(),
            started_at: chrono::Utc::now(),
            finished_at: None,
            user_message_preview: preview,
            total_iterations: 0,
            total_tool_calls: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            context_window_size,
            peak_context_usage_pct: 0.0,
            tools_offered: 0,
            cache_hit: false,
            route_kind: None,
            task_plan: None,
            compaction_count: 0,
            outcome: TraceOutcome::Success,
            error_message: None,
            model_id: model_id.to_string(),
            steps: Vec::new(),
        }
    }

    /// Record a completed iteration step.
    pub fn add_step(&mut self, step: TraceStep) {
        if step.tool_name.is_some() {
            self.total_tool_calls += 1;
        }
        self.total_input_tokens += step.input_tokens;
        self.total_output_tokens += step.output_tokens;
        if step.context_usage_pct > self.peak_context_usage_pct {
            self.peak_context_usage_pct = step.context_usage_pct;
        }
        if step.was_compacted {
            self.compaction_count += 1;
        }
        self.total_iterations = self.total_iterations.max(step.iteration + 1);
        self.steps.push(step);
    }

    /// Finalize the trace with an outcome.
    pub fn finish(&mut self, outcome: TraceOutcome, error_message: Option<String>) {
        self.finished_at = Some(chrono::Utc::now());
        self.outcome = outcome;
        self.error_message = error_message;
    }
}

impl std::fmt::Display for TraceOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceOutcome::Success => write!(f, "success"),
            TraceOutcome::MaxIterations => write!(f, "max_iterations"),
            TraceOutcome::Error => write!(f, "error"),
            TraceOutcome::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Aggregated analytics summary across all agent traces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceSummary {
    pub total_sessions: u64,
    pub total_tool_calls: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub avg_iterations_per_session: f64,
    pub avg_tools_per_session: f64,
    pub avg_context_usage_pct: f64,
    /// Fraction of sessions that ended with `Success`.
    pub success_rate: f64,
    /// Fraction of sessions served from cache.
    pub cache_hit_rate: f64,
    /// Most-used tools with their call counts.
    pub top_tools: Vec<(String, u64)>,
    pub sessions_last_7_days: u64,
    pub tokens_last_7_days: u64,
}

impl std::str::FromStr for TraceOutcome {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "success" => Ok(TraceOutcome::Success),
            "max_iterations" => Ok(TraceOutcome::MaxIterations),
            "error" => Ok(TraceOutcome::Error),
            "cancelled" => Ok(TraceOutcome::Cancelled),
            other => Err(format!("unknown trace outcome: {other}")),
        }
    }
}
