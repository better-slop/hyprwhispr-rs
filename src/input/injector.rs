use anyhow::{Context, Result};
use arboard::Clipboard;
use enigo::{Enigo, Keyboard, Settings};
use regex::Regex;
use std::collections::HashMap;
use tracing::{debug, info, warn};

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
        
        let clipboard = Clipboard::new()
            .context("Failed to initialize clipboard")?;

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
        self.enigo.text(&processed)
            .context("Failed to inject text with Enigo")?;

        debug!("Text injected successfully");
        Ok(())
    }



    fn preprocess_text(&self, text: &str) -> String {
        debug!("=== Text Preprocessing Pipeline ===");
        debug!("1. Original text: {:?}", text);
        
        let mut processed = text.to_string();

        // Normalize line breaks to spaces to avoid unintended Enter
        processed = processed.replace("\r\n", " ");
        processed = processed.replace('\r', " ");
        processed = processed.replace('\n', " ");
        debug!("2. After line break normalization: {:?}", processed);

        // Apply user-defined word overrides
        processed = self.apply_word_overrides(&processed);
        debug!("3. After word overrides: {:?}", processed);

        // Apply built-in speech-to-text replacements
        processed = self.apply_speech_replacements(&processed);
        debug!("4. After speech replacements: {:?}", processed);

        // Collapse multiple spaces
        let space_regex = Regex::new(r" +").unwrap();
        processed = space_regex.replace_all(&processed, " ").to_string();
        debug!("5. After space collapsing: {:?}", processed);

        // Trim whitespace
        let final_result = processed.trim().to_string();
        debug!("6. Final result: {:?}", final_result);
        debug!("=== End Preprocessing ===");
        
        final_result
    }

    fn apply_word_overrides(&self, text: &str) -> String {
        let mut result = text.to_string();

        if self.word_overrides.is_empty() {
            debug!("   → No word overrides configured");
            return result;
        }

        debug!("   → Applying {} word override(s):", self.word_overrides.len());
        for (original, replacement) in &self.word_overrides {
            // Case-insensitive word boundary replacement
            let pattern = format!(r"\b{}\b", regex::escape(original));
            if let Ok(re) = Regex::new(&format!("(?i){}", pattern)) {
                let before = result.clone();
                result = re.replace_all(&result, replacement.as_str()).to_string();
                if before != result {
                    debug!("     ✓ {:?} → {:?}", original, replacement);
                }
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
        let mut applied_count = 0;

        debug!("   → Applying built-in speech replacements:");
        for (pattern, replacement) in &replacements {
            if let Ok(re) = Regex::new(&format!("(?i){}", pattern)) {
                let before = result.clone();
                result = re.replace_all(&result, *replacement).to_string();
                if before != result {
                    debug!("     ✓ {} → {:?}", pattern, replacement);
                    applied_count += 1;
                }
            }
        }
        
        if applied_count == 0 {
            debug!("     (no built-in replacements matched)");
        }

        result
    }


}
