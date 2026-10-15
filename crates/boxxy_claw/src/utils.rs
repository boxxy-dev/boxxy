use log::warn;

const CLAW_MD: &str = include_str!("../../../resources/prompts/claw.md");
const MEMORY_FLUSH_MD: &str = include_str!("../../../resources/prompts/memory_flush.md");
const BOOKMARK_GENERATOR_MD: &str =
    include_str!("../../../resources/prompts/bookmark_generator.md");
const AI_CHAT_MD: &str = include_str!("../../../resources/prompts/ai_chat.md");
const PRIVACY_POLICY_MD: &str = include_str!("../../../resources/prompts/privacy_policy.md");

pub fn load_prompt_fallback(_resource_path: &str, filename: &str) -> String {
    match filename {
        "claw.md" => CLAW_MD.to_string(),
        "memory_flush.md" => MEMORY_FLUSH_MD.to_string(),
        "bookmark_generator.md" => BOOKMARK_GENERATOR_MD.to_string(),
        "ai_chat.md" => AI_CHAT_MD.to_string(),
        "privacy_policy.md" => PRIVACY_POLICY_MD.to_string(),
        _ => {
            warn!("Unknown prompt file requested: {filename}");
            format!("Fallback content for {filename}")
        }
    }
}
