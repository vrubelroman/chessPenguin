#!/usr/bin/env python3
"""Reads a PyTorch .pt (zip + pickle) without torch and writes our weights file:
magic b'MAIA3W02', u32 count, then per tensor: u16 name len, name, u8 ndim, u32 dims..., u64 offset;
then aligned f16 little-endian data. Usage: convert.py in.pt out.bin"""
import pickle, struct, sys, zipfile, collections

src, dst = sys.argv[1], sys.argv[2]
z = zipfile.ZipFile(src)
root = z.namelist()[0].split('/')[0]
pkl = z.read(f'{root}/data.pkl')

class Storage:
    def __init__(self, key, dtype): self.key, self.dtype = key, dtype

def rebuild(storage, offset, size, stride, *rest):
    return ('T', storage, offset, tuple(size), tuple(stride))

class U(pickle.Unpickler):
    def find_class(self, mod, name):
        if mod == 'torch._utils' and name == '_rebuild_tensor_v2': return rebuild
        if mod == 'torch' and name.endswith('Storage'): return name
        if mod == 'collections' and name == 'OrderedDict': return collections.OrderedDict
        if mod == 'torch._utils' and name == '_rebuild_parameter':
            return lambda t, *a: t
        raise pickle.UnpicklingError(f'refused {mod}.{name}')
    def persistent_load(self, pid):
        _, dtype, key, loc, numel = pid
        return Storage(key, dtype)

obj = U(__import__('io').BytesIO(pkl)).load()
sd = obj['model_state_dict'] if isinstance(obj, dict) and 'model_state_dict' in obj else obj
out = []
for name, t in sd.items():
    if not (isinstance(t, tuple) and t and t[0] == 'T'): continue
    _, st, off, size, stride = t
    assert st.dtype == 'FloatStorage', (name, st.dtype)
    raw = z.read(f'{root}/data/{st.key}')
    n = 1
    for d in size: n *= d
    # contiguous check
    exp = []; acc = 1
    for d in reversed(size): exp.append(acc); acc *= d
    assert tuple(reversed(exp)) == stride or n <= 1, (name, size, stride)
    data = raw[off*4:(off+n)*4]
    vals = struct.unpack(f'<{n}f', data)
    data = struct.pack(f'<{n}e', *vals)
    name = name.replace('smolgen', 'gab')
    # The attention-bias weight is one tensor shared by all the layers: kept once.
    if '.self_attn.gab_weight' in name: continue
    out.append((name, size, data))
with open(dst, 'wb') as f:
    f.write(b'MAIA3W02'); f.write(struct.pack('<I', len(out)))
    header_len = 8 + 4 + sum(2 + len(n.encode()) + 1 + 4*len(s) + 8 for n, s, _ in out)
    off = (header_len + 63) // 64 * 64
    offs = []
    for n, s, d in out:
        offs.append(off); off += (len(d) + 63) // 64 * 64
    for (n, s, d), o in zip(out, offs):
        nb = n.encode(); f.write(struct.pack('<H', len(nb))); f.write(nb)
        f.write(struct.pack('<B', len(s)))
        for x in s: f.write(struct.pack('<I', x))
        f.write(struct.pack('<Q', o))
    for (n, s, d), o in zip(out, offs):
        f.write(b'\0' * (o - f.tell())); f.write(d)
for n, s, _ in out: print(n, s)
