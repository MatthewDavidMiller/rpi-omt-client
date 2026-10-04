#!/usr/bin/env python3
# Copyright (c) 2026 Matthew David Miller
# SPDX-License-Identifier: MIT
"""Compile src/web/templates/*.html into C rendering functions.

The Web frontend renders a fixed set of pages, so their templates are compiled
ahead of time rather than interpreted: the appliance carries no template
parser, and every interpolation goes through tv_render, which HTML-escapes.
The supported language is exactly what the templates use -- extends/block,
if/elif/else, for (with tuple unpacking), set, inline `x if c else y`,
and/or/not, ==/!=, `is [not] none`, string literals, dotted names, and
{# comments #} -- and anything else is a compile error, not a silent gap.

Semantics follow minijinja's defaults: one trailing newline is stripped from
each template, block tags keep the whitespace around them, and missing names
are undefined (empty and false). `--check` regenerates in memory and fails
when the committed output differs.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TEMPLATES = ROOT / "src/web/templates"
STATIC = ROOT / "src/web/static"
STATIC_TYPES = {".css": "text/css; charset=utf-8", ".svg": "image/svg+xml"}
OUTPUT = ROOT / "src/web/templates_gen.c"
TOKEN = re.compile(r"({{.*?}}|{%.*?%}|{#.*?#})", re.DOTALL)


class TemplateError(Exception):
    pass


# ---------------------------------------------------------------- parsing


def tokenize(source):
    for part in TOKEN.split(source):
        if not part:
            continue
        if part.startswith("{{"):
            yield ("out", part[2:-2].strip())
        elif part.startswith("{%"):
            yield ("tag", part[2:-2].strip())
        elif part.startswith("{#"):
            continue
        else:
            yield ("text", part)


def parse(source, name):
    if source.endswith("\n"):
        source = source[:-1]
    tokens = list(tokenize(source))
    position = 0

    def body(stop):
        nonlocal position
        nodes = []
        while position < len(tokens):
            kind, value = tokens[position]
            if kind == "tag":
                word = value.split()[0]
                if word in stop:
                    return nodes, value
                position += 1
                if word == "if":
                    branches = []
                    condition = value[2:].strip()
                    while True:
                        inner, closing = body(("elif", "else", "endif"))
                        branches.append((condition, inner))
                        position += 1
                        if closing.startswith("elif"):
                            condition = closing[4:].strip()
                            continue
                        otherwise = None
                        if closing == "else":
                            otherwise, closing = body(("endif",))
                            position += 1
                        nodes.append(("if", branches, otherwise))
                        break
                elif word == "for":
                    match = re.fullmatch(r"for\s+(.+?)\s+in\s+(.+)", value)
                    if not match:
                        raise TemplateError(f"{name}: bad for: {value}")
                    targets = [t.strip() for t in match.group(1).split(",")]
                    inner, _ = body(("endfor",))
                    position += 1
                    nodes.append(("for", targets, match.group(2).strip(), inner))
                elif word == "set":
                    match = re.fullmatch(r"set\s+(\w+)\s*=\s*(.+)", value)
                    if not match:
                        raise TemplateError(f"{name}: bad set: {value}")
                    nodes.append(("set", match.group(1), match.group(2).strip()))
                elif word == "block":
                    block = value.split()[1]
                    inner, _ = body(("endblock",))
                    position += 1
                    nodes.append(("block", block, inner))
                elif word == "extends":
                    nodes.append(("extends", value.split(None, 1)[1].strip().strip("\"'")))
                else:
                    raise TemplateError(f"{name}: unsupported tag: {value}")
            elif kind == "out":
                position += 1
                nodes.append(("out", value))
            else:
                position += 1
                nodes.append(("text", value))
        if stop:
            raise TemplateError(f"{name}: missing {stop}")
        return nodes, None

    nodes, _ = body(())
    return nodes


# ------------------------------------------------------------ expressions

EXPR_TOKEN = re.compile(
    r"\s*(?:(==|!=|\(|\))|('(?:[^'\\]|\\.)*'|\"(?:[^\"\\]|\\.)*\")|([A-Za-z_][A-Za-z0-9_.]*))"
)


class Expr:
    def __init__(self, text, literal):
        self.tokens = []
        position = 0
        text = text.strip()
        while position < len(text):
            match = EXPR_TOKEN.match(text, position)
            if not match:
                raise TemplateError(f"bad expression: {text}")
            self.tokens.append(next(group for group in match.groups() if group is not None))
            position = match.end()
        self.position = 0
        self.literal = literal

    def peek(self):
        return self.tokens[self.position] if self.position < len(self.tokens) else None

    def take(self, expected=None):
        token = self.peek()
        if token is None or (expected is not None and token != expected):
            raise TemplateError(
                f"expected {expected!r}, found {token!r} in {' '.join(self.tokens)}"
            )
        self.position += 1
        return token

    def compile(self, scope):
        result = self.ternary(scope)
        if self.peek() is not None:
            raise TemplateError(f"trailing {self.peek()!r} in {' '.join(self.tokens)}")
        return result

    def ternary(self, scope):
        value = self.or_(scope)
        if self.peek() == "if":
            self.take("if")
            condition = self.or_(scope)
            self.take("else")
            otherwise = self.ternary(scope)
            return f"(tv_truthy({condition}) ? {value} : {otherwise})"
        return value

    def or_(self, scope):
        value = self.and_(scope)
        while self.peek() == "or":
            self.take()
            right = self.and_(scope)
            value = f"tv_from_bool(tv_truthy({value}) || tv_truthy({right}))"
        return value

    def and_(self, scope):
        value = self.not_(scope)
        while self.peek() == "and":
            self.take()
            right = self.not_(scope)
            value = f"tv_from_bool(tv_truthy({value}) && tv_truthy({right}))"
        return value

    def not_(self, scope):
        if self.peek() == "not":
            self.take()
            return f"tv_from_bool(!tv_truthy({self.not_(scope)}))"
        return self.compare(scope)

    def compare(self, scope):
        value = self.primary(scope)
        token = self.peek()
        if token in ("==", "!="):
            self.take()
            right = self.primary(scope)
            negate = "!" if token == "!=" else ""
            return f"tv_from_bool({negate}tv_equal({value}, {right}))"
        if token == "is":
            self.take()
            negate = self.peek() == "not"
            if negate:
                self.take()
            self.take("none")
            operator = "!=" if negate else "=="
            return f"tv_from_bool(({value})->kind {operator} TV_NONE)"
        return value

    def primary(self, scope):
        token = self.take()
        if token == "(":
            value = self.ternary(scope)
            self.take(")")
            return value
        if token[0] in "'\"":
            return self.literal(token[1:-1])
        if token in ("if", "else", "and", "or", "not", "is", "none"):
            raise TemplateError(f"unexpected {token!r}")
        parts = token.split(".")
        value = f'tv_lookup({scope}, "{parts[0]}")'
        for attribute in parts[1:]:
            value = f'tv_attr({value}, "{attribute}")'
        return value


# --------------------------------------------------------------- codegen


def c_string(text):
    out = []
    for byte in text.encode("utf-8"):
        char = chr(byte)
        if char == "\\":
            out.append("\\\\")
        elif char == '"':
            out.append('\\"')
        elif char == "\n":
            out.append("\\n")
        elif 32 <= byte < 127:
            out.append(char)
        else:
            out.append(f"\\{byte:03o}")
    return '"' + "".join(out) + '"'


class Generator:
    def __init__(self):
        self.literals = {}
        self.functions = []
        self.counter = 0

    def literal(self, text):
        if text not in self.literals:
            self.literals[text] = f"lit_{len(self.literals)}"
        return f"&{self.literals[text]}"

    def fresh(self, prefix):
        self.counter += 1
        return f"{prefix}_{self.counter}"

    def emit(self, nodes, scope, indent, lines, blocks, template):
        pad = "    " * indent
        for node in nodes:
            kind = node[0]
            if kind == "text":
                if node[1]:
                    lines.append(
                        f"{pad}omt_buf_append(out, {c_string(node[1])}, {len(node[1].encode())});"
                    )
            elif kind == "out":
                lines.append(f"{pad}tv_render(out, {Expr(node[1], self.literal).compile(scope)});")
            elif kind == "if":
                for index, (condition, inner) in enumerate(node[1]):
                    keyword = "if" if index == 0 else "} else if"
                    lines.append(
                        f"{pad}{keyword} "
                        f"(tv_truthy({Expr(condition, self.literal).compile(scope)})) {{"
                    )
                    self.emit(inner, scope, indent + 1, lines, blocks, template)
                if node[2] is not None:
                    lines.append(f"{pad}}} else {{")
                    self.emit(node[2], scope, indent + 1, lines, blocks, template)
                lines.append(f"{pad}}}")
            elif kind == "for":
                iterable = self.fresh("it")
                index = self.fresh("i")
                item = self.fresh("item")
                lines.append(f"{pad}{{")
                lines.append(
                    f"{pad}    const tv *{iterable} = {Expr(node[2], self.literal).compile(scope)};"
                )
                lines.append(
                    f"{pad}    for (size_t {index} = 0; "
                    f"{index} < tv_length({iterable}); {index}++) {{"
                )
                lines.append(f"{pad}        const tv *{item} = tv_index({iterable}, {index});")
                inner_scope = scope
                for position, target in enumerate(node[1]):
                    frame = self.fresh("sc")
                    value = item if len(node[1]) == 1 else f"tv_index({item}, {position})"
                    lines.append(
                        f"{pad}        const tv_scope {frame} = "
                        f'{{{inner_scope}, "{target}", {value}, NULL}};'
                    )
                    inner_scope = f"&{frame}"
                self.emit(node[3], inner_scope, indent + 2, lines, blocks, template)
                lines.append(f"{pad}    }}")
                lines.append(f"{pad}}}")
            elif kind == "set":
                frame = self.fresh("sc")
                lines.append(
                    f'{pad}const tv_scope {frame} = {{{scope}, "{node[1]}", '
                    f"{Expr(node[2], self.literal).compile(scope)}, NULL}};"
                )
                scope = f"&{frame}"
            elif kind == "block":
                lines.append(f"{pad}if (blocks && blocks->{node[1]})")
                lines.append(f"{pad}    blocks->{node[1]}(out, {scope});")
                lines.append(f"{pad}else {{")
                self.emit(node[2], scope, indent + 1, lines, blocks, template)
                lines.append(f"{pad}}}")
                blocks.add(node[1])
            elif kind == "extends":
                raise TemplateError(f"{template}: extends must be the first tag")
            else:
                raise TemplateError(f"{template}: unknown node {kind}")

    def render(self, templates):
        base_nodes = parse(templates["base.html"], "base.html")
        block_names = set()
        base_lines = []
        self.emit(base_nodes, "scope", 1, base_lines, block_names, "base.html")
        block_names = sorted(block_names)
        children = []
        for name in sorted(templates):
            if name == "base.html":
                continue
            nodes = [
                n for n in parse(templates[name], name) if not (n[0] == "text" and not n[1].strip())
            ]
            if not nodes or nodes[0] != ("extends", "base.html"):
                raise TemplateError(f"{name}: every page must extend base.html")
            overrides = {}
            for node in nodes[1:]:
                if node[0] != "block":
                    raise TemplateError(f"{name}: content outside a block")
                if node[1] not in block_names:
                    raise TemplateError(f"{name}: base.html has no block {node[1]}")
                overrides[node[1]] = node[2]
            children.append((name, overrides))

        def identifier(name: str) -> str:
            return re.sub(r"\W", "_", name.removesuffix(".html"))

        lines = [
            "/* Generated by tools/gen/gen_templates.py from src/web/templates. Do not edit. */",
            '#include "web/templates.h"',
            "",
            "#include <string.h>",
            "",
            "typedef void (*block_fn)(omt_buf *out, const tv_scope *scope);",
            "typedef struct {",
        ]
        lines += [f"    block_fn {block};" for block in block_names]
        lines += ["} blocks_t;", ""]
        literal_index = len(lines)
        lines += [
            "static void render_base(omt_buf *out, const tv_scope *scope, "
            "const blocks_t *blocks) {",
            *base_lines,
            "}",
            "",
        ]
        table = []
        for name, overrides in children:
            ident = identifier(name)
            initializers = []
            for block in block_names:
                if block in overrides:
                    function = f"{ident}_{block}"
                    body = []
                    used = set()
                    self.emit(overrides[block], "scope", 1, body, used, name)
                    if used:
                        raise TemplateError(f"{name}: nested blocks are not supported")
                    lines.append(f"static void {function}(omt_buf *out, const tv_scope *scope) {{")
                    lines += ["    (void)out;", "    (void)scope;"] + body
                    lines += ["}", ""]
                    initializers.append(f".{block} = {function}")
            lines.append(f"static const blocks_t {ident}_blocks = {{{', '.join(initializers)}}};")
            lines.append("")
            table.append((name, ident))
        lines.append(
            "bool omt_template_render(const char *name, const tv *context, omt_buf *out) {"
        )
        lines.append("    const tv_scope root = {NULL, NULL, NULL, context};")
        for name, ident in table:
            lines.append(f'    if (strcmp(name, "{name}") == 0) {{')
            lines.append(f"        render_base(out, &root, &{ident}_blocks);")
            lines.append("        return !out->failed;")
            lines.append("    }")
        lines.append("    return false;")
        lines.append("}")
        lines.append("")
        lines.append(
            "bool omt_static_asset(const char *name, const char **data, size_t *len, "
            "const char **type) {"
        )
        for path in sorted(STATIC.iterdir()):
            content = path.read_text(encoding="utf-8")
            lines.append(f'    if (strcmp(name, "{path.name}") == 0) {{')
            lines.append(f"        static const char body[] = {c_string(content)};")
            lines.append("        *data = body;")
            lines.append("        *len = sizeof(body) - 1;")
            lines.append(f'        *type = "{STATIC_TYPES[path.suffix]}";')
            lines.append("        return true;")
            lines.append("    }")
        lines.append("    return false;")
        lines.append("}")
        lines.append("")
        literal_lines = [
            f"static const tv {ident} = {{.kind = TV_STR, .s = {c_string(text)}, "
            f".len = {len(text.encode())}}};"
            for text, ident in self.literals.items()
        ]
        lines[literal_index:literal_index] = literal_lines + [""]
        return "\n".join(lines)


def render():
    templates = {
        path.name: path.read_text(encoding="utf-8") for path in sorted(TEMPLATES.glob("*.html"))
    }
    return Generator().render(templates)


def main():
    try:
        text = render()
    except TemplateError as error:
        print(f"template error: {error}", file=sys.stderr)
        return 1
    if sys.argv[1:] == ["--check"]:
        if OUTPUT.read_text(encoding="utf-8") != text:
            print(f"{OUTPUT} is stale; run {sys.argv[0]}", file=sys.stderr)
            return 1
        return 0
    OUTPUT.write_text(text, encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
