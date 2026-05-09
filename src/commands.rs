#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlashCommand {
    Auth,
    Clear,
    Exit,
    Model,
}

impl SlashCommand {
    pub fn all() -> &'static [SlashCommand] {
        &[Self::Model, Self::Auth, Self::Clear, Self::Exit]
    }

    pub fn command(self) -> &'static str {
        match self {
            Self::Auth  => "auth",
            Self::Clear => "clear",
            Self::Exit  => "exit",
            Self::Model => "model",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Auth  => "set up an api key",
            Self::Clear => "clear the conversation",
            Self::Exit  => "exit chatter",
            Self::Model => "choose a model",
        }
    }
}

pub enum CommandError {
    Empty,
    Unknown(String),
}

pub fn parse_slash_command(input: &str) -> Result<SlashCommand, CommandError> {
    let name = input.split_whitespace().next().unwrap_or_default();
    if name.is_empty() {
        return Err(CommandError::Empty);
    }

    match name {
        "auth" => Ok(SlashCommand::Auth),
        "clear" => Ok(SlashCommand::Clear),
        "exit" | "quit" => Ok(SlashCommand::Exit),
        "model" => Ok(SlashCommand::Model),
        other => Err(CommandError::Unknown(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auth_command() {
        assert!(matches!(
            parse_slash_command("auth"),
            Ok(SlashCommand::Auth)
        ));
        assert!(matches!(
            parse_slash_command("auth extra args"),
            Ok(SlashCommand::Auth)
        ));
    }

    #[test]
    fn parses_exit_aliases() {
        assert!(matches!(
            parse_slash_command("exit"),
            Ok(SlashCommand::Exit)
        ));
        assert!(matches!(
            parse_slash_command("quit now"),
            Ok(SlashCommand::Exit)
        ));
    }

    #[test]
    fn parses_model_command() {
        assert!(matches!(
            parse_slash_command("model"),
            Ok(SlashCommand::Model)
        ));
        assert!(matches!(
            parse_slash_command("model extra args"),
            Ok(SlashCommand::Model)
        ));
    }

    #[test]
    fn parses_clear_command() {
        assert!(matches!(
            parse_slash_command("clear"),
            Ok(SlashCommand::Clear)
        ));
        assert!(matches!(
            parse_slash_command("clear ignored args"),
            Ok(SlashCommand::Clear)
        ));
    }

    #[test]
    fn rejects_empty_command() {
        assert!(matches!(
            parse_slash_command("   "),
            Err(CommandError::Empty)
        ));
    }

    #[test]
    fn reports_unknown_command_name() {
        match parse_slash_command("bogus arg") {
            Err(CommandError::Unknown(name)) => assert_eq!(name, "bogus"),
            _ => panic!("expected unknown command"),
        }
    }
}
