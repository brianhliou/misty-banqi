#![allow(dead_code)]
use banqi_engine::{features::N_FEATURES, State, Status};
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    pub fn index(&mut self, n: usize) -> usize {
        self.next() as usize % n
    }
    fn weight(&mut self, scale: f32) -> f32 {
        (self.index(2001) as f32 / 1000. - 1.) * scale
    }
}

/// Deterministic legal playouts; the hidden deal is used only to resolve flips.
pub fn positions(count: usize) -> Vec<State> {
    let mut out = Vec::new();
    let mut rng = Rng(300);
    while out.len() < count {
        let mut s = State::initial_site();
        let mut deal = Vec::new();
        for (code, &n) in s.bag.iter().enumerate() {
            deal.extend(std::iter::repeat_n(code as i16, n as usize));
        }
        for i in (1..32).rev() {
            deal.swap(i, rng.index(i + 1));
        }
        for _ in 0..180 {
            if !matches!(s.status(), Status::Playing(_)) || out.len() == count {
                break;
            }
            out.push(s.clone());
            let legal = s.legal_moves();
            let mv = legal[rng.index(legal.len())];
            s = s
                .apply(mv, (mv.0 == mv.1).then_some(deal[mv.0 as usize]))
                .unwrap();
        }
    }
    out
}

pub fn net_bytes(hidden: usize) -> Vec<u8> {
    let mut rng = Rng(2026 + hidden as u64);
    let mut weights = Vec::new();
    weights.extend((0..N_FEATURES * hidden).map(|_| rng.weight(0.01)));
    weights.extend(std::iter::repeat_n(0.2f32, hidden));
    weights.extend((0..32 * hidden).map(|_| rng.weight(0.2 / (hidden as f32).sqrt())));
    weights.extend(std::iter::repeat_n(0.2f32, 16));
    weights.extend((0..512).map(|_| rng.weight(0.1)));
    weights.extend(std::iter::repeat_n(0.2f32, 32));
    weights.extend((0..32).map(|_| rng.weight(0.1)));
    weights.push(0.6);
    let mut bytes = b"BQNN".to_vec();
    for word in [2, N_FEATURES, hidden, 16, 32] {
        bytes.extend((word as u32).to_le_bytes());
    }
    for weight in weights {
        bytes.extend(weight.to_le_bytes());
    }
    bytes
}

pub struct TestFile {
    pub path: PathBuf,
    directory: PathBuf,
}
impl TestFile {
    pub fn new(bytes: &[u8]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "banqi net test {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("generated net.bqnn");
        std::fs::write(&path, bytes).unwrap();
        Self { path, directory }
    }
}
impl Drop for TestFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).unwrap();
        std::fs::remove_dir(&self.directory).unwrap();
    }
}

/// Trained-net checks are opt-in; synthetic-net checks always run.
pub fn external_net() -> Option<PathBuf> {
    match std::env::var_os("MISTY_BANQI_EVALFILE") {
        Some(path) => Some(PathBuf::from(path)),
        None => {
            // Write directly so cargo's default test capture still shows the skip.
            writeln!(
                std::io::stderr(),
                "SKIP trained-net check: MISTY_BANQI_EVALFILE is unset"
            )
            .unwrap();
            None
        }
    }
}

pub struct Uci {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Uci {
    pub fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_banqi-engine"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            input,
            output,
        }
    }
    pub fn send(&mut self, command: &str) {
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
    }
    pub fn until(&mut self, prefix: &str) -> Vec<String> {
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            assert!(
                self.output.read_line(&mut line).unwrap() > 0,
                "UCI closed before {prefix}"
            );
            let line = line.trim().to_owned();
            let done = line.starts_with(prefix);
            lines.push(line);
            if done {
                return lines;
            }
        }
    }
    pub fn ready(&mut self) -> Vec<String> {
        self.send("isready");
        self.until("readyok")
    }
    pub fn load(&mut self, path: &std::path::Path) {
        self.send(&format!("setoption name EvalFile value {}", path.display()));
        assert_eq!(self.ready(), ["readyok"]);
    }
    pub fn go(&mut self, position: &str, go: &str) -> Vec<String> {
        self.send(&format!("position fen {position}"));
        self.send(go);
        self.until("bestmove ")
    }
}
impl Drop for Uci {
    fn drop(&mut self) {
        self.send("quit");
        assert!(self.child.wait().unwrap().success());
    }
}
