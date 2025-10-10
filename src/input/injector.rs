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
        let replacements = [
            (r"\bperiod\b", "."),
            (r"\bcomma\b", ","),
            (r"\bquestion mark\b", "?"),
            (r"\bexclamation mark\b", "!"),
            (r"\bexclamation point\b", "!"),
            (r"\bcolon\b", ":"),
            (r"\bsemicolon\b", ";"),
            (r"\bnew line\b", "\n"),
            (r"\btab\b", "\t"),
            (r"\bdash\b", "-"),
            (r"\bhyphen\b", "-"),
            (r"\bunderscore\b", "_"),
            (r"\bopen paren\b", "("),
            (r"\bclose paren\b", ")"),
            (r"\bopen bracket\b", "["),
            (r"\bclose bracket\b", "]"),
            (r"\bopen brace\b", "{"),
            (r"\bclose brace\b", "}"),
            (r"\bat symbol\b", "@"),
            (r"\bhash\b", "#"),
            (r"\bdollar sign\b", "$"),
            (r"\bpercent\b", "%"),
            (r"\bcaret\b", "^"),
            (r"\bampersand\b", "&"),
            (r"\basterisk\b", "*"),
            (r"\bplus\b", "+"),
            (r"\bequals\b", "="),
            (r"\bless than\b", "<"),
            (r"\bgreater than\b", ">"),
            (r"\bslash\b", "/"),
            (r"\bbackslash\b", r"\"),
            (r"\bpipe\b", "|"),
            (r"\btilde\b", "~"),
            (r"\bgrave\b", "`"),
            (r"\bquote\b", "\""),
            (r"\bdouble quote\b", "\""),
            (r"\bapostrophe\b", "'"),
            (r"\bsingle quote\b", "'"),
        ];

        let mut result = text.to_string();
        let mut count = 0;

        for (pattern, replacement) in &replacements {
            if let Ok(re) = Regex::new(&format!("(?i){}", pattern)) {
                let before = result.clone();
                result = re.replace_all(&result, *replacement).to_string();
                if before != result {
                    count += 1;
                }
            }
        }

        (result, count)
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
