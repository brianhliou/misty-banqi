use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const OPENING: &str = "XXXXXXXX/XXXXXXXX/XXXXXXXX/XXXXXXXX - G1A2E2R2H2C2S5g1a2e2r2h2c2s5 0 1";

fn run(commands: &str, timeout: Duration) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_banqi-engine"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(commands.as_bytes())
        .unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() >= timeout {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "UCI search exceeded {timeout:?}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    line.split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1])
}

#[test]
fn advertises_exact_play_options_and_release_version() {
    let output = run("uci\nisready\nquit\n", Duration::from_secs(2));
    assert!(output.contains("id name MistyBanqi 0.3.0\n"));
    assert_eq!(
        output
            .lines()
            .filter(|l| l.starts_with("option "))
            .collect::<Vec<_>>(),
        [
            "option name NoProgressLimit type spin default 40 min 1 max 1000",
            "option name Noise type string default 0",
            "option name NoiseDepth type spin default 1 min 1 max 8",
            "option name Seed type string default 0",
            "option name EvalFile type string default <empty>",
        ]
    );
    assert!(output.ends_with("uciok\nreadyok\n"));
}

#[test]
fn seeded_choices_replay_vary_and_survive_newgame() {
    let mut commands = String::new();
    for seed in 1..=8 {
        commands.push_str(&format!(
            "setoption name Noise value 0.3\nsetoption name NoiseDepth value 2\n\
             setoption name Seed value {seed}\nucinewgame\nposition fen {OPENING}\n\
             go nodes 1 movetime 0\ngo nodes 1 movetime 0\n"
        ));
    }
    commands.push_str("quit\n");
    let output = run(&commands, Duration::from_secs(5));
    let moves: Vec<_> = output
        .lines()
        .filter(|l| l.starts_with("bestmove "))
        .collect();
    assert_eq!(moves.len(), 16, "{output}");
    for pair in moves.chunks_exact(2) {
        assert_eq!(pair[0], pair[1]);
    }
    assert!(moves.iter().collect::<std::collections::HashSet<_>>().len() > 1);
    for line in output.lines().filter(|l| l.starts_with("info ")) {
        assert_eq!(field(line, "depth"), Some("1"));
        assert!(field(line, "nodes").unwrap().parse::<u64>().unwrap() > 1);
    }
}

#[test]
fn zero_noise_matches_recorded_moves_scores_depths_and_nodes() {
    // Recorded with the previous release, before adding weak play.
    let positions = [
        (OPENING, None, "401", "0", "a0a0"),
        ("8/8/8/G1s5 r - 0 1", Some("13"), "401", "-1000", "a0a1"),
        (
            "8/2X5/1X6/A4X1s r E2g1 0 1",
            Some("2"),
            "401",
            "-189",
            "a0b0",
        ),
        ("8/8/8/As6 r - 0 1", Some("24"), "294", "1000", "a0b0"),
    ];
    for seed in [0, 1, 123] {
        let mut commands = format!(
            "setoption name Noise value 0\nsetoption name NoiseDepth value 8\n\
             setoption name Seed value {seed}\nucinewgame\n"
        );
        for (fen, ..) in positions {
            commands.push_str(&format!("position fen {fen}\ngo nodes 400 movetime 0\n"));
        }
        commands.push_str("quit\n");
        let output = run(&commands, Duration::from_secs(2));
        let info: Vec<_> = output.lines().filter(|l| l.starts_with("info ")).collect();
        assert_eq!(info.len(), positions.len(), "{output}");
        for (line, (_, depth, nodes, cp, pv)) in info.iter().zip(positions) {
            for (name, expected) in [
                ("depth", depth),
                ("nodes", Some(nodes)),
                ("cp", Some(cp)),
                ("pv", Some(pv)),
            ] {
                assert_eq!(field(line, name), expected, "{output}");
            }
        }
        assert_eq!(
            output
                .lines()
                .filter(|l| l.starts_with("bestmove "))
                .collect::<Vec<_>>(),
            positions
                .iter()
                .map(|p| format!("bestmove {}", p.4))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn noisy_depth_four_obeys_time_and_node_limits() {
    for limits in [
        "movetime 1000",
        "nodes 10000 movetime 0",
        "nodes 10000 movetime 1000",
    ] {
        let commands = format!(
            "setoption name Noise value 0.01\nsetoption name NoiseDepth value 4\n\
             setoption name Seed value 123\nposition fen {OPENING}\ngo {limits}\nquit\n"
        );
        let start = Instant::now();
        let output = run(&commands, Duration::from_secs(2));
        assert!(start.elapsed() < Duration::from_secs(2));
        let line = output.lines().find(|l| l.starts_with("info ")).unwrap();
        let depth = field(line, "depth").unwrap().parse::<i32>().unwrap();
        assert!((1..4).contains(&depth), "{output}");
        let nodes = field(line, "nodes").unwrap().parse::<u64>().unwrap();
        if limits.starts_with("nodes") {
            assert_eq!(nodes, 10_001);
        }
        assert!(nodes > 0);
        assert!(output
            .lines()
            .any(|l| l.starts_with("bestmove ") && l != "bestmove (none)"));
    }
}

#[test]
fn nonbinding_limits_replay_same_move_score_and_consumption() {
    let output = run(
        &format!(
            "setoption name Noise value 0.3\nsetoption name NoiseDepth value 2\n\
         setoption name Seed value 123\nposition fen {OPENING}\n\
         go nodes 10000000 movetime 10000\ngo nodes 10000000 movetime 10000\nquit\n"
        ),
        Duration::from_secs(5),
    );
    let info: Vec<_> = output.lines().filter(|l| l.starts_with("info ")).collect();
    assert_eq!(info.len(), 2, "{output}");
    for name in ["depth", "nodes", "cp", "pv"] {
        assert_eq!(field(info[0], name), field(info[1], name), "{output}");
    }
    assert_eq!(field(info[0], "depth"), Some("2"));
}

#[test]
fn noisy_search_keeps_existing_root_filter_and_no_progress_option() {
    let output = run(
        &format!(
            "setoption name Noise value 0.3\nsetoption name NoiseDepth value 2\n\
         setoption name Seed value 18446744073709551615\nucinewgame\n\
         position fen {OPENING}\ngo nodes 1000 searchmoves a0a0\n\
         setoption name NoProgressLimit value 60\nucinewgame\n\
         position fen 7s/8/R7/GR6 r - 45 30\ngo nodes 1000 movetime 0\nquit\n"
        ),
        Duration::from_secs(2),
    );
    assert!(output.contains("bestmove a0a0\n"), "{output}");
    let info: Vec<_> = output.lines().filter(|l| l.starts_with("info ")).collect();
    assert_eq!(info.len(), 2, "{output}");
    assert!(
        field(info[1], "cp").unwrap().parse::<i64>().unwrap() > 200,
        "{output}"
    );
}
