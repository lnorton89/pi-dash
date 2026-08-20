//! Finding the API's local-agent token.
//!
//! The API writes one at startup into the directory it already shares with this
//! unit's host-side agents, mode 0640. A process that can read it is, by that
//! fact, running as the account that owns the deployment — which is the whole
//! argument for letting it read the API without a person pasting a session
//! cookie out of a browser first.
//!
//! Nothing here is a fallback for a missing session: it is the ordinary way
//! pi-dash authenticates on the box it is watching. `CLASSG_SESSION` stays
//! ahead of it, because someone who set that meant it — pointing this build at
//! a *different* unit over the network, say, where no local file could help.
//!
//! Every lookup failure is silent and returns `None`. A dashboard that cannot
//! find a token draws the degraded pane it has always drawn; one that printed a
//! diagnostic per missing candidate would be unreadable on the common
//! deployment where authentication is off and none of this matters.

use std::path::{Path, PathBuf};

/// The filename `services/api/internal/auth/localagent.go` writes.
const TOKEN_FILE: &str = "local-api-token";

/// The agents' state directory name inside a checkout, from
/// `scripts/pi-autodeploy.sh`: `STATE_DIR="${CLASSG_DEPLOY_STATE:-$REPO_DIR/.agent-state}"`.
const STATE_DIR: &str = ".agent-state";

/// Reads the local-agent token, or `None` when there is not one to read.
pub(crate) fn discover() -> Option<String> {
    candidates().into_iter().find_map(|path| read(&path))
}

/// Where to look, in order.
///
/// Deliberately short. Each entry is a place the token genuinely is on some
/// real deployment, rather than a guess:
///
/// 1. `CLASSG_LOCAL_TOKEN` — an explicit path, for a layout none of the rest
///    describes and for tests.
/// 2. `CLASSG_DEPLOY_STATE` — the same variable the deploy and watchdog agents
///    read, so a unit that has moved its state directory has moved this too.
/// 3. Upwards from this binary. pi-dash is built inside the checkout it
///    watches (`<repo>/tools/pi-dash/target/release/pi-dash`), so the repo root
///    — and `.agent-state` beside it — is a few parents up. This is what makes
///    the common case need no configuration at all.
/// 4. `~/.local/state/classg` — the agents' documented default when they are
///    not running out of a checkout.
fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();

    if let Some(explicit) = non_empty_env("CLASSG_LOCAL_TOKEN") {
        out.push(PathBuf::from(explicit));
    }
    if let Some(dir) = non_empty_env("CLASSG_DEPLOY_STATE") {
        out.push(Path::new(&dir).join(TOKEN_FILE));
    }
    if let Ok(exe) = std::env::current_exe() {
        // Bounded rather than "walk to /": four is enough for
        // target/release/pi-dash inside tools/pi-dash, and an unbounded walk
        // would happily find a stranger's state directory on the way up.
        let mut dir = exe.parent();
        for _ in 0..5 {
            let Some(here) = dir else { break };
            out.push(here.join(STATE_DIR).join(TOKEN_FILE));
            dir = here.parent();
        }
    }
    if let Some(home) = dirs::home_dir() {
        out.push(
            home.join(".local")
                .join("state")
                .join("classg")
                .join(TOKEN_FILE),
        );
    }
    out
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Reads one candidate. The file ends with a newline so that `cat` and shell
/// capture behave; the credential is what is left after trimming it.
fn read(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let token = raw.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory of our own. No dev-dependency for three tests: this
    /// crate is built on the Pi it runs on, where every added crate is a
    /// download over whatever link that Pi has.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pi-dash-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create scratch dir");
            Scratch(dir)
        }

        fn file(&self, body: &str) -> PathBuf {
            let path = self.0.join(TOKEN_FILE);
            std::fs::write(&path, body).expect("write");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reads_a_token_and_strips_the_trailing_newline() {
        let scratch = Scratch::new("reads");
        let path = scratch.file("abc123\n");
        assert_eq!(read(&path).as_deref(), Some("abc123"));
    }

    #[test]
    fn an_empty_file_is_not_a_token() {
        // The API writes and renames, so a reader should never see a partial
        // file -- but an empty one must mean "no token" rather than an empty
        // credential the API would answer 401 to on every poll.
        let scratch = Scratch::new("empty");
        for body in ["", "\n", "   \n"] {
            let path = scratch.file(body);
            assert_eq!(read(&path), None, "body {body:?} was read as a token");
        }
    }

    #[test]
    fn a_missing_file_is_silent() {
        assert_eq!(read(Path::new("/nonexistent/classg/local-api-token")), None);
    }

    #[test]
    fn the_search_is_bounded() {
        // An unbounded walk toward / would eventually test paths outside this
        // deployment entirely. Five parents covers
        // <repo>/tools/pi-dash/target/release/pi-dash with room to spare.
        let paths = candidates();
        assert!(
            paths.len() < 12,
            "the candidate list has grown to {} entries; every one of them is a file this \
             process stats on every start",
            paths.len()
        );
    }
}
