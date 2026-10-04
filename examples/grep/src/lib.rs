//! A streaming search service: finds lines containing a substring in all
//! files below a directory and streams the matches to Emacs.
//!
//! ```elisp
//! (setq g (emrs-start 'grep-rs "grep" nil))
//! (setq call (emrs-call-stream g '(:grep :pattern "TODO" :root "/src")
//!              :on-batch (lambda (items) (message "%d matches" (length items)))
//!              :on-end (lambda (status err) (message "grep: %s" status))))
//! (emrs-cancel call)   ; e.g. when the user changes the query
//! ```

use emrs::{CancelToken, Context, Error, Reply, Service, StreamSender};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
enum Request {
    Grep { pattern: String, root: PathBuf },
}

#[derive(Serialize)]
struct Match {
    path: PathBuf,
    line_number: u64,
    preview: String,
}

struct Grep;

/// Search `dir` recursively. Returns false once the stream is closed.
fn search(dir: &Path, pattern: &str, out: &StreamSender<Match>, token: &CancelToken) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return true;
    };
    for entry in entries.flatten() {
        if token.is_cancelled() {
            return false;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            if !search(&path, pattern, out, token) {
                return false;
            }
        } else if file_type.is_file() {
            let Ok(file) = std::fs::File::open(&path) else { continue };
            for (index, line) in BufReader::new(file).lines().enumerate() {
                // A line that is not UTF-8 means a binary file: skip the rest.
                let Ok(line) = line else { break };
                if line.contains(pattern) {
                    let found = Match { path: path.clone(), line_number: index as u64 + 1, preview: line };
                    if out.send(found).is_err() {
                        return false;
                    }
                }
            }
        }
    }
    true
}

impl Service for Grep {
    const NAME: &'static str = "grep";
    type Args = ();
    type Request = Request;
    type Event = ();

    fn start(_: (), _: &Context<Self>) -> Result<Self, Error> {
        Ok(Grep)
    }

    fn handle(&mut self, request: Request, reply: Reply, ctx: &Context<Self>) {
        match request {
            Request::Grep { pattern, root } => {
                if !root.is_dir() {
                    reply.err(Error::new(format!("{} is not a directory", root.display())));
                    return;
                }
                let out = reply.stream::<Match>(1024);
                ctx.spawn(move |_| {
                    let token = out.token();
                    if search(&root, &pattern, &out, &token) {
                        out.finish();
                    }
                });
            }
        }
    }
}

emrs::module!("grep-rs", [Grep]);
