pub struct PromptBlueprint<'a> {
    custom: Option<&'a str>,
    fallback: &'a str,
}

impl<'a> PromptBlueprint<'a> {
    pub fn new(custom: Option<&'a str>, fallback: &'a str) -> Self {
        Self { custom, fallback }
    }

    pub fn resolve(self) -> String {
        self.custom.unwrap_or(self.fallback).to_owned()
    }
}
