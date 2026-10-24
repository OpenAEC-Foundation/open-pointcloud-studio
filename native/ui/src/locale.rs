use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language { #[default] Dutch, English }
impl Language {
    pub const ALL: [Self; 2] = [Self::Dutch, Self::English];
    pub fn text(self, nl: &'static str, en: &'static str) -> &'static str {
        match self { Self::Dutch => nl, Self::English => en }
    }
}
impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self { Self::Dutch => "Nederlands", Self::English => "English" })
    }
}
