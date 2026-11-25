//! Results of a history import.
use std::fmt;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub imported: usize,
    pub skipped: usize,
    pub unresolved: usize,
    pub activities: usize,
    pub categories: usize,
    pub tags: usize,
}

impl fmt::Display for ImportSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} new entries ({} unresolved), {} unchanged; new vocabulary: {} activities, {} categories, {} tags",
            self.imported,
            self.unresolved,
            self.skipped,
            self.activities,
            self.categories,
            self.tags
        )
    }
}
