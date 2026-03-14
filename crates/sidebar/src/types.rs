use rig::message::Message;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn to_rig_message(&self) -> Message {
        match self.role {
            Role::User | Role::System => Message::user(&self.content),
            Role::Assistant => Message::assistant(&self.content),
        }
    }
}
