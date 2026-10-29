//! `peet serve`, as a program driving it sees it: a process that stays up, one operation
//! written, one reply read, with the documents kept open in between.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct Server {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl Server {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_peet"))
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
        }
    }

    /// Writes one line and reads the one reply to it. The reply must come while the
    /// input is still open: the server answers each line before reading the next.
    fn line(&mut self, line: &str) -> Value {
        let input = self.input.as_mut().unwrap();
        writeln!(input, "{line}").unwrap();
        input.flush().unwrap();
        let mut reply = String::new();
        self.output.read_line(&mut reply).unwrap();
        serde_json::from_str(&reply).unwrap_or_else(|e| panic!("{e}: {reply:?}"))
    }

    fn send(&mut self, op: Value) -> Value {
        self.line(&op.to_string())
    }

    /// Ends the input and waits: the exit code, and anything still written.
    fn finish(mut self) -> (i32, String) {
        drop(self.input.take());
        let mut rest = String::new();
        std::io::Read::read_to_string(&mut self.output, &mut rest).unwrap();
        (self.child.wait().unwrap().code().unwrap(), rest)
    }
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("peet-serve-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn documents_stay_open_between_operations() {
    let dir = folder("documents");
    let plate = dir.join("plate.peet");
    let frame = dir.join("frame.peet");
    let mut s = Server::start();

    // A part, built over several calls and saved.
    let r = s.send(json!({"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [60, 40]}]}));
    assert_eq!(r["ok"], true, "{r}");
    let r = s.send(json!({"op": "extrude", "sketch": "Sketch1", "depth": 6}));
    assert_eq!(r["created"][0]["name"], "Extrude1", "{r}");
    let r = s.send(json!({"op": "save", "path": plate}));
    assert_eq!(r["ok"], true, "{r}");

    // An assembly beside it, which inserts the saved part, then the part changed again.
    let r = s.send(json!({"op": "new", "assembly": true, "keep": true}));
    assert_eq!(r["document"], 2, "{r}");
    let r = s.send(json!({"op": "insert", "path": plate, "link": true}));
    assert_eq!(r["component"]["name"], "plate-1", "{r}");
    let r = s.send(json!({"op": "save", "path": frame}));
    assert_eq!(r["ok"], true, "{r}");
    let r = s.send(json!({"op": "edit", "feature": "Extrude1", "depth": 9, "document": 1}));
    assert_eq!(r["ok"], true, "{r}");
    let r = s.send(json!({"op": "documents"}));
    let docs = r["documents"].as_array().unwrap();
    assert_eq!(docs.len(), 2, "{r}");
    assert_eq!(docs[0]["modified"], true);
    assert_eq!(docs[1]["current"], true);

    // Undo reaches back across calls.
    let r = s.send(json!({"op": "undo", "document": 1}));
    assert_eq!(r["ok"], true, "{r}");

    // Mistakes are replies, and the server carries on.
    let r = s.line("{\"op\": \"extrude\", ");
    assert_eq!(r["ok"], false);
    let r = s.line("[{\"op\": \"status\"}]");
    assert_eq!(r["ok"], false);
    let r = s.send(json!({"op": "extrude", "sketch": "Nope", "depth": 1, "document": 1}));
    assert_eq!(r["ok"], false);
    assert!(r["error"].as_str().unwrap().contains("Nope"), "{r}");
    let r = s.send(json!({"op": "switch", "document": "frame.peet"}));
    assert_eq!(r["ok"], true, "{r}");
    let r = s.send(json!({"op": "components"}));
    assert_eq!(r["components"][0]["name"], "plate-1", "{r}");

    // Closing, and opening again.
    let r = s.send(json!({"op": "close"}));
    assert_eq!(r["closed"], "frame.peet", "{r}");
    let r = s.send(json!({"op": "open", "path": frame, "keep": true}));
    assert_eq!(r["kind"], "assembly", "{r}");
    let r = s.send(json!({"op": "close", "document": "plate.peet", "discard": true}));
    assert_eq!(r["ok"], true, "{r}");

    let r = s.send(json!({"op": "exit"}));
    assert_eq!(r, json!({"ok": true, "op": "exit", "unsaved": []}));
    let (code, rest) = s.finish();
    assert_eq!(code, 0);
    assert_eq!(rest, "", "nothing after the reply to exit");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_end_of_the_input_ends_it() {
    let mut s = Server::start();
    // Blank lines and comments are not operations: no reply.
    let r = s.line("\n# a comment\n{\"op\": \"status\"}");
    assert_eq!(r["kind"], "part", "{r}");
    let (code, rest) = s.finish();
    assert_eq!(code, 0);
    assert_eq!(rest, "");
}
