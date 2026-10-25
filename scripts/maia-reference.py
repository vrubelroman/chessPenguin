#!/usr/bin/env python3
# Writes crates/maia/tests/reference.json from the original Maia-3 code and weights:
#   python3 -m venv v && v/bin/pip install torch chess --index-url ... ; git clone https://github.com/CSSLab/maia3
#   v/bin/python scripts/maia-reference.py maia3/ maia3-5m.pt crates/maia/tests/reference.json
import sys, json, types
sys.path.insert(0, sys.argv[1])
import torch, chess
from collections import deque
from maia3.models import MAIA3Model
from maia3.dataset import tokenize_board, get_legal_moves_mask, get_historical_tokens
from maia3.utils import get_all_possible_moves, mirror_move
from maia3.model_registry import resolve_model_spec, BASE_SIZE_CONFIG
spec = resolve_model_spec('maia3-5m')
cfg = types.SimpleNamespace(**spec.config); cfg.device='cpu'
cfg.include_time_info=False
model = MAIA3Model(cfg)
sd = torch.load(sys.argv[2], map_location='cpu', weights_only=True)
sd = sd.get('model_state_dict', sd)
model.load_state_dict({k.replace('smolgen','gab'):v for k,v in sd.items()}, strict=False)
model.eval()
moves = get_all_possible_moves(); idx = {m:i for i,m in enumerate(moves)}
cases = [
  ('startpos', [], 1500, 1500),
  ('startpos', ['e2e4'], 1100, 1900),
  ('startpos', ['e2e4','c7c5','g1f3','d7d6','d2d4','c5d4','f3d4','g8f6','b1c3'], 2200, 2200),
  ('fen 8/P7/8/8/8/8/k6K/8 w - - 0 1', [], 1500, 1500),
  ('fen r3k2r/8/8/8/8/8/8/R3K2R b KQkq - 0 1', [], 800, 2500),
]
out = []
for start, mv, se, oe in cases:
    b = chess.Board() if start == 'startpos' else chess.Board(start[4:])
    hist = deque(maxlen=8); hist.append(tokenize_board(b))
    for m in mv:
        b.push(chess.Move.from_uci(m)); hist.append(tokenize_board(b))
    for use_hist in (False, True):
        h = hist if use_hist else deque([tokenize_board(b)], maxlen=8)
        t = get_historical_tokens(h, cfg, 0,0,0,0).unsqueeze(0)
        with torch.no_grad():
            lm, lv, _ = model(t, torch.tensor([se]), torch.tensor([oe]))
        mask = get_legal_moves_mask(b, idx)
        lg = lm[0].masked_fill(~mask, float('-inf'))
        p = torch.softmax(lg, -1)
        top = torch.topk(p, 5)
        res = []
        for pr, i in zip(top.values.tolist(), top.indices.tolist()):
            u = moves[i]
            if b.turn == chess.BLACK: u = mirror_move(u)
            res.append([u, pr])
        out.append({'start': start, 'moves': mv, 'self': se, 'oppo': oe, 'history': use_hist,
                    'top': res, 'value': torch.softmax(lv[0],-1).tolist(),
                    'raw0': lm[0][:8].tolist()})
json.dump(out, open(sys.argv[3],'w'), indent=1)
for o in out: print(o['start'][:20], len(o['moves']), o['history'], o['top'][:3], [round(v,3) for v in o['value']])
