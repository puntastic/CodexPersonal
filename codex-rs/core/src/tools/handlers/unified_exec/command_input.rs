use super::ExecCommandArgs;
use serde_json::Value;
use serde_json::json;

pub(super) enum CommandInput<'a> {
    Script(&'a str),
    Argv(&'a [String]),
}

impl ExecCommandArgs {
    pub(super) fn input(&self) -> Result<CommandInput<'_>, String> {
        match (self.cmd.as_deref(), self.argv.as_deref()) {
            (Some(script), None) => Ok(CommandInput::Script(script)),
            (None, Some(argv)) => {
                if argv.first().is_none_or(String::is_empty) {
                    return Err("argv must contain a nonempty executable followed by its arguments".to_string());
                }
                if argv.iter().any(|arg| arg.contains('\0')) {
                    return Err("argv cannot contain NUL bytes".to_string());
                }
                if self.shell.is_some() || self.login == Some(true) {
                    return Err("shell and login:true apply to cmd, not direct argv".to_string());
                }
                Ok(CommandInput::Argv(argv))
            }
            _ => Err("provide exactly one of cmd (shell script) or argv (direct executable and arguments)".to_string()),
        }
    }

    pub(crate) fn command_for_inspection(&self) -> Result<String, String> {
        match self.input()? {
            CommandInput::Script(script) => Ok(script.to_string()),
            // Display only: execution uses the original vector, never this shell text.
            CommandInput::Argv(argv) => shlex::try_join(argv.iter().map(String::as_str))
                .map_err(|err| err.to_string()),
        }
    }

    pub(super) fn hook_input(&self) -> Result<Value, String> {
        let mut input = json!({ "command": self.command_for_inspection()? });
        if let CommandInput::Argv(argv) = self.input()? {
            input["argv"] = json!(argv);
        }
        Ok(input)
    }

    pub(super) fn rewrite_argv_hook_input(
        &self,
        arguments: &str,
        updated_input: &Value,
    ) -> Result<String, String> {
        let argv = updated_input.get("argv").ok_or_else(|| {
            "a direct argv hook rewrite must provide argv; command text alone cannot rewrite direct execution".to_string()
        })?;
        let argv: Vec<String> = serde_json::from_value(argv.clone()).map_err(|err| err.to_string())?;
        if self.argv.as_ref() == Some(&argv)
            && let Some(command) = updated_input.get("command")
            && command != &json!(self.command_for_inspection()?)
        {
            return Err("hook changed command text without changing argv; no command was executed".to_string());
        }
        let mut rewritten: Value = serde_json::from_str(arguments).map_err(|err| err.to_string())?;
        rewritten["argv"] = json!(argv);
        let checked: ExecCommandArgs = serde_json::from_value(rewritten.clone()).map_err(|err| err.to_string())?;
        checked.input()?;
        serde_json::to_string(&rewritten).map_err(|err| err.to_string())
    }
}

#[cfg(test)]
#[path = "command_input_tests.rs"]
mod tests;
