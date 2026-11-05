use anyhow::{Context, Result};
use log::warn;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisIssue {
    pub stage: String,
    pub subject: String,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct AnalysisReport {
    pub allow_partial: bool,
    pub issues: Vec<AnalysisIssue>,
}

impl AnalysisReport {
    pub fn new(allow_partial: bool) -> Self {
        Self {
            allow_partial,
            issues: Vec::new(),
        }
    }

    /// Optional omissions are reported even in strict mode.
    pub fn note(&mut self, stage: &str, subject: &str, message: impl Into<String>) {
        let message = message.into();
        warn!("incomplete {stage} for {subject}: {message}");
        self.issues.push(AnalysisIssue {
            stage: stage.into(),
            subject: subject.into(),
            message,
        });
    }

    /// Only explicit opt-in permits recovering from a required read failure.
    /// A missing value stays absent: no zero or guessed value is manufactured.
    pub fn recover<T>(
        &mut self,
        stage: &str,
        subject: &str,
        value: Result<T>,
    ) -> Result<Option<T>> {
        match value.with_context(|| format!("failed {stage} for {subject}")) {
            Ok(value) => Ok(Some(value)),
            Err(err) if self.allow_partial => {
                self.note(stage, subject, format!("{err:#}"));
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn strict_is_the_default() {
        let mut report = AnalysisReport::default();
        let err = report
            .recover::<u32>("offsets", "client.dll", Err(anyhow!("outdated pattern")))
            .unwrap_err();
        assert!(format!("{err:#}").contains("client.dll"));
        assert!(report.issues.is_empty());
    }

    #[test]
    fn partial_failure_is_absent_not_zero_and_keeps_context() {
        let mut report = AnalysisReport::new(true);
        assert_eq!(
            report
                .recover::<u32>("offsets", "missing", Err(anyhow!("read failed")))
                .unwrap(),
            None
        );
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].subject, "missing");
        assert!(report.issues[0].message.contains("read failed"));
        assert_eq!(
            report.recover("offsets", "valid_zero", Ok(0_u32)).unwrap(),
            Some(0)
        );
    }

    #[test]
    fn healthy_items_survive_a_failed_neighbor() {
        let mut report = AnalysisReport::new(true);
        let items = [Ok(16), Err(anyhow!("unreadable class")), Ok(32)];
        let mut kept = Vec::new();
        for (index, result) in items.into_iter().enumerate() {
            if let Some(value) = report
                .recover("schemas", &index.to_string(), result)
                .unwrap()
            {
                kept.push(value);
            }
        }
        assert_eq!(kept, [16, 32]);
        assert_eq!(report.issues.len(), 1);
    }

    #[test]
    fn optional_omissions_are_recorded_in_strict_mode() {
        let mut report = AnalysisReport::default();
        report.note("schemas.parent", "Child", "unresolved parent");
        assert_eq!(report.issues.len(), 1);
        assert!(!report.allow_partial);
    }
}
