//! Getting the painting. The Starry Night (1889) is in the public domain; the
//! copy used is Wikimedia Commons' scan of the Google Art Project image, at a
//! standard thumbnail width. It is fetched once, on first run, into the user's
//! cache directory — never into the repository — and read from there after.

use std::path::PathBuf;
use std::process::Command;

const FILE: &str = "Van_Gogh_-_Starry_Night_-_Google_Art_Project.jpg";
const WIDTHS: [u32; 2] = [960, 1280];
const USER_AGENT: &str = "ascii-engine-starry-night/0.1";

fn url(width: u32) -> String {
    format!("https://upload.wikimedia.org/wikipedia/commons/thumb/e/ea/{FILE}/{width}px-{FILE}")
}

/// `$XDG_CACHE_HOME/ascii-engine`, else `~/.cache/ascii-engine`.
pub fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("ascii-engine"))
}

/// The cached painting, downloading it first if it is not there yet. Each
/// attempt is bounded (`curl --max-time`), retried at most twice by curl, and
/// a second thumbnail size is tried before giving up with a message that says
/// how to supply the picture by hand.
pub fn painting() -> Result<PathBuf, String> {
    let dir = cache_dir().ok_or("no HOME or XDG_CACHE_HOME to keep a cache in; pass --image PATH")?;
    let path = dir.join("starry-night.jpg");
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let part = dir.join("starry-night.jpg.part");
    let mut last = String::new();
    for w in WIDTHS {
        let u = url(w);
        eprintln!("fetching The Starry Night ({w}px) from Wikimedia Commons into {} ...", dir.display());
        let run = Command::new("curl")
            .args(["-fsSL", "--connect-timeout", "15", "--max-time", "60", "--retry", "2"])
            .args(["--retry-delay", "1", "-A", USER_AGENT, "-o"])
            .arg(&part)
            .arg(&u)
            .status();
        match run {
            Err(e) => {
                return Err(format!(
                    "could not run curl to fetch the painting ({e}).\n  \
                     Download it yourself from {} and pass --image PATH.",
                    url(WIDTHS[0])
                ))
            }
            Ok(s) if s.success() => {
                let ok = std::fs::read(&part).map(|d| d.starts_with(&[0xFF, 0xD8])).unwrap_or(false);
                if ok {
                    std::fs::rename(&part, &path)
                        .map_err(|e| format!("cannot move the download into place: {e}"))?;
                    return Ok(path);
                }
                last = format!("{u} did not return a JPEG");
            }
            Ok(s) => last = format!("curl exited {s} for {u}"),
        }
    }
    let _ = std::fs::remove_file(&part);
    Err(format!(
        "could not fetch the painting: {last}.\n  \
         Check the network, or download it yourself from {} and pass --image PATH.",
        url(WIDTHS[0])
    ))
}
