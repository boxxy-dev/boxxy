use crate::catalog::CommandItem;
use frizbee::{Config as FrizbeeConfig, Matcher};

/// Pre-normalized search fields for a single [`CommandItem`], computed once at registration time
/// so keystroke filtering and sorting never re-normalize static strings.
#[derive(Clone, Debug)]
pub(super) struct IndexedEntry {
    pub(super) item: CommandItem,
    pub(super) title_norm: String,
    pub(super) title_words: Vec<String>,
    pub(super) subtitle_lower: String,
    pub(super) subtitle_words: Vec<String>,
    pub(super) keyword_words: Vec<String>,
    pub(super) action_norm: String,
    pub(super) action_words: Vec<String>,
}

impl IndexedEntry {
    #[must_use]
    pub(super) fn new(item: CommandItem) -> Self {
        let title_norm = normalize_for_search(&item.title);
        let title_words = split_words(&title_norm);

        let subtitle_lower = item.display_subtitle().to_lowercase();
        let subtitle_norm = normalize_for_search(&subtitle_lower);
        let subtitle_words = split_words(&subtitle_norm);

        let mut keyword_words = Vec::new();
        for kw in &item.keywords {
            let norm = normalize_for_search(kw);
            for word in norm.split_whitespace() {
                if !keyword_words.iter().any(|existing| existing == word) {
                    keyword_words.push(word.to_string());
                }
            }
        }

        let action_norm = normalize_for_search(&item.action);
        let action_words = split_words(&action_norm);

        Self {
            item,
            title_norm,
            title_words,
            subtitle_lower,
            subtitle_words,
            keyword_words,
            action_norm,
            action_words,
        }
    }

    fn matches_token_anywhere(&self, token: &str) -> (bool, bool, bool) {
        let in_title = self.title_words.iter().any(|w| w.starts_with(token))
            || (token.len() >= 3 && self.title_norm.contains(token));
        let in_meta = self.subtitle_words.iter().any(|w| w.starts_with(token))
            || self.keyword_words.iter().any(|w| w.starts_with(token));
        let in_action = self.action_words.iter().any(|w| w.starts_with(token))
            || (token.len() >= 3 && self.action_norm.contains(token));
        (in_title, in_meta, in_action)
    }
}

fn split_words(normalized: &str) -> Vec<String> {
    normalized
        .split_whitespace()
        .map(ToString::to_string)
        .collect()
}

/// Normalizes user queries and item labels into lowercase space-separated tokens:
/// - Strips apostrophes (`'` / `’`) so `"whats new"` matches `"What's New"`.
/// - Expands `&` into `" and "` so both `"claw ui sounds"` and `"claw ui and sounds"` match.
/// - Treats `-`, `_`, `.`, `/`, `:`, and `+` as word separators and collapses whitespace.
#[must_use]
pub(super) fn normalize_for_search(input: &str) -> String {
    let mut raw = String::with_capacity(input.len() + 4);
    for ch in input.chars() {
        match ch {
            '\'' | '’' => {}
            '&' => raw.push_str(" and "),
            '-' | '_' | '.' | '/' | ':' | '+' => raw.push(' '),
            c => {
                for lower in c.to_lowercase() {
                    raw.push(lower);
                }
            }
        }
    }

    let mut collapsed = String::with_capacity(raw.len());
    for word in raw.split_whitespace() {
        if !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push_str(word);
    }
    collapsed
}

/// Scores every entry in `entries` against `raw_query` in a single batch pass and writes
/// `Some(score)` for matching items (higher is more relevant) or `None` for filtered-out items.
pub(super) fn score_entries(
    entries: &[IndexedEntry],
    raw_query: &str,
    out_scores: &mut Vec<Option<u32>>,
) {
    out_scores.clear();
    if entries.is_empty() {
        return;
    }

    let q_norm = normalize_for_search(raw_query);
    if q_norm.is_empty() {
        out_scores.resize(entries.len(), Some(0));
        return;
    }

    let raw_trimmed_lower = raw_query.trim().to_lowercase();
    let q_tokens: Vec<&str> = q_norm.split_whitespace().collect();
    out_scores.reserve(entries.len());

    let mut any_lexical_match = false;
    for entry in entries {
        let score = lexical_score(entry, &raw_trimmed_lower, &q_norm, &q_tokens);
        if score.is_some() {
            any_lexical_match = true;
        }
        out_scores.push(score);
    }

    // If no entry matched lexically (e.g. acronym like "tccb" or typo like "prefrences"),
    // rescue via Frizbee's SIMD Smith-Waterman fuzzy matcher against command titles.
    if !any_lexical_match && q_norm.len() >= 2 {
        let haystacks: Vec<&str> = entries.iter().map(|e| e.title_norm.as_str()).collect();
        let exact_cfg = FrizbeeConfig::default().max_typos(Some(0));
        let mut fuzzy_matches = Matcher::new(&q_norm, &exact_cfg).match_list(&haystacks);

        if fuzzy_matches.is_empty() && q_norm.len() >= 4 {
            let typo_cfg = FrizbeeConfig::default().max_typos(Some(1));
            fuzzy_matches = Matcher::new(&q_norm, &typo_cfg).match_list(&haystacks);
        }

        let min_score = u16::try_from(q_norm.len().saturating_mul(8)).unwrap_or(u16::MAX);
        for m in fuzzy_matches {
            if m.score >= min_score
                && let Some(slot) = out_scores.get_mut(m.index as usize)
            {
                *slot = Some(1_000 + u32::from(m.score));
            }
        }
    }
}

fn lexical_score(
    entry: &IndexedEntry,
    raw_trimmed_lower: &str,
    q_norm: &str,
    q_tokens: &[&str],
) -> Option<u32> {
    let len_penalty = u32::try_from(entry.title_norm.len().min(200)).unwrap_or(200);

    // Direct namespace ID query (e.g. "pref.", "pref.apis", "win.model", "bookmark.").
    if raw_trimmed_lower.contains('.') && !raw_trimmed_lower.contains(' ') {
        if entry.subtitle_lower == raw_trimmed_lower {
            return Some(10_500);
        }
        if entry.subtitle_lower.starts_with(raw_trimmed_lower) {
            let sub_penalty = if raw_trimmed_lower.ends_with('.') {
                0
            } else {
                u32::try_from(entry.subtitle_lower.len().min(200)).unwrap_or(200)
            };
            return Some(8_900 - sub_penalty);
        }
    }

    // Tier 1: Exact normalized title match.
    if entry.title_norm == q_norm {
        return Some(10_000);
    }

    // Tier 2: Normalized title starts directly with the full query (e.g. "api" -> "APIs").
    if entry.title_norm.starts_with(q_norm) {
        return Some(8_500 - len_penalty);
    }

    // Tier 3: Every query token prefixes a word in the title (e.g. "copy out" -> "Copy Last Command Output").
    if all_tokens_prefix_title_words(&entry.title_words, q_tokens) {
        let first_word_bonus = u32::from(
            entry
                .title_words
                .first()
                .zip(q_tokens.first())
                .is_some_and(|(w, q)| w.starts_with(q)),
        ) * 350;
        let ordered_bonus = u32::from(tokens_match_in_order(&entry.title_words, q_tokens)) * 200;
        return Some(6_800 + first_word_bonus + ordered_bonus - len_penalty);
    }

    // Tier 4: Contiguous substring inside title (for queries >= 2 chars).
    if q_norm.len() >= 2
        && let Some(pos) = entry.title_norm.find(q_norm)
    {
        let pos_penalty = u32::try_from(pos.min(100)).unwrap_or(100) * 4;
        return Some(5_500 - pos_penalty - len_penalty);
    }

    // Tier 5: Multi-field token match across title, subtitle ("Preferences"), keywords, and action ID.
    let mut title_hits = 0u32;
    let mut meta_hits = 0u32;
    for &tok in q_tokens {
        let (in_title, in_meta, in_action) = entry.matches_token_anywhere(tok);
        if !in_title && !in_meta && !in_action {
            return None;
        }
        if in_title {
            title_hits += 1;
        } else if in_meta {
            meta_hits += 1;
        }
    }

    Some(3_500 + title_hits * 500 + meta_hits * 150 - len_penalty)
}

fn all_tokens_prefix_title_words(title_words: &[String], q_tokens: &[&str]) -> bool {
    if q_tokens.is_empty() || q_tokens.len() > title_words.len() {
        return false;
    }
    let mut used = vec![false; title_words.len()];
    for &qt in q_tokens {
        if let Some(pos) = title_words
            .iter()
            .enumerate()
            .position(|(i, tw)| !used[i] && tw.starts_with(qt))
        {
            used[pos] = true;
        } else {
            return false;
        }
    }
    true
}

fn tokens_match_in_order(title_words: &[String], q_tokens: &[&str]) -> bool {
    let mut cursor = 0usize;
    for &qt in q_tokens {
        let Some(rel) = title_words[cursor..]
            .iter()
            .position(|tw| tw.starts_with(qt))
        else {
            return false;
        };
        cursor += rel + 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CommandKind, builtin_commands};

    fn ranked_titles(entries: &[IndexedEntry], query: &str) -> Vec<(String, CommandKind)> {
        let mut scores = Vec::new();
        score_entries(entries, query, &mut scores);

        let mut matched: Vec<(usize, u32)> = scores
            .into_iter()
            .enumerate()
            .filter_map(|(idx, sc)| sc.map(|s| (idx, s)))
            .collect();

        matched
            .sort_by(|(idx_a, sc_a), (idx_b, sc_b)| sc_b.cmp(sc_a).then_with(|| idx_a.cmp(idx_b)));

        matched
            .into_iter()
            .map(|(idx, _)| (entries[idx].item.title.clone(), entries[idx].item.kind))
            .collect()
    }

    fn indexed_builtins() -> Vec<IndexedEntry> {
        builtin_commands()
            .into_iter()
            .map(IndexedEntry::new)
            .collect()
    }

    #[test]
    fn normalize_handles_punctuation_ampersands_and_apostrophes() {
        assert_eq!(normalize_for_search("What's New"), "whats new");
        assert_eq!(
            normalize_for_search("Claw UI & Sounds"),
            "claw ui and sounds"
        );
        assert_eq!(
            normalize_for_search("  win.open-preferences_page  "),
            "win open preferences page"
        );
    }

    #[test]
    fn query_api_ranks_preferences_apis_category_first_without_noise() {
        let entries = indexed_builtins();
        let results = ranked_titles(&entries, "API");
        assert_eq!(
            results.first(),
            Some(&("APIs".to_string(), CommandKind::PreferencesCategory))
        );
        assert!(
            !results.iter().any(|(title, _)| title == "Swap Pane Right"),
            "subsequence noise should not pollute lexical matches: {results:?}"
        );
    }

    #[test]
    fn query_preferences_lists_main_dialog_then_all_12_categories() {
        let entries = indexed_builtins();
        let results = ranked_titles(&entries, "preferences");
        assert_eq!(results[0], ("Preferences".to_string(), CommandKind::Action));

        let category_hit_count = results
            .iter()
            .filter(|(_, kind)| *kind == CommandKind::PreferencesCategory)
            .count();
        assert_eq!(category_hit_count, 12);
    }

    #[test]
    fn multi_word_and_keyword_queries_find_expected_commands() {
        let entries = indexed_builtins();

        let pref_api = ranked_titles(&entries, "preferences api");
        assert_eq!(
            pref_api.first(),
            Some(&("APIs".to_string(), CommandKind::PreferencesCategory))
        );

        let copy_out = ranked_titles(&entries, "copy output");
        assert_eq!(
            copy_out.first(),
            Some(&("Copy Last Command Output".to_string(), CommandKind::Action))
        );

        let claw_sounds = ranked_titles(&entries, "claw ui sounds");
        assert_eq!(
            claw_sounds.first(),
            Some(&(
                "Claw UI & Sounds".to_string(),
                CommandKind::PreferencesCategory
            ))
        );

        let shortcuts = ranked_titles(&entries, "keyboard shortcuts");
        assert_eq!(
            shortcuts.first(),
            Some(&("Shortcuts".to_string(), CommandKind::PreferencesCategory))
        );

        let app_themes = ranked_titles(&entries, "app themes");
        assert_eq!(
            app_themes.first(),
            Some(&("Themes".to_string(), CommandKind::PreferencesCategory))
        );

        let pref_namespace = ranked_titles(&entries, "pref.");
        assert_eq!(
            pref_namespace.first(),
            Some(&("Terminal".to_string(), CommandKind::PreferencesCategory))
        );

        let pref_apis = ranked_titles(&entries, "pref.apis");
        assert_eq!(
            pref_apis.first(),
            Some(&("APIs".to_string(), CommandKind::PreferencesCategory))
        );
    }

    #[test]
    fn fuzzy_fallback_rescues_typos_and_acronyms() {
        let entries = indexed_builtins();

        let typo = ranked_titles(&entries, "prefrences");
        assert!(
            typo.iter().any(|(title, _)| title == "Preferences"),
            "expected typo 'prefrences' to match Preferences, got: {typo:?}"
        );

        let acronym = ranked_titles(&entries, "tccb");
        assert_eq!(
            acronym.first(),
            Some(&(
                "Toggle Claw Code & Diff Blocks".to_string(),
                CommandKind::Action
            ))
        );
    }
}
