mod common;
use banqi_engine::features::N_FEATURES;
use common::{external_net, net_bytes, TestFile, Uci};
use std::time::Instant;

const SPARSE: &str = "8/8/8/G6r r - 0 1";
const OPENING: &str = "XXXXXXXX/XXXXXXXX/XXXXXXXX/XXXXXXXX - G1A2E2R2H2C2S5g1a2e2r2h2c2s5 0 1";

// Two flips that score exactly the same (cp -21 at depth 1), where macOS/aarch64
// and Linux/x86-64 break the tie differently. Every release from 0.2.7 on does
// this, so the fixture (recorded on macOS) also accepts the Linux move.
const PLATFORM_TIES: &[(&str, &str)] = &[(
    "SXXXXXRX/cXXXXHXX/XXXXXXXX/XXAXXXXX b G1A1E2R1H1C2S4g1a2e2r2h2c1s5 0 1",
    "bestmove h2h2",
)];

fn fixture(text: &str, net: Option<&std::path::Path>) {
    let mut uci = Uci::new();
    if let Some(path) = net {
        uci.load(path);
    }
    let mut count = 0;
    let mut stages = [0; 3];
    for row in text
        .lines()
        .filter(|s| !s.starts_with('#') && !s.is_empty())
    {
        let fields: Vec<_> = row.split('\t').collect();
        assert_eq!(fields.len(), 6);
        let [stage, position, nodes, limit, moves, expected] = fields.as_slice() else {
            unreachable!()
        };
        match *stage {
            "opening" => stages[0] += 1,
            "midgame" => stages[1] += 1,
            "endgame" => stages[2] += 1,
            _ => {}
        }
        uci.send("ucinewgame");
        uci.send(&format!("setoption name NoProgressLimit value {limit}"));
        let mut go = format!("go nodes {nodes} movetime 0");
        if !moves.is_empty() {
            go.push_str(&format!(" searchmoves {moves}"));
        }
        let reply = uci.go(position, &go);
        let got = reply.last().unwrap();
        let tie = PLATFORM_TIES
            .iter()
            .any(|(fen, alt)| fen == position && got == alt);
        assert!(
            got == expected || tie,
            "position {count}: {position}: got {got}, expected {expected}"
        );
        count += 1;
    }
    assert!(count >= 50);
    assert!(stages.iter().all(|&n| n >= 20));
}

#[test]
fn classical_matches_v027_fixture() {
    fixture(include_str!("fixtures/uci-v027.tsv"), None);
}

#[test]
fn external_net_matches_reference_fixture() {
    let Some(path) = external_net() else { return };
    fixture(include_str!("fixtures/uci-net.tsv"), Some(&path));
}

#[test]
fn handshake_advertises_eval_options_and_release_version() {
    let mut uci = Uci::new();
    uci.send("uci");
    let reply = uci.until("uciok");
    assert!(reply.contains(&"id name MistyBanqi 0.3.0".into()));
    assert!(reply.contains(&"option name EvalFile type string default <empty>".into()));
    assert!(
        reply.contains(&"option name NoProgressLimit type spin default 40 min 1 max 1000".into())
    );
}

fn zero_node_reply(uci: &mut Uci) -> Vec<String> {
    uci.go(SPARSE, "go nodes 0 movetime 0")
        .into_iter()
        .map(|line| {
            // Wall time is diagnostic, never part of deterministic assertions.
            if line.starts_with("info ") {
                format!("score cp {}", line.split("score cp ").nth(1).unwrap())
            } else {
                line
            }
        })
        .collect()
}

#[test]
fn load_paths_with_spaces_clear_and_newgame_persistence() {
    let file = TestFile::new(&net_bytes(7));
    let mut uci = Uci::new();
    let classical = zero_node_reply(&mut uci);
    uci.send(&format!(
        "setoption name eVaLfIlE value {}",
        file.path.display()
    ));
    assert_eq!(uci.ready(), ["readyok"]);
    let net = zero_node_reply(&mut uci);
    assert_ne!(classical, net, "nonzero net bias must be observable");
    uci.send("ucinewgame");
    assert_eq!(zero_node_reply(&mut uci), net);
    for clear in [
        "setoption name EvalFile value <empty>",
        "setoption name EvalFile value ",
    ] {
        uci.load(&file.path);
        uci.send(clear);
        assert_eq!(uci.ready(), ["readyok"]);
        assert_eq!(zero_node_reply(&mut uci), classical);
    }
}

#[test]
fn failed_loads_report_errors_and_clear_previous_net() {
    let valid = TestFile::new(&net_bytes(7));
    let bad = TestFile::new(b"invalid BQNN");
    let missing = bad.path.with_file_name("absent net.bqnn");
    let mut unsafe_bytes = net_bytes(1);
    for i in 0..N_FEATURES {
        unsafe_bytes[24 + 4 * i..28 + 4 * i].copy_from_slice(&2f32.to_le_bytes());
    }
    let unsafe_net = TestFile::new(&unsafe_bytes);
    let mut uci = Uci::new();
    let classical = zero_node_reply(&mut uci);
    for path in [&bad.path, &missing, &unsafe_net.path] {
        uci.load(&valid.path);
        assert_ne!(zero_node_reply(&mut uci), classical);
        uci.send(&format!("setoption name EvalFile value {}", path.display()));
        let reply = uci.ready();
        assert_eq!(reply.len(), 2);
        assert!(reply[0].starts_with("info string error EvalFile: "));
        assert!(reply[0].ends_with("; using classical"));
        assert_eq!(zero_node_reply(&mut uci), classical);
    }
}

#[test]
fn synthetic_net_search_is_deterministic_and_root_restricted() {
    let mut uci = Uci::new();
    let file = TestFile::new(&net_bytes(7));
    uci.load(&file.path);
    for mv in ["a0a0", "h3h3"] {
        let go = format!("go nodes 5000 movetime 0 searchmoves {mv}");
        let first = uci.go(OPENING, &go);
        let second = uci.go(OPENING, &go);
        assert_eq!(first.last(), second.last());
        assert_eq!(first.last().unwrap(), &format!("bestmove {mv}"));
    }
}

#[test]
fn movetime_and_node_limits_are_preserved_in_both_modes() {
    for net in [false, true] {
        let mut uci = Uci::new();
        if net {
            let file = TestFile::new(&net_bytes(7));
            uci.load(&file.path);
        }
        let reply = uci.go(OPENING, "go nodes 5000 movetime 0");
        let info = &reply[0];
        let nodes: u64 = info
            .split(" nodes ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            nodes, 5001,
            "same visited-node accounting, including the aborting node"
        );
        let started = Instant::now();
        let reply = uci.go(OPENING, "go nodes 100000000 movetime 1");
        assert!(started.elapsed().as_secs() < 5);
        assert!(reply.last().unwrap().starts_with("bestmove "));
        let nodes: u64 = reply[0]
            .split(" nodes ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            nodes < 100000000,
            "wall clock should bind before the node ceiling"
        );
    }
}

#[test]
fn no_progress_option_survives_net_switching_and_newgame() {
    let mut uci = Uci::new();
    uci.send("setoption name NoProgressLimit value 60");
    let file = TestFile::new(&net_bytes(7));
    uci.load(&file.path);
    uci.send("ucinewgame");
    let position = "8/8/8/G6r r - 45 1";
    assert_ne!(
        uci.go(position, "go nodes 5000 movetime 0").last().unwrap(),
        "bestmove (none)"
    );
    uci.send("setoption name NoProgressLimit value 40");
    assert_eq!(
        uci.go(position, "go nodes 5000 movetime 0").last().unwrap(),
        "bestmove (none)"
    );
    uci.send("setoption name EvalFile value <empty>");
    let reply = uci.go(position, "go nodes 5000 movetime 0");
    assert!(reply[0].contains("score cp -100 "));
}

#[test]
fn noisy_play_uses_net_and_preserves_seed_depth_and_options_across_switches() {
    // A constant logit makes a one-ply NNUE score distinguishable from classical.
    let mut bytes = net_bytes(1);
    bytes[24..].fill(0);
    let end = bytes.len();
    bytes[end - 4..].copy_from_slice(&0.6f32.to_le_bytes());
    let file = TestFile::new(&bytes);
    let mut uci = Uci::new();
    uci.send("setoption name Noise value 0.3");
    uci.send("setoption name NoiseDepth value 2");
    let mut choices = std::collections::HashSet::new();
    for seed in 1..=8 {
        uci.send(&format!("setoption name Seed value {seed}"));
        uci.load(&file.path);
        let first = uci.go(OPENING, "go nodes 1 movetime 0");
        assert!(first[0].starts_with("info depth 1 nodes "), "{first:?}");
        assert!(first[0].contains("score cp -291 "), "{first:?}");
        choices.insert(first.last().unwrap().clone());
        uci.send("setoption name EvalFile value <empty>");
        uci.send("ucinewgame");
        uci.load(&file.path);
        assert_eq!(
            uci.go(OPENING, "go nodes 1 movetime 0").last(),
            first.last()
        );
    }
    assert!(
        choices.len() > 1,
        "different seeds must vary noisy NNUE choices"
    );
    let restricted = uci.go(OPENING, "go nodes 1 movetime 0 searchmoves h3h3");
    assert_eq!(restricted.last().unwrap(), "bestmove h3h3");
    // A small position completes both passes with a generous node budget.
    let deeper = uci.go(SPARSE, "go nodes 10000 movetime 0");
    assert!(deeper[0].starts_with("info depth 2 nodes "), "{deeper:?}");
    let win = uci.go("8/8/8/As6 r - 0 1", "go nodes 1 movetime 0");
    assert!(win[0].contains("score cp 1000 "), "{win:?}");
    assert_eq!(win.last().unwrap(), "bestmove a0b0");
    uci.send("setoption name Noise value 0");
    let net = zero_node_reply(&mut uci);
    assert!(net[0].starts_with("score cp 291 "));
    uci.send("setoption name EvalFile value <empty>");
    assert_ne!(zero_node_reply(&mut uci), net);
}
