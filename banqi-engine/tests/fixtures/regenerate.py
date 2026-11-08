#!/usr/bin/env python3
"""Regenerate fixed-node UCI fixtures from a supplied reference binary.

Classical: regenerate.py classical --binary BIN --output uci-v027.tsv
NNUE:     regenerate.py net --binary BIN --net NET --output uci-net.tsv
The net reference must accept --rules site --eval NET. Time ceilings are
large enough that the node budget alone determines every recorded move.
"""
import argparse
import random
import subprocess
from pathlib import Path

LETTERS = 'GAERHCSgaerhcs'
COUNTS = [1, 2, 2, 2, 2, 2, 5] * 2


def fen(board, bag, turn, clock=0):
    ranks = []
    for rank in range(3, -1, -1):
        text, empty = '', 0
        for c in board[rank * 8:rank * 8 + 8]:
            if c == -1:
                empty += 1
            else:
                if empty:
                    text += str(empty)
                    empty = 0
                text += 'X' if c == -2 else LETTERS[c]
        ranks.append(text + (str(empty) if empty else ''))
    pool = ''.join(f'{c}{n}' for c, n in zip(LETTERS, bag) if n) or '-'
    return f'{"/".join(ranks)} {turn} {pool} {clock} 1'


def positions():
    rng = random.Random(300)
    # Public snapshots: each piece comes from the initial multiset, and the
    # bag always contains exactly the identities of the remaining covered tiles.
    for stage in ['opening', 'midgame', 'endgame']:
        for i in range(24):
            pieces = [c for c, n in enumerate(COUNTS) for _ in range(n)]
            rng.shuffle(pieces)
            board, bag = [-1] * 32, [0] * 14
            squares = list(range(32))
            rng.shuffle(squares)
            if stage == 'opening':
                revealed, covered = i % 9, 32 - i % 9
            elif stage == 'midgame':
                revealed, covered = 8 + i % 9, 2 + i % 7
            else:
                revealed, covered = 2 + i % 7, 0
            # Keep both colours present in bound positions.
            if stage == 'endgame':
                red = next(c for c in pieces if c < 7)
                black = next(c for c in pieces if c >= 7)
                pieces.remove(red)
                pieces.remove(black)
                pieces = [red, black] + pieces
            for sq, c in zip(squares[:revealed], pieces[:revealed]):
                board[sq] = c
            for sq, c in zip(squares[revealed:revealed + covered],
                             pieces[revealed:revealed + covered]):
                board[sq] = -2
                bag[c] += 1
            turn = '-' if revealed == 0 else ('r' if i % 2 == 0 else 'b')
            clock = (i % 10) if stage == 'endgame' else 0
            yield stage, fen(board, bag, turn, clock), 5000, 40, ''
    # Host options, root restrictions, and a quiet repetition window.
    sparse = '8/8/8/G6r r -'
    for limit in [40, 60]:
        yield 'clock', f'{sparse} 45 1', 5000, limit, ''
    for moves in ['a0b0', 'a0a1', 'h3h3']:
        yield 'restricted', f'{sparse} 0 1', 5000, 40, moves
    yield 'history', f'{sparse} 0 1 moves a0b0 h0h1 b0a0 h1h0', 5000, 40, ''


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('mode', choices=['classical', 'net'])
    p.add_argument('--binary', type=Path, required=True)
    p.add_argument('--net', type=Path)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    command = [str(args.binary.resolve())]
    if args.mode == 'net':
        if args.net is None:
            p.error('--net is required in net mode')
        command += ['--rules', 'site', '--eval', str(args.net.resolve())]
    rows = list(positions())
    lines = ['uci']
    for _, position, nodes, limit, restricted in rows:
        lines += ['ucinewgame', f'setoption name NoProgressLimit value {limit}',
                  f'position fen {position}',
                  f'go nodes {nodes} movetime 600000' +
                  (f' searchmoves {restricted}' if restricted else '')]
    lines += ['quit']
    result = subprocess.run(command, input='\n'.join(lines) + '\n', text=True,
                            capture_output=True, check=True, timeout=180)
    best = [line for line in result.stdout.splitlines() if line.startswith('bestmove ')]
    if len(best) != len(rows):
        raise RuntimeError(f'expected {len(rows)} bestmoves, got {len(best)}')
    # A fixed-node fixture must never depend on the generous wall-clock ceiling.
    for line in result.stdout.splitlines():
        if line.startswith('info ') and ' time ' in line:
            assert int(line.split(' time ')[1].split()[0]) < 600000
    with args.output.open('w') as out:
        out.write('# category\tposition (FEN plus optional moves)\tnodes\tNoProgressLimit\tsearchmoves\texpected\n')
        for row, move in zip(rows, best):
            out.write('\t'.join(map(str, row)) + '\t' + move + '\n')
    print(f'{args.output.name}: {len(rows)} positions recorded')


if __name__ == '__main__':
    main()
