pub const DEFAULT_PROMPT: &str = "Transcribe with proper capitalization, including sentence beginnings, proper nouns, titles, and standard English capitalization rules.";

pub struct PromptBlueprint<'a> {
    candidate: Option<&'a str>,
    fallback: &'a str,
}

impl<'a> PromptBlueprint<'a> {
    pub fn new(candidate: Option<&'a str>, fallback: &'a str) -> Self {
        Self {
            candidate,
            fallback,
        }
    }

    pub fn from(candidate: &'a str) -> Self {
        Self {
            candidate: Some(candidate),
            fallback: DEFAULT_PROMPT,
        }
    }

    pub fn with_default(candidate: Option<&'a str>) -> Self {
        Self {
            candidate,
            fallback: DEFAULT_PROMPT,
        }
    }

    pub fn resolve(self) -> String {
        let chosen = self.candidate.unwrap_or(self.fallback);
        chosen.trim().to_string()
    }
}
