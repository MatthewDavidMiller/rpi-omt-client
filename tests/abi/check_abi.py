#!/usr/bin/env python3
"""Holds omt-sys's hand-written foreign declarations to the real C headers.

omt-sys declares every libc, kernel, ALSA, OpenSSL, and Win32 item the Rust
tree uses by hand, so nothing checks a declaration against the platform the
way a C compiler would. This does: it reads the bindings themselves -- every
top-level integer `pub const` and every `#[repr(C)]` struct with its `pub`
fields -- and writes two probes from the same list:

  * a C file in which each item becomes an absolute symbol whose value the
    compiler computes from the real headers (`sizeof`, `offsetof`, or the
    constant itself), and
  * a Rust crate in which `global_asm!` const operands do the same from the
    omt-sys declarations.

Both are compiled -- never run -- for each shipped target, and the symbol
values are compared with llvm-nm. Compile-only is what lets the Windows and
aarch64 declarations be checked on an x86-64 Linux host.

    python3 tests/abi/check_abi.py [target...]
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from collections.abc import Collection
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "crates" / "omt-sys" / "src"
WORK = ROOT / "target" / "abi-probe"
SYSROOT = os.environ.get("OMT_SYSROOT", "/opt/sysroot-aarch64")

HEADERS = {
    "posix": [
        "errno.h",
        "fcntl.h",
        "poll.h",
        "signal.h",
        "time.h",
        "termios.h",
        "sys/ioctl.h",
        "sys/mman.h",
        "sys/socket.h",
        "netinet/in.h",
    ],
    # xf86drmMode.h names the connection state and the plane types, which
    # the kernel uapi leaves as bare property values.
    "drm": ["drm.h", "drm_mode.h", "drm_fourcc.h", "xf86drmMode.h"],
    "alsa": ["alsa/asoundlib.h"],
    "evp": ["openssl/bn.h", "openssl/core.h", "openssl/evp.h"],
    "ssl": ["openssl/err.h", "openssl/ssl.h", "openssl/x509_vfy.h"],
    "win32": ["winsock2.h", "windows.h", "wincrypt.h", "bcrypt.h"],
}

TARGETS = {
    "x86_64-unknown-linux-musl": {
        "cc": ["gcc", "-I/usr/include/libdrm"],
        "cfg": {"unix", "linux", "x86_64"},
        "modules": ["posix", "drm", "alsa", "evp", "ssl"],
        "features": ["ssl", "alsa"],
    },
    # The DRM structs are fixed-width kernel types, identical on both 64-bit
    # targets, and the sysroot carries no libdrm; OpenSSL's constants do not
    # vary by architecture.
    "aarch64-unknown-linux-musl": {
        "cc": ["clang", "--target=aarch64-alpine-linux-musl", f"--sysroot={SYSROOT}"],
        "cfg": {"unix", "linux", "aarch64"},
        "modules": ["posix", "alsa"],
        "features": ["alsa"],
    },
    "x86_64-pc-windows-gnu": {
        "cc": ["x86_64-w64-mingw32-gcc"],
        "cfg": {"windows", "x86_64"},
        "modules": ["win32"],
        "features": [],
    },
}

# Where the C spelling differs from the Rust one.
C_STRUCT = {
    ("posix", "sigaction_t"): "struct sigaction",
    ("posix", "sigset_t"): "sigset_t",
    ("evp", "OSSL_PARAM"): "OSSL_PARAM",
}
C_FIELD: dict[tuple[str, str, str], str | None] = {
    # `type` is a Rust keyword.
    ("drm", "drm_mode_modeinfo", "kind"): "type",
    ("drm", "drm_event", "kind"): "type",
    # musl reserves the names.
    ("posix", "termios", "c_ispeed"): "__c_ispeed",
    ("posix", "termios", "c_ospeed"): "__c_ospeed",
    # Padding the kernel leaves implicit and Rust spells out; the struct
    # sizes and the following fields' offsets still hold it in place.
    ("drm", "drm_mode_get_plane_res", "pad"): None,
    ("drm", "drm_mode_obj_get_properties", "pad"): None,
    ("drm", "drm_mode_fb_cmd2", "pad"): None,
}
C_CONST = {
    ("ssl", "TLSEXT_NAMETYPE_HOST_NAME"): "TLSEXT_NAMETYPE_host_name",
}
SKIP: set[tuple[str, str]] = set()

INTEGER_TYPES = re.compile(
    r"^(c_u?(int|short|long|longlong|char)|[iu](8|16|32|64|size)|DWORD|WORD|BOOL|Request<.*>)$"
)


def cfg_holds(attr: str, cfg: Collection[str]) -> bool:
    """Evaluates the simple cfg predicates omt-sys uses."""
    expr = attr.strip()
    m = re.fullmatch(r"not\((.*)\)", expr)
    if m:
        return not cfg_holds(m.group(1), cfg)
    m = re.fullmatch(r"(all|any)\((.*)\)", expr)
    if m:
        parts = [p for p in re.split(r",\s*(?![^()]*\))", m.group(2)) if p.strip()]
        results = [cfg_holds(p, cfg) for p in parts]
        return all(results) if m.group(1) == "all" else any(results)
    m = re.fullmatch(r'target_arch\s*=\s*"(\w+)"', expr)
    if m:
        return m.group(1) in cfg
    m = re.fullmatch(r'target_os\s*=\s*"(\w+)"', expr)
    if m:
        return m.group(1) in cfg
    if expr in ("unix", "windows"):
        return expr in cfg
    if re.fullmatch(r'feature\s*=\s*"\w+"', expr):
        return True
    raise SystemExit(f"unsupported cfg in omt-sys: {attr}")


def parse(module: str, cfg: Collection[str]) -> list[tuple[str, str, str]]:
    """(symbol, rust expression, C expression) for every checkable item."""
    items: list[tuple[str, str, str]] = []
    text = (SRC / f"{module}.rs").read_text(encoding="utf-8")
    depth = 0
    attrs: list[str] = []
    lines = iter(text.splitlines())
    for line in lines:
        stripped = line.strip()
        if depth == 0 and stripped.startswith("#["):
            attrs.append(stripped)
            continue
        if depth == 0:
            active = all(
                cfg_holds(m.group(1), cfg)
                for a in attrs
                for m in [re.fullmatch(r"#\[cfg\((.*)\)\]", a)]
                if m
            )
            repr_c = any(a == "#[repr(C)]" for a in attrs)
            const = re.match(r"pub const (\w+): ([^=]+?)\s*=", stripped)
            struct = re.match(r"pub struct (\w+) \{$", stripped)
            if const and active and (module, const.group(1)) not in SKIP:
                name, ty = const.group(1), const.group(2).strip()
                if INTEGER_TYPES.match(ty):
                    rust = f"omt_sys::{module}::{name}"
                    if ty.startswith("Request<"):
                        rust += ".number()"
                    items.append((f"{module}_{name}", rust, C_CONST.get((module, name), name)))
            if struct and active and repr_c:
                name = struct.group(1)
                c_type = C_STRUCT.get(
                    (module, name), name if module in ("win32",) else f"struct {name}"
                )
                rust_type = f"omt_sys::{module}::{name}"
                items.append(
                    (f"{module}_{name}_size", f"size_of::<{rust_type}>()", f"sizeof({c_type})")
                )
                for field_line in lines:
                    f = field_line.strip()
                    if f.startswith("}"):
                        break
                    fm = re.match(r"pub (\w+):", f)
                    if not fm:
                        continue
                    field = fm.group(1)
                    c_field = C_FIELD.get((module, name, field), field)
                    if c_field is None:
                        continue
                    items.append(
                        (
                            f"{module}_{name}_{field}",
                            f"offset_of!({rust_type}, {field})",
                            f"offsetof({c_type}, {c_field})",
                        )
                    )
                attrs = []
                continue
        # Doc comments sit between an item's attributes and the item.
        if depth == 0 and stripped and not stripped.startswith("//"):
            attrs = []
        code = line.split("//", 1)[0]
        depth += code.count("{") - code.count("}")
    return items


def symbols(nm_output: str, prefix: str) -> dict[str, int]:
    out: dict[str, int] = {}
    for line in nm_output.splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[2].startswith(prefix):
            out[parts[2][len(prefix) :]] = int(parts[0], 16)
    return out


def check(target: str) -> int:
    conf = TARGETS[target]
    items = [i for m in conf["modules"] for i in parse(m, conf["cfg"])]
    work = WORK / target
    (work / "src").mkdir(parents=True, exist_ok=True)

    headers = [h for m in conf["modules"] for h in HEADERS[m]]
    c_lines = ["#include <stddef.h>"] + [f"#include <{h}>" for h in headers]
    c_lines.append("void omt_abi_probe(void) {")
    # Four 16-bit operands: GCC's x86-64 `%c` refuses an immediate that does
    # not fit a signed 32-bit field, and the assembler rebuilds all 64 bits.
    c_lines.insert(
        1, "#define PIECE(v, n) ((unsigned)(((unsigned long long)(v) >> (n)) & 0xffffu))"
    )
    for sym, _, c_expr in items:
        c_lines.append(
            f'    __asm__ volatile(".globl cabi_{sym}\\n.set cabi_{sym}, '
            f'(%c0 << 48) | (%c1 << 32) | (%c2 << 16) | %c3" '
            f':: "i"(PIECE({c_expr}, 48)), "i"(PIECE({c_expr}, 32)), '
            f'"i"(PIECE({c_expr}, 16)), "i"(PIECE({c_expr}, 0)));'
        )
    c_lines.append("}")
    (work / "probe.c").write_text("\n".join(c_lines) + "\n", encoding="utf-8")

    features = ", ".join(f'"{f}"' for f in conf["features"])
    (work / "Cargo.toml").write_text(
        '[package]\nname = "omt-abi-probe"\nversion = "0.0.0"\nedition = "2024"\n'
        'publish = false\n\n[lib]\npath = "src/lib.rs"\n\n[dependencies]\n'
        f'omt-sys = {{ path = "{ROOT / "crates" / "omt-sys"}", features = [{features}] }}\n\n'
        "[workspace]\n",
        encoding="utf-8",
    )
    rust_lines = ["#![allow(unused_imports)]", "use std::mem::{offset_of, size_of};"]
    for sym, rust_expr, _ in items:
        rust_lines.append(
            f'std::arch::global_asm!(".globl rabi_{sym}", '
            f'".set rabi_{sym}, {{}}", const {rust_expr});'
        )
    (work / "src" / "lib.rs").write_text("\n".join(rust_lines) + "\n", encoding="utf-8")

    obj = work / "probe.o"
    subprocess.run([*conf["cc"], "-c", "-o", str(obj), str(work / "probe.c")], check=True)
    subprocess.run(
        [
            "cargo",
            "build",
            "--offline",
            "--quiet",
            "--lib",
            "--target",
            target,
            "--manifest-path",
            str(work / "Cargo.toml"),
            "--target-dir",
            str(WORK / "build"),
        ],
        check=True,
        cwd=ROOT,
    )
    rlib = WORK / "build" / target / "debug" / "libomt_abi_probe.rlib"

    def nm(path: Path) -> str:
        # An rlib also holds rustc's metadata, which is not an object file.
        return subprocess.run(["llvm-nm", str(path)], capture_output=True, text=True).stdout

    c_values = symbols(nm(obj), "cabi_")
    rust_values = symbols(nm(rlib), "rabi_")
    failures = 0
    for sym, _, c_expr in items:
        c, r = c_values.get(sym), rust_values.get(sym)
        if c is None or r is None or c != r:
            print(f"FAIL {target}: {sym}: C {c_expr} = {c}, Rust = {r}", file=sys.stderr)
            failures += 1
    print(f"{target}: {len(items) - failures}/{len(items)} declarations match the headers")
    return failures


def main() -> int:
    targets = sys.argv[1:] or list(TARGETS)
    failures = sum(check(t) for t in targets)
    if failures:
        print(f"ABI probe: {failures} mismatch(es)", file=sys.stderr)
        return 1
    print("ABI probe passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
