use boxxy_claw_protocol::characters::{CHARACTER_CACHE, CLAIMS_CACHE, CharacterInfo, HolderKind};
use std::collections::VecDeque;

/// Maximum number of autonomous character-to-character turns allowed before
/// automatically pausing for user input.
pub const MAX_ROUNDTABLE_ROUNDS: u8 = 20;

/// Marker a character can emit at the end of its response when a debate/discussion
/// has reached a natural conclusion or requires user input to proceed.
pub const YIELD_TO_USER_MARKER: &str = "[YIELD_TO_USER]";

/// Identity of the character executing a specific turn in a pane's ClawSession.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeakerIdentity {
    pub character_id: String,
    pub display_name: String,
    pub slug: String,
    pub is_guest: bool,
    /// If this character is currently claimed by another live terminal pane in the swarm,
    /// holds that pane's ID so we can bridge its live CWD, status, and terminal snapshot.
    pub home_pane_id: Option<String>,
}

/// Parsed roundtable intent from a user's message in the overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserRoundtableIntent {
    /// User explicitly asked to stop the ongoing multi-character conversation/debate.
    Stop,
    /// User invoked `/drama` with no arguments to toggle Visual Novel Drama Mode ON/OFF.
    ToggleDrama,
    /// User invoked `/drama [@char1] [@char2] [topic]` to start a Visual Novel Drama Mode chat.
    Drama {
        guests: Vec<SpeakerIdentity>,
        topic: String,
    },
    /// User invoked `/debate` (with optional `@character` mentions and a topic).
    Debate {
        guests: Vec<SpeakerIdentity>,
        topic: String,
    },
    /// User mentioned one or more `@character`s in a regular message.
    Mention {
        mentioned: Vec<SpeakerIdentity>,
        /// True if the prompt asks characters to converse/debate with each other
        /// or mentions multiple characters.
        multi_turn: bool,
        prompt: String,
    },
    /// Standard message with no new `@character` mentions or `/debate` command.
    None,
}

/// Per-session state tracking active multi-character roundtable / debate turns.
#[derive(Debug, Clone, Default)]
pub struct RoundtableState {
    /// Ordered roster of participants (Host + invited Guests) in the current roundtable.
    pub participants: Vec<SpeakerIdentity>,
    /// Queue of speakers scheduled to take the next turn(s).
    pub speaker_queue: VecDeque<SpeakerIdentity>,
    /// Remaining autonomous character-to-character turns (capped at `MAX_ROUNDTABLE_ROUNDS`).
    pub remaining_rounds: u8,
    /// True when characters should automatically alternate turns (e.g. `/debate` or "talk to each other").
    pub auto_alternate: bool,
    /// True when Visual Novel Drama Mode is enabled (1-2 sentence casual spoken dialogue + typewriter effect).
    pub drama_mode: bool,
    /// The original user prompt or debate topic that started the current roundtable.
    pub topic: String,
    /// Last character ID that spoke, used to pick the next participant in `auto_alternate` mode.
    pub last_speaker_id: Option<String>,
}

impl RoundtableState {
    pub fn is_active(&self) -> bool {
        self.remaining_rounds > 0 && (!self.speaker_queue.is_empty() || self.auto_alternate)
    }

    pub fn clear(&mut self) {
        let keep_drama = self.drama_mode;
        self.participants.clear();
        self.speaker_queue.clear();
        self.remaining_rounds = 0;
        self.auto_alternate = false;
        self.drama_mode = keep_drama;
        self.topic.clear();
        self.last_speaker_id = None;
    }

    /// Pauses autonomous turn generation when the user clicks the Cancel (`×`) button,
    /// while preserving `participants` and `auto_alternate` so a follow-up user message
    /// (like `"btw ..."`) seamlessly resumes the roundtable with all participants!
    pub fn pause_for_interjection(&mut self) {
        self.speaker_queue.clear();
        self.remaining_rounds = 0;
    }

    /// Resumes an active roundtable after the user interjects a follow-up message.
    pub fn resume_after_interjection(&mut self, host: SpeakerIdentity, user_input: String) {
        self.speaker_queue.clear();
        self.topic = user_input;
        self.remaining_rounds = MAX_ROUNDTABLE_ROUNDS;
        self.auto_alternate = true;

        if !self
            .participants
            .iter()
            .any(|p| p.character_id == host.character_id)
        {
            self.participants.insert(0, host.clone());
        }

        // Queue Host first, followed by Guest participant(s)
        self.speaker_queue.push_back(host.clone());
        for p in &self.participants {
            if p.character_id != host.character_id {
                self.speaker_queue.push_back(p.clone());
            }
        }
    }

    /// Configures a multi-round debate between the Host and one or more Guests.
    pub fn start_debate(
        &mut self,
        host: SpeakerIdentity,
        guests: Vec<SpeakerIdentity>,
        topic: String,
    ) {
        self.clear();
        self.topic = topic;
        self.remaining_rounds = MAX_ROUNDTABLE_ROUNDS;
        self.auto_alternate = true;

        // Build deduplicated participant roster: Host + Guests
        self.participants.push(host.clone());
        for g in &guests {
            if !self
                .participants
                .iter()
                .any(|p| p.character_id == g.character_id)
            {
                self.participants.push(g.clone());
            }
        }

        // Queue initial turns: Host opens first, then Guest(s) reply, then auto-alternation continues!
        self.speaker_queue.push_back(host.clone());
        for g in guests {
            if g.character_id != host.character_id {
                self.speaker_queue.push_back(g);
            }
        }
    }

    /// Configures a `@mention` turn or multi-character conversation.
    pub fn start_mention_turn(
        &mut self,
        host: SpeakerIdentity,
        mentioned: Vec<SpeakerIdentity>,
        multi_turn: bool,
        prompt: String,
    ) {
        self.speaker_queue.clear();
        self.topic = prompt.clone();
        self.auto_alternate = multi_turn;
        self.remaining_rounds = if multi_turn {
            MAX_ROUNDTABLE_ROUNDS
        } else {
            // Even in a standard `@character` mention, allow a natural follow-up handoff
            // if the guest explicitly `@mentions` another character in its reply.
            4
        };

        if self.participants.is_empty() {
            self.participants.push(host.clone());
        }
        for spk in &mentioned {
            if !self
                .participants
                .iter()
                .any(|p| p.character_id == spk.character_id)
            {
                self.participants.push(spk.clone());
            }
        }

        // If `multi_turn` is true (e.g. "hey Niko ... talk about it with @levi-kujo") or if the prompt
        // does not start directly with `@<guest>`, the Host character speaks FIRST, followed by the Guest(s)!
        let starts_with_guest_mention = prompt.trim_start().starts_with('@')
            && mentioned
                .first()
                .is_some_and(|first| first.character_id != host.character_id);

        if multi_turn || !starts_with_guest_mention {
            self.speaker_queue.push_back(host.clone());
            for spk in mentioned {
                if spk.character_id != host.character_id {
                    self.speaker_queue.push_back(spk);
                }
            }
        } else {
            for spk in mentioned {
                self.speaker_queue.push_back(spk);
            }
        }
    }

    /// Invites a guest character mid-turn (e.g., via the `invite_character` tool)
    /// so they speak immediately after the current speaker finishes.
    pub fn invite_guest(
        &mut self,
        host: SpeakerIdentity,
        guest: SpeakerIdentity,
        topic_or_prompt: String,
    ) {
        if self.participants.is_empty() {
            self.participants.push(host.clone());
        }
        if !self
            .participants
            .iter()
            .any(|p| p.character_id == guest.character_id)
        {
            self.participants.push(guest.clone());
        }
        if self.topic.is_empty() {
            self.topic = topic_or_prompt;
        }
        if self.remaining_rounds == 0 {
            self.remaining_rounds = MAX_ROUNDTABLE_ROUNDS;
        }
        // Push to the front of the queue so the invited character speaks next
        if !self
            .speaker_queue
            .iter()
            .any(|s| s.character_id == guest.character_id)
        {
            self.speaker_queue.push_front(guest);
        }
    }

    /// Called when a turn finishes. Evaluates the speaker's response and determines
    /// whether another character should speak next, returning `(next_speaker, handoff_prompt)`.
    pub fn on_turn_completed(
        &mut self,
        speaker: &SpeakerIdentity,
        response: &str,
        had_blocking_proposal: bool,
        host_pane_id: &str,
        catalog: &[CharacterInfo],
    ) -> Option<(SpeakerIdentity, String)> {
        self.last_speaker_id = Some(speaker.character_id.clone());

        // If the turn proposed a command/file edit waiting on user approval, or
        // the speaker explicitly yielded to the user, pause autonomous turns
        // (while keeping `participants` and `auto_alternate` so the roundtable can resume).
        if had_blocking_proposal || response.contains(YIELD_TO_USER_MARKER) {
            self.speaker_queue.clear();
            self.remaining_rounds = 0;
            return None;
        }

        // 1. Check if the speaker's response explicitly `@mentioned` another character
        let clean_resp = strip_code_blocks(response);
        let mentioned_in_reply = extract_mentioned_characters(&clean_resp, host_pane_id, catalog);
        for mut target in mentioned_in_reply {
            if target.character_id != speaker.character_id {
                if let Some(existing) = self
                    .participants
                    .iter()
                    .find(|p| p.character_id == target.character_id)
                {
                    target.is_guest = existing.is_guest;
                }
                if !self
                    .participants
                    .iter()
                    .any(|p| p.character_id == target.character_id)
                {
                    self.participants.push(target.clone());
                }
                if !self
                    .speaker_queue
                    .iter()
                    .any(|s| s.character_id == target.character_id)
                {
                    self.speaker_queue.push_front(target);
                }
                if self.remaining_rounds == 0 {
                    self.remaining_rounds = 2;
                }
            }
        }

        if self.remaining_rounds == 0 {
            self.speaker_queue.clear();
            return None;
        }

        // 2. Pop the next queued speaker, or if `auto_alternate` is active, pick the next participant
        let next_speaker = if let Some(queued) = self.speaker_queue.pop_front() {
            if queued.character_id == speaker.character_id {
                self.speaker_queue.pop_front().or_else(|| {
                    if self.auto_alternate {
                        self.pick_next_after(&speaker.character_id)
                    } else {
                        None
                    }
                })
            } else {
                Some(queued)
            }
        } else if self.auto_alternate && self.participants.len() >= 2 {
            self.pick_next_after(&speaker.character_id)
        } else {
            None
        }?;

        self.remaining_rounds = self.remaining_rounds.saturating_sub(1);

        let other_names: Vec<String> = self
            .participants
            .iter()
            .filter(|p| p.character_id != next_speaker.character_id)
            .map(|p| format!("{} (@{})", p.display_name, p.slug))
            .collect();

        let style_instructions = if self.drama_mode {
            "- DRAMA / VISUAL NOVEL MODE IS ACTIVE: Reply in ONLY 1 to 2 short, punchy spoken sentences (under 35 words max)!\n\
            - STRICTLY FORBIDDEN: NO bullet points, NO numbered lists, NO markdown headers, NO multi-paragraph essays, and NO routine tool calls!\n\
            - Speak like a live anime / visual novel character reacting directly to what was just said, and end with a quick hook or reaction to the other character!"
        } else {
            "- Challenge, agree, banter, or build on the previous speaker's specific points from your own technical perspective and personality.\n\
            - Do NOT re-run system inspection commands that were already executed in the previous turn unless checking a brand-new fact.\n\
            - End your message by addressing the other character with a sharp counter-point or follow-up question so the conversation continues naturally!"
        };

        let prompt = format!(
            "[ROUNDTABLE TURN — Round {}/{}]\n\
            **{}** (`@{}`) just spoke in the overlay conversation (see their latest message in history).\n\
            Other participants in this conversation: {}.\n\
            Original Topic / Context: {}\n\
            INSTRUCTIONS FOR YOUR TURN:\n\
            - Respond directly in your own character's voice (**{}**) to **{}** (`@{}`) and the user.\n\
            {}",
            MAX_ROUNDTABLE_ROUNDS.saturating_sub(self.remaining_rounds),
            MAX_ROUNDTABLE_ROUNDS,
            speaker.display_name,
            speaker.slug,
            if other_names.is_empty() {
                "Host".to_string()
            } else {
                other_names.join(", ")
            },
            if self.topic.is_empty() {
                "General discussion"
            } else {
                &self.topic
            },
            next_speaker.display_name,
            speaker.display_name,
            speaker.slug,
            style_instructions,
        );

        Some((next_speaker, prompt))
    }

    fn pick_next_after(&self, current_character_id: &str) -> Option<SpeakerIdentity> {
        if self.participants.len() < 2 {
            return None;
        }
        let idx = self
            .participants
            .iter()
            .position(|p| p.character_id == current_character_id)
            .unwrap_or(0);
        let next_idx = (idx + 1) % self.participants.len();
        let candidate = &self.participants[next_idx];
        if candidate.character_id != current_character_id {
            Some(candidate.clone())
        } else {
            None
        }
    }
}

/// Loads the character catalog from `CHARACTER_CACHE`, falling back to disk if the cache is empty.
pub fn get_active_catalog() -> Vec<CharacterInfo> {
    let cached = CHARACTER_CACHE.load();
    if !cached.is_empty() {
        return (**cached).clone();
    }
    boxxy_claw_protocol::character_loader::load_characters().unwrap_or_default()
}

/// Formats a character slug like `"niko-una"` or `"levi-kujo"` into `"Niko Una"` / `"Levi Kujo"`,
/// matching `boxxy_claw_ui`'s header rendering.
pub fn format_character_title(slug_or_name: &str) -> String {
    slug_or_name
        .split(['-', ' '])
        .filter(|w| !w.is_empty())
        .map(|word| {
            let mut c = word.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// Builds a `SpeakerIdentity` from a `CharacterInfo`, checking `CLAIMS_CACHE` for an active swarm pane.
pub fn speaker_from_info(
    info: &CharacterInfo,
    host_character_id: &str,
    host_pane_id: &str,
) -> SpeakerIdentity {
    let claims = CLAIMS_CACHE.load();
    let home_pane_id = claims
        .iter()
        .find(|c| {
            c.character_id == info.config.id
                && c.holder_kind == HolderKind::Pane
                && c.holder_id != host_pane_id
        })
        .map(|c| c.holder_id.clone());

    let display_name = if info.config.name.contains('-') {
        format_character_title(&info.config.name)
    } else if !info.config.display_name.trim().is_empty() {
        format_character_title(&info.config.display_name)
    } else {
        format_character_title(&info.config.name)
    };

    SpeakerIdentity {
        character_id: info.config.id.clone(),
        display_name,
        slug: info.config.name.clone(),
        is_guest: info.config.id != host_character_id,
        home_pane_id,
    }
}

/// Resolves a character token (slug like `levi-kujo`, first name like `levi`, display name,
/// UUID, or active swarm petname) into a `SpeakerIdentity`.
pub fn resolve_speaker_token(
    token: &str,
    host_character_id: &str,
    host_pane_id: &str,
    catalog: &[CharacterInfo],
) -> Option<SpeakerIdentity> {
    let cleaned = token
        .trim()
        .trim_start_matches('@')
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
    if cleaned.is_empty() {
        return None;
    }

    // 1. Check exact ID, slug (`config.name`), or `display_name`
    if let Some(info) = catalog.iter().find(|c| {
        c.config.id.eq_ignore_ascii_case(cleaned)
            || c.config.name.eq_ignore_ascii_case(cleaned)
            || c.config.display_name.eq_ignore_ascii_case(cleaned)
    }) {
        return Some(speaker_from_info(info, host_character_id, host_pane_id));
    }

    // 2. Check first-name prefix match (e.g. `@levi` -> `levi-kujo`, `@niko` -> `niko-una`)
    if cleaned.len() >= 2
        && let Some(info) = catalog.iter().find(|c| {
            let first_slug = c.config.name.split('-').next().unwrap_or_default();
            let first_disp = c
                .config
                .display_name
                .split_whitespace()
                .next()
                .unwrap_or_default();
            first_slug.eq_ignore_ascii_case(cleaned) || first_disp.eq_ignore_ascii_case(cleaned)
        })
    {
        return Some(speaker_from_info(info, host_character_id, host_pane_id));
    }

    // 3. Check active swarm petname in `CLAIMS_CACHE`
    let claims = CLAIMS_CACHE.load();
    if let Some(claim) = claims
        .iter()
        .find(|cl| cl.petname.eq_ignore_ascii_case(cleaned))
        && let Some(info) = catalog.iter().find(|c| c.config.id == claim.character_id)
    {
        return Some(speaker_from_info(info, host_character_id, host_pane_id));
    }

    None
}

/// Extracts all `@<character>` mentions from a text string in order of appearance.
pub fn extract_mentioned_characters(
    text: &str,
    host_pane_id: &str,
    catalog: &[CharacterInfo],
) -> Vec<SpeakerIdentity> {
    let mut results = Vec::new();
    for word in text.split_whitespace() {
        if let Some(rest) = word.strip_prefix('@') {
            let candidate =
                rest.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
            if !candidate.is_empty()
                && let Some(spk) = resolve_speaker_token(candidate, "", host_pane_id, catalog)
                && !results
                    .iter()
                    .any(|r: &SpeakerIdentity| r.character_id == spk.character_id)
            {
                results.push(spk);
            }
        }
    }
    results
}

/// Parses a user message to determine whether it triggers `/stop`, `/debate`, or `@character` roundtable chat.
pub fn parse_user_roundtable_intent(
    raw_input: &str,
    host: &SpeakerIdentity,
    roundtable_active: bool,
    catalog: &[CharacterInfo],
) -> UserRoundtableIntent {
    let trimmed = raw_input.trim();
    if trimmed.is_empty() {
        return UserRoundtableIntent::None;
    }

    let lower = trimmed.to_ascii_lowercase();

    // 1. Explicit `/stop` command or natural stop command while a roundtable is active
    if lower == "/stop"
        || (roundtable_active
            && matches!(
                lower.trim_end_matches(['!', '.']),
                "stop"
                    | "stop debate"
                    | "stop chatting"
                    | "stop talking"
                    | "enough"
                    | "pause"
                    | "quiet"
            ))
    {
        return UserRoundtableIntent::Stop;
    }

    // 2. `/drama` command: `/drama` (toggle) or `/drama [@char1] [@char2] [topic]`
    if lower == "/drama" || lower.starts_with("/drama ") {
        let rest = trimmed.get("/drama".len()..).unwrap_or("").trim();
        if rest.is_empty() {
            return UserRoundtableIntent::ToggleDrama;
        }

        let mut guests = Vec::new();
        let mut topic_words = Vec::new();

        for word in rest.split_whitespace() {
            if word.starts_with('@')
                && let Some(mut spk) =
                    resolve_speaker_token(word, &host.character_id, "", catalog)
            {
                spk.is_guest = spk.character_id != host.character_id;
                if spk.character_id != host.character_id
                    && !guests
                        .iter()
                        .any(|g: &SpeakerIdentity| g.character_id == spk.character_id)
                {
                    guests.push(spk);
                }
                continue;
            }
            topic_words.push(word);
        }

        let topic = if topic_words.is_empty() {
            "Start a lively, dramatic in-character visual-novel banter scene together!".to_string()
        } else {
            topic_words.join(" ")
        };

        return UserRoundtableIntent::Drama { guests, topic };
    }

    // 3. `/debate` command: `/debate [@char1] [@char2] <topic>`
    if lower == "/debate" || lower.starts_with("/debate ") {
        let rest = trimmed.get("/debate".len()..).unwrap_or("").trim();
        let mut guests = Vec::new();
        let mut topic_words = Vec::new();

        for word in rest.split_whitespace() {
            if word.starts_with('@')
                && let Some(mut spk) =
                    resolve_speaker_token(word, &host.character_id, "", catalog)
            {
                spk.is_guest = spk.character_id != host.character_id;
                if spk.character_id != host.character_id
                    && !guests
                        .iter()
                        .any(|g: &SpeakerIdentity| g.character_id == spk.character_id)
                {
                    guests.push(spk);
                }
                continue;
            }
            topic_words.push(word);
        }

        // If the user didn't explicitly `@mention` a guest in `/debate <topic>`,
        // check if the first word of `<topic>` is a character name (e.g. `/debate levi rust vs c`),
        // otherwise pick the first other character in the catalog.
        if guests.is_empty() && !topic_words.is_empty() {
            if let Some(mut spk) =
                resolve_speaker_token(topic_words[0], &host.character_id, "", catalog)
                && spk.character_id != host.character_id
            {
                spk.is_guest = true;
                guests.push(spk);
                topic_words.remove(0);
            }
        }

        if guests.is_empty() {
            if let Some(other) = catalog.iter().find(|c| c.config.id != host.character_id) {
                guests.push(speaker_from_info(other, &host.character_id, ""));
            }
        }

        let topic = if topic_words.is_empty() {
            "Have a lively in-character conversation and debate about the current terminal workspace and your favorite engineering approaches.".to_string()
        } else {
            topic_words.join(" ")
        };

        if !guests.is_empty() {
            return UserRoundtableIntent::Debate { guests, topic };
        }
    }

    // 3. Check for `@character` mentions anywhere in the user message
    let mut mentioned = Vec::new();
    for mut spk in extract_mentioned_characters(trimmed, "", catalog) {
        spk.is_guest = spk.character_id != host.character_id;
        mentioned.push(spk);
    }

    let has_guest = mentioned.iter().any(|m| m.is_guest);
    if has_guest {
        // Detect if the user wants a multi-turn back-and-forth (e.g. mentioned multiple characters,
        // or didn't just start the message with `@guest`, or used conversational keywords like
        // "talk", "chat", "debate", "discuss", "argue", "with @", etc.)
        let starts_with_at = trimmed.starts_with('@');
        let wants_multi_turn = mentioned.len() >= 2
            || !starts_with_at
            || lower.contains("debate")
            || lower.contains("each other")
            || lower.contains("discuss")
            || lower.contains("talk")
            || lower.contains("chat")
            || lower.contains("argue")
            || lower.contains("with @");

        return UserRoundtableIntent::Mention {
            mentioned,
            multi_turn: wants_multi_turn,
            prompt: trimmed.to_string(),
        };
    }

    UserRoundtableIntent::None
}

fn strip_code_blocks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_fence = false;
    for line in s.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_claw_protocol::characters::{CharacterConfig, CharacterStatus};

    fn sample_catalog() -> Vec<CharacterInfo> {
        vec![
            CharacterInfo {
                config: CharacterConfig {
                    id: "id-niko".to_string(),
                    name: "niko-una".to_string(),
                    display_name: "Niko Una".to_string(),
                    color: "#ff6699".to_string(),
                    duties: "Rust engineer".to_string(),
                    personality: "Energetic".to_string(),
                },
                status: CharacterStatus::Available,
                has_avatar: true,
            },
            CharacterInfo {
                config: CharacterConfig {
                    id: "id-levi".to_string(),
                    name: "levi-kujo".to_string(),
                    display_name: "Levi Kujo".to_string(),
                    color: "#3399ff".to_string(),
                    duties: "Systems architect".to_string(),
                    personality: "Calm and analytical".to_string(),
                },
                status: CharacterStatus::Available,
                has_avatar: true,
            },
            CharacterInfo {
                config: CharacterConfig {
                    id: "id-kuro".to_string(),
                    name: "kuro".to_string(),
                    display_name: "Kuro".to_string(),
                    color: "#9966ff".to_string(),
                    duties: "Security specialist".to_string(),
                    personality: "Dry wit".to_string(),
                },
                status: CharacterStatus::Available,
                has_avatar: true,
            },
        ]
    }

    fn host_niko() -> SpeakerIdentity {
        SpeakerIdentity {
            character_id: "id-niko".to_string(),
            display_name: "Niko Una".to_string(),
            slug: "niko-una".to_string(),
            is_guest: false,
            home_pane_id: None,
        }
    }

    #[test]
    fn resolves_by_slug_first_name_and_display_name() {
        let cat = sample_catalog();
        let s1 = resolve_speaker_token("@levi-kujo", "id-niko", "pane-1", &cat).unwrap();
        assert_eq!(s1.character_id, "id-levi");
        assert!(s1.is_guest);

        let s2 = resolve_speaker_token("levi", "id-niko", "pane-1", &cat).unwrap();
        assert_eq!(s2.character_id, "id-levi");

        let s3 = resolve_speaker_token("@kuro,", "id-niko", "pane-1", &cat).unwrap();
        assert_eq!(s3.character_id, "id-kuro");
    }

    #[test]
    fn parses_debate_and_mention_intents() {
        let cat = sample_catalog();
        let host = host_niko();

        let intent = parse_user_roundtable_intent(
            "/debate @levi-kujo Rust vs C++",
            &host,
            false,
            &cat,
        );
        assert_eq!(
            intent,
            UserRoundtableIntent::Debate {
                guests: vec![SpeakerIdentity {
                    character_id: "id-levi".to_string(),
                    display_name: "Levi Kujo".to_string(),
                    slug: "levi-kujo".to_string(),
                    is_guest: true,
                    home_pane_id: None,
                }],
                topic: "Rust vs C++".to_string(),
            }
        );

        // `/debate <topic>` without explicit @mention picks first non-host character
        let auto_intent =
            parse_user_roundtable_intent("/debate async vs threads", &host, false, &cat);
        if let UserRoundtableIntent::Debate { guests, topic } = auto_intent {
            assert_eq!(guests.len(), 1);
            assert_eq!(guests[0].character_id, "id-levi");
            assert_eq!(topic, "async vs threads");
        } else {
            panic!("Expected Debate intent");
        }

        // `@levi` mention
        let mention = parse_user_roundtable_intent(
            "@levi what do you think of this?",
            &host,
            false,
            &cat,
        );
        if let UserRoundtableIntent::Mention {
            mentioned,
            multi_turn,
            ..
        } = mention
        {
            assert_eq!(mentioned.len(), 1);
            assert_eq!(mentioned[0].character_id, "id-levi");
            assert!(!multi_turn);
        } else {
            panic!("Expected Mention intent");
        }

        // Stop detection when active
        let stop = parse_user_roundtable_intent("stop", &host, true, &cat);
        assert_eq!(stop, UserRoundtableIntent::Stop);

        // `/drama` with no arguments toggles drama mode
        let toggle = parse_user_roundtable_intent("/drama", &host, false, &cat);
        assert_eq!(toggle, UserRoundtableIntent::ToggleDrama);

        // `/drama @levi-kujo` starts Drama mode with Levi
        let drama =
            parse_user_roundtable_intent("/drama @levi-kujo cursed kernel", &host, false, &cat);
        if let UserRoundtableIntent::Drama { guests, topic } = drama {
            assert_eq!(guests.len(), 1);
            assert_eq!(guests[0].character_id, "id-levi");
            assert_eq!(topic, "cursed kernel");
        } else {
            panic!("Expected Drama intent");
        }

        // `/drama <prompt>` without `@character` stays single-character (no auto-picked guest)
        let solo_drama =
            parse_user_roundtable_intent("/drama hello Rikka!", &host, false, &cat);
        if let UserRoundtableIntent::Drama { guests, topic } = solo_drama {
            assert!(guests.is_empty());
            assert_eq!(topic, "hello Rikka!");
        } else {
            panic!("Expected single-character Drama intent");
        }
    }

    #[test]
    fn roundtable_enforces_20_round_guard_and_yield_marker() {
        let cat = sample_catalog();
        let host = host_niko();
        let levi = resolve_speaker_token("levi", "id-niko", "pane-1", &cat).unwrap();

        let mut rt = RoundtableState::default();
        rt.start_debate(host.clone(), vec![levi.clone()], "Tokio vs async-std".into());
        assert_eq!(rt.remaining_rounds, MAX_ROUNDTABLE_ROUNDS);

        // Pop initial speaker (Host: Niko)
        let first = rt.speaker_queue.pop_front().unwrap();
        assert_eq!(first.character_id, "id-niko");

        // Simulate 20 back-and-forth rounds
        let mut current = first;
        let mut count = 0;
        while let Some((next, _prompt)) =
            rt.on_turn_completed(&current, "I disagree with your point!", false, "pane-1", &cat)
        {
            assert_ne!(next.character_id, current.character_id);
            current = next;
            count += 1;
        }
        assert_eq!(count, MAX_ROUNDTABLE_ROUNDS as usize);
        assert_eq!(rt.remaining_rounds, 0);

        // Yield marker stops immediately
        rt.start_debate(host, vec![levi], "Test yield".into());
        let first = rt.speaker_queue.pop_front().unwrap();
        let next = rt.on_turn_completed(
            &first,
            "We agree on this. [YIELD_TO_USER]",
            false,
            "pane-1",
            &cat,
        );
        assert!(next.is_none());
        assert_eq!(rt.remaining_rounds, 0);
    }
}
