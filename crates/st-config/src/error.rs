//! Errors produced while locating, reading, parsing or writing `config.toml`.

use std::path::PathBuf;

/// Anything that can go wrong in this crate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// Neither `$XDG_CONFIG_HOME` nor `$HOME` is set, so the config directory
    /// cannot be derived. Set `$SUPERTERMINAL_CONFIG` to point at a file
    /// directly.
    #[error(
        "cannot determine the configuration directory: none of $SUPERTERMINAL_CONFIG, \
         $XDG_CONFIG_HOME or $HOME is set"
    )]
    NoConfigDir,

    /// `$HOME` is not set and the requested directory has no other fallback.
    #[error("cannot determine the {what} directory: $HOME is not set")]
    NoHomeDir {
        /// Which directory was being resolved (`state`, `cache`, …).
        what: &'static str,
    },

    /// A file could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// The file we tried to read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A file could not be written.
    #[error("cannot write {path}: {source}")]
    Write {
        /// The file we tried to write.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A directory could not be created (or its mode could not be set).
    #[error("cannot create directory {path}: {source}")]
    CreateDir {
        /// The directory we tried to create.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The configuration file is not valid TOML, or a value has the wrong
    /// shape (an unknown enum variant, a malformed colour, …).
    ///
    /// [`ConfigError::line`] and [`ConfigError::column`] expose the 1-based
    /// position reported by the TOML parser; the [`Display`](std::fmt::Display)
    /// form already contains the parser's annotated snippet.
    #[error("invalid configuration in {origin}: {source}")]
    Parse {
        /// Where the text came from: a path, or `<string>` for in-memory input.
        origin: String,
        /// 1-based line of the offending span, when the parser reported one.
        line: Option<usize>,
        /// 1-based column of the offending span, when the parser reported one.
        column: Option<usize>,
        /// The underlying TOML error, including its annotated snippet.
        #[source]
        source: Box<toml::de::Error>,
    },

    /// The configuration could not be turned back into TOML.
    #[error("cannot serialise the configuration: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl ConfigError {
    /// The 1-based line of a [`ConfigError::Parse`], if known.
    pub fn line(&self) -> Option<usize> {
        match self {
            Self::Parse { line, .. } => *line,
            _ => None,
        }
    }

    /// The 1-based column of a [`ConfigError::Parse`], if known.
    pub fn column(&self) -> Option<usize> {
        match self {
            Self::Parse { column, .. } => *column,
            _ => None,
        }
    }

    /// The path this error is about, if it is about a file.
    pub fn path(&self) -> Option<&std::path::Path> {
        match self {
            Self::Read { path, .. } | Self::Write { path, .. } | Self::CreateDir { path, .. } => {
                Some(path)
            }
            _ => None,
        }
    }

    /// Wraps a TOML deserialisation error, computing its line/column from the
    /// reported byte span and the source text.
    pub(crate) fn parse(origin: impl Into<String>, text: &str, source: toml::de::Error) -> Self {
        let (line, column) = match source.span() {
            Some(span) => {
                let (l, c) = line_col(text, span.start);
                (Some(l), Some(c))
            }
            None => (None, None),
        };
        Self::Parse {
            origin: origin.into(),
            line,
            column,
            source: Box::new(source),
        }
    }
}

/// Converts a byte offset into a 1-based `(line, column)` pair.
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = text[line_start..offset].chars().count() + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::line_col;

    #[test]
    fn computes_line_and_column() {
        let text = "a = 1\nbb = 2\nccc = 3\n";
        assert_eq!(line_col(text, 0), (1, 1));
        assert_eq!(line_col(text, 6), (2, 1));
        assert_eq!(line_col(text, 8), (2, 3));
        assert_eq!(line_col(text, 9999), (4, 1));
    }
}
