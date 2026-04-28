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

use boxxy_core_widgets::autocomplete::{CompletionItem, CompletionProvider};
use std::rc::Rc;

pub struct SidebarCommandProvider {
    pub registry: Rc<CommandRegistry>,
}

impl CompletionProvider for SidebarCommandProvider {
    fn trigger(&self) -> String {
        "/".to_string()
    }

    fn get_completions(&self, query: &str) -> Vec<CompletionItem> {
        let completions = self.registry.get_completions(&format!("/{}", query));
        completions
            .into_iter()
            .map(|cmd| CompletionItem {
                display_name: cmd.to_string(),
                replacement_text: cmd.to_string(),
                icon_name: Some("boxxy-running-symbolic".to_string()),
                icon_path: None,
                secondary_text: None,
                badge_text: None,
                badge_color: None,
                disabled: false,
            })
            .collect()
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
