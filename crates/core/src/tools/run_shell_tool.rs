//! RunShellTool — execute argv-style commands with a configurable shell access
//! policy.
//!
//! The model-facing contract and validation constants live in
//! `run_shell_contract`; this module implements that contract. Keep any rule
//! that affects prompts, schema, recoverable errors, or validation in the
//! contract module first, then consume it here.

mod environment;
mod file_tracking;
mod native_fs;
mod parser;
mod policy;
mod shell_adapter;
mod tool_impl;

pub use tool_impl::RunShellTool;

pub(crate) fn uses_managed_background(parsed_args: &serde_json::Value) -> bool {
    let Ok(parsed) = serde_json::from_value::<parser::RunShellArgs>(parsed_args.clone()) else {
        return false;
    };
    if !parsed
        .service_action
        .as_deref()
        .unwrap_or("run")
        .trim()
        .eq_ignore_ascii_case("run")
    {
        return false;
    }
    if parsed.background
        || parsed
            .ready_url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty())
    {
        return true;
    }

    let invocation = if let Some(program) = parsed.program.as_deref() {
        Some((program.to_string(), parsed.args))
    } else if let Some(command) = parsed.command.as_deref() {
        parser::split_simple_command_string(command)
            .ok()
            .and_then(|parts| {
                let (program, args) = parts.split_first()?;
                Some((program.clone(), args.to_vec()))
            })
    } else {
        None
    };

    invocation
        .is_some_and(|(program, args)| tool_impl::looks_like_persistent_service(&program, &args))
}

#[cfg(test)]
use std::path::{Path, PathBuf};

#[cfg(test)]
use std::time::{Duration, Instant};

#[cfg(test)]
use super::run_shell_contract::{
    DEFAULT_TIMEOUT_SECS, MAX_OUTPUT_BYTES, MAX_SINGLE_ARG_BYTES, MAX_STDIN_BYTES,
};
#[cfg(test)]
use super::Tool;
#[cfg(test)]
use crate::app_settings::ShellAccessMode;
#[cfg(test)]
use crate::db::Database;
#[cfg(test)]
use crate::execution_environment::{ExecutionDecisionKind, ExecutionEnvironment, ExecutionRequest};
#[cfg(test)]
use crate::models::Source;
#[cfg(test)]
use environment::LocalRunShellExecutionEnvironment;
#[cfg(test)]
use native_fs::execute_native_filesystem;
#[cfg(test)]
use parser::parse_run_shell_args;
#[cfg(test)]
use policy::{
    collect_positional_args, normalize_run_shell_invocation, validate_args, validate_program,
    validate_scoped_args, validate_stdin,
};
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use shell_adapter::{
    build_env_from, bytes_to_clamped_string, clamp_timeout, execute_inner, format_output,
    resolve_program, RunShellOutput,
};
#[cfg(test)]
use tool_impl::looks_like_persistent_service;

#[cfg(test)]
mod tests;
