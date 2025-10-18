use regex::Regex;

const NON_SPEECH_MARKERS: &[&str] = &["BLANK_AUDIO", "INAUDIBLE", "NO_SPEECH", "SILENCE"];

pub fn clean_transcription(transcription: &str, prompt: &str) -> String {
    let trimmed = transcription.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if is_prompt_artifact(trimmed, prompt) {
        return String::new();
    }

    if contains_only_non_speech_markers(trimmed) {
        return String::new();
    }

    trimmed.to_string()
}

pub fn contains_only_non_speech_markers(transcription: &str) -> bool {
    let mut found_marker = false;

    for raw_token in transcription.split_whitespace() {
        let token = raw_token.trim_matches(|c: char| matches!(c, '.' | ',' | '!' | '?' | '"'));
        if token.is_empty() {
            continue;
        }

        if !token.starts_with('[') || !token.ends_with(']') {
            return false;
        }

        let inner = token[1..token.len() - 1].trim();
        if inner.is_empty() {
            return false;
        }

        let normalized: String = inner.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        let upper = normalized.to_ascii_uppercase();

        if !NON_SPEECH_MARKERS.iter().any(|marker| *marker == upper) {
            return false;
        }

        found_marker = true;
    }

    found_marker
}

pub fn is_prompt_artifact(transcription: &str, prompt: &str) -> bool {
    let trimmed_prompt = prompt.trim();
    if trimmed_prompt.is_empty() {
        return false;
    }

    let mut phrases = vec![trimmed_prompt.to_string()];
    phrases.extend(
        trimmed_prompt
            .split(|c| c == '.' || c == '!' || c == '?')
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
            .map(|segment| segment.to_string()),
    );

    let transcription_core = transcription.trim_matches(|c: char| c.is_ascii_whitespace());

    for phrase in phrases {
        let escaped = regex::escape(&phrase);
        let pattern = format!(r#"(?i)^(?:{}\s*[.!?\s"]*)+$"#, escaped);
        if let Ok(re) = Regex::new(&pattern) {
            if re.is_match(transcription_core) {
                return true;
            }
        }
    }

    false
}
