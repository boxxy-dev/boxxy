use crate::component::AiSidebarComponent;

pub trait ChatCommand {
    fn name(&self) -> &'static str;
    fn execute(&self, chat: &AiSidebarComponent, args: &str);
}

pub struct ClearCommand;

impl ChatCommand for ClearCommand {
    fn name(&self) -> &'static str {
        "/clear"
    }

    fn execute(&self, chat: &AiSidebarComponent, _args: &str) {
        chat.clear_history();
    }
}

pub struct ModelCommand;

impl ChatCommand for ModelCommand {
    fn name(&self) -> &'static str {
        "/model"
    }

    fn execute(&self, chat: &AiSidebarComponent, _args: &str) {
        chat.show_model_selector();
    }
}

pub struct CommandRegistry {
    pub(crate) commands: Vec<Box<dyn ChatCommand>>,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: vec![Box::new(ClearCommand), Box::new(ModelCommand)],
        }
    }

    pub fn get_completions(&self, prefix: &str) -> Vec<&'static str> {
        self.commands
            .iter()
            .map(|c| c.name())
            .filter(|name| name.starts_with(prefix))
            .collect()
    }

    pub fn handle(&self, cmd_string: &str, chat: &AiSidebarComponent) -> bool {
        let mut parts = cmd_string.trim().splitn(2, ' ');
        let cmd_name = parts.next().unwrap_or("");
        let args = parts.next().unwrap_or("");

        for cmd in &self.commands {
            if cmd.name() == cmd_name {
                cmd.execute(chat, args);
                return true;
            }
        }
        false
    }
}
