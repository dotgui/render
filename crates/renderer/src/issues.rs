//! What a document did wrong, collected while it is read.
//!
//! A 0.3 document is held to the rules RFC-0042, RFC-0043 and RFC-0044 make
//! errors: it fails loudly and is not drawn. A document declaring 0.2 was
//! written before those rules existed, so the same findings only warn and it
//! renders exactly as it did before.

#[derive(Debug, Default)]
pub(crate) struct Issues {
    strict: bool,
    pub(crate) errors: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

impl Issues {
    pub(crate) fn new(strict: bool) -> Self {
        Self {
            strict,
            ..Self::default()
        }
    }

    /// Whether the document is read under 0.3 rules.
    pub(crate) fn strict(&self) -> bool {
        self.strict
    }

    /// A broken rule: an error under 0.3, a warning under 0.2.
    pub(crate) fn violation(&mut self, message: impl Into<String>) {
        if self.strict {
            self.errors.push(message.into());
        } else {
            self.warnings.push(message.into());
        }
    }

    /// An error whatever the version, for rules that only exist in 0.3 structure.
    pub(crate) fn error(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
    }

    /// Advice that never stops a render, such as `slot-min`.
    pub(crate) fn advise(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
}
