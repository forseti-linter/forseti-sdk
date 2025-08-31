use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// Minimal host for spawning an engine process that speaks Forseti NDJSON.
pub struct EngineProcess {
    #[allow(dead_code)]
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl EngineProcess {
    pub fn spawn(cmd: &str, args: &[&str]) -> std::io::Result<Self> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    pub fn send_line(&mut self, line: &str) -> std::io::Result<()> {
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()
    }

    /// Blocking read of one NDJSON line from engine stdout.
    pub fn read_line(&mut self) -> std::io::Result<String> {
        let mut buf = String::new();
        self.stdout.read_line(&mut buf)?;
        Ok(buf)
    }
}
