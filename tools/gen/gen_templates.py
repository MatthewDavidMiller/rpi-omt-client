#!/usr/bin/env python3
# Copyright (c) 2026 Matthew David Miller
# SPDX-License-Identifier: MIT
"""Compile crates/omt-web/templates/*.html into Rust rendering functions.

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
TEMPLATES = ROOT / "crates/omt-web/templates"
STATIC = ROOT / "crates/omt-web/static"
STATIC_TYPES = {".css": "text/css; charset=utf-8", ".svg": "image/svg+xml"}
RUST_OUTPUT = ROOT / "crates/omt-web/src/templates_gen.rs"
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
            return self.out_ternary(condition, value, otherwise)
        return value

    def or_(self, scope):
        value = self.and_(scope)
        while self.peek() == "or":
            self.take()
            right = self.and_(scope)
            value = self.out_or(value, right)
        return value

    def and_(self, scope):
        value = self.not_(scope)
        while self.peek() == "and":
            self.take()
            right = self.not_(scope)
            value = self.out_and(value, right)
        return value

    def not_(self, scope):
        if self.peek() == "not":
            self.take()
            return self.out_not(self.not_(scope))
        return self.compare(scope)

    def compare(self, scope):
        value = self.primary(scope)
        token = self.peek()
        if token in ("==", "!="):
            self.take()
            right = self.primary(scope)
            return self.out_equal(value, right, token == "!=")
        if token == "is":
            self.take()
            negate = self.peek() == "not"
            if negate:
                self.take()
            self.take("none")
            return self.out_is_none(value, negate)
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
        value = self.out_lookup(scope, parts[0])
        for attribute in parts[1:]:
            value = self.out_attr(value, attribute)
        return value

    # The C spelling of each operation; RustExpr overrides them.

    def out_ternary(self, condition, value, otherwise):
        return f"(tv_truthy({condition}) ? {value} : {otherwise})"

    def out_or(self, left, right):
        return f"tv_from_bool(tv_truthy({left}) || tv_truthy({right}))"

    def out_and(self, left, right):
        return f"tv_from_bool(tv_truthy({left}) && tv_truthy({right}))"

    def out_not(self, value):
        return f"tv_from_bool(!tv_truthy({value}))"

    def out_equal(self, left, right, negate):
        bang = "!" if negate else ""
        return f"tv_from_bool({bang}tv_equal({left}, {right}))"

    def out_is_none(self, value, negate):
        operator = "!=" if negate else "=="
        return f"tv_from_bool(({value})->kind {operator} TV_NONE)"

    def out_lookup(self, scope, name):
        return f'tv_lookup({scope}, "{name}")'

    def out_attr(self, value, name):
        return f'tv_attr({value}, "{name}")'


class RustExpr(Expr):
    """The same grammar, compiled to calls on crate::tv."""

    def out_ternary(self, condition, value, otherwise):
        return f"if tv::truthy({condition}) {{ {value} }} else {{ {otherwise} }}"

    def out_or(self, left, right):
        return f"tv::from_bool(tv::truthy({left}) || tv::truthy({right}))"

    def out_and(self, left, right):
        return f"tv::from_bool(tv::truthy({left}) && tv::truthy({right}))"

    def out_not(self, value):
        return f"tv::from_bool(!tv::truthy({value}))"

    def out_equal(self, left, right, negate):
        bang = "!" if negate else ""
        return f"tv::from_bool({bang}tv::equal({left}, {right}))"

    def out_is_none(self, value, negate):
        bang = "!" if negate else ""
        return f"tv::from_bool({bang}tv::is_none({value}))"

    def out_lookup(self, scope, name):
        return f'tv::lookup({scope}, "{name}")'

    def out_attr(self, value, name):
        return f'tv::attr({value}, "{name}")'


# --------------------------------------------------------------- codegen


def rust_string(text):
    out = []
    for char in text:
        if char == "\\":
            out.append("\\\\")
        elif char == '"':
            out.append('\\"')
        elif char == "\n":
            out.append("\\n")
        elif char == "\t":
            out.append("\\t")
        elif ord(char) < 32 or ord(char) == 127:
            out.append(f"\\u{{{ord(char):x}}}")
        else:
            out.append(char)
    return '"' + "".join(out) + '"'


def split_pages(templates):
    """The base template's nodes and each page's block overrides."""
    base_nodes = parse(templates["base.html"], "base.html")
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
            overrides[node[1]] = node[2]
        children.append((name, overrides))
    return base_nodes, children


class RustGenerator:
    """Compiles the same templates to crates/omt-web/src/templates_gen.rs."""

    def __init__(self):
        self.literals = {}
        self.counter = 0

    def literal(self, text):
        if text not in self.literals:
            self.literals[text] = f"LIT_{len(self.literals)}"
        return f"&{self.literals[text]}"

    def fresh(self, prefix):
        self.counter += 1
        return f"{prefix}_{self.counter}"

    def expr(self, text, scope):
        return RustExpr(text, self.literal).compile(scope)

    def emit(self, nodes, scope, indent, lines, blocks, template):
        pad = "    " * indent
        for node in nodes:
            kind = node[0]
            if kind == "text":
                if node[1]:
                    lines.append(f"{pad}out.push_str({rust_string(node[1])});")
            elif kind == "out":
                lines.append(f"{pad}tv::render(out, {self.expr(node[1], scope)});")
            elif kind == "if":
                for index, (condition, inner) in enumerate(node[1]):
                    keyword = "if" if index == 0 else "} else if"
                    lines.append(f"{pad}{keyword} tv::truthy({self.expr(condition, scope)}) {{")
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
                lines.append(f"{pad}    let {iterable} = {self.expr(node[2], scope)};")
                lines.append(f"{pad}    for {index} in 0..tv::length({iterable}) {{")
                lines.append(f"{pad}        let {item} = tv::index({iterable}, {index});")
                inner_scope = scope
                for position, target in enumerate(node[1]):
                    frame = self.fresh("sc")
                    value = item if len(node[1]) == 1 else f"tv::index({item}, {position})"
                    bind = f'Scope::bind({inner_scope}, "{target}", {value})'
                    lines.append(f"{pad}        let {frame} = {bind};")
                    inner_scope = f"&{frame}"
                self.emit(node[3], inner_scope, indent + 2, lines, blocks, template)
                lines.append(f"{pad}    }}")
                lines.append(f"{pad}}}")
            elif kind == "set":
                frame = self.fresh("sc")
                lines.append(
                    f'{pad}let {frame} = Scope::bind({scope}, "{node[1]}", '
                    f"{self.expr(node[2], scope)});"
                )
                scope = f"&{frame}"
            elif kind == "block":
                lines.append(f"{pad}if let Some(block) = blocks.{node[1]} {{")
                lines.append(f"{pad}    block(out, {scope});")
                lines.append(f"{pad}}} else {{")
                self.emit(node[2], scope, indent + 1, lines, blocks, template)
                lines.append(f"{pad}}}")
                blocks.add(node[1])
            elif kind == "extends":
                raise TemplateError(f"{template}: extends must be the first tag")
            else:
                raise TemplateError(f"{template}: unknown node {kind}")

    def render(self, templates):
        base_nodes, children = split_pages(templates)
        block_names = set()
        base_lines = []
        self.emit(base_nodes, "scope", 1, base_lines, block_names, "base.html")
        block_names = sorted(block_names)
        for name, overrides in children:
            for block in overrides:
                if block not in block_names:
                    raise TemplateError(f"{name}: base.html has no block {block}")

        def identifier(name: str) -> str:
            return re.sub(r"\W", "_", name.removesuffix(".html"))

        lines = [
            "// Generated by tools/gen/gen_templates.py from crates/omt-web/templates. "
            "Do not edit.",
            "#![allow(clippy::all, clippy::pedantic, unused_variables)]",
            "",
            "use std::borrow::Cow;",
            "",
            "use crate::tv::{self, Scope, Value};",
            "",
            "type BlockFn = for<'a> fn(&mut String, &Scope<'a>);",
            "",
            "struct Blocks {",
        ]
        lines += [f"    {block}: Option<BlockFn>," for block in block_names]
        lines += ["}", ""]
        literal_index = len(lines)
        lines += ["fn render_base(out: &mut String, scope: &Scope<'_>, blocks: &Blocks) {"]
        lines += base_lines
        lines += ["}", ""]
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
                    lines.append(f"fn {function}(out: &mut String, scope: &Scope<'_>) {{")
                    lines += body
                    lines += ["}", ""]
                    initializers.append(f"{block}: Some({function})")
                else:
                    initializers.append(f"{block}: None")
            lines.append(f"static {ident.upper()}_BLOCKS: Blocks = Blocks {{")
            lines += [f"    {item}," for item in initializers]
            lines += ["};", ""]
            table.append((name, ident))
        lines.append("/// Renders the named page into `out`; false for an unknown page.")
        lines.append("pub fn render(name: &str, context: &Value, out: &mut String) -> bool {")
        lines.append("    let blocks = match name {")
        for name, ident in table:
            lines.append(f'        "{name}" => &{ident.upper()}_BLOCKS,')
        lines.append("        _ => return false,")
        lines.append("    };")
        lines.append("    render_base(out, &Scope::root(context), blocks);")
        lines.append("    true")
        lines.append("}")
        lines.append("")
        lines.append("/// The embedded static files and their media types.")
        lines.append("pub fn static_asset(name: &str) -> Option<(&'static str, &'static str)> {")
        lines.append("    match name {")
        for path in sorted(STATIC.iterdir()):
            content = path.read_text(encoding="utf-8")
            lines.append(
                f'        "{path.name}" => Some(({rust_string(content)}, '
                f'"{STATIC_TYPES[path.suffix]}")),'
            )
        lines.append("        _ => None,")
        lines.append("    }")
        lines.append("}")
        lines.append("")
        literal_lines = [
            f"static {ident}: Value = Value::Str(Cow::Borrowed({rust_string(text)}));"
            for text, ident in self.literals.items()
        ]
        lines[literal_index:literal_index] = literal_lines + [""]
        return "\n".join(lines)


def load_templates():
    return {
        path.name: path.read_text(encoding="utf-8") for path in sorted(TEMPLATES.glob("*.html"))
    }


def render_rust():
    return RustGenerator().render(load_templates())


def main():
    try:
        outputs = {RUST_OUTPUT: render_rust()}
    except TemplateError as error:
        print(f"template error: {error}", file=sys.stderr)
        return 1
    if sys.argv[1:] == ["--check"]:
        stale = [path for path, text in outputs.items() if path.read_text(encoding="utf-8") != text]
        for path in stale:
            print(f"{path} is stale; run {sys.argv[0]}", file=sys.stderr)
        return 1 if stale else 0
    for path, text in outputs.items():
        path.write_text(text, encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
