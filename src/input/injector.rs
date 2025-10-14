use crate::logging::{record_text_pipeline, PipelineStepRecord, TextPipelineRecord};
use anyhow::{Context, Result};
use arboard::Clipboard;
use enigo::{Enigo, Keyboard, Settings};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{debug, info, warn};

static SPACE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r" +").expect("valid space collapse regex"));
static CONTROL_PUNCT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([\n\t])\s*[.!?,;:]+").expect("valid control artifact cleanup regex")
});
static CONTROL_TRAILING_SPACE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]+([\n\t])").expect("valid trailing space cleanup regex"));
static SYMBOL_PUNCT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([()\[\]\{\}])\s*[.,;]+").expect("valid symbol artifact cleanup regex")
});
static OPEN_PAREN_SPACE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\( +").expect("valid open paren space cleanup regex"));
static CLOSE_PAREN_SPACE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r" +\)").expect("valid close paren space cleanup regex"));
static OPEN_PAREN_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\s*,\s*").expect("valid open paren comma cleanup regex"));
static CLOSE_PAREN_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*,\s*\)").expect("valid close paren comma cleanup regex"));
static OPEN_BRACKET_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\s*,\s*").expect("valid open bracket comma cleanup regex"));
static CLOSE_BRACKET_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*,\s*\]").expect("valid close bracket comma cleanup regex"));
static OPEN_BRACE_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\s*,\s*").expect("valid open brace comma cleanup regex"));
static CLOSE_BRACE_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*,\s*\}").expect("valid close brace comma cleanup regex"));
static SPACE_BEFORE_PUNCT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[ \t]+([,.;:!?])").expect("valid punctuation spacing cleanup regex")
});
static DUPLICATE_COMMA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r",(?:\s*,)+").expect("valid duplicate comma cleanup regex"));
static SPACE_BEFORE_NEWLINE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[ \t]+\n").expect("valid space before newline regex"));
static SPACE_AFTER_NEWLINE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n[ \t]+").expect("valid space after newline regex"));
const MERGE_SYMBOLS: &[char] = &['-', '_', '+', '*', '/', '=', '~', '^'];
static MERGE_SYMBOL_PATTERNS: LazyLock<Vec<(char, Regex)>> = LazyLock::new(|| {
    MERGE_SYMBOLS
        .iter()
        .map(|sym| {
            let escaped = regex::escape(&sym.to_string());
            let pattern = format!(r"{escaped}\s+{escaped}");
            (
                *sym,
                Regex::new(&pattern)
                    .expect("valid identical symbol merge regex for specific symbol"),
            )
        })
        .collect()
});
static UNDERSCORE_BRIDGE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([^\s_])\s+(_+)\s+([^\s_])").expect("valid underscore bridge regex")
});

#[derive(Clone, Copy)]
struct SpeechReplacement {
    phrase: &'static str,
    replacement: &'static str,
    adjust_preceding_punct: bool,
}

static SPEECH_REPLACEMENTS: &[SpeechReplacement] = &[
    SpeechReplacement {
        phrase: "period",
        replacement: ".",
        adjust_preceding_punct: true,
    },
    SpeechReplacement {
        phrase: "comma",
        replacement: ",",
        adjust_preceding_punct: true,
    },
    SpeechReplacement {
        phrase: "question mark",
        replacement: "?",
        adjust_preceding_punct: true,
    },
    SpeechReplacement {
        phrase: "exclamation mark",
        replacement: "!",
        adjust_preceding_punct: true,
    },
    SpeechReplacement {
        phrase: "exclamation point",
        replacement: "!",
        adjust_preceding_punct: true,
    },
    SpeechReplacement {
        phrase: "colon",
        replacement: ":",
        adjust_preceding_punct: true,
    },
    SpeechReplacement {
        phrase: "semicolon",
        replacement: ";",
        adjust_preceding_punct: true,
    },
    SpeechReplacement {
        phrase: "new line",
        replacement: "\n",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "tab",
        replacement: "\t",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "dash",
        replacement: "-",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "dash dash",
        replacement: "--",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "hyphen",
        replacement: "-",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "underscore",
        replacement: "_",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "open paren",
        replacement: "(",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "open parenthesis",
        replacement: "(",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "open parentheses",
        replacement: "(",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "close paren",
        replacement: ")",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "close parenthesis",
        replacement: ")",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "close parentheses",
        replacement: ")",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "open bracket",
        replacement: "[",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "close bracket",
        replacement: "]",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "open brace",
        replacement: "{",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "close brace",
        replacement: "}",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "at symbol",
        replacement: "@",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "hash",
        replacement: "#",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "dollar sign",
        replacement: "$",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "percent",
        replacement: "%",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "caret",
        replacement: "^",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "ampersand",
        replacement: "&",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "asterisk",
        replacement: "*",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "plus",
        replacement: "+",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "equals",
        replacement: "=",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "less than",
        replacement: "<",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "greater than",
        replacement: ">",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "slash",
        replacement: "/",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "backslash",
        replacement: "\\",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "pipe",
        replacement: "|",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "tilde",
        replacement: "~",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "grave",
        replacement: "`",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "quote",
        replacement: "\"",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "double quote",
        replacement: "\"",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "apostrophe",
        replacement: "'",
        adjust_preceding_punct: false,
    },
    SpeechReplacement {
        phrase: "single quote",
        replacement: "'",
        adjust_preceding_punct: false,
    },
];

static SPEECH_REPLACEMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    let mut entries: Vec<&SpeechReplacement> = SPEECH_REPLACEMENTS.iter().collect();
    entries.sort_by(|a, b| b.phrase.len().cmp(&a.phrase.len()));

    let alternates = entries
        .into_iter()
        .map(|entry| regex::escape(entry.phrase))
        .collect::<Vec<_>>()
        .join("|");

    let pattern = format!(r"(?i)\b(?P<command>{})\b[.!?,;:]*", alternates);
    Regex::new(&pattern).expect("valid speech replacement regex")
});

static SPEECH_REPLACEMENT_LOOKUP: LazyLock<HashMap<&'static str, &'static SpeechReplacement>> =
    LazyLock::new(|| {
        let mut map = HashMap::new();
        for entry in SPEECH_REPLACEMENTS {
            map.insert(entry.phrase, entry);
        }
        map
    });

fn apply_speech_replacements(text: &str) -> (String, usize) {
    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;
    let mut count = 0;

    for caps in SPEECH_REPLACEMENT_REGEX.captures_iter(text) {
        let matched = caps.get(0).expect("regex match");
        result.push_str(&text[last_end..matched.start()]);

        if let Some(command) = caps.name("command") {
            let key = command.as_str().to_ascii_lowercase();
            if let Some(entry) = SPEECH_REPLACEMENT_LOOKUP.get(key.as_str()) {
                apply_speech_replacement_entry(&mut result, entry);
                count += 1;
            }
        }

        last_end = matched.end();
    }

    result.push_str(&text[last_end..]);
    (result, count)
}

fn sanitize_word_overrides(mut overrides: HashMap<String, String>) -> HashMap<String, String> {
    overrides.retain(|key, _| !key.eq_ignore_ascii_case("em dash"));
    overrides
}

fn apply_speech_replacement_entry(buffer: &mut String, entry: &SpeechReplacement) {
    if entry.adjust_preceding_punct {
        let mut trailing_ws: Vec<char> = Vec::new();

        loop {
            if buffer.ends_with(' ') {
                buffer.pop();
                trailing_ws.push(' ');
            } else if buffer.ends_with('\t') {
                buffer.pop();
                trailing_ws.push('\t');
            } else {
                break;
            }
        }

        loop {
            let Some(ch) = buffer.chars().last() else {
                break;
            };
            if matches!(ch, '.' | ',' | '!' | '?' | ';' | ':') {
                buffer.pop();
            } else {
                break;
            }
        }

        buffer.push_str(entry.replacement);
        for ch in trailing_ws.into_iter().rev() {
            buffer.push(ch);
        }
    } else {
        buffer.push_str(entry.replacement);
    }
}

fn capitalize_after_period(input: &str) -> (String, usize) {
    let mut result = String::with_capacity(input.len());
    let mut capitalize_next = true;
    let mut awaiting_space_after_punct = false;
    let mut count = 0;

    for ch in input.chars() {
        if awaiting_space_after_punct {
            if ch == ' ' {
                capitalize_next = true;
            } else if !ch.is_whitespace() {
                awaiting_space_after_punct = false;
            }
        }

        let mut output_char = ch;

        if capitalize_next {
            if ch.is_ascii_lowercase() {
                output_char = ch.to_ascii_uppercase();
                count += 1;
                capitalize_next = false;
                awaiting_space_after_punct = false;
            } else if ch.is_ascii_uppercase() || ch.is_ascii_digit() {
                capitalize_next = false;
                awaiting_space_after_punct = false;
            } else if !ch.is_whitespace() {
                capitalize_next = false;
                awaiting_space_after_punct = false;
            }
        }

        result.push(output_char);

        match ch {
            '.' | '!' | '?' => {
                capitalize_next = false;
                awaiting_space_after_punct = true;
            }
            '\n' => {
                capitalize_next = true;
                awaiting_space_after_punct = false;
            }
            _ => {}
        }
    }

    (result, count)
}

fn merge_separated_identical_symbols(input: &str) -> (String, usize) {
    let mut total_count = 0;
    let mut current = input.to_string();

    for (sym, regex) in MERGE_SYMBOL_PATTERNS.iter() {
        let replacement = format!("{sym}{sym}");

        loop {
            let matches = regex.find_iter(&current).count();
            if matches == 0 {
                break;
            }

            total_count += matches;
            current = regex
                .replace_all(&current, replacement.as_str())
                .into_owned();
        }
    }

    (current, total_count)
}

fn collapse_underscore_spacing(input: &str) -> (String, usize) {
    let mut total_count = 0;
    let mut current = input.to_string();

    loop {
        let matches = UNDERSCORE_BRIDGE_REGEX.captures_iter(&current).count();
        if matches == 0 {
            break;
        }

        total_count += matches;
        current = UNDERSCORE_BRIDGE_REGEX
            .replace_all(&current, "$1$2$3")
            .into_owned();
    }

    (current, total_count)
}

fn trim_spaces_around_newlines(input: &str) -> (String, usize) {
    let mut count = 0;

    let trailing_matches = SPACE_BEFORE_NEWLINE_REGEX.find_iter(input).count();
    let without_trailing = SPACE_BEFORE_NEWLINE_REGEX
        .replace_all(input, "\n")
        .into_owned();
    count += trailing_matches;

    let leading_matches = SPACE_AFTER_NEWLINE_REGEX
        .find_iter(&without_trailing)
        .count();
    let final_result = SPACE_AFTER_NEWLINE_REGEX
        .replace_all(&without_trailing, "\n")
        .into_owned();
    count += leading_matches;

    (final_result, count)
}

pub struct TextInjector {
    enigo: Enigo,
    clipboard: Clipboard,
    word_overrides: HashMap<String, String>,
    auto_copy_clipboard: bool,
}

impl TextInjector {
    pub fn new(
        _shift_paste: bool,
        word_overrides: HashMap<String, String>,
        auto_copy_clipboard: bool,
    ) -> Result<Self> {
        let enigo = Enigo::new(&Settings::default())
            .context("Failed to initialize Enigo for text injection")?;

        let clipboard = Clipboard::new().context("Failed to initialize clipboard")?;

        let sanitized_overrides = sanitize_word_overrides(word_overrides);

        Ok(Self {
            enigo,
            clipboard,
            word_overrides: sanitized_overrides,
            auto_copy_clipboard,
        })
    }

    pub async fn inject_text(&mut self, text: &str) -> Result<()> {
        if text.trim().is_empty() {
            debug!("No text to inject (empty or whitespace)");
            return Ok(());
        }

        // Preprocess text
        let processed = self.preprocess_text(text);

        info!("Injecting text: {} characters", processed.len());

        // Copy to clipboard - required for paste injection method
        self.clipboard.set_text(&processed)
            .context("Failed to copy text to clipboard")?;
        debug!("Text copied to clipboard");

        // Small delay to ensure window focus is ready for input (especially on Wayland/XWayland)
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Use Ctrl+Shift+V to paste from clipboard (works in terminals and GUI apps)
        debug!("Injecting text via Ctrl+Shift+V paste...");
        use enigo::{Direction, Key};
        
        // Press Ctrl+Shift+V
        self.enigo.key(Key::Control, Direction::Press)
            .context("Failed to press Ctrl")?;
        self.enigo.key(Key::Shift, Direction::Press)
            .context("Failed to press Shift")?;
        self.enigo.key(Key::Unicode('v'), Direction::Click)
            .context("Failed to press V")?;
        self.enigo.key(Key::Shift, Direction::Release)
            .context("Failed to release Shift")?;
        self.enigo.key(Key::Control, Direction::Release)
            .context("Failed to release Ctrl")?;

        info!("✅ Text injected successfully via paste");
        Ok(())
    }

    fn preprocess_text(&self, text: &str) -> String {
        let mut steps = if tracing::level_enabled!(tracing::Level::DEBUG) {
            Some(Vec::new())
        } else {
            None
        };
        let mut current = text.to_string();

        let normalized = normalize_line_breaks(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "normalize_line_breaks",
                current.clone(),
                normalized.clone(),
                None,
            ));
        }
        current = normalized;

        let (after_overrides, override_count) = self.apply_word_overrides_with_count(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "word_overrides",
                current.clone(),
                after_overrides.clone(),
                if override_count > 0 {
                    Some(override_count)
                } else {
                    None
                },
            ));
        }
        current = after_overrides;

        let (after_speech, speech_count) = self.apply_speech_replacements_with_count(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "speech_replacements",
                current.clone(),
                after_speech.clone(),
                if speech_count > 0 {
                    Some(speech_count)
                } else {
                    None
                },
            ));
        }
        current = after_speech;

        let cleaned_control = clean_control_artifacts(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "control_artifact_cleanup",
                current.clone(),
                cleaned_control.clone(),
                None,
            ));
        }
        current = cleaned_control;

        let collapsed = collapse_spaces(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "collapse_spaces",
                current.clone(),
                collapsed.clone(),
                None,
            ));
        }
        current = collapsed;

        let (newline_cleaned, newline_trim_count) = trim_spaces_around_newlines(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "trim_spaces_around_newlines",
                current.clone(),
                newline_cleaned.clone(),
                if newline_trim_count > 0 {
                    Some(newline_trim_count)
                } else {
                    None
                },
            ));
        }
        current = newline_cleaned;

        let (merged_symbols, merge_count) = merge_separated_identical_symbols(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "merge_identical_symbols",
                current.clone(),
                merged_symbols.clone(),
                if merge_count > 0 {
                    Some(merge_count)
                } else {
                    None
                },
            ));
        }
        current = merged_symbols;

        let (bridged_underscores, underscore_count) = collapse_underscore_spacing(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "collapse_underscore_spacing",
                current.clone(),
                bridged_underscores.clone(),
                if underscore_count > 0 {
                    Some(underscore_count)
                } else {
                    None
                },
            ));
        }
        current = bridged_underscores;

        let (capitalized, capitalized_count) = capitalize_after_period(&current);
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "capitalize_after_period",
                current.clone(),
                capitalized.clone(),
                if capitalized_count > 0 {
                    Some(capitalized_count)
                } else {
                    None
                },
            ));
        }
        current = capitalized;

        let trimmed = current.trim().to_string();
        if let Some(ref mut logged_steps) = steps {
            logged_steps.push(PipelineStepRecord::new(
                "trim_whitespace",
                current.clone(),
                trimmed.clone(),
                None,
            ));
        }

        let final_result = trimmed;

        if let Some(logged_steps) = steps {
            record_text_pipeline(TextPipelineRecord::new(
                text.to_string(),
                final_result.clone(),
                logged_steps,
            ));
        }

        final_result
    }

    fn apply_word_overrides_with_count(&self, text: &str) -> (String, usize) {
        let mut result = text.to_string();
        let mut count = 0;

        if self.word_overrides.is_empty() {
            return (result, 0);
        }

        for (original, replacement) in &self.word_overrides {
            // Case-insensitive word boundary replacement
            let pattern = format!(r"\b{}\b", regex::escape(original));
            if let Ok(re) = Regex::new(&format!("(?i){}", pattern)) {
                let before = result.clone();
                result = re.replace_all(&result, replacement.as_str()).to_string();
                if before != result {
                    count += 1;
                }
            }
        }

        (result, count)
    }

    fn apply_speech_replacements_with_count(&self, text: &str) -> (String, usize) {
        // Built-in speech-to-text replacements
        apply_speech_replacements(text)
    }
}

fn normalize_line_breaks(input: &str) -> String {
    if input.contains(['\r', '\n']) {
        input
            .replace("\r\n", " ")
            .replace('\r', " ")
            .replace('\n', " ")
    } else {
        input.to_string()
    }
}

fn collapse_spaces(input: &str) -> String {
    SPACE_REGEX.replace_all(input, " ").to_string()
}

fn clean_control_artifacts(input: &str) -> String {
    let without_control_punct = CONTROL_PUNCT_REGEX.replace_all(input, "$1");
    let without_trailing_space =
        CONTROL_TRAILING_SPACE_REGEX.replace_all(&without_control_punct, "$1");
    let without_symbol_punct = SYMBOL_PUNCT_REGEX.replace_all(&without_trailing_space, "$1");
    let collapsed_open = OPEN_PAREN_SPACE_REGEX.replace_all(&without_symbol_punct, "(");
    let collapsed_close = CLOSE_PAREN_SPACE_REGEX.replace_all(&collapsed_open, ")");
    let no_open_comma = OPEN_PAREN_COMMA_REGEX.replace_all(&collapsed_close, "(");
    let no_close_comma = CLOSE_PAREN_COMMA_REGEX.replace_all(&no_open_comma, ")");
    let no_open_bracket_comma = OPEN_BRACKET_COMMA_REGEX.replace_all(&no_close_comma, "[ ");
    let no_close_bracket_comma =
        CLOSE_BRACKET_COMMA_REGEX.replace_all(&no_open_bracket_comma, " ]");
    let no_open_brace_comma = OPEN_BRACE_COMMA_REGEX.replace_all(&no_close_bracket_comma, "{ ");
    let no_close_brace_comma = CLOSE_BRACE_COMMA_REGEX.replace_all(&no_open_brace_comma, " }");
    let no_space_before_punct = SPACE_BEFORE_PUNCT_REGEX.replace_all(&no_close_brace_comma, "$1");
    DUPLICATE_COMMA_REGEX
        .replace_all(&no_space_before_punct, ",")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_parenthesis_commas_and_spaces() {
        let input = "(, value, )";
        assert_eq!(clean_control_artifacts(input), "(value)");
    }

    #[test]
    fn cleans_bracket_and_brace_commas() {
        let bracket_input = "[, option, ]";
        let brace_input = "{, field, }";
        assert_eq!(clean_control_artifacts(bracket_input), "[ option ]");
        assert_eq!(clean_control_artifacts(brace_input), "{ field }");
    }

    #[test]
    fn keeps_internal_commas_inside_collections() {
        let bracket_list = "[ first, second, third, ]";
        let brace_list = "{ alpha, beta, gamma, }";
        assert_eq!(
            clean_control_artifacts(bracket_list),
            "[ first, second, third ]"
        );
        assert_eq!(
            clean_control_artifacts(brace_list),
            "{ alpha, beta, gamma }"
        );
    }

    #[test]
    fn removes_clause_commas_before_closing_delimiter() {
        let brace_input = "{ fuck, }";
        let bracket_input = "[ awesome, ]";
        assert_eq!(clean_control_artifacts(brace_input), "{ fuck }");
        assert_eq!(clean_control_artifacts(bracket_input), "[ awesome ]");
    }

    #[test]
    fn cleans_demo_sentence_bracket_artifacts() {
        let input =
            "Hello, hello, testing 123, [, fuck fuck fuck fuck fuck fuck fuck fuck fuck fuck, ].";
        assert_eq!(
            clean_control_artifacts(input),
            "Hello, hello, testing 123, [ fuck fuck fuck fuck fuck fuck fuck fuck fuck fuck ]"
        );
    }

    #[test]
    fn strips_space_before_punctuation() {
        let input = "hello , world ! what ; is : this ?";
        assert_eq!(
            clean_control_artifacts(input),
            "hello, world! what; is: this?"
        );
    }

    #[test]
    fn removes_duplicate_commas_from_transcript_artifacts() {
        let input = "{ fuck fuck fuck fuck, ,, fuck, }.";
        assert_eq!(
            clean_control_artifacts(input),
            "{ fuck fuck fuck fuck, fuck }"
        );
    }

    #[test]
    fn speech_replacements_normalize_commanded_punctuation() {
        let input = "This is awesome. Period. I love this. Comma. Fuck. Yeah. Comma. Fuck. Period.";
        let (after_speech, count) = apply_speech_replacements(input);
        let cleaned = clean_control_artifacts(&after_speech);
        let collapsed = collapse_spaces(&cleaned);

        assert_eq!(
            collapsed.trim(),
            "This is awesome. I love this, Fuck. Yeah, Fuck."
        );
        assert_eq!(count, 4);
    }

    #[test]
    fn capitalizes_lowercase_after_period_space() {
        let input = "This. is awesome. already Capitalized. stays.";
        let (capitalized, count) = capitalize_after_period(input);
        assert_eq!(capitalized, "This. Is awesome. Already Capitalized. Stays.");
        assert_eq!(count, 3);
    }

    #[test]
    fn speech_replacements_collapse_dash_dash() {
        let input = "prepare dash dash go";
        let (after_speech, count) = apply_speech_replacements(input);
        assert_eq!(after_speech, "prepare -- go");
        assert_eq!(count, 1);
    }

    #[test]
    fn control_cleanup_preserves_colon_after_symbols() {
        let input = "— { chaos,  yes }:  coordinate";
        let cleaned = clean_control_artifacts(input);
        let collapsed = collapse_spaces(&cleaned);
        assert_eq!(collapsed, "— { chaos, yes }: coordinate");
    }

    #[test]
    fn control_cleanup_keeps_exclamation_after_closing_symbol() {
        let input = "phoenix [ alpha, beta ]!";
        let cleaned = clean_control_artifacts(input);
        assert_eq!(cleaned, "phoenix [ alpha, beta ]!");
    }

    #[test]
    fn merge_identical_symbols_collapses_spaced_pairs() {
        let input = "77 - - go and _ _ done";
        let (merged, count) = merge_separated_identical_symbols(input);
        assert_eq!(merged, "77 -- go and __ done");
        assert_eq!(count, 2);
    }

    #[test]
    fn collapse_underscore_spacing_links_tokens() {
        let input = "align __ sync and foo _ bar";
        let (collapsed, count) = collapse_underscore_spacing(input);
        assert_eq!(collapsed, "align__sync and foo_bar");
        assert_eq!(count, 2);
    }

    #[test]
    fn trim_spaces_around_newlines_removes_padding() {
        let input = "Line one  \n  Line two\n\n   Line three";
        let (trimmed, count) = trim_spaces_around_newlines(input);
        assert_eq!(trimmed, "Line one\nLine two\n\nLine three");
        assert!(count >= 2);
    }

    #[test]
    fn capitalizes_after_newline_break() {
        let input = "first line.\nnext starts here.";
        let (capitalized, count) = capitalize_after_period(input);
        assert_eq!(capitalized, "First line.\nNext starts here.");
        assert_eq!(count, 2);
    }

    #[test]
    fn sanitize_word_overrides_drops_em_dash() {
        let overrides = HashMap::from([
            ("em dash".to_string(), "—".to_string()),
            ("under score".to_string(), "_".to_string()),
        ]);
        let sanitized = sanitize_word_overrides(overrides);
        assert!(!sanitized.contains_key("em dash"));
        assert_eq!(sanitized.get("under score").unwrap(), "_");
    }
}
