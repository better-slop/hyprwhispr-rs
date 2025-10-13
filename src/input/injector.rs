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
    Regex::new(r"([()\[\]\{\}])\s*[.!?,;:]+").expect("valid symbol artifact cleanup regex")
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
    let alternates = SPEECH_REPLACEMENTS
        .iter()
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

        Ok(Self {
            enigo,
            clipboard,
            word_overrides,
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

        // Copy to clipboard (for backup/manual paste if needed) - if enabled
        if self.auto_copy_clipboard {
            if let Err(e) = self.clipboard.set_text(&processed) {
                warn!("Failed to copy to clipboard: {}", e);
            } else {
                debug!("Text copied to clipboard");
            }
        }

        // Inject text directly with enigo
        self.enigo
            .text(&processed)
            .context("Failed to inject text with Enigo")?;

        debug!("Text injected successfully");
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
}
