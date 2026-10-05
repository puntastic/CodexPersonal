use crate::sandboxing::SandboxPermissions;
use crate::shell::Shell;
use crate::shell::ShellType;
use crate::shell::get_shell;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::hook_names::HookToolName;
use crate::tools::registry::PostToolUsePayload;
use codex_exec_server::Environment;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_shell_command::shell_detect::detect_shell_type;
use codex_tools::UnifiedExecShellMode;
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;

#[cfg(test)]
use crate::tools::handlers::parse_arguments;

mod exec_command;
mod command_input;
mod write_stdin;

use command_input::CommandInput;

pub use exec_command::ExecCommandHandler;
pub(crate) use exec_command::ExecCommandHandlerOptions;
pub use write_stdin::WriteStdinHandler;

#[derive(Debug, Deserialize)]
pub(crate) struct ExecCommandArgs {
    #[serde(default)]
    pub(crate) cmd: Option<String>,
    #[serde(default)]
    argv: Option<Vec<String>>,
    #[serde(default)]
    shell: Option<String>,
    #[serde(default)]
    login: Option<bool>,
    #[serde(default = "default_tty")]
    tty: bool,
    #[serde(default = "default_exec_yield_time_ms")]
    yield_time_ms: u64,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
    #[serde(default)]
    sandbox_permissions: Option<SandboxPermissions>,
    #[serde(default)]
    additional_permissions: Option<AdditionalPermissionProfile>,
    #[serde(default)]
    justification: Option<String>,
    #[serde(default)]
    prefix_rule: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ExecCommandEnvironmentArgs {
    #[serde(default)]
    environment_id: Option<String>,
    // Keep this raw until after environment selection; relative paths must be
    // resolved against the selected environment cwd, not the process cwd.
    #[serde(default)]
    workdir: Option<String>,
}

fn default_exec_yield_time_ms() -> u64 {
    10_000
}

fn default_write_stdin_yield_time_ms() -> u64 {
    250
}

fn default_tty() -> bool {
    false
}

#[derive(Debug)]
pub(crate) struct ResolvedCommand {
    pub(crate) command: Vec<String>,
    pub(crate) shell_type: Option<ShellType>,
}

fn post_unified_exec_tool_use_payload(
    invocation: &ToolInvocation,
    result: &dyn ToolOutput,
) -> Option<PostToolUsePayload> {
    let ToolPayload::Function { .. } = &invocation.payload else {
        return None;
    };

    let tool_input = result.post_tool_use_input(&invocation.payload)?;
    let tool_use_id = result.post_tool_use_id(&invocation.call_id);
    let tool_response = result.post_tool_use_response(&tool_use_id, &invocation.payload)?;
    Some(PostToolUsePayload {
        tool_name: HookToolName::bash(),
        tool_use_id,
        tool_input,
        tool_response,
    })
}

pub(crate) fn get_command(
    args: &ExecCommandArgs,
    session_shell: Arc<Shell>,
    shell_mode: &UnifiedExecShellMode,
    allow_login_shell: bool,
) -> Result<ResolvedCommand, String> {
    let script = match args.input()? {
        CommandInput::Argv(argv) => {
            if !matches!(shell_mode, UnifiedExecShellMode::Direct) {
                return Err("direct argv is unavailable with zsh-fork; use cmd for that execution mode".to_string());
            }
            return Ok(ResolvedCommand {
                command: argv.to_vec(),
                shell_type: None,
            });
        }
        CommandInput::Script(script) => script,
    };
    let use_login_shell = match args.login {
        Some(true) if !allow_login_shell => {
            return Err(
                "login shell is disabled by config; omit `login` or set it to false.".to_string(),
            );
        }
        Some(use_login_shell) => use_login_shell,
        None => allow_login_shell,
    };

    match shell_mode {
        UnifiedExecShellMode::Direct => {
            let model_shell = args
                .shell
                .as_ref()
                .map(|shell_str| {
                    let shell_type = detect_shell_type(Path::new(shell_str)).ok_or_else(|| {
                        "unsupported requested shell type; supported types are bash, zsh, sh, powershell and cmd".to_string()
                    })?;
                    get_shell(shell_type).ok_or_else(|| {
                        format!("requested {} shell is unavailable; no command was executed", shell_type.name())
                    })
                })
                .transpose()?;
            let shell = model_shell.as_ref().unwrap_or(session_shell.as_ref());
            Ok(ResolvedCommand {
                command: shell.derive_exec_args(script, use_login_shell),
                shell_type: Some(shell.shell_type),
            })
        }
        UnifiedExecShellMode::ZshFork(zsh_fork_config) => {
            if args.shell.is_some() {
                return Err(
                    "`shell` is not supported for local zsh-fork exec; omit `shell` to use zsh-fork, or target a remote environment where `shell` is supported.".to_string(),
                );
            }

            Ok(ResolvedCommand {
                command: vec![
                    zsh_fork_config.shell_zsh_path.to_string_lossy().to_string(),
                    if use_login_shell { "-lc" } else { "-c" }.to_string(),
                    script.to_string(),
                ],
                shell_type: Some(ShellType::Zsh),
            })
        }
    }
}

pub(crate) fn shell_mode_for_environment(
    turn_shell_mode: &UnifiedExecShellMode,
    environment: &Environment,
) -> UnifiedExecShellMode {
    if environment.is_remote() {
        UnifiedExecShellMode::Direct
    } else {
        turn_shell_mode.clone()
    }
}

#[cfg(test)]
#[path = "unified_exec_tests.rs"]
mod tests;
