use std::fmt;

/// Why an abbreviation can't be expanded.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The abbreviation doesn't parse; `position` is the byte offset where
    /// parsing stopped.
    Syntax {
        position: usize,
        message: &'static str,
    },
    /// A name that can't be an element (`1div`, `h$` outside a repeat).
    InvalidName(String),
    /// A CSS abbreviation that is neither a known property abbreviation nor
    /// `property:value`.
    UnknownAbbreviation(String),
    /// The expansion would exceed a limit: more than 1000 repetitions, too
    /// many elements, too deep, or more than 1 MB of output.
    TooLarge,
    /// The operation doesn't apply to the syntax.
    Unsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { position, message } => {
                write!(f, "invalid abbreviation at {position}: {message}")
            }
            Self::InvalidName(name) => write!(f, "`{name}` is not a valid element name"),
            Self::UnknownAbbreviation(abbreviation) => {
                write!(f, "unknown CSS abbreviation `{abbreviation}`")
            }
            Self::TooLarge => f.write_str("the expansion is too large"),
            Self::Unsupported(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for Error {}
