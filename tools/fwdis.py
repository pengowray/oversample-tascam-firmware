"""Small helpers for poking at the X8 main application with capstone.

Loads a flash image from `x8fw.py unpack` (default work/v203.bin, or the
X8_IMAGE environment variable) with the main application mapped at 0x80000000.
Ghidra is the better tool for real work; this is for quick questions such as
"who calls this" and "what loads this string".

Needs: pip install capstone numpy

    import fwdis as f
    f.show(0x80df0f78, 40)                 # disassemble main()
    f.callers(0x80d9ff2c)                  # BL/B.W sites that call an address
    f.lit_refs(0x817ef1c4)                 # literal-pool words equal to an address
    f.adr_refs(0x80dfc780)                 # ADR instructions pointing at an address
"""
import struct, re
from capstone import Cs, CS_ARCH_ARM, CS_MODE_THUMB, CS_MODE_MCLASS
import os
IMG = os.environ.get('X8_IMAGE', os.path.join(os.path.dirname(__file__), '..', 'work', 'v203.bin'))
d = open(IMG, 'rb').read()
BASE = 0x80000000; OFF = 0x20000
def a2o(a): return a - BASE + OFF
def o2a(o): return o - OFF + BASE
md = Cs(CS_ARCH_ARM, CS_MODE_THUMB | CS_MODE_MCLASS); md.detail = True
def u32(o): return struct.unpack_from('<I', d, o)[0]
def cstr(o):
    e = d.index(b'\0', o); return d[o:e].decode('latin1')
def find_all(b, start=OFF):
    out=[]; i=d.find(b,start)
    while i!=-1: out.append(i); i=d.find(b,i+1)
    return out
def lit_refs(addr):
    """offsets of 4-byte aligned literal words equal to addr"""
    pat = struct.pack('<I', addr); return [o for o in find_all(pat) if o % 4 == 0]
def dis(addr, n=40):
    o = a2o(addr); out=[]
    for ins in md.disasm(d[o:o+n*4], addr):
        out.append(ins)
        if len(out)>=n: break
    return out
def show(addr, n=40):
    for ins in dis(addr, n):
        extra=''
        if ins.mnemonic.startswith('ldr') and 'pc' in ins.op_str and '[' in ins.op_str:
            try:
                imm = ins.operands[1].mem.disp
                la = ((ins.address + 4) & ~3) + imm
                v = u32(a2o(la)); extra = f'   ; =0x{v:08x}'
                if BASE <= v < BASE+0x1830000:
                    try:
                        s = cstr(a2o(v))
                        if len(s)>=3 and all(32<=ord(c)<127 or c in '\r\n\t' for c in s): extra += f' "{s[:50]}"'
                    except Exception: pass
            except Exception: pass
        print(f'{ins.address:08x}: {ins.mnemonic:8s} {ins.op_str}{extra}')
def func_start(addr, maxback=0x2000):
    """scan back for a push {...lr} (16-bit b5xx or 32-bit e92d 4xxx)"""
    o = a2o(addr)
    for k in range(o & ~1, o - maxback, -2):
        h = struct.unpack_from('<H', d, k)[0]
        if (h & 0xff00) == 0xb500: return o2a(k)
        if h == 0xe92d:
            h2 = struct.unpack_from('<H', d, k+2)[0]
            if h2 & 0x4000: return o2a(k)
    return None
def code_refs_to(addr):
    """find BL/B.W to addr by brute force decoding of 32-bit BL at every halfword"""
    res=[]
    tgt=addr & ~1
    for k in range(OFF, len(d)-4, 2):
        h1, h2 = struct.unpack_from('<HH', d, k)
        if (h1 & 0xf800) == 0xf000 and (h2 & 0xd000) in (0xd000, 0x9000):
            S=(h1>>10)&1; imm10=h1&0x3ff; J1=(h2>>13)&1; J2=(h2>>11)&1; imm11=h2&0x7ff
            I1=1-(J1^S); I2=1-(J2^S)
            off=(S<<24)|(I1<<23)|(I2<<22)|(imm10<<12)|(imm11<<1)
            if S: off-= (1<<25)
            if o2a(k)+4+off == tgt: res.append((o2a(k), 'bl' if (h2&0xd000)==0xd000 else 'b.w'))
    return res

def _movwt(h1, h2):
    i=(h1>>10)&1; imm4=h1&0xf; imm3=(h2>>12)&7; rd=(h2>>8)&0xf; imm8=h2&0xff
    return rd, (imm4<<12)|(i<<11)|(imm3<<8)|imm8
_MOVW_INDEX=None
def movw_movt_index():
    """map 32-bit value -> list of code addresses where a movw/movt pair builds it"""
    global _MOVW_INDEX
    if _MOVW_INDEX is not None: return _MOVW_INDEX
    import collections
    idx=collections.defaultdict(list)
    n=len(d)
    hw=struct.unpack_from('<%dH'%((n-OFF)//2), d, OFF)
    movw={}  # pos -> (rd, imm)
    L=len(hw)
    for k in range(L-1):
        h1=hw[k]
        if (h1 & 0xfbf0)==0xf240 and (hw[k+1]&0x8000)==0:
            rd,imm=_movwt(h1,hw[k+1])
            # look ahead up to 12 halfwords for movt same rd
            for j in range(k+2, min(k+24, L-1)):
                g=hw[j]
                if (g & 0xfbf0)==0xf2c0 and (hw[j+1]&0x8000)==0:
                    rd2,imm2=_movwt(g,hw[j+1])
                    if rd2==rd:
                        idx[(imm2<<16)|imm].append(o2a(OFF+2*k)); break
    _MOVW_INDEX=idx
    return idx

def adr_refs(target, window=0x1000):
    """ADR (16-bit add rd,pc,#imm and 32-bit adr.w) instructions within window before/after target"""
    res=[]
    lo=max(OFF, a2o(target)-window); hi=min(len(d)-4, a2o(target)+window)
    for k in range(lo, hi, 2):
        h1,h2=struct.unpack_from('<HH',d,k); a=o2a(k)
        if (h1 & 0xf800)==0xa000:
            if ((a+4)&~3)+((h1&0xff)<<2)==target: res.append(a)
        if (h1 & 0xfbff)==0xf20f and (h2&0x8000)==0:
            imm=((h1>>10)&1)<<11 | ((h2>>12)&7)<<8 | (h2&0xff)
            if ((a+4)&~3)+imm==target: res.append(a)
        if (h1 & 0xfbff)==0xf2af and (h2&0x8000)==0:
            imm=((h1>>10)&1)<<11 | ((h2>>12)&7)<<8 | (h2&0xff)
            if ((a+4)&~3)-imm==target: res.append(a)
    return res

def build_bl_index():
    import numpy as np, collections
    n=(len(d)-OFF)//2
    hw=np.frombuffer(d, dtype='<u2', count=n, offset=OFF).astype(np.int64)
    h1=hw[:-1]; h2=hw[1:]
    m=((h1 & 0xf800)==0xf000) & (((h2 & 0xd000)==0xd000) | ((h2 & 0xd000)==0x9000))
    k=np.nonzero(m)[0]
    a1=h1[k]; a2=h2[k]
    S=(a1>>10)&1; imm10=a1&0x3ff; J1=(a2>>13)&1; J2=(a2>>11)&1; imm11=a2&0x7ff
    I1=1-(J1^S); I2=1-(J2^S)
    off=(S<<24)|(I1<<23)|(I2<<22)|(imm10<<12)|(imm11<<1)
    off=np.where(S==1, off-(1<<25), off)
    src=BASE+2*k
    tgt=src+4+off
    isbl=(a2&0xd000)==0xd000
    idx=collections.defaultdict(list)
    for s,t,b in zip(src.tolist(), tgt.tolist(), isbl.tolist()):
        idx[t].append((s, 'bl' if b else 'b.w'))
    return idx
_BL=None
def callers(f):
    global _BL
    if _BL is None:
        import pickle, os
        p=IMG + '.bl.pkl'
        if os.path.exists(p): _BL=pickle.load(open(p,'rb'))
        else:
            _BL=dict(build_bl_index()); pickle.dump(_BL, open(p,'wb'))
    return _BL.get(f & ~1, [])
def tree(f, depth=5, indent=0, seen=None):
    seen = seen or set()
    for (a,k) in callers(f):
        fs=func_start(a)
        print('  '*indent + f'{k} from {a:08x} in func {fs:08x}' if fs else '  '*indent + f'{k} from {a:08x} (no func start)')
        if fs and fs not in seen and depth>1:
            seen.add(fs); tree(fs, depth-1, indent+1, seen)
