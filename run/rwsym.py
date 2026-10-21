#!/usr/bin/env python3
"""rwsym.py — offline symbolizer for RWFRZ park-stack dumps.

Input: lines from the kernel log of the form
  RWFRZ:   task rip=0x... vma=0x..-0x.. fd=N fileoff=0x.. [bytes not on rip lines]
  RWFRZ:   frame ustack[i]=0x... vma=0x..-0x.. fd=N fileoff=0x.. bytes=0x0123456789abcdef
Method (Session F byte-match, verified twice):
  1. For every candidate lib (.so under /tmp/oh-libs/{syslib64,vendorlib64}),
     parse ELF phdrs -> map fileoff -> vaddr.
  2. Byte-verify: 8 bytes at fileoff in the file must equal the frame's
     bytes field. First lib that verifies wins.
  3. Symbolize vaddr against .dynsym (nearest symbol below, with size).
Usage: rwsym.py <serial.log> [more logs]
"""
import re
import struct
import sys
from pathlib import Path

LIBDIRS = ["/tmp/oh-libs/syslib64", "/tmp/oh-libs/vendorlib64", "/tmp/oh-libs/samgr-bin", "/tmp/oh-libs/subdirs", "/tmp/oh-libs/platformsdk"]


class Lib:
    def __init__(self, path):
        self.path = path
        self.name = Path(path).name
        self.data = Path(path).read_bytes()
        self.loads = []  # (p_offset, p_filesz, p_vaddr)
        self.symbols = []  # (vaddr, size, name)
        self._parse()

    def _parse(self):
        d = self.data
        if d[:4] != b"\x7fELF":
            raise ValueError("not ELF")
        is64 = d[4] == 2
        if not is64:
            raise ValueError("not elf64")
        e_phoff, = struct.unpack_from("<Q", d, 0x20)
        e_phentsize, e_phnum = struct.unpack_from("<HH", d, 0x36)
        for i in range(e_phnum):
            off = e_phoff + i * e_phentsize
            p_type, p_flags = struct.unpack_from("<II", d, off)
            p_offset, p_vaddr, _pa, p_filesz, _msz, _align = struct.unpack_from(
                "<QQQQQQ", d, off + 8)
            if p_type == 1:  # PT_LOAD
                self.loads.append((p_offset, p_filesz, p_vaddr))
        # section headers for .dynsym
        e_shoff, = struct.unpack_from("<Q", d, 0x28)
        e_shentsize, e_shnum, e_shstrndx = struct.unpack_from("<HHH", d, 0x3a)
        shstr_off = 0
        if e_shstrndx < e_shnum:
            shstr_off = struct.unpack_from("<Q", d, e_shoff + e_shstrndx * e_shentsize + 0x18)[0]
        dynsym_off = dynsym_sz = dynstr_off = 0
        for i in range(e_shnum):
            off = e_shoff + i * e_shentsize
            sh_name, sh_type = struct.unpack_from("<II", d, off)
            sh_offset, sh_size = struct.unpack_from("<QQ", d, off + 0x18)
            if sh_type == 11:  # SHT_DYNSYM
                dynsym_off, dynsym_sz = sh_offset, sh_size
                # linked strtab
                sh_link = struct.unpack_from("<I", d, off + 0x28)[0]
                loff = e_shoff + sh_link * e_shentsize
                dynstr_off = struct.unpack_from("<Q", d, loff + 0x18)[0]
                break
        if dynsym_off:
            n = dynsym_sz // 24
            for i in range(n):
                so = dynsym_off + i * 24
                st_name, st_info, _o, st_shndx, st_value, st_size = struct.unpack_from(
                    "<IBBHQQ", d, so)
                if st_value == 0 or st_shndx == 0:
                    continue
                end = d.index(b"\0", dynstr_off + st_name)
                nm = d[dynstr_off + st_name:end].decode("utf-8", "replace")
                if nm:
                    self.symbols.append((st_value, st_size, nm))
            self.symbols.sort()

    def fileoff_to_vaddr(self, off):
        for p_offset, p_filesz, p_vaddr in self.loads:
            if p_offset <= off < p_offset + p_filesz:
                return p_vaddr + (off - p_offset)
        return None

    def bytes_at_fileoff(self, off):
        return self.data[off:off + 8]

    def sym(self, vaddr):
        best = None
        for v, sz, nm in self.symbols:
            if v <= vaddr:
                if sz and vaddr < v + sz:
                    return f"{nm}+0x{vaddr - v:x} (in-size)"
                best = (v, nm)
            else:
                break
        if best:
            return f"{best[1]}+0x{vaddr - best[0]:x}"
        return "?"


def load_libs():
    libs = []
    for dd in LIBDIRS:
        p = Path(dd)
        if not p.is_dir():
            continue
        for f in sorted(p.iterdir()):
            try:
                libs.append(Lib(str(f)))
            except Exception:
                pass
    return libs


FRAME_RE = re.compile(
    r"(?:ustack\[(\d+)\]|task)=?(?:rip=)?0x([0-9a-f]+) vma=0x([0-9a-f]+)-0x([0-9a-f]+)"
    r" fd=(-?\d+) fileoff=0x([0-9a-f]+)(?: bytes=0x([0-9a-f]+))?")


def main():
    logs = sys.argv[1:]
    libs = load_libs()
    print(f"# {len(libs)} libs loaded")
    seen = {}
    for lg in logs:
        for line in open(lg, errors="replace"):
            m = FRAME_RE.search(line)
            if not m:
                continue
            idx, vaddr_s, _vs, _ve, fd, fileoff_s, bytes_s = m.groups()
            fileoff = int(fileoff_s, 16)
            vaddr = int(vaddr_s, 16)
            key = (fileoff, bytes_s or "")
            if key in seen:
                res = seen[key]
            else:
                res = "?"
                bl = bytes.fromhex(bytes_s.rjust(16, "0")) if bytes_s else None
                for lib in libs:
                    va = lib.fileoff_to_vaddr(fileoff)
                    if va is None:
                        continue
                    if bl is None or lib.bytes_at_fileoff(fileoff) == bl:
                        res = f"{lib.name}!{lib.sym(va)}"
                        if bl is not None:
                            res += f" [byte-verified vaddr=0x{va:x}]"
                        break
                seen[key] = res
            frame = f"frame[{idx}]" if idx else "rip"
            print(f"0x{vaddr:x} fd={fd} fileoff=0x{fileoff:x} -> {res}")


if __name__ == "__main__":
    main()
