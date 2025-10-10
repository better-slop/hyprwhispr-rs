use anyhow::{Context, Result};
use arboard::Clipboard;
use regex::Regex;
use std::collections::HashMap;
use std::process::Command;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, info, warn};

pub struct TextInjector {
    clipboard: Clipboard,
    shift_paste: bool,
    word_overrides: HashMap<String, String>,
    clipboard_behavior: bool,
    clipboard_clear_delay: Duration,
}

impl TextInjector {
    pub fn new(
        shift_paste: bool,
        word_overrides: HashMap<String, String>,
        clipboard_behavior: bool,
        clipboard_clear_delay: f32,
    ) -> Result<Self> {
        let clipboard = Clipboard::new()
            .context("Failed to initialize clipboard")?;

        Ok(Self {
            clipboard,
            shift_paste,
            word_overrides,
            clipboard_behavior,
            clipboard_clear_delay: Duration::from_secs_f32(clipboard_clear_delay),
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

        // Copy to clipboard
        self.clipboard.set_text(&processed)
            .context("Failed to copy text to clipboard")?;

        // Small delay to let clipboard propagate
        sleep(Duration::from_millis(120)).await;

        // Simulate paste keystroke with ydotool
        self.simulate_paste().await?;

        // Schedule clipboard clearing if enabled
        if self.clipboard_behavior {
            let delay = self.clipboard_clear_delay;
            tokio::spawn(async move {
                sleep(delay).await;
                if let Ok(mut clipboard) = Clipboard::new() {
                    let _ = clipboard.clear();
                    debug!("Clipboard cleared after delay");
                }
            });
        }

        Ok(())
    }

    async fn simulate_paste(&self) -> Result<()> {
        // Linux evdev keycodes:
        // 29 = LeftCtrl
        // 42 = LeftShift
        // 47 = V key

        let result = if self.shift_paste {
            // Ctrl+Shift+V (works in terminals)
            Command::new("ydotool")
                .args(&["key", "29:1", "42:1", "47:1", "47:0", "42:0", "29:0"])
                .output()
                .context("Failed to execute ydotool")?
        } else {
            // Ctrl+V (traditional paste)
            Command::new("ydotool")
                .args(&["key", "29:1", "47:1", "47:0", "29:0"])
                .output()
                .context("Failed to execute ydotool")?
        };

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            warn!("ydotool paste failed: {}", stderr);
            return Err(anyhow::anyhow!("ydotool paste failed: {}", stderr));
        }

        debug!("Paste keystroke simulated successfully");
        Ok(())
    }

    fn preprocess_text(&self, text: &str) -> String {
        let mut processed = text.to_string();

        // Normalize line breaks to spaces to avoid unintended Enter
        processed = processed.replace("\r\n", " ");
        processed = processed.replace('\r', " ");
        processed = processed.replace('\n', " ");

        // Apply user-defined word overrides
        processed = self.apply_word_overrides(&processed);

        // Apply built-in speech-to-text replacements
        processed = self.apply_speech_replacements(&processed);

        // Collapse multiple spaces
        let space_regex = Regex::new(r" +").unwrap();
        processed = space_regex.replace_all(&processed, " ").to_string();

        // Trim whitespace
        processed.trim().to_string()
    }

    fn apply_word_overrides(&self, text: &str) -> String {
        let mut result = text.to_string();

        for (original, replacement) in &self.word_overrides {
            // Case-insensitive word boundary replacement
            let pattern = format!(r"\b{}\b", regex::escape(original));
            if let Ok(re) = Regex::new(&format!("(?i){}", pattern)) {
                result = re.replace_all(&result, replacement.as_str()).to_string();
            }
        }

        result
    }

    fn apply_speech_replacements(&self, text: &str) -> String {
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

        for (pattern, replacement) in &replacements {
            if let Ok(re) = Regex::new(&format!("(?i){}", pattern)) {
                result = re.replace_all(&result, *replacement).to_string();
            }
        }

        result
    }

    pub fn check_ydotool_available() -> bool {
        Command::new("which")
            .arg("ydotool")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
}
