//! A bearer kept in a file, re-read when the file changes.
//!
//! Reading it once at start is how a console ends up answering 401 for good: the
//! engine mints its token at boot, so a console that starts first finds no file,
//! and one that outlives a re-mint holds the old word (#163, forge1). It costs one
//! `stat` a request.

use std::path::PathBuf;
use std::sync::RwLock;
use std::time::SystemTime;

enum Source {
    Fixed(Option<String>),
    File { path: PathBuf, seen: RwLock<(Option<SystemTime>, Option<String>)> },
}

pub struct FileToken(Source);

impl FileToken {
    /// A word that does not change, or none.
    pub fn fixed(token: Option<String>) -> Self {
        FileToken(Source::Fixed(clean(token)))
    }

    /// A word in a file, which may not be there yet.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        FileToken(Source::File { path: path.into(), seen: RwLock::new((None, None)) })
    }

    /// The word as it is now.
    pub fn get(&self) -> Option<String> {
        match &self.0 {
            Source::Fixed(t) => t.clone(),
            Source::File { path, seen } => {
                let stamp = std::fs::metadata(path).and_then(|m| m.modified()).ok();
                {
                    let s = seen.read().unwrap_or_else(|e| e.into_inner());
                    if s.0 == stamp && s.1.is_some() {
                        return s.1.clone();
                    }
                }
                let word = clean(std::fs::read_to_string(path).ok());
                let mut s = seen.write().unwrap_or_else(|e| e.into_inner());
                if word.is_none() && stamp != s.0 {
                    tracing::warn!(file = %path.display(), "token file missing, empty or unreadable");
                }
                *s = (stamp, word.clone());
                word
            }
        }
    }
}

fn clean(t: Option<String>) -> Option<String> {
    t.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_followed_from_missing_through_renewed_to_gone() {
        let d = std::env::temp_dir().join(format!("token-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("api_token");
        let _ = std::fs::remove_file(&p);
        let t = FileToken::file(&p);
        assert_eq!(t.get(), None, "not minted yet");
        std::fs::write(&p, "one\n").unwrap();
        assert_eq!(t.get().as_deref(), Some("one"), "picked up with no restart");
        std::fs::write(&p, "two").unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(later).unwrap();
        assert_eq!(t.get().as_deref(), Some("two"), "renewed in place");
        std::fs::remove_file(&p).unwrap();
        assert_eq!(t.get(), None);
        assert_eq!(FileToken::fixed(Some(" x ".into())).get().as_deref(), Some("x"));
        assert_eq!(FileToken::fixed(Some("  ".into())).get(), None);
    }
}
